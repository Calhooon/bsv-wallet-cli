//! The broadcast reconciler: what a served wallet does about transactions
//! its broadcaster accepted but the network never got.
//!
//! # The incident (2026-09-02, beta, real sats)
//!
//! Toolbox 0.3.56 let a provider's acceptance stand in for network presence:
//! four CLI wallets (served with `ARC_MODE=arcade`, no monitor) sent EF
//! children alone after Arcade had 202'd their parents. The parents never
//! propagated, the children were orphans forever, the overlay admitted and
//! later evicted them, one wallet internalized 10,000 sats of a phantom
//! upvote, and three wallets built later transactions on phantom change.
//! `abortAction` refused (`unproven`), `cleanup-abandoned` found nothing (the
//! broadcaster still HELD the bytes), `tick` re-relays nothing.
//!
//! A `serve` has no monitor, so nothing ever re-examined those transactions.
//! This module is that examination, on three fronts:
//!
//! 1. **Verdicts.** In Arcade mode the per-wallet SSE stream is drained once
//!    per pass (a fresh connect replays every non-terminal status), and the
//!    verdicts go through the same mapping as the monitor's SSE task and the
//!    webhook (`SEEN_*` lifts the req, `REJECTED` fails the tx).
//! 2. **Probes.** `unproven` transactions without fresh network evidence are
//!    probed with the [`BroadcastVerifier`] (the broadcaster we submitted
//!    through, the chain index, the third-party stores), paced, at most
//!    `max_probes` per pass. Network evidence records `seen` / `mined` for
//!    the transaction and `seen` for its unproven parents (presence of the
//!    child implies the parents connected). A fatal verdict retires. An
//!    absence from every network source starts the **absence clock** (the
//!    `unknown` row's `seen_at` in the broadcast memory); after
//!    `BROADCAST_ABSENCE_MINUTES` (default 30) of absence the transaction is
//!    retired.
//! 3. **Poisoned chains.** Anything established as a phantom (a fatal
//!    verdict, the absence clock, a `rejected` memory row, an already
//!    `failed` transaction with unproven children) is retired with every
//!    unproven descendant through the toolbox's RELEASE-RULE path
//!    (`StorageSqlx::retire_poisoned_chain`): outside inputs released only
//!    on chain verification, the set's outputs unspendable, internalized
//!    payments that trace to a phantom source marked unspendable and logged.
//!
//! `serve` runs a pass every 60 s in the background (bounded, one summary
//! line per pass); `bsv-wallet reconcile-broadcasts` runs one pass by hand
//! (dry run by default); the daemon's ticker runs the poisoned-chain sweep.

use std::collections::HashSet;
use std::sync::atomic::AtomicBool;
use std::time::Duration;

use anyhow::Result;
use bsv_wallet_toolbox::monitor::ArcadeEventsTask;
use bsv_wallet_toolbox::services::providers::arcade::statuses;
use bsv_wallet_toolbox::{
    ArcadeSseClient, ArcadeStatusEvent, BroadcastMemory, BroadcastStatus, Chain, MonitorStorage,
    PoisonOutcome, PoisonReport, Services, StorageSqlx, Wallet, BROADCAST_PROVIDER_NETWORK,
    BROADCAST_SEEN_STALE_SECS, BROADCAST_STATUS_MINED, BROADCAST_STATUS_SEEN,
    BROADCAST_STATUS_UNKNOWN, PROVIDER_ARCADE_V2,
};
use sqlx::Row;

use crate::broadcast_verify::{BroadcastVerifier, NetworkEvidence, PresenceReport};

/// Default minutes of continuous absence from every network source before
/// an unproven transaction is retired (`BROADCAST_ABSENCE_MINUTES`).
pub const DEFAULT_ABSENCE_MINUTES: i64 = 30;
/// Default seconds between two passes of the served loop
/// (`BROADCAST_RECONCILE_INTERVAL_SECS`).
pub const DEFAULT_INTERVAL_SECS: u64 = 60;
/// Default probes per pass of the served loop
/// (`BROADCAST_RECONCILE_MAX_PROBES`).
pub const DEFAULT_MAX_PROBES: usize = 20;
/// The served loop only probes transactions younger than this.
pub const SERVE_MAX_AGE_HOURS: i64 = 24;
/// Pause between two probes of one pass (WhatsOnChain's public rate).
const PROBE_PACE: Duration = Duration::from_millis(350);
/// Wall-clock budget of one SSE drain.
const SSE_BUDGET: Duration = Duration::from_secs(8);
/// An SSE stream idle for this long has replayed everything it had.
const SSE_IDLE: Duration = Duration::from_secs(2);

