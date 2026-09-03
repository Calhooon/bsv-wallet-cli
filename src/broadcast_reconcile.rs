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
//! The first cure (0.3.58) trusted a broadcaster's `SEEN_*` as network
//! evidence. Live, Arcade kept reporting `SEEN_MULTIPLE_NODES` for the
//! phantom roots two hours later while WhatsOnChain answered 404, and the
//! SSE drain refreshed that `seen` every pass, so the roots were never
//! probed and the real coin they had spent (224,575 sats, kept locked by one
//! rate-limited lookup) was never retried. Hence the rules below.
//!
//! # The rules (0.3.59)
//!
//! 1. **A broadcaster's SEEN is not chain evidence.** For an unproven
//!    transaction older than `BROADCAST_ABSENCE_MINUTES` (default 30) only a
//!    fresh CHAIN-INDEX row (`chain|seen` within 10 minutes, or `chain|mined`)
//!    exempts it from a probe, whatever the broadcaster said. A younger
//!    transaction is provisionally trusted on any fresh network-evidence row.
//! 2. **The chain index decides.** Every probe asks the broadcaster we
//!    submitted through AND WhatsOnChain. A chain-index hit records `chain`
//!    evidence for the transaction and its unproven parents (presence of the
//!    child implies the parents connected). A chain-index 404 with the
//!    broadcaster merely holding or having seen the transaction is
//!    `network_absent`; past the threshold it retires.
//! 3. **The poison runs both ways.** A retire climbs UP first (a phantom's
//!    parent that is unproven and unknown to the chain is part of the same
//!    poison; the climb stops at the first transaction the chain knows) and
//!    then retires the root with every unproven descendant through the
//!    toolbox's RELEASE-RULE path: outside inputs restored only on chain
//!    verification, the set's outputs unspendable, internalized payments that
//!    trace to a phantom marked unspendable and logged. The climb never
//!    passes through a parent younger than the absence threshold (toolbox
//!    0.3.60): a transaction the chain index has not seen yet is not absent,
//!    and the verdict was about the child. On 2026-09-03 (beta, fleet w2)
//!    the live bytes transaction of a lost head race was retired on a 404
//!    48 s after broadcast and the five coins it had spent were released;
//!    the next two actions double-spent them and were rejected in turn.
//!    A "not in the unspent set" answer for a coin whose source the index
//!    does not know is likewise `unknown` (re-checked), not `spent`.
//! 4. **Kept-locked inputs are retried.** An outside input the chain could
//!    not vouch for is re-checked every pass with exponential backoff until
//!    it is verifiably unspent (restored) or spent (left); nothing stays
//!    locked forever unattended.
//!
//! In Arcade mode the per-wallet SSE stream is drained once per pass first
//! (verdicts go through the same mapping as the monitor's SSE task and the
//! webhook). `serve` runs a pass every 60 s in the background (bounded, one
//! summary line per pass); `bsv-wallet reconcile-broadcasts` runs one pass
//! by hand (dry run by default); the daemon's ticker runs the sweep and the
//! locked-input re-checks.

use std::collections::HashSet;
use std::sync::atomic::AtomicBool;
use std::time::Duration;

use anyhow::Result;
use bsv_wallet_toolbox::monitor::ArcadeEventsTask;
use bsv_wallet_toolbox::services::providers::arcade::statuses;
use bsv_wallet_toolbox::{
    ArcadeSseClient, ArcadeStatusEvent, BroadcastMemory, BroadcastSeenRecord, BroadcastStatus,
    Chain, LockedInputReport, MonitorStorage, PoisonOutcome, PoisonReport, Services, StorageSqlx,
    Wallet, WalletServices, BROADCAST_PROVIDER_CHAIN, BROADCAST_PROVIDER_NETWORK,
    BROADCAST_SEEN_STALE_SECS, BROADCAST_STATUS_MINED, BROADCAST_STATUS_SEEN,
    BROADCAST_STATUS_UNKNOWN, PROVIDER_ARCADE_V2,
};
use chrono::{DateTime, NaiveDateTime, TimeZone, Utc};
use sqlx::Row;

use crate::broadcast_verify::{
    BroadcastVerification, BroadcastVerifier, ChainIndexAnswer, NetworkEvidence, PresenceReport,
};

/// Default minutes an unproven transaction must be old before a chain-index
/// absence retires it (`BROADCAST_ABSENCE_MINUTES`).
pub const DEFAULT_ABSENCE_MINUTES: i64 = 30;
/// Default seconds between two passes of the served loop
/// (`BROADCAST_RECONCILE_INTERVAL_SECS`).
pub const DEFAULT_INTERVAL_SECS: u64 = 60;
/// Default probes per pass of the served loop
/// (`BROADCAST_RECONCILE_MAX_PROBES`).
pub const DEFAULT_MAX_PROBES: usize = 20;
/// Default locked-input re-checks per pass
/// (`BROADCAST_RECONCILE_MAX_LOCKED_CHECKS`).
pub const DEFAULT_MAX_LOCKED_CHECKS: usize = 20;
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
    /// Minutes an unproven transaction must be old before a chain-index
    /// absence retires it.
    pub absence_minutes: i64,
    /// Locked-input re-checks per pass.
    pub max_locked_checks: usize,
    /// The Arcade SSE stream to drain first, when the wallet has one.
    pub sse: Option<ArcadeSse>,
}