/// The wallet this module works on.
pub type ServedWallet = Wallet<StorageSqlx, Services>;

/// Where the Arcade SSE stream of this wallet is.
#[derive(Debug, Clone)]
pub struct ArcadeSse {
    /// Arcade base URL.
    pub url: String,
    /// The wallet's callback token (scopes the stream). Never logged.
    pub token: String,
}

/// Knobs of one pass.
#[derive(Debug, Clone)]
pub struct ReconcileOptions {
    /// Apply changes (`false` = report only; probes still run, they are
    /// read-only).
    pub execute: bool,
    /// Probes per pass.
    pub max_probes: usize,
    /// Only transactions created within this many hours are probed (`None`
    /// = every unproven transaction).
    pub max_age_hours: Option<i64>,
    /// Minutes of continuous network absence before a retire.
    pub absence_minutes: i64,
    /// The Arcade SSE stream to drain first, when the wallet has one.
    pub sse: Option<ArcadeSse>,
}

impl ReconcileOptions {
    /// The served loop's options: apply, bounded probes, 24 h window, the
    /// env knobs (`BROADCAST_ABSENCE_MINUTES`, `BROADCAST_RECONCILE_MAX_PROBES`).
    pub fn for_serve(sse: Option<ArcadeSse>) -> Self {
        Self {
            execute: true,
            max_probes: env_parse("BROADCAST_RECONCILE_MAX_PROBES", DEFAULT_MAX_PROBES),
            max_age_hours: Some(SERVE_MAX_AGE_HOURS),
            absence_minutes: absence_minutes_from_env(),
            sse,
        }
    }

    /// The command's options: every unproven transaction, `max_probes` of
    /// them probed, applied only with `execute`.
    pub fn for_command(execute: bool, max_probes: usize, sse: Option<ArcadeSse>) -> Self {
        Self {
            execute,
            max_probes,
            max_age_hours: None,
            absence_minutes: absence_minutes_from_env(),
            sse,
        }
    }
}

/// `BROADCAST_ABSENCE_MINUTES`, default [`DEFAULT_ABSENCE_MINUTES`].
pub fn absence_minutes_from_env() -> i64 {
    env_parse("BROADCAST_ABSENCE_MINUTES", DEFAULT_ABSENCE_MINUTES)
}

fn env_parse<T: std::str::FromStr>(key: &str, default: T) -> T {
    std::env::var(key)
        .ok()
        .and_then(|v| v.trim().parse::<T>().ok())
        .unwrap_or(default)
}

/// The Arcade SSE location for a wallet at `db_path`, when it runs in Arcade
/// mode (mirrors `services_env::arcade_runtime`).
pub fn arcade_sse_for(db_path: &str) -> Option<ArcadeSse> {
    crate::services_env::arcade_runtime(db_path)
        .ok()
        .flatten()
        .map(|rt| ArcadeSse {
            url: rt.url,
            token: rt.callback_token,
        })
}

/// What one pass found and did.
#[derive(Debug, Default, Clone)]
pub struct ReconcileBroadcastsReport {
    /// SSE status events applied.
    pub sse_events: u64,
    /// SSE fatal verdicts (txids).
    pub sse_fatal: Vec<String>,
    /// Unproven (or stale `sending`) transactions in the window.
    pub candidates: usize,
    /// Candidates skipped for fresh network evidence.
    pub fresh: usize,
    /// Candidates probed this pass.
    pub probed: usize,
    /// Seen on the network (txids).
    pub seen: Vec<String>,
    /// Mined per a probe (txids).
    pub mined: Vec<String>,
    /// Held by a store, no network evidence, chain index has it
    /// (txids).
    pub held: Vec<String>,
    /// Nothing decisive (txids).
    pub inconclusive: Vec<String>,
    /// Absent from every network source: `(txid, minutes absent so far)`.
    pub absent: Vec<(String, i64)>,
    /// Fatal verdict from the broadcaster (txids).
    pub fatal: Vec<String>,
    /// The poison retirements run (or, on a dry run, simulated), in order.
    pub retired: Vec<PoisonReport>,
}

impl ReconcileBroadcastsReport {
    /// Transactions the retirements touched (retirable statuses).
    pub fn retired_txids(&self) -> Vec<String> {
        self.retired
            .iter()
            .filter(|r| r.outcome == PoisonOutcome::Retired)
            .flat_map(|r| r.retirable_txids())
            .collect()
    }

    /// One line for the log.
    pub fn summary(&self, execute: bool) -> String {
        let retired: Vec<&PoisonReport> = self
            .retired
            .iter()
            .filter(|r| r.outcome == PoisonOutcome::Retired)
            .collect();
        let txs = self.retired_txids().len();
        let restored: u32 = retired.iter().map(|r| r.restored).sum();
        let restored_sats: i64 = retired.iter().map(|r| r.restored_sats).sum();
        let invalidated: u32 = retired.iter().map(|r| r.invalidated).sum();
        let invalidated_sats: i64 = retired.iter().map(|r| r.invalidated_sats).sum();
        let internalized: usize = retired.iter().map(|r| r.internalized.len()).sum();
        let alive = self
            .retired
            .iter()
            .filter(|r| r.outcome == PoisonOutcome::Alive)
            .count();
        let refused = self
            .retired
            .iter()
            .filter(|r| matches!(r.outcome, PoisonOutcome::Refused { .. }))
            .count();
        format!(
            "reconcile-broadcasts{}: sse_events={} candidates={} fresh={} probed={} seen={} mined={} held={} inconclusive={} absent={} fatal={} retired_roots={} retired_txs={} restored={} ({} sats) invalidated={} ({} sats) internalized={} alive={} refused={}",
            if execute { "" } else { " (dry run)" },
            self.sse_events,
            self.candidates,
            self.fresh,
            self.probed,
            self.seen.len(),
            self.mined.len(),
            self.held.len(),
            self.inconclusive.len(),
            self.absent.len(),
            self.fatal.len(),
            retired.len(),
            txs,
            restored,
            restored_sats,
            invalidated,
            invalidated_sats,
            internalized,
            alive,
            refused,
        )
    }
}