impl ReconcileOptions {
    /// The served loop's options: apply, bounded probes, 24 h window, the
    /// env knobs (`BROADCAST_ABSENCE_MINUTES`, `BROADCAST_RECONCILE_MAX_PROBES`,
    /// `BROADCAST_RECONCILE_MAX_LOCKED_CHECKS`).
    pub fn for_serve(sse: Option<ArcadeSse>) -> Self {
        Self {
            execute: true,
            max_probes: env_parse("BROADCAST_RECONCILE_MAX_PROBES", DEFAULT_MAX_PROBES),
            max_age_hours: Some(SERVE_MAX_AGE_HOURS),
            absence_minutes: absence_minutes_from_env(),
            max_locked_checks: env_parse(
                "BROADCAST_RECONCILE_MAX_LOCKED_CHECKS",
                DEFAULT_MAX_LOCKED_CHECKS,
            ),
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
            max_locked_checks: max_probes.max(DEFAULT_MAX_LOCKED_CHECKS),
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

/// An unproven (or stale `sending`) transaction of the wallet.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Candidate {
    /// Transaction id.
    pub txid: String,
    /// When the wallet created it.
    pub created_at: DateTime<Utc>,
}

impl Candidate {
    /// Minutes since creation (never negative).
    pub fn age_minutes(&self, now: DateTime<Utc>) -> i64 {
        (now - self.created_at).num_minutes().max(0)
    }
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
    /// Candidates skipped for fresh evidence (see [`needs_probe`]).
    pub fresh: usize,
    /// Candidates probed this pass.
    pub probed: usize,
    /// Seen by the chain index (txids).
    pub seen: Vec<String>,
    /// Mined per the chain index (txids).
    pub mined: Vec<String>,
    /// Held or seen by a store, no chain-index answer (txids).
    pub held: Vec<String>,
    /// Nothing decisive (txids).
    pub inconclusive: Vec<String>,
    /// Absent from the chain index while the broadcaster holds it:
    /// `(txid, age in minutes)`.
    pub absent: Vec<(String, i64)>,
    /// Fatal verdict from the broadcaster (txids).
    pub fatal: Vec<String>,
    /// The poison retirements run (or, on a dry run, simulated), in order.
    pub retired: Vec<PoisonReport>,
    /// The locked-input re-checks of this pass.
    pub locked: LockedInputReport,
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

    /// Whether the pass did or found anything worth an info line.
    pub fn is_quiet(&self) -> bool {
        self.probed == 0
            && self.sse_events == 0
            && self.retired.is_empty()
            && self.locked.due == 0
            && self.candidates == 0
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
        let kept: u32 = retired.iter().map(|r| r.kept).sum();
        let invalidated: u32 = retired.iter().map(|r| r.invalidated).sum();
        let invalidated_sats: i64 = retired.iter().map(|r| r.invalidated_sats).sum();
        let internalized: usize = retired.iter().map(|r| r.internalized.len()).sum();
        let climbed: usize = retired.iter().map(|r| r.climbed.len()).sum();
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
            "reconcile-broadcasts{}: sse_events={} candidates={} fresh={} probed={} chain_seen={} chain_mined={} held={} inconclusive={} absent={} fatal={} retired_roots={} retired_txs={} climbed={} restored={} ({} sats) kept_locked={} invalidated={} ({} sats) internalized={} alive={} refused={} locked_due={} locked_restored={} ({} sats) locked_spent={} locked_unknown={}",
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
            climbed,
            restored,
            restored_sats,
            kept,
            invalidated,
            invalidated_sats,
            internalized,
            alive,
            refused,
            self.locked.due,
            self.locked.restored,
            self.locked.restored_sats,
            self.locked.spent,
            self.locked.unknown,
        )
    }
}

/// Whether a candidate of `age_minutes` needs a probe this pass, given its
/// memory rows. Rule 1 of the module docs: past `absence_minutes` only fresh
/// chain-index evidence (`chain|seen` within [`BROADCAST_SEEN_STALE_SECS`])
/// or `chain|mined` exempts it; before that any fresh network-evidence row
/// does. A `chain|mined` row exempts forever.
pub fn needs_probe(
    age_minutes: i64,
    absence_minutes: i64,
    records: &[BroadcastSeenRecord],
    now: DateTime<Utc>,
) -> bool {
    let fresh =
        |r: &BroadcastSeenRecord| (now - r.seen_at).num_seconds() <= BROADCAST_SEEN_STALE_SECS;
    let chain_mined = records.iter().any(|r| {
        r.provider == BROADCAST_PROVIDER_CHAIN && r.ladder_status() == Some(BroadcastStatus::Mined)
    });
    if chain_mined {
        return false;
    }
    if age_minutes >= absence_minutes {
        !records.iter().any(|r| {
            r.provider == BROADCAST_PROVIDER_CHAIN
                && r.ladder_status() == Some(BroadcastStatus::Seen)
                && fresh(r)
        })
    } else {
        !records
            .iter()
            .any(|r| r.ladder_status().is_some_and(|s| s.is_network_evidence()) && fresh(r))
    }
}

/// One pass: SSE drain, probes, poison retirements, locked-input re-checks.
/// See the module docs.
pub async fn run_pass(
    storage: &StorageSqlx,
    services: &dyn WalletServices,
    verifier: &BroadcastVerifier,
    opts: &ReconcileOptions,
) -> Result<ReconcileBroadcastsReport> {
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
    let txids: Vec<String> = candidates.iter().map(|c| c.txid.clone()).collect();
    let records = storage.broadcast_records(None, &txids).await?;
    let now = Utc::now();
    let mut to_probe: Vec<(String, i64)> = Vec::new();
    let mut roots: Vec<(String, &'static str)> = Vec::new();
    for candidate in &candidates {
        let mine: Vec<BroadcastSeenRecord> = records
            .iter()
            .filter(|r| r.txid == candidate.txid)
            .cloned()
            .collect();
        if mine
            .iter()
            .any(|r| r.ladder_status() == Some(BroadcastStatus::Rejected))
        {
            roots.push((candidate.txid.clone(), "rejected memory row"));
            continue;
        }
        let age = candidate.age_minutes(now);
        if !needs_probe(age, opts.absence_minutes, &mine, now) {
            report.fresh += 1;
            continue;
        }
        to_probe.push((candidate.txid.clone(), age));
    }
    for (index, (txid, age)) in to_probe.iter().take(opts.max_probes).enumerate() {
        if index > 0 {
            tokio::time::sleep(PROBE_PACE).await;
        }
        report.probed += 1;
        let presence = verifier.verify_report(txid).await;
        // The broadcaster's (or a peer node's) word is that provider's
        // evidence: good for its reduced sends, recorded under its name.
        if let Some(evidence) = presence.evidence {
            if !matches!(presence.chain_index, ChainIndexAnswer::Present(_)) {
                credit_provider(storage, txid, evidence, presence.evidence_provider).await;
            }
        }
        match classify(&presence) {
            Probe::Chain(evidence) => {
                credit_chain(storage, txid, evidence).await;
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
                note_absence(storage, txid, *age, opts.absence_minutes).await;
                report.absent.push((txid.clone(), *age));
                if *age >= opts.absence_minutes {
                    roots.push((
                        txid.clone(),
                        "absent from the chain index past the threshold",
                    ));
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
            .retire_poisoned_chain_from(
                services,
                &root,
                "invalid",
                opts.execute,
                opts.absence_minutes,
            )
            .await?;
        log_poison(&poison, reason, opts.execute);
        covered.insert(poison.origin.clone());
        covered.extend(poison.climbed.iter().cloned());
        for tx in &poison.chain {
            covered.insert(tx.txid.clone());
        }
        report.retired.push(poison);
    }

    // 4. Kept-locked inputs.
    report.locked = storage
        .recheck_locked_inputs(services, opts.max_locked_checks, opts.execute)
        .await?;

    Ok(report)
}

/// The poisoned-chain sweep and the locked-input re-checks alone (no SSE,
/// no probes): the daemon's ticker and `cleanup-abandoned` call it after
/// their own verdicts.
pub struct SweepReport {
    /// The poison retirements run.
    pub poison: Vec<PoisonReport>,
    /// The locked-input re-checks.
    pub locked: LockedInputReport,
}

/// See [`SweepReport`].
pub async fn run_sweep(
    storage: &StorageSqlx,
    services: &dyn WalletServices,
    execute: bool,
) -> Result<SweepReport> {
    let mut poison = Vec::new();
    let mut covered: HashSet<String> = HashSet::new();
    let mut roots = sweep_roots(storage).await?;
    roots.extend(rejected_roots(storage).await?);
    for root in roots {
        if covered.contains(&root) {
            continue;
        }
        let report = storage
            .retire_poisoned_chain_from(
                services,
                &root,
                "invalid",
                execute,
                absence_minutes_from_env(),
            )
            .await?;
        log_poison(&report, "sweep", execute);
        covered.insert(report.origin.clone());
        covered.extend(report.climbed.iter().cloned());
        for tx in &report.chain {
            covered.insert(tx.txid.clone());
        }
        poison.push(report);
    }
    let locked = storage
        .recheck_locked_inputs(services, DEFAULT_MAX_LOCKED_CHECKS, execute)
        .await?;
    Ok(SweepReport { poison, locked })
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
        max_locked_checks = opts.max_locked_checks,
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
            match run_pass(wallet.storage(), wallet.services(), &verifier, &opts).await {
                Ok(report) => {
                    if report.is_quiet() {
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
    /// The chain index holds it.
    Chain(NetworkEvidence),
    /// The broadcaster reports a fatal verdict.
    Fatal,
    /// The chain index answered 404 while the broadcaster holds or has seen
    /// it, and no peer node vouches for it.
    Absent,
    /// A store holds or has seen it and the chain index gave no answer.
    Held,
    /// Nothing decisive.
    Inconclusive,
}

fn classify(report: &PresenceReport) -> Probe {
    if let ChainIndexAnswer::Present(evidence) = report.chain_index {
        return Probe::Chain(evidence);
    }
    if report.broadcaster_fatal {
        return Probe::Fatal;
    }
    if report.network_absent {
        return Probe::Absent;
    }
    match report.verification {
        BroadcastVerification::Confirmed => Probe::Held,
        _ => Probe::Inconclusive,
    }
}

fn log_poison(poison: &PoisonReport, reason: &str, execute: bool) {
    match &poison.outcome {
        PoisonOutcome::Retired => {
            tracing::warn!(
                root = %poison.root,
                origin = %poison.origin,
                climbed = poison.climbed.len(),
                reason,
                txs = poison.chain.len(),
                restored = poison.restored,
                kept_locked = poison.kept,
                executed = poison.executed,
                "reconcile-broadcasts: poisoned chain {}",
                if execute { "retired" } else { "would be retired" }
            );
        }
        PoisonOutcome::Alive => {
            tracing::info!(root = %poison.root, origin = %poison.origin, reason, "reconcile-broadcasts: root is alive per the status service, kept");
        }
        PoisonOutcome::Refused { proven_txid } => {
            tracing::error!(root = %poison.root, proven = %proven_txid, reason, "reconcile-broadcasts: retire refused, a descendant is proven");
        }
        PoisonOutcome::NotFound => {}
    }
}

/// Record provider-level evidence for `txid` (a broadcaster's or a peer
/// node's word): the ladder row under that provider, and the same status
/// lift a push `SEEN_ON_NETWORK` performs.
async fn credit_provider(
    storage: &StorageSqlx,
    txid: &str,
    evidence: NetworkEvidence,
    provider: &'static str,
) {
    if let Err(e) = storage
        .mark_transaction_seen_on_network_by(txid, provider)
        .await
    {
        tracing::warn!(txid = %txid, error = %e, "could not record the provider's evidence");
    }
    if evidence == NetworkEvidence::Mined {
        if let Err(e) = storage
            .record_broadcast_status(txid, provider, BROADCAST_STATUS_MINED)
            .await
        {
            tracing::warn!(txid = %txid, error = %e, "could not record the provider's mined verdict");
        }
    }
    tracing::debug!(txid = %txid, ?evidence, provider, "provider evidence recorded");
}

/// Record chain evidence for `txid` and `chain|seen` for its unproven
/// parents (presence of the child on the chain index implies the parents
/// connected).
async fn credit_chain(storage: &StorageSqlx, txid: &str, evidence: NetworkEvidence) {
    if let Err(e) = storage
        .mark_transaction_seen_on_network_by(txid, BROADCAST_PROVIDER_CHAIN)
        .await
    {
        tracing::warn!(txid = %txid, error = %e, "could not record the chain presence");
    }
    if evidence == NetworkEvidence::Mined {
        if let Err(e) = storage
            .record_broadcast_status(txid, BROADCAST_PROVIDER_CHAIN, BROADCAST_STATUS_MINED)
            .await
        {
            tracing::warn!(txid = %txid, error = %e, "could not record the mined evidence");
        }
    }
    match unproven_parents(storage, txid).await {
        Ok(parents) if !parents.is_empty() => {
            if let Err(e) = storage
                .sqlx_broadcast_memory()
                .record_broadcast_status_many(
                    BROADCAST_PROVIDER_CHAIN,
                    BROADCAST_STATUS_SEEN,
                    &parents,
                )
                .await
            {
                tracing::warn!(txid = %txid, error = %e, "could not credit the parents");
            }
            tracing::info!(
                txid = %txid,
                ?evidence,
                parents = parents.len(),
                "chain index has it: the transaction and its unproven parents connected"
            );
        }
        Ok(_) => {
            tracing::info!(txid = %txid, ?evidence, "chain index has it");
        }
        Err(e) => tracing::warn!(txid = %txid, error = %e, "could not list the parents"),
    }
}

/// Note a chain-index absence: the `network|unknown` row (its `seen_at` is
/// the first observed absence, for diagnostics) and a log line with the
/// transaction's age against the threshold.
async fn note_absence(storage: &StorageSqlx, txid: &str, age_minutes: i64, threshold: i64) {
    if let Err(e) = storage
        .record_broadcast_status(txid, BROADCAST_PROVIDER_NETWORK, BROADCAST_STATUS_UNKNOWN)
        .await
    {
        tracing::warn!(txid = %txid, error = %e, "could not record the absence");
    }
    if age_minutes >= threshold {
        tracing::warn!(
            txid = %txid,
            age_minutes,
            threshold,
            "absent from the chain index past the threshold while the broadcaster holds it: a phantom"
        );
    } else {
        tracing::info!(
            txid = %txid,
            age_minutes,
            threshold,
            "absent from the chain index (the broadcaster holds it); retired once older than the threshold"
        );
    }
}

/// Parse a timestamp the way the wallet database writes them: RFC 3339
/// (the toolbox binds `DateTime<Utc>`) or SQLite's `CURRENT_TIMESTAMP`.
fn parse_db_timestamp(text: &str) -> DateTime<Utc> {
    let text = text.trim();
    if let Ok(dt) = DateTime::parse_from_rfc3339(text) {
        return dt.with_timezone(&Utc);
    }
    for format in [
        "%Y-%m-%d %H:%M:%S%.f",
        "%Y-%m-%d %H:%M:%S",
        "%Y-%m-%dT%H:%M:%S%.f",
    ] {
        if let Ok(naive) = NaiveDateTime::parse_from_str(text, format) {
            return Utc.from_utc_datetime(&naive);
        }
    }
    Utc.timestamp_opt(0, 0).single().unwrap_or_default()
}

/// `unproven` transactions (and `sending` ones older than ten minutes),
/// oldest first, optionally limited to the last `max_age_hours`.
async fn select_candidates(
    storage: &StorageSqlx,
    max_age_hours: Option<i64>,
) -> Result<Vec<Candidate>> {
    let modifier = max_age_hours.map(|h| format!("-{} hours", h.max(0)));
    let rows = sqlx::query(
        "SELECT txid, CAST(created_at AS TEXT) AS created_at FROM transactions \
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
        .filter_map(|r| {
            let txid: String = r.get("txid");
            let created_at: String = r.get("created_at");
            seen.insert(txid.clone()).then(|| Candidate {
                txid,
                created_at: parse_db_timestamp(&created_at),
            })
        })
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
    use axum::http::StatusCode;
    use axum::routing::get;
    use axum::Router;
    use bsv_wallet_toolbox::services::mock::MockWalletServices;
    use bsv_wallet_toolbox::WalletStorageWriter;
    use std::net::SocketAddr;

    fn rec(txid: &str, provider: &str, status: &str, age_secs: i64) -> BroadcastSeenRecord {
        BroadcastSeenRecord {
            txid: txid.to_string(),
            provider: provider.to_string(),
            status: status.to_string(),
            seen_at: Utc::now() - chrono::Duration::seconds(age_secs),
        }
    }

    #[test]
    fn a_probe_report_classifies_in_evidence_order() {
        let mut r = PresenceReport::from_verification(BroadcastVerification::Confirmed);
        assert_eq!(classify(&r), Probe::Held);
        r.evidence = Some(NetworkEvidence::Seen);
        r.evidence_provider = PROVIDER_ARCADE_V2;
        assert_eq!(
            classify(&r),
            Probe::Held,
            "a broadcaster's seen without a chain answer is held"
        );
        r.network_absent = true;
        r.chain_index = ChainIndexAnswer::Absent;
        assert_eq!(
            classify(&r),
            Probe::Absent,
            "seen by the broadcaster, absent from the chain index"
        );
        r.broadcaster_fatal = true;
        assert_eq!(classify(&r), Probe::Fatal);
        r.chain_index = ChainIndexAnswer::Present(NetworkEvidence::Seen);
        assert_eq!(classify(&r), Probe::Chain(NetworkEvidence::Seen));
        let i = PresenceReport::from_verification(BroadcastVerification::Inconclusive);
        assert_eq!(classify(&i), Probe::Inconclusive);
        let mut rejected = PresenceReport::from_verification(BroadcastVerification::Rejected);
        rejected.network_absent = true;
        assert_eq!(classify(&rejected), Probe::Absent);
    }

    #[test]
    fn only_chain_evidence_exempts_an_old_transaction_from_a_probe() {
        let t = "aa".repeat(32);
        let now = Utc::now();
        let arcade_seen = vec![rec(&t, PROVIDER_ARCADE_V2, BROADCAST_STATUS_SEEN, 5)];
        let chain_seen = vec![rec(&t, BROADCAST_PROVIDER_CHAIN, BROADCAST_STATUS_SEEN, 5)];
        let chain_stale = vec![rec(
            &t,
            BROADCAST_PROVIDER_CHAIN,
            BROADCAST_STATUS_SEEN,
            1_000,
        )];
        let chain_mined = vec![rec(
            &t,
            BROADCAST_PROVIDER_CHAIN,
            BROADCAST_STATUS_MINED,
            90_000,
        )];
        let network_seen = vec![rec(
            &t,
            BROADCAST_PROVIDER_NETWORK,
            BROADCAST_STATUS_SEEN,
            5,
        )];
        // Old (past the threshold): the broadcaster's word does not count.
        assert!(needs_probe(31, 30, &arcade_seen, now));
        assert!(needs_probe(31, 30, &network_seen, now));
        assert!(!needs_probe(31, 30, &chain_seen, now));
        assert!(needs_probe(31, 30, &chain_stale, now));
        assert!(
            !needs_probe(31, 30, &chain_mined, now),
            "mined exempts forever"
        );
        assert!(needs_probe(31, 30, &[], now));
        // Young: any fresh network evidence is provisionally trusted.
        assert!(!needs_probe(5, 30, &arcade_seen, now));
        assert!(!needs_probe(5, 30, &network_seen, now));
        assert!(needs_probe(5, 30, &[], now));
        assert!(needs_probe(5, 30, &chain_stale, now));
    }

    #[tokio::test]
    async fn a_broadcaster_status_refreshed_every_pass_never_becomes_chain_evidence() {
        // The serve loop drains Arcade's SSE stream every minute, and every
        // SEEN frame refreshes the broadcaster's row (`seen_at` = now). For
        // a transaction past the absence threshold that refresh must never
        // exempt it from the chain-index probe, however many passes repeat
        // it (2026-09-03, fleet w2).
        let (storage, _user_id, _basket) = wallet_storage().await;
        let t = "ab".repeat(32);
        for pass in 0..5 {
            storage
                .mark_transaction_seen_on_network_by(&t, PROVIDER_ARCADE_V2)
                .await
                .unwrap();
            storage.mark_transaction_seen_on_network(&t).await.unwrap();
            let records = storage
                .broadcast_records(None, std::slice::from_ref(&t))
                .await
                .unwrap();
            assert_eq!(records.len(), 2, "arcade and network rows, pass {}", pass);
            assert!(
                records
                    .iter()
                    .all(|r| (Utc::now() - r.seen_at).num_seconds() < 5),
                "every row refreshed to now on pass {}",
                pass
            );
            assert!(
                needs_probe(31, 30, &records, Utc::now()),
                "past the threshold the refreshed rows still do not exempt it (pass {})",
                pass
            );
            assert!(
                !needs_probe(5, 30, &records, Utc::now()),
                "a young transaction is provisionally trusted on them"
            );
        }
        // Only the chain index's own row exempts it.
        credit_chain(&storage, &t, NetworkEvidence::Seen).await;
        let records = storage
            .broadcast_records(None, std::slice::from_ref(&t))
            .await
            .unwrap();
        assert!(!needs_probe(31, 30, &records, Utc::now()));
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
        assert!(command.max_locked_checks >= 7);
    }

    #[test]
    fn the_summary_is_one_line() {
        let report = ReconcileBroadcastsReport::default();
        let line = report.summary(false);
        assert!(line.starts_with("reconcile-broadcasts (dry run):"));
        assert!(!line.contains('\n'));
        assert!(report.retired_txids().is_empty());
        assert!(report.is_quiet());
    }

    #[test]
    fn db_timestamps_parse_in_both_forms() {
        let iso = parse_db_timestamp("2026-09-02T23:12:00.123456+00:00");
        let sqlite = parse_db_timestamp("2026-09-02 23:12:00");
        assert_eq!(iso.timestamp(), sqlite.timestamp());
        assert_eq!(parse_db_timestamp("nonsense").timestamp(), 0);
    }

    /// A migrated in-memory wallet with one user and its default basket.
    async fn wallet_storage() -> (StorageSqlx, i64, i64) {
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
        (storage, user.user_id, basket)
    }

    async fn insert_tx(
        storage: &StorageSqlx,
        user_id: i64,
        txid: &str,
        status: &str,
        created_at: &str,
    ) -> i64 {
        sqlx::query(
            "INSERT INTO transactions (user_id, status, reference, is_outgoing, satoshis, version, lock_time, description, txid, raw_tx, created_at, updated_at) \
             VALUES (?, ?, ?, 1, 0, 1, 0, 'd', ?, X'01000000', ?, ?)",
        )
        .bind(user_id)
        .bind(status)
        .bind(&txid[..6])
        .bind(txid)
        .bind(created_at)
        .bind(Utc::now())
        .execute(storage.pool())
        .await
        .unwrap()
        .last_insert_rowid()
    }

    async fn insert_output(
        storage: &StorageSqlx,
        user_id: i64,
        basket: i64,
        tx_row: i64,
        txid: &str,
        spendable: bool,
        spent_by: Option<i64>,
    ) -> i64 {
        let lock = hex::decode("76a914dbc0a7c84983c5bf199b7b2d41b3acf0408ee5aa88ac").unwrap();
        let now = Utc::now();
        sqlx::query(
            "INSERT INTO outputs (user_id, transaction_id, basket_id, vout, satoshis, locking_script, txid, type, spendable, change, spent_by, provided_by, purpose, output_description, created_at, updated_at) \
             VALUES (?, ?, ?, 0, 1000, ?, ?, 'P2PKH', ?, 1, ?, 'storage', 'change', 'c', ?, ?)",
        )
        .bind(user_id)
        .bind(tx_row)
        .bind(basket)
        .bind(&lock)
        .bind(txid)
        .bind(spendable as i64)
        .bind(spent_by)
        .bind(now)
        .bind(now)
        .execute(storage.pool())
        .await
        .unwrap()
        .last_insert_rowid()
    }

    async fn storage_with_chain() -> StorageSqlx {
        let (storage, user_id, basket) = wallet_storage().await;
        let now = Utc::now();
        let mut ids = Vec::new();
        for (txid, status, created) in [
            (
                "aa".repeat(32),
                "failed",
                "2020-01-01T00:00:00+00:00".to_string(),
            ),
            (
                "bb".repeat(32),
                "unproven",
                "2020-01-02T00:00:00+00:00".to_string(),
            ),
            (
                "cc".repeat(32),
                "sending",
                "2020-01-03 00:00:00".to_string(),
            ),
            ("dd".repeat(32), "unproven", now.to_rfc3339()),
        ] {
            ids.push(insert_tx(&storage, user_id, &txid, status, &created).await);
        }
        // aa (failed) -> bb (unproven) -> dd (unproven); cc alone.
        for (tx_row, txid, spent_by) in [
            (ids[0], "aa".repeat(32), Some(ids[1])),
            (ids[1], "bb".repeat(32), Some(ids[3])),
            (ids[3], "dd".repeat(32), None),
        ] {
            insert_output(
                &storage,
                user_id,
                basket,
                tx_row,
                &txid,
                spent_by.is_none(),
                spent_by,
            )
            .await;
        }
        storage
    }

    #[tokio::test]
    async fn candidates_parents_and_sweep_roots_come_from_the_wallet_graph() {
        let storage = storage_with_chain().await;
        let all = select_candidates(&storage, None).await.unwrap();
        let txids: Vec<String> = all.iter().map(|c| c.txid.clone()).collect();
        assert_eq!(
            txids,
            vec!["bb".repeat(32), "cc".repeat(32), "dd".repeat(32)],
            "unproven and stale sending, oldest first"
        );
        assert!(all[0].age_minutes(Utc::now()) > 60 * 24 * 365);
        assert_eq!(all[2].age_minutes(Utc::now()), 0);
        let recent = select_candidates(&storage, Some(24)).await.unwrap();
        assert_eq!(recent.len(), 1);
        assert_eq!(recent[0].txid, "dd".repeat(32));

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
    async fn an_absence_note_keeps_its_first_observation() {
        let storage = storage_with_chain().await;
        let txid = "bb".repeat(32);
        note_absence(&storage, &txid, 5, 30).await;
        let first = storage
            .broadcast_status_of(&txid, BROADCAST_PROVIDER_NETWORK)
            .await
            .unwrap()
            .expect("row")
            .seen_at;
        sqlx::query(
            "UPDATE broadcast_seen SET seen_at = datetime('now', '-31 minutes') WHERE txid = ?",
        )
        .bind(&txid)
        .execute(storage.pool())
        .await
        .unwrap();
        note_absence(&storage, &txid, 40, 30).await;
        let again = storage
            .broadcast_status_of(&txid, BROADCAST_PROVIDER_NETWORK)
            .await
            .unwrap()
            .expect("row");
        assert_eq!(again.status, BROADCAST_STATUS_UNKNOWN);
        assert!(
            again.seen_at < first,
            "the backdated first observation stands"
        );
    }

    /// A local mock answering every status path with `code` and `body`.
    async fn mock_server(code: StatusCode, body: &'static str) -> String {
        let handler = move || async move {
            let mut resp = axum::response::Response::new(axum::body::Body::from(body));
            *resp.status_mut() = code;
            resp.headers_mut().insert(
                reqwest::header::CONTENT_TYPE.as_str(),
                "application/json".parse().unwrap(),
            );
            resp
        };
        let app = Router::new()
            .route("/tx/{txid}", get(handler))
            .route("/v1/tx/{txid}", get(handler))
            .route("/tx/hash/{txid}", get(handler));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr: SocketAddr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(listener, app).await.ok();
        });
        format!("http://{}", addr)
    }

    async fn tx_status(storage: &StorageSqlx, txid: &str) -> String {
        sqlx::query_scalar("SELECT status FROM transactions WHERE txid = ?")
            .bind(txid)
            .fetch_one(storage.pool())
            .await
            .unwrap()
    }

    async fn output_state(storage: &StorageSqlx, id: i64) -> (i64, Option<i64>) {
        sqlx::query_as("SELECT spendable, spent_by FROM outputs WHERE output_id = ?")
            .bind(id)
            .fetch_one(storage.pool())
            .await
            .unwrap()
    }

    /// THE live shape (2026-09-02, w0): the broadcaster still says
    /// SEEN_MULTIPLE_NODES, the chain index says 404. G (on chain) -> X
    /// (unproven, 31 min old) -> C (unproven, 31 min old); Y (unproven, just
    /// created) on its own. X and C are phantoms and retire, G's coin comes
    /// back, Y is too young to judge.
    #[tokio::test]
    async fn a_seen_but_absent_phantom_is_retired_after_the_threshold() {
        let (storage, user_id, basket) = wallet_storage().await;
        let old = (Utc::now() - chrono::Duration::minutes(31)).to_rfc3339();
        let g = insert_tx(&storage, user_id, &"11".repeat(32), "completed", &old).await;
        let x = insert_tx(&storage, user_id, &"22".repeat(32), "unproven", &old).await;
        let c = insert_tx(&storage, user_id, &"33".repeat(32), "unproven", &old).await;
        let _y = insert_tx(
            &storage,
            user_id,
            &"44".repeat(32),
            "unproven",
            &Utc::now().to_rfc3339(),
        )
        .await;
        let g0 = insert_output(
            &storage,
            user_id,
            basket,
            g,
            &"11".repeat(32),
            false,
            Some(x),
        )
        .await;
        let x0 = insert_output(
            &storage,
            user_id,
            basket,
            x,
            &"22".repeat(32),
            false,
            Some(c),
        )
        .await;
        let c0 = insert_output(&storage, user_id, basket, c, &"33".repeat(32), true, None).await;
        // The SSE drain of an earlier pass left the broadcaster's word.
        storage
            .record_broadcast_status(&"22".repeat(32), PROVIDER_ARCADE_V2, BROADCAST_STATUS_SEEN)
            .await
            .unwrap();

        let broadcaster = mock_server(
            StatusCode::OK,
            r#"{"txid":"x","txStatus":"SEEN_MULTIPLE_NODES"}"#,
        )
        .await;
        let chain = mock_server(
            StatusCode::NOT_FOUND,
            r#"{"error":"transaction not found"}"#,
        )
        .await;
        let verifier = BroadcastVerifier::explicit(true, &broadcaster, Some(&chain));
        // The status service knows nothing (not alive); the UTXO lookup
        // vouches for G's coin.
        let services = MockWalletServices::new();
        let opts = ReconcileOptions {
            execute: true,
            max_probes: 20,
            max_age_hours: None,
            absence_minutes: 30,
            max_locked_checks: 20,
            sse: None,
        };

        let report = run_pass(&storage, &services, &verifier, &opts)
            .await
            .unwrap();
        assert_eq!(report.candidates, 3);
        assert_eq!(
            report.fresh, 0,
            "a broadcaster's seen exempts nothing past the threshold"
        );
        assert_eq!(report.probed, 3);
        assert_eq!(report.absent.len(), 3);
        assert!(report.seen.is_empty() && report.fatal.is_empty());
        // One retire from X covers C (its descendant); Y is too young.
        let retired: Vec<&PoisonReport> = report
            .retired
            .iter()
            .filter(|r| r.outcome == PoisonOutcome::Retired)
            .collect();
        assert_eq!(retired.len(), 1, "{:?}", report.retired);
        assert_eq!(retired[0].root, "22".repeat(32));
        assert_eq!(
            retired[0].retirable_txids(),
            vec!["22".repeat(32), "33".repeat(32)]
        );
        assert_eq!(tx_status(&storage, &"22".repeat(32)).await, "failed");
        assert_eq!(tx_status(&storage, &"33".repeat(32)).await, "failed");
        assert_eq!(tx_status(&storage, &"44".repeat(32)).await, "unproven");
        assert_eq!(tx_status(&storage, &"11".repeat(32)).await, "completed");
        assert_eq!(
            output_state(&storage, g0).await,
            (1, None),
            "G's coin is back"
        );
        assert_eq!(output_state(&storage, x0).await.0, 0);
        assert_eq!(output_state(&storage, c0).await.0, 0);
        // Y's absence is on the clock, nothing more.
        let y_row = storage
            .broadcast_status_of(&"44".repeat(32), BROADCAST_PROVIDER_NETWORK)
            .await
            .unwrap()
            .expect("absence row");
        assert_eq!(y_row.status, BROADCAST_STATUS_UNKNOWN);
        // The retired ones are remembered as rejected everywhere.
        let x_arcade = storage
            .broadcast_status_of(&"22".repeat(32), PROVIDER_ARCADE_V2)
            .await
            .unwrap()
            .expect("row");
        assert_eq!(x_arcade.status, "rejected");
        assert!(!report.summary(true).contains('\n'));

        // The next pass: X and C are failed (not candidates), Y still young
        // and absent, nothing to retire, no locked inputs.
        let again = run_pass(&storage, &services, &verifier, &opts)
            .await
            .unwrap();
        assert_eq!(again.candidates, 1);
        assert!(again.retired.is_empty());
        assert_eq!(again.locked.due, 0);
    }

    /// The climb through the reconciler: the verdict lands on the child,
    /// the poison starts at its unproven, absent parent.
    #[tokio::test]
    async fn an_absent_child_retires_from_its_absent_parent() {
        let (storage, user_id, basket) = wallet_storage().await;
        let old = (Utc::now() - chrono::Duration::minutes(45)).to_rfc3339();
        let g = insert_tx(&storage, user_id, &"11".repeat(32), "completed", &old).await;
        let p = insert_tx(&storage, user_id, &"22".repeat(32), "unproven", &old).await;
        let c = insert_tx(&storage, user_id, &"33".repeat(32), "unproven", &old).await;
        let g0 = insert_output(
            &storage,
            user_id,
            basket,
            g,
            &"11".repeat(32),
            false,
            Some(p),
        )
        .await;
        let _p0 = insert_output(
            &storage,
            user_id,
            basket,
            p,
            &"22".repeat(32),
            false,
            Some(c),
        )
        .await;
        let _c0 = insert_output(&storage, user_id, basket, c, &"33".repeat(32), true, None).await;
        // P has fresh chain evidence in the memory from a stale earlier pass
        // (it is not probed this pass); C is probed and absent.
        storage
            .record_broadcast_status(
                &"22".repeat(32),
                BROADCAST_PROVIDER_CHAIN,
                BROADCAST_STATUS_SEEN,
            )
            .await
            .unwrap();
        let broadcaster =
            mock_server(StatusCode::OK, r#"{"txid":"x","txStatus":"RECEIVED"}"#).await;
        let chain = mock_server(
            StatusCode::NOT_FOUND,
            r#"{"error":"transaction not found"}"#,
        )
        .await;
        let verifier = BroadcastVerifier::explicit(true, &broadcaster, Some(&chain));
        let services = MockWalletServices::new();
        let opts = ReconcileOptions {
            execute: true,
            max_probes: 20,
            max_age_hours: None,
            absence_minutes: 30,
            max_locked_checks: 20,
            sse: None,
        };
        let report = run_pass(&storage, &services, &verifier, &opts)
            .await
            .unwrap();
        assert_eq!(report.fresh, 1, "P exempt on fresh chain evidence");
        assert_eq!(report.probed, 1, "C");
        let retired: Vec<&PoisonReport> = report
            .retired
            .iter()
            .filter(|r| r.outcome == PoisonOutcome::Retired)
            .collect();
        assert_eq!(retired.len(), 1);
        assert_eq!(retired[0].origin, "33".repeat(32));
        assert_eq!(
            retired[0].root,
            "22".repeat(32),
            "climbed to the absent parent"
        );
        assert_eq!(retired[0].climbed, vec!["33".repeat(32)]);
        assert_eq!(tx_status(&storage, &"22".repeat(32)).await, "failed");
        assert_eq!(tx_status(&storage, &"33".repeat(32)).await, "failed");
        assert_eq!(output_state(&storage, g0).await, (1, None));
    }
}