/// One pass: SSE drain, probes, poison retirements. See the module docs.
pub async fn run_pass(
    wallet: &ServedWallet,
    verifier: &BroadcastVerifier,
    opts: &ReconcileOptions,
) -> Result<ReconcileBroadcastsReport> {
    let storage = wallet.storage();
    let mut report = ReconcileBroadcastsReport::default();

    // 1. Verdicts pushed by Arcade.
    if let Some(sse) = &opts.sse {
        let (events, fatal) = drain_sse(storage, sse).await;
        report.sse_events = events;
        report.sse_fatal = fatal;
    }

    // 2. Probes.
    let candidates = select_candidates(storage, opts.max_age_hours).await?;
    report.candidates = candidates.len();
    let records = storage.broadcast_records(None, &candidates).await?;
    let now = chrono::Utc::now();
    let mut to_probe: Vec<String> = Vec::new();
    let mut roots: Vec<(String, &'static str)> = Vec::new();
    for txid in &candidates {
        let mine: Vec<_> = records.iter().filter(|r| &r.txid == txid).collect();
        if mine
            .iter()
            .any(|r| r.ladder_status() == Some(BroadcastStatus::Rejected))
        {
            roots.push((txid.clone(), "rejected memory row"));
            continue;
        }
        let fresh = mine.iter().any(|r| {
            r.ladder_status().is_some_and(|s| s.is_network_evidence())
                && (now - r.seen_at).num_seconds() <= BROADCAST_SEEN_STALE_SECS
        });
        if fresh {
            report.fresh += 1;
            continue;
        }
        to_probe.push(txid.clone());
    }
    for (index, txid) in to_probe.iter().take(opts.max_probes).enumerate() {
        if index > 0 {
            tokio::time::sleep(PROBE_PACE).await;
        }
        report.probed += 1;
        let presence = verifier.verify_report(txid).await;
        match classify(&presence) {
            Probe::Evidence(evidence) => {
                credit_presence(storage, txid, evidence, presence.evidence_provider).await;
                match evidence {
                    NetworkEvidence::Seen => report.seen.push(txid.clone()),
                    NetworkEvidence::Mined => report.mined.push(txid.clone()),
                }
            }
            Probe::Fatal => {
                report.fatal.push(txid.clone());
                roots.push((txid.clone(), "fatal verdict from the broadcaster"));
            }
            Probe::Absent => {
                let minutes = absence_minutes(storage, txid).await;
                report.absent.push((txid.clone(), minutes));
                if minutes >= opts.absence_minutes {
                    roots.push((txid.clone(), "absent from every network source"));
                }
            }
            Probe::Held => report.held.push(txid.clone()),
            Probe::Inconclusive => report.inconclusive.push(txid.clone()),
        }
    }

    // 3. Poisoned chains: this pass's verdicts plus whatever earlier
    // verdicts (SSE, webhook, cleanup) left half done.
    for root in sweep_roots(storage).await? {
        if !roots.iter().any(|(t, _)| t == &root) {
            roots.push((root, "failed transaction with unproven descendants"));
        }
    }
    let mut covered: HashSet<String> = HashSet::new();
    for (root, reason) in roots {
        if covered.contains(&root) {
            continue;
        }
        let poison = storage
            .retire_poisoned_chain(wallet.services(), &root, "invalid", opts.execute)
            .await?;
        match &poison.outcome {
            PoisonOutcome::Retired => {
                tracing::warn!(
                    root = %root,
                    reason,
                    txs = poison.chain.len(),
                    executed = poison.executed,
                    "reconcile-broadcasts: poisoned chain {}",
                    if opts.execute { "retired" } else { "would be retired" }
                );
            }
            PoisonOutcome::Alive => {
                // The status service knows it: that is network evidence, and
                // it stops the same root from being re-examined every pass.
                if opts.execute {
                    if let Err(e) = storage
                        .record_broadcast_status(
                            &root,
                            BROADCAST_PROVIDER_NETWORK,
                            BROADCAST_STATUS_SEEN,
                        )
                        .await
                    {
                        tracing::warn!(root = %root, error = %e, "could not record the alive verdict");
                    }
                }
                tracing::info!(root = %root, reason, "reconcile-broadcasts: root is alive per the status service, kept");
            }
            PoisonOutcome::Refused { proven_txid } => {
                tracing::error!(root = %root, proven = %proven_txid, reason, "reconcile-broadcasts: retire refused, a descendant is proven");
            }
            PoisonOutcome::NotFound => {}
        }
        for tx in &poison.chain {
            covered.insert(tx.txid.clone());
        }
        report.retired.push(poison);
    }

    Ok(report)
}

/// The poisoned-chain sweep alone (no SSE, no probes): the daemon's ticker
/// and `cleanup-abandoned` call it after their own verdicts.
pub async fn run_sweep(wallet: &ServedWallet, execute: bool) -> Result<Vec<PoisonReport>> {
    let storage = wallet.storage();
    let mut reports = Vec::new();
    let mut covered: HashSet<String> = HashSet::new();
    let mut roots = sweep_roots(storage).await?;
    roots.extend(rejected_roots(storage).await?);
    for root in roots {
        if covered.contains(&root) {
            continue;
        }
        let poison = storage
            .retire_poisoned_chain(wallet.services(), &root, "invalid", execute)
            .await?;
        for tx in &poison.chain {
            covered.insert(tx.txid.clone());
        }
        reports.push(poison);
    }
    Ok(reports)
}

/// Spawn the served loop: one pass every `BROADCAST_RECONCILE_INTERVAL_SECS`
/// (default 60), applied, bounded, one summary line per pass. Disabled by
/// `BROADCAST_RECONCILE=0`.
pub fn spawn_serve_loop(
    wallet: std::sync::Arc<ServedWallet>,
    chain: Chain,
    db_path: &str,
) -> Option<tokio::task::JoinHandle<()>> {
    let enabled = std::env::var("BROADCAST_RECONCILE")
        .map(|v| v != "0" && !v.eq_ignore_ascii_case("false"))
        .unwrap_or(true);
    if !enabled {
        tracing::info!("broadcast reconcile loop disabled (BROADCAST_RECONCILE=0)");
        return None;
    }
    let opts = ReconcileOptions::for_serve(arcade_sse_for(db_path));
    let interval_secs =
        env_parse("BROADCAST_RECONCILE_INTERVAL_SECS", DEFAULT_INTERVAL_SECS).max(5);
    let verifier = BroadcastVerifier::single_pass(chain);
    tracing::info!(
        interval_secs,
        max_probes = opts.max_probes,
        absence_minutes = opts.absence_minutes,
        sse = opts.sse.is_some(),
        "broadcast reconcile loop started"
    );
    Some(tokio::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_secs(interval_secs));
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        // The first tick fires at once; let the server settle first.
        interval.tick().await;
        loop {
            interval.tick().await;
            match run_pass(&wallet, &verifier, &opts).await {
                Ok(report) => {
                    let quiet = report.probed == 0
                        && report.sse_events == 0
                        && report.retired.is_empty()
                        && report.candidates == 0;
                    if quiet {
                        tracing::debug!("{}", report.summary(true));
                    } else {
                        tracing::info!("{}", report.summary(true));
                    }
                }
                Err(e) => tracing::warn!(error = %e, "broadcast reconcile pass failed"),
            }
        }
    }))
}

/// What a probe report means to the reconciler.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Probe {
    Evidence(NetworkEvidence),
    Fatal,
    Absent,
    Held,
    Inconclusive,
}

fn classify(report: &PresenceReport) -> Probe {
    if let Some(evidence) = report.evidence {
        return Probe::Evidence(evidence);
    }
    if report.broadcaster_fatal {
        return Probe::Fatal;
    }
    if report.network_absent {
        return Probe::Absent;
    }
    match report.verification {
        crate::broadcast_verify::BroadcastVerification::Confirmed => Probe::Held,
        _ => Probe::Inconclusive,
    }
}

/// Record network evidence for `txid` and `seen` for its unproven parents.
async fn credit_presence(
    storage: &StorageSqlx,
    txid: &str,
    evidence: NetworkEvidence,
    provider: &'static str,
) {
    if let Err(e) = storage
        .mark_transaction_seen_on_network_by(txid, provider)
        .await
    {
        tracing::warn!(txid = %txid, error = %e, "could not record the network presence");
    }
    if evidence == NetworkEvidence::Mined {
        if let Err(e) = storage
            .record_broadcast_status(txid, provider, BROADCAST_STATUS_MINED)
            .await
        {
            tracing::warn!(txid = %txid, error = %e, "could not record the mined evidence");
        }
    }
    match unproven_parents(storage, txid).await {
        Ok(parents) if !parents.is_empty() => {
            if let Err(e) = storage
                .sqlx_broadcast_memory()
                .record_broadcast_status_many(provider, BROADCAST_STATUS_SEEN, &parents)
                .await
            {
                tracing::warn!(txid = %txid, error = %e, "could not credit the parents");
            }
            tracing::info!(
                txid = %txid,
                ?evidence,
                provider,
                parents = parents.len(),
                "network presence: the transaction and its unproven parents connected"
            );
        }
        Ok(_) => {
            tracing::info!(txid = %txid, ?evidence, provider, "network presence recorded");
        }
        Err(e) => tracing::warn!(txid = %txid, error = %e, "could not list the parents"),
    }
}

/// Start (or read) the absence clock of `txid`: an `unknown` row under the
/// network pseudo-provider whose `seen_at` is the first observed absence.
/// Returns the minutes elapsed since it started.
async fn absence_minutes(storage: &StorageSqlx, txid: &str) -> i64 {
    if let Err(e) = storage
        .record_broadcast_status(txid, BROADCAST_PROVIDER_NETWORK, BROADCAST_STATUS_UNKNOWN)
        .await
    {
        tracing::warn!(txid = %txid, error = %e, "could not record the absence");
        return 0;
    }
    match storage
        .broadcast_status_of(txid, BROADCAST_PROVIDER_NETWORK)
        .await
    {
        Ok(Some(row)) if row.ladder_status() == Some(BroadcastStatus::Unknown) => {
            let minutes = (chrono::Utc::now() - row.seen_at).num_minutes().max(0);
            tracing::info!(
                txid = %txid,
                minutes,
                "absent from every network source (absence clock running)"
            );
            minutes
        }
        _ => 0,
    }
}

/// `unproven` transactions (and `sending` ones older than ten minutes),
/// oldest first, optionally limited to the last `max_age_hours`.
async fn select_candidates(
    storage: &StorageSqlx,
    max_age_hours: Option<i64>,
) -> Result<Vec<String>> {
    let modifier = max_age_hours.map(|h| format!("-{} hours", h.max(0)));
    let rows = sqlx::query(
        "SELECT txid FROM transactions \
         WHERE txid IS NOT NULL \
           AND (status = 'unproven' \
                OR (status = 'sending' AND datetime(created_at) <= datetime('now', '-600 seconds'))) \
           AND (? IS NULL OR datetime(created_at) >= datetime('now', ?)) \
         ORDER BY datetime(created_at) ASC, transaction_id ASC",
    )
    .bind(&modifier)
    .bind(&modifier)
    .fetch_all(storage.pool())
    .await?;
    let mut seen = HashSet::new();
    Ok(rows
        .iter()
        .map(|r| r.get::<String, _>("txid"))
        .filter(|t| seen.insert(t.clone()))
        .collect())
}

/// Unproven parents of `txid` in this wallet: the transactions whose
/// outputs it spends.
async fn unproven_parents(storage: &StorageSqlx, txid: &str) -> Result<Vec<String>> {
    let rows = sqlx::query(
        "SELECT DISTINCT t.txid FROM outputs o \
         JOIN transactions t ON o.transaction_id = t.transaction_id \
         WHERE o.spent_by = (SELECT transaction_id FROM transactions WHERE txid = ? LIMIT 1) \
           AND t.status IN ('unproven', 'sending') AND t.txid IS NOT NULL",
    )
    .bind(txid)
    .fetch_all(storage.pool())
    .await?;
    Ok(rows.iter().map(|r| r.get::<String, _>("txid")).collect())
}

/// Roots of poisoned chains an earlier verdict left half done: `failed`
/// transactions with unproven spenders of their outputs.
async fn sweep_roots(storage: &StorageSqlx) -> Result<Vec<String>> {
    let rows = sqlx::query(
        "SELECT DISTINCT p.txid FROM transactions p \
         JOIN outputs o ON o.transaction_id = p.transaction_id \
         JOIN transactions c ON c.transaction_id = o.spent_by \
         WHERE p.status = 'failed' AND p.txid IS NOT NULL \
           AND c.status IN ('unproven', 'sending', 'nosend') \
         ORDER BY p.transaction_id ASC",
    )
    .fetch_all(storage.pool())
    .await?;
    Ok(rows.iter().map(|r| r.get::<String, _>("txid")).collect())
}

/// Unproven transactions the memory holds a `rejected` row for.
async fn rejected_roots(storage: &StorageSqlx) -> Result<Vec<String>> {
    let rows = sqlx::query(
        "SELECT DISTINCT t.txid FROM transactions t \
         JOIN broadcast_seen b ON b.txid = t.txid \
         WHERE t.status IN ('unproven', 'sending') AND b.status = 'rejected' \
         ORDER BY t.transaction_id ASC",
    )
    .fetch_all(storage.pool())
    .await?;
    Ok(rows.iter().map(|r| r.get::<String, _>("txid")).collect())
}

/// Drain the wallet's Arcade SSE stream once: connect, apply every status
/// frame the replay delivers, stop when the stream idles or the budget is
/// spent. Returns `(events applied, fatal txids)`.
async fn drain_sse(storage: &StorageSqlx, sse: &ArcadeSse) -> (u64, Vec<String>) {
    let mut client = match ArcadeSseClient::new(&sse.url, &sse.token) {
        Ok(c) => c,
        Err(e) => {
            tracing::warn!(error = %e, "arcade SSE client could not be built");
            return (0, Vec::new());
        }
    };
    let (tx, mut rx) = tokio::sync::mpsc::channel::<ArcadeStatusEvent>(256);
    let stream = tokio::spawn(async move { client.stream_once(tx).await });
    let deadline = tokio::time::Instant::now() + SSE_BUDGET;
    let trigger = AtomicBool::new(false);
    let mut applied = 0u64;
    let mut fatal = Vec::new();
    loop {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        if remaining.is_zero() {
            break;
        }
        match tokio::time::timeout(SSE_IDLE.min(remaining), rx.recv()).await {
            Ok(Some(ev)) => {
                match ArcadeEventsTask::<StorageSqlx>::apply_event(storage, &ev, &trigger).await {
                    Ok(updated) => {
                        applied += 1;
                        tracing::debug!(txid = %ev.txid, status = %ev.tx_status, updated, "arcade SSE status");
                    }
                    Err(e) => {
                        tracing::warn!(txid = %ev.txid, status = %ev.tx_status, error = %e, "arcade SSE status not applied");
                    }
                }
                if ev.tx_status == statuses::MINED {
                    if let Err(e) = storage
                        .record_broadcast_status(
                            &ev.txid,
                            PROVIDER_ARCADE_V2,
                            BROADCAST_STATUS_MINED,
                        )
                        .await
                    {
                        tracing::warn!(txid = %ev.txid, error = %e, "could not record the mined verdict");
                    }
                }
                if bsv_wallet_toolbox::is_fatal_status(&ev.tx_status) {
                    fatal.push(ev.txid.clone());
                }
            }
            Ok(None) => break,
            Err(_) => break,
        }
    }
    stream.abort();
    (applied, fatal)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::broadcast_verify::BroadcastVerification;

    #[test]
    fn a_probe_report_classifies_in_evidence_order() {
        let mut r = PresenceReport::from_verification(BroadcastVerification::Confirmed);
        assert_eq!(classify(&r), Probe::Held);
        r.network_absent = true;
        assert_eq!(
            classify(&r),
            Probe::Absent,
            "held by the broadcaster, absent from the network"
        );
        r.broadcaster_fatal = true;
        assert_eq!(classify(&r), Probe::Fatal);
        r.evidence = Some(NetworkEvidence::Seen);
        assert_eq!(classify(&r), Probe::Evidence(NetworkEvidence::Seen));
        let i = PresenceReport::from_verification(BroadcastVerification::Inconclusive);
        assert_eq!(classify(&i), Probe::Inconclusive);
        let mut rejected = PresenceReport::from_verification(BroadcastVerification::Rejected);
        rejected.network_absent = true;
        assert_eq!(classify(&rejected), Probe::Absent);
    }

    #[test]
    fn options_read_the_env_knobs_with_defaults() {
        let serve = ReconcileOptions::for_serve(None);
        assert!(serve.execute);
        assert_eq!(serve.max_age_hours, Some(SERVE_MAX_AGE_HOURS));
        let command = ReconcileOptions::for_command(false, 7, None);
        assert!(!command.execute);
        assert_eq!(command.max_probes, 7);
        assert_eq!(command.max_age_hours, None);
        assert!(command.absence_minutes >= 1);
    }

    #[test]
    fn the_summary_is_one_line() {
        let report = ReconcileBroadcastsReport::default();
        let line = report.summary(false);
        assert!(line.starts_with("reconcile-broadcasts (dry run):"));
        assert!(!line.contains('\n'));
        assert!(report.retired_txids().is_empty());
    }

    async fn storage_with_chain() -> StorageSqlx {
        use bsv_wallet_toolbox::WalletStorageWriter;
        let storage = StorageSqlx::in_memory().await.unwrap();
        storage
            .migrate("reconcile-tests", &("02".to_string() + &"ab".repeat(32)))
            .await
            .unwrap();
        storage.make_available().await.unwrap();
        let (user, _) = storage
            .find_or_insert_user(&("02".to_string() + &"cd".repeat(32)))
            .await
            .unwrap();
        let basket = storage
            .find_or_create_default_basket(user.user_id)
            .await
            .unwrap()
            .basket_id;
        let now = chrono::Utc::now();
        let mut ids = Vec::new();
        for (txid, status, created) in [
            ("aa".repeat(32), "failed", "2020-01-01T00:00:00+00:00"),
            ("bb".repeat(32), "unproven", "2020-01-02T00:00:00+00:00"),
            ("cc".repeat(32), "sending", "2020-01-03 00:00:00"),
            ("dd".repeat(32), "unproven", ""),
        ] {
            let created_at: String = if created.is_empty() {
                now.to_rfc3339()
            } else {
                created.to_string()
            };
            let id = sqlx::query(
                "INSERT INTO transactions (user_id, status, reference, is_outgoing, satoshis, version, lock_time, description, txid, raw_tx, created_at, updated_at) \
                 VALUES (?, ?, ?, 1, 0, 1, 0, 'd', ?, X'01000000', ?, ?)",
            )
            .bind(user.user_id)
            .bind(status)
            .bind(&txid[..6])
            .bind(&txid)
            .bind(&created_at)
            .bind(now)
            .execute(storage.pool())
            .await
            .unwrap()
            .last_insert_rowid();
            ids.push(id);
        }
        // aa (failed) -> bb (unproven) -> dd (unproven); cc alone.
        let lock = hex::decode("76a914dbc0a7c84983c5bf199b7b2d41b3acf0408ee5aa88ac").unwrap();
        for (tx_row, txid, spent_by) in [
            (ids[0], "aa".repeat(32), Some(ids[1])),
            (ids[1], "bb".repeat(32), Some(ids[3])),
            (ids[3], "dd".repeat(32), None),
        ] {
            sqlx::query(
                "INSERT INTO outputs (user_id, transaction_id, basket_id, vout, satoshis, locking_script, txid, type, spendable, change, spent_by, provided_by, purpose, output_description, created_at, updated_at) \
                 VALUES (?, ?, ?, 0, 1000, ?, ?, 'P2PKH', ?, 1, ?, 'storage', 'change', 'c', ?, ?)",
            )
            .bind(user.user_id)
            .bind(tx_row)
            .bind(basket)
            .bind(&lock)
            .bind(&txid)
            .bind(spent_by.is_none() as i64)
            .bind(spent_by)
            .bind(now)
            .bind(now)
            .execute(storage.pool())
            .await
            .unwrap();
        }
        storage
    }

    #[tokio::test]
    async fn candidates_parents_and_sweep_roots_come_from_the_wallet_graph() {
        let storage = storage_with_chain().await;
        let all = select_candidates(&storage, None).await.unwrap();
        assert_eq!(
            all,
            vec!["bb".repeat(32), "cc".repeat(32), "dd".repeat(32)],
            "unproven and stale sending, oldest first"
        );
        let recent = select_candidates(&storage, Some(24)).await.unwrap();
        assert_eq!(recent, vec!["dd".repeat(32)]);

        let parents = unproven_parents(&storage, &"dd".repeat(32)).await.unwrap();
        assert_eq!(parents, vec!["bb".repeat(32)]);
        assert!(
            unproven_parents(&storage, &"bb".repeat(32))
                .await
                .unwrap()
                .is_empty(),
            "a failed parent is not credited"
        );

        let roots = sweep_roots(&storage).await.unwrap();
        assert_eq!(
            roots,
            vec!["aa".repeat(32)],
            "failed with unproven children"
        );

        storage
            .record_broadcast_status(&"cc".repeat(32), "ArcadeV2", "rejected")
            .await
            .unwrap();
        assert_eq!(
            rejected_roots(&storage).await.unwrap(),
            vec!["cc".repeat(32)]
        );
    }

    #[tokio::test]
    async fn the_absence_clock_starts_on_the_first_absence_and_keeps_its_start() {
        let storage = storage_with_chain().await;
        let txid = "bb".repeat(32);
        assert_eq!(absence_minutes(&storage, &txid).await, 0);
        sqlx::query(
            "UPDATE broadcast_seen SET seen_at = datetime('now', '-31 minutes') WHERE txid = ?",
        )
        .bind(&txid)
        .execute(storage.pool())
        .await
        .unwrap();
        let minutes = absence_minutes(&storage, &txid).await;
        assert!((30..=32).contains(&minutes), "{}", minutes);
        // Network evidence resets it.
        storage
            .record_broadcast_status(&txid, BROADCAST_PROVIDER_NETWORK, BROADCAST_STATUS_SEEN)
            .await
            .unwrap();
        assert_eq!(absence_minutes(&storage, &txid).await, 0);
    }
}
