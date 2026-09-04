use anyhow::{Context, Result};
use bsv_wallet_toolbox::Chain;
use sqlx::Row;
use std::future::Future;

use crate::broadcast_reconcile::absence_minutes_from_env;
use crate::broadcast_verify::{
    BroadcastVerification, BroadcastVerifier, ChainIndexAnswer, PresenceReport,
};
use crate::commands::receive;
use crate::context::WalletContext;

/// What the chain says about ONE input outpoint of a candidate transaction
/// (THE RELEASE RULE's per-input primitive, 2026-08-29).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InputSpend {
    /// The outpoint is unspent and its parent is on chain — safe to release.
    Unspent,
    /// The outpoint is spent by `txid`; `confirmed` = that spend is mined.
    SpentBy { txid: String, confirmed: bool },
    /// Cannot say (probe fault, phantom parent) — never released.
    Unknown,
}

/// `proven_tx_reqs` statuses the monitor's proof pass (and the send/no-send
/// tasks) keep polling. A req a retire leaves in one of these keeps the
/// wallet asking every provider about a transaction it already gave up on,
/// every minute, forever (the soak wallet, 2026-06-29..09-04: three reqs).
const POLLED_REQ_STATUSES: &str =
    "'unmined', 'unknown', 'callback', 'sending', 'unconfirmed', 'unsent', 'nosend'";

/// Summary of one reconcile pass over abandoned transactions.
#[derive(Default, Debug, Clone)]
pub struct ReconcileReport {
    /// The absence threshold (minutes) this pass ran with.
    pub absence_minutes: i64,
    /// `unproven` txs inspected (after the min-age filter).
    pub checked: usize,
    /// txids some source still holds (kept spendable).
    pub kept: Vec<String>,
    /// txids whose absence was NOT definitive (a lone index miss, a source
    /// fault, probing disabled) — kept, because an unknown never releases
    /// money. Surfaced so an operator sees what the sweep could not decide.
    pub inconclusive: Vec<String>,
    /// txids absent from the chain index while the broadcaster holds or has
    /// seen them, YOUNGER than the threshold: `(txid, age in minutes)`. Kept:
    /// the absence clock is running.
    pub absent_on_clock: Vec<(String, i64)>,
    /// txids DEFINITIVELY absent (broadcaster + chain index both 404, no
    /// source holding them) — abandoned.
    pub abandoned: Vec<String>,
    /// txids absent from the chain index PAST the threshold while the
    /// broadcaster still holds or has seen them: `(txid, age in minutes)`.
    /// A broadcaster's SEEN is not chain evidence; the absence clock is.
    /// Abandoned like the absent set.
    pub absent_past_threshold: Vec<(String, i64)>,
    /// txids DEFINITIVELY DEAD BY CONFLICT: an input of theirs is chain-spent
    /// by a DIFFERENT, CONFIRMED txid, so the bytes can never mine however
    /// many sources still hold them (the run-B heal's sharp edge: 11
    /// phantom-parent outputs counted as balance behind one "held" tx).
    /// Abandoned like the absent set.
    pub conflicted: Vec<String>,
    /// Proof requests still polled for transactions this wallet already
    /// failed (after the min-age filter): inspected.
    pub stale_reqs_checked: usize,
    /// Those retired (or, on a dry run, to retire): the transaction is
    /// definitively absent, or absent from the chain index past the
    /// threshold. txids.
    pub stale_reqs_retired: Vec<String>,
    /// Those the chain index KNOWS: the wallet's `failed` verdict is the
    /// suspect one, so the req is left to the proof pass and the unfail
    /// path. txids.
    pub stale_reqs_known: Vec<String>,
    /// Those neither absent for certain nor known: kept, asked again next
    /// pass. txids.
    pub stale_reqs_kept: Vec<String>,
    /// Whether `execute` actually applied the cleanup.
    pub applied: bool,
    /// Transactions transitioned `unproven`/`sending` -> `failed`.
    pub failed: u64,
    /// `proven_tx_reqs` rows moved to `invalid` (the failed transactions'
    /// own reqs plus the stale ones).
    pub reqs_retired: u64,
    /// Inputs of abandoned txs VERIFIED unspent and restored to spendable.
    pub restored_count: u64,
    pub restored_sats: u64,
    /// Inputs of abandoned txs chain-spent by another confirmed tx —
    /// RELINQUISHED (unspendable, unlocked): they are gone, never balance.
    pub relinquished_count: u64,
    pub relinquished_sats: u64,
    /// Inputs of abandoned txs the chain could not vouch for — kept LOCKED
    /// (an unknown never releases money; an operator can revisit).
    pub kept_locked_count: u64,
    /// Phantom outputs of abandoned txs invalidated (spendable=0).
    pub phantom_count: u64,
    pub phantom_sats: u64,
}

impl ReconcileReport {
    /// Every transaction this pass abandons (or would): absent, absent past
    /// the threshold, dead by conflict.
    pub fn dead_txids(&self) -> Vec<String> {
        self.abandoned
            .iter()
            .cloned()
            .chain(self.absent_past_threshold.iter().map(|(t, _)| t.clone()))
            .chain(self.conflicted.iter().cloned())
            .collect()
    }

    /// Whether an `--execute` run would write anything.
    pub fn has_work(&self) -> bool {
        !self.abandoned.is_empty()
            || !self.absent_past_threshold.is_empty()
            || !self.conflicted.is_empty()
            || !self.stale_reqs_retired.is_empty()
    }
}

/// Core reconcile, shared by the CLI `cleanup-abandoned` command and the daemon's
/// periodic ticker.
///
/// Scans `status='unproven'` (and ≥10-min-old `status='sending'`) transactions
/// that are at least `min_age_secs` old and decides each on TWO chain
/// questions (THE RELEASE RULE, 2026-08-29):
///
/// 1. **Is any input chain-spent by a DIFFERENT, CONFIRMED tx?** Then the tx is
///    DEFINITIVELY DEAD whatever the presence probe says — a peer's orphan pool
///    "holding" it is holding bytes that can never mine (run B's heal: one
///    kept tx froze 11 phantom-parent outputs as balance). Abandoned.
/// 2. Otherwise, **is the tx present anywhere?** `BroadcastVerifier::single_pass`
///    (the broadcaster it was submitted to, GorillaPool ARC, TAAL ARC and
///    WhatsOnChain), read as a [`PresenceReport`]: the chain index holding it
///    ⇒ kept; DEFINITIVE absence (broadcaster JSON-404 AND index 404) ⇒
///    abandoned; the chain index answering 404 while the broadcaster merely
///    holds or has seen it ⇒ the absence clock: kept while younger than
///    `BROADCAST_ABSENCE_MINUTES` (default 30), abandoned past it. A
///    broadcaster's SEEN is not chain evidence (2026-09-02: Arcade reported
///    `SEEN_MULTIPLE_NODES` for hours for phantoms the chain index never saw,
///    and the daemon's sweep, which runs THIS rule and not the served loop's,
///    kept them as "held"). A lone index miss with the broadcaster silent, a
///    source fault, probing disabled ⇒ `Inconclusive`, kept.
///
/// Abandoning is per-input verified, never blind: an input the chain says is
/// UNSPENT is restored; one spent by another confirmed tx is RELINQUISHED
/// (unspendable, unlocked — it is gone); one the chain cannot vouch for stays
/// LOCKED. The tx's own outputs go unspendable, the tx `failed`, and its
/// `proven_tx_reqs` row `invalid` so the proof pass stops asking about it.
///
/// A second scan covers the proof requests an earlier verdict left behind:
/// reqs still in a polled status for transactions already `failed`. Each is
/// probed the same way, never retired blind: definitive absence (or absence
/// past the threshold) retires the req; a transaction the chain index knows
/// is left alone (the `failed` verdict is the suspect one there).
///
/// Operates on the caller-provided pool so the daemon reuses its existing
/// connection rather than opening a second one (the wallet DB is not in WAL
/// mode and a fresh pool would lack the daemon's `busy_timeout`).
pub async fn reconcile(
    pool: &sqlx::SqlitePool,
    chain: Chain,
    min_age_secs: i64,
    execute: bool,
) -> Result<ReconcileReport> {
    let verifier = BroadcastVerifier::single_pass(chain);
    let client = reqwest::Client::new();
    let base = receive::woc_base(chain);
    reconcile_with(
        pool,
        min_age_secs,
        absence_minutes_from_env(),
        execute,
        |txid: String| {
            let v = verifier.clone();
            async move { v.verify_report(&txid).await }
        },
        |src: String, vout: u32| {
            let c = client.clone();
            async move { probe_input_spend(&c, base, &src, vout).await }
        },
    )
    .await
}

/// The chain's answer for one outpoint through the wallet's existing spend
/// oracle (the same endpoints `sync --reconcile-spent` uses): the spend
/// probe names the spender; a 404 there is `Unspent` ONLY when the parent
/// itself is on chain (a phantom parent is `Unknown` — the 2026-08-27
/// float-recovery lesson); the spender's own record says whether it mined.
pub(crate) async fn probe_input_spend(
    client: &reqwest::Client,
    base: &str,
    src: &str,
    vout: u32,
) -> InputSpend {
    tokio::time::sleep(std::time::Duration::from_millis(350)).await;
    let spent = match client
        .get(format!("{}/tx/{}/{}/spent", base, src, vout))
        .send()
        .await
    {
        Ok(r) if r.status().is_success() => match r.json::<serde_json::Value>().await {
            Ok(v) => v
                .get("txid")
                .and_then(|t| t.as_str())
                .map(|t| t.to_ascii_lowercase()),
            Err(_) => return InputSpend::Unknown,
        },
        Ok(r) if r.status().as_u16() == 404 => None,
        _ => return InputSpend::Unknown,
    };
    match spent {
        Some(spender) => {
            tokio::time::sleep(std::time::Duration::from_millis(350)).await;
            let confirmed = match client
                .get(format!("{}/tx/hash/{}", base, spender))
                .send()
                .await
            {
                Ok(r) if r.status().is_success() => r
                    .json::<serde_json::Value>()
                    .await
                    .ok()
                    .and_then(|v| v.get("confirmations").and_then(|c| c.as_i64()))
                    .is_some_and(|c| c >= 1),
                _ => false,
            };
            InputSpend::SpentBy {
                txid: spender,
                confirmed,
            }
        }
        None => {
            tokio::time::sleep(std::time::Duration::from_millis(350)).await;
            match client.get(format!("{}/tx/hash/{}", base, src)).send().await {
                Ok(r) if r.status().is_success() => InputSpend::Unspent,
                _ => InputSpend::Unknown,
            }
        }
    }
}

/// A candidate to abandon: `(transaction_id, txid, per-input chain answers)`.
type DeadCandidate = (i64, String, Vec<(TrackedInput, InputSpend)>);

/// One tracked input of a candidate tx: the coin it locks and where it came from.
struct TrackedInput {
    output_id: i64,
    satoshis: i64,
    source_txid: String,
    vout: u32,
}

async fn tracked_inputs(pool: &sqlx::SqlitePool, tx_id: i64) -> Result<Vec<TrackedInput>> {
    let rows = sqlx::query(
        "SELECT o.output_id, o.satoshis, o.vout, t.txid AS src \
         FROM outputs o JOIN transactions t ON t.transaction_id = o.transaction_id \
         WHERE o.spent_by = ?",
    )
    .bind(tx_id)
    .fetch_all(pool)
    .await?;
    Ok(rows
        .iter()
        .map(|r| TrackedInput {
            output_id: r.get("output_id"),
            satoshis: r.get::<i64, _>("satoshis"),
            source_txid: r.get::<String, _>("src"),
            vout: r.get::<i64, _>("vout") as u32,
        })
        .collect())
}

/// The pure decision for one candidate given its chain answers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    /// The chain index holds it, or a source holds it and the chain index
    /// gave no answer.
    Kept,
    /// Nothing decisive (a probe fault, probing disabled).
    Inconclusive,
    /// Absent from the chain index while the broadcaster holds or has seen
    /// it, younger than the threshold: kept, the clock is running.
    AbsentOnClock,
    /// Definitive absence: the broadcaster answered JSON-404 (or a fatal
    /// verdict) AND the chain index answered 404.
    DeadAbsent,
    /// Absent from the chain index past the threshold while the broadcaster
    /// still holds or has seen it: a phantom the broadcaster never let go of.
    DeadAbsentPastThreshold,
    /// An input is chain-spent by a DIFFERENT, CONFIRMED tx.
    DeadConflict,
}

impl Verdict {
    /// Whether this verdict abandons the transaction.
    pub fn is_dead(self) -> bool {
        matches!(
            self,
            Verdict::DeadAbsent | Verdict::DeadAbsentPastThreshold | Verdict::DeadConflict
        )
    }
}

/// The presence half of the rule, for a transaction `age_minutes` old.
///
/// The chain index's own answer settles it when it has one. Otherwise the
/// verifier's `Rejected` (broadcaster JSON-404 or fatal AND chain index 404)
/// is definitive absence, and `network_absent` (chain index 404 while the
/// broadcaster holds or has seen it, no peer node vouching) is the absence
/// clock: past `absence_minutes` it is a verdict, before that a wait. A
/// held transaction with no chain-index answer at all (the index was
/// unreachable) is kept; nothing else decides anything.
pub fn presence_verdict(
    presence: &PresenceReport,
    age_minutes: i64,
    absence_minutes: i64,
) -> Verdict {
    if matches!(presence.chain_index, ChainIndexAnswer::Present(_)) {
        return Verdict::Kept;
    }
    match presence.verification {
        BroadcastVerification::Rejected => Verdict::DeadAbsent,
        _ if presence.network_absent => {
            if age_minutes >= absence_minutes {
                Verdict::DeadAbsentPastThreshold
            } else {
                Verdict::AbsentOnClock
            }
        }
        BroadcastVerification::Confirmed => Verdict::Kept,
        BroadcastVerification::Inconclusive => Verdict::Inconclusive,
    }
}

/// THE two-question rule, as a value: a confirmed spend of any input by a
/// DIFFERENT txid is dead however held; otherwise [`presence_verdict`]
/// decides.
pub fn verdict_for(
    our_txid: &str,
    inputs: &[InputSpend],
    presence: &PresenceReport,
    age_minutes: i64,
    absence_minutes: i64,
) -> Verdict {
    if has_conflict(our_txid, inputs) {
        return Verdict::DeadConflict;
    }
    presence_verdict(presence, age_minutes, absence_minutes)
}

fn has_conflict(our_txid: &str, inputs: &[InputSpend]) -> bool {
    inputs.iter().any(|i| {
        matches!(i, InputSpend::SpentBy { txid, confirmed: true }
                     if !txid.eq_ignore_ascii_case(our_txid))
    })
}

/// [`reconcile`] with both chain probes injected — the seam the cells drive
/// (the real probes hit four networks; the RULE is what is under test).
pub async fn reconcile_with<P, PF, I, IF>(
    pool: &sqlx::SqlitePool,
    min_age_secs: i64,
    absence_minutes: i64,
    execute: bool,
    probe: P,
    input_spend: I,
) -> Result<ReconcileReport>
where
    P: Fn(String) -> PF,
    PF: Future<Output = PresenceReport>,
    I: Fn(String, u32) -> IF,
    IF: Future<Output = InputSpend>,
{
    let mut report = ReconcileReport {
        absence_minutes,
        ..ReconcileReport::default()
    };
    let rows = select_stale_unproven(pool, min_age_secs).await?;
    report.checked = rows.len();

    // (tx_id, txid, per-input answers) for every tx to abandon.
    let mut dead: Vec<DeadCandidate> = Vec::new();
    for row in &rows {
        let tx_id: i64 = row.get("transaction_id");
        let txid: String = row.get("txid");
        let age_minutes: i64 = row.get("age_minutes");
        let inputs = tracked_inputs(pool, tx_id).await?;
        let mut answers: Vec<(TrackedInput, InputSpend)> = Vec::with_capacity(inputs.len());
        for i in inputs {
            let a = input_spend(i.source_txid.clone(), i.vout).await;
            answers.push((i, a));
        }
        let spends: Vec<InputSpend> = answers.iter().map(|(_, a)| a.clone()).collect();
        // The conflict question is decided from the inputs alone; the
        // presence probe is only asked when no input says "dead".
        let presence = if has_conflict(&txid, &spends) {
            PresenceReport::from_verification(BroadcastVerification::Inconclusive)
        } else {
            probe(txid.clone()).await
        };
        let verdict = verdict_for(&txid, &spends, &presence, age_minutes, absence_minutes);
        match verdict {
            Verdict::Kept => report.kept.push(txid.clone()),
            Verdict::Inconclusive => report.inconclusive.push(txid.clone()),
            Verdict::AbsentOnClock => report.absent_on_clock.push((txid.clone(), age_minutes)),
            Verdict::DeadAbsent => report.abandoned.push(txid.clone()),
            Verdict::DeadAbsentPastThreshold => report
                .absent_past_threshold
                .push((txid.clone(), age_minutes)),
            Verdict::DeadConflict => report.conflicted.push(txid.clone()),
        }
        if verdict.is_dead() {
            dead.push((tx_id, txid, answers));
        }
    }

    // Proof requests an earlier verdict left polled: (req id, txid, status).
    let stale = select_stale_reqs(pool, min_age_secs).await?;
    report.stale_reqs_checked = stale.len();
    let mut retire_reqs: Vec<(i64, String)> = Vec::new();
    for row in &stale {
        let req_id: i64 = row.get("proven_tx_req_id");
        let txid: String = row.get("txid");
        let req_status: String = row.get("req_status");
        let age_minutes: i64 = row.get("age_minutes");
        let presence = probe(txid.clone()).await;
        match presence_verdict(&presence, age_minutes, absence_minutes) {
            Verdict::DeadAbsent | Verdict::DeadAbsentPastThreshold => {
                report.stale_reqs_retired.push(txid);
                retire_reqs.push((req_id, req_status));
            }
            Verdict::Kept if matches!(presence.chain_index, ChainIndexAnswer::Present(_)) => {
                report.stale_reqs_known.push(txid);
            }
            _ => report.stale_reqs_kept.push(txid),
        }
    }

    if !execute || (dead.is_empty() && retire_reqs.is_empty()) {
        return Ok(report);
    }

    if !dead.is_empty() {
        let mut tx = pool.begin().await?;
        for (tx_id, our_txid, answers) in &dead {
            for (input, answer) in answers {
                match answer {
                    InputSpend::Unspent => {
                        sqlx::query(
                            "UPDATE outputs SET spendable = 1, spent_by = NULL, \
                             updated_at = CURRENT_TIMESTAMP WHERE output_id = ? AND spent_by = ?",
                        )
                        .bind(input.output_id)
                        .bind(tx_id)
                        .execute(&mut *tx)
                        .await?;
                        report.restored_count += 1;
                        report.restored_sats += input.satoshis.max(0) as u64;
                    }
                    InputSpend::SpentBy {
                        txid,
                        confirmed: true,
                    } if !txid.eq_ignore_ascii_case(our_txid) => {
                        sqlx::query(
                            "UPDATE outputs SET spendable = 0, spent_by = NULL, \
                             updated_at = CURRENT_TIMESTAMP WHERE output_id = ? AND spent_by = ?",
                        )
                        .bind(input.output_id)
                        .bind(tx_id)
                        .execute(&mut *tx)
                        .await?;
                        report.relinquished_count += 1;
                        report.relinquished_sats += input.satoshis.max(0) as u64;
                    }
                    _ => {
                        // Unknown, or a spend the chain would not vouch for as
                        // confirmed: stays LOCKED by the failed tx.
                        report.kept_locked_count += 1;
                    }
                }
            }
        }
        tx.commit().await.context("commit per-input release")?;
        let ids: Vec<i64> = dead.iter().map(|(id, _, _)| *id).collect();
        let phantoms = remove_phantom_outputs(pool, &ids).await?;
        let targets: Vec<(i64, String)> = dead
            .iter()
            .map(|(id, txid, _)| (*id, txid.clone()))
            .collect();
        let (failed, reqs) = mark_failed(pool, &targets).await?;
        report.failed = failed;
        report.reqs_retired += reqs;
        report.phantom_count = phantoms.0;
        report.phantom_sats = phantoms.1;
    }
    report.reqs_retired += retire_stale_reqs(pool, &retire_reqs).await?;
    report.applied = true;
    Ok(report)
}

/// SQL for a row's age in whole minutes, tolerant of both timestamp forms
/// the wallet writes (see [`select_stale_unproven`]).
const AGE_MINUTES_SQL: &str =
    "CAST((julianday('now') - julianday(datetime(created_at))) * 1440 AS INTEGER)";

/// `unproven` transactions at least `min_age_secs` old, with their age.
///
/// `created_at` is written by the toolbox as ISO-8601 (`…T…+00:00`) but by
/// this module's own UPDATEs as `CURRENT_TIMESTAMP` (space-separated), and a
/// bare string comparison against `datetime('now')` never matches the ISO form
/// (`'T' > ' '`), which silently disabled this sweep for every toolbox-written
/// row. `datetime(created_at)` normalizes BOTH forms (and applies the +00:00
/// offset) before comparing.
///
/// `min_age_secs == 0` -> `'-0 seconds'` == now, i.e. no effective age guard
/// (the CLI command's historical behavior); the guard exists so a freshly-
/// broadcast tx that hasn't propagated to WoC yet is never mis-classified.
async fn select_stale_unproven(
    pool: &sqlx::SqlitePool,
    min_age_secs: i64,
) -> Result<Vec<sqlx::sqlite::SqliteRow>> {
    let age_modifier = format!("-{} seconds", min_age_secs.max(0));
    // 'sending' rows too (2026-08-27, the p3 float-recovery find): a broadcast
    // that never resolved sits at status='sending' FOREVER — same phantom
    // outputs, same frozen coin selection, invisible to the 'unproven' filter
    // (12,101 sats of a fleet wallet were stranded behind exactly one). A
    // genuinely in-flight tx must never be failed, so 'sending' rows carry
    // their OWN floor — at least 10 minutes old — regardless of the caller's
    // min_age (the CLI passes 0). WoC-404 remains the only abandonment
    // verdict either way.
    Ok(sqlx::query(&format!(
        "SELECT transaction_id, txid, {AGE_MINUTES_SQL} AS age_minutes FROM transactions \
         WHERE (status='unproven' AND datetime(created_at) <= datetime('now', ?)) \
            OR (status='sending' AND datetime(created_at) <= datetime('now', ?) \
                AND datetime(created_at) <= datetime('now', '-600 seconds'))"
    ))
    .bind(&age_modifier)
    .bind(&age_modifier)
    .fetch_all(pool)
    .await?)
}

/// Proof requests still in a polled status for transactions this wallet
/// already `failed`, at least `min_age_secs` old (the transaction's age):
/// `(proven_tx_req_id, txid, req_status, age_minutes)`.
///
/// The soak wallet's shape (2026-09-04): three transactions this sweep failed
/// on 2026-06-29 kept their reqs at `unmined`, because `mark_failed` never
/// touched `proven_tx_reqs`, and the monitor asked Arcade, WhatsOnChain and
/// Bitails about all three every minute for 67 days. The sweep itself never
/// saw them again: its candidates are `unproven`/`sending` rows.
async fn select_stale_reqs(
    pool: &sqlx::SqlitePool,
    min_age_secs: i64,
) -> Result<Vec<sqlx::sqlite::SqliteRow>> {
    let age_modifier = format!("-{} seconds", min_age_secs.max(0));
    let age = AGE_MINUTES_SQL.replace("created_at", "t.created_at");
    Ok(sqlx::query(&format!(
        "SELECT r.proven_tx_req_id, r.txid, r.status AS req_status, {age} AS age_minutes \
         FROM proven_tx_reqs r JOIN transactions t ON t.txid = r.txid \
         WHERE t.status = 'failed' AND r.status IN ({POLLED_REQ_STATUSES}) \
           AND datetime(t.created_at) <= datetime('now', ?) \
         ORDER BY r.proven_tx_req_id ASC"
    ))
    .bind(&age_modifier)
    .fetch_all(pool)
    .await?)
}

pub async fn run(ctx: &WalletContext, db_path: &str, execute: bool) -> Result<()> {
    let pool = sqlx::SqlitePool::connect(&format!("sqlite:{}", db_path))
        .await
        .with_context(|| format!("failed to open {} (is the daemon running?)", db_path))?;

    // Operator-initiated: no age guard (inspect every unproven tx).
    let report = reconcile(&pool, ctx.chain, 0, execute).await?;

    if report.checked == 0 && report.stale_reqs_checked == 0 {
        println!("No unproven (or stale sending) transactions found, and no stale proof requests.");
        return Ok(());
    }
    println!(
        "Found {} unproven/stale-sending transaction(s); probed every broadcast source (absence threshold {} min).",
        report.checked, report.absence_minutes
    );
    println!(
        "  Definitively absent: {}    Absent past the threshold: {}    Dead by conflict: {}    Held by a source: {}    On the clock: {}    Undecidable (kept): {}",
        report.abandoned.len(),
        report.absent_past_threshold.len(),
        report.conflicted.len(),
        report.kept.len(),
        report.absent_on_clock.len(),
        report.inconclusive.len()
    );
    for txid in &report.kept {
        println!("    keep: {}", txid);
    }
    for (txid, age) in &report.absent_on_clock {
        println!(
            "    keep (absent from the chain index for {} min while the broadcaster holds it; retired once past {} min): {}",
            age, report.absence_minutes, txid
        );
    }
    for txid in &report.inconclusive {
        println!(
            "    keep (inconclusive — an unknown never releases money): {}",
            txid
        );
    }
    for txid in &report.abandoned {
        println!("    fail (absent everywhere): {}", txid);
    }
    for (txid, age) in &report.absent_past_threshold {
        println!(
            "    fail (absent from the chain index for {} min, past the {}-min threshold; a broadcaster's SEEN is not chain evidence): {}",
            age, report.absence_minutes, txid
        );
    }
    for txid in &report.conflicted {
        println!(
            "    fail (an input is chain-spent by another confirmed tx — dead however held): {}",
            txid
        );
    }

    if report.stale_reqs_checked > 0 {
        println!(
            "Proof requests still polled for {} failed transaction(s): retire {}, chain index knows {}, undecided {}.",
            report.stale_reqs_checked,
            report.stale_reqs_retired.len(),
            report.stale_reqs_known.len(),
            report.stale_reqs_kept.len()
        );
        for txid in &report.stale_reqs_retired {
            println!("    retire proof request (absent everywhere): {}", txid);
        }
        for txid in &report.stale_reqs_known {
            println!(
                "    keep proof request (the chain index KNOWS this failed transaction; left to the proof pass and the unfail path): {}",
                txid
            );
        }
        for txid in &report.stale_reqs_kept {
            println!(
                "    keep proof request (undecided, asked again next pass): {}",
                txid
            );
        }
    }

    if !report.has_work() {
        println!("Nothing to clean up.");
        return Ok(());
    }

    // Everything this wallet built on an abandoned transaction is a phantom
    // too (the poisoned chain, 2026-09-02): the toolbox walks the unproven
    // descendants under THE RELEASE RULE. On a dry run it only lists them.
    let mut descendants_retired = 0usize;
    for txid in report.dead_txids() {
        let poison = ctx
            .wallet
            .storage()
            .retire_poisoned_chain_from(
                ctx.wallet.services(),
                &txid,
                "invalid",
                execute,
                report.absence_minutes,
            )
            .await?;
        let descendants: Vec<_> = poison.chain.iter().filter(|t| t.depth > 0).collect();
        if descendants.is_empty() {
            continue;
        }
        println!(
            "    {} unproven descendant(s) of {} {}:",
            descendants.len(),
            txid,
            if execute {
                "retired"
            } else {
                "would be retired"
            }
        );
        for tx in &descendants {
            println!(
                "        depth {} {} ({}{})",
                tx.depth,
                tx.txid,
                tx.status,
                if tx.is_outgoing { "" } else { ", received" }
            );
        }
        if poison.executed {
            descendants_retired += poison.retirable_txids().len();
            for p in &poison.internalized {
                println!(
                    "        internalized payment {}:{} ({} sats) traces to a phantom source: unspendable",
                    p.txid, p.vout, p.satoshis
                );
            }
        }
    }

    if !execute {
        println!();
        println!("Dry run. Re-run with --execute to apply.");
        return Ok(());
    }

    println!();
    println!("Applied:");
    if descendants_retired > 0 {
        println!("  Descendant transactions retired: {}", descendants_retired);
    }
    println!("  Transactions marked failed: {}", report.failed);
    println!(
        "  Proof requests retired (invalid): {} ({} of them for transactions failed earlier)",
        report.reqs_retired,
        report.stale_reqs_retired.len()
    );
    println!(
        "  Inputs restored to spendable (verified unspent): {} ({} sats)",
        report.restored_count, report.restored_sats
    );
    println!(
        "  Inputs relinquished (chain-spent by another tx): {} ({} sats)    Inputs kept locked (unknown): {}",
        report.relinquished_count, report.relinquished_sats, report.kept_locked_count
    );
    println!(
        "  Phantom outputs unspendable: {} ({} sats)",
        report.phantom_count, report.phantom_sats
    );
    println!();
    println!(
        "Net balance delta: {:+} sats. Restart the daemon to refresh its in-memory view.",
        report.restored_sats as i64 - report.phantom_sats as i64 - report.relinquished_sats as i64
    );

    Ok(())
}

async fn remove_phantom_outputs(pool: &sqlx::SqlitePool, ids: &[i64]) -> Result<(u64, u64)> {
    let mut count = 0u64;
    let mut sats = 0u64;
    let mut tx = pool.begin().await?;
    for id in ids {
        let outs = sqlx::query(
            "SELECT output_id, satoshis FROM outputs \
             WHERE transaction_id = ? AND spendable = 1",
        )
        .bind(id)
        .fetch_all(&mut *tx)
        .await?;
        for r in &outs {
            count += 1;
            sats += r.get::<i64, _>("satoshis") as u64;
        }
        sqlx::query(
            "UPDATE outputs SET spendable = 0, updated_at = CURRENT_TIMESTAMP \
             WHERE transaction_id = ?",
        )
        .bind(id)
        .execute(&mut *tx)
        .await?;
    }
    tx.commit().await?;
    Ok((count, sats))
}

/// Fail the transactions and retire their proof requests: `(transactions
/// failed, reqs retired)`. The req goes to `invalid` (the toolbox's status
/// for a phantom) so the monitor's proof pass stops asking every provider
/// about it every minute; the unfail canary still re-verifies `invalid`
/// reqs of failed transactions against the chain with backoff, so a wrong
/// verdict here is recoverable.
async fn mark_failed(pool: &sqlx::SqlitePool, targets: &[(i64, String)]) -> Result<(u64, u64)> {
    let mut tx = pool.begin().await?;
    let mut failed = 0u64;
    let mut reqs = 0u64;
    for (id, txid) in targets {
        let res = sqlx::query(
            "UPDATE transactions SET status = 'failed', \
             updated_at = CURRENT_TIMESTAMP WHERE transaction_id = ? AND status IN ('unproven', 'sending')",
        )
        .bind(id)
        .execute(&mut *tx)
        .await?;
        failed += res.rows_affected();
        let res = sqlx::query(&format!(
            "UPDATE proven_tx_reqs SET status = 'invalid', attempts = attempts + 1, \
             updated_at = CURRENT_TIMESTAMP WHERE txid = ? AND status IN ({POLLED_REQ_STATUSES})"
        ))
        .bind(txid)
        .execute(&mut *tx)
        .await?;
        reqs += res.rows_affected();
    }
    tx.commit().await?;
    Ok((failed, reqs))
}

/// Retire stale proof requests: each `(proven_tx_req_id, status seen)` goes
/// to `invalid` only if it is still at the status the pass saw (a proof
/// that landed meanwhile wins).
async fn retire_stale_reqs(pool: &sqlx::SqlitePool, reqs: &[(i64, String)]) -> Result<u64> {
    let mut count = 0u64;
    let mut tx = pool.begin().await?;
    for (id, status) in reqs {
        let res = sqlx::query(
            "UPDATE proven_tx_reqs SET status = 'invalid', attempts = attempts + 1, \
             updated_at = CURRENT_TIMESTAMP WHERE proven_tx_req_id = ? AND status = ?",
        )
        .bind(id)
        .bind(status)
        .execute(&mut *tx)
        .await?;
        count += res.rows_affected();
    }
    tx.commit().await?;
    Ok(count)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::broadcast_verify::NetworkEvidence;
    use bsv_wallet_toolbox::{BROADCAST_PROVIDER_CHAIN, PROVIDER_ARCADE_V2};
    use sqlx::Row;

    async fn mem_pool_with(rows: &[(&str, &str, &str)]) -> sqlx::SqlitePool {
        let pool = sqlx::SqlitePool::connect("sqlite::memory:").await.unwrap();
        sqlx::query(
            "CREATE TABLE transactions (
                transaction_id INTEGER PRIMARY KEY AUTOINCREMENT,
                txid TEXT, status TEXT NOT NULL, created_at TEXT NOT NULL,
                updated_at TEXT)",
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "CREATE TABLE outputs (
                output_id INTEGER PRIMARY KEY AUTOINCREMENT,
                transaction_id INTEGER NOT NULL, spendable INTEGER NOT NULL DEFAULT 0,
                spent_by INTEGER, satoshis INTEGER NOT NULL DEFAULT 0, vout INTEGER NOT NULL DEFAULT 0,
                updated_at TEXT)",
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "CREATE TABLE proven_tx_reqs (
                proven_tx_req_id INTEGER PRIMARY KEY AUTOINCREMENT,
                status TEXT NOT NULL DEFAULT 'unknown', attempts INTEGER NOT NULL DEFAULT 0,
                txid TEXT NOT NULL UNIQUE, updated_at TEXT)",
        )
        .execute(&pool)
        .await
        .unwrap();
        for (txid, status, created_at) in rows {
            sqlx::query("INSERT INTO transactions (txid, status, created_at) VALUES (?,?,?)")
                .bind(txid)
                .bind(status)
                .bind(created_at)
                .execute(&pool)
                .await
                .unwrap();
        }
        pool
    }

    async fn insert_req(pool: &sqlx::SqlitePool, txid: &str, status: &str, attempts: i64) {
        sqlx::query(
            "INSERT INTO proven_tx_reqs (txid, status, attempts, updated_at) VALUES (?,?,?,'2026-06-29T15:43:34.936649+00:00')",
        )
        .bind(txid)
        .bind(status)
        .bind(attempts)
        .execute(pool)
        .await
        .unwrap();
    }

    async fn req_state(pool: &sqlx::SqlitePool, txid: &str) -> (String, i64) {
        sqlx::query_as("SELECT status, attempts FROM proven_tx_reqs WHERE txid = ?")
            .bind(txid)
            .fetch_one(pool)
            .await
            .unwrap()
    }

    async fn tx_state(pool: &sqlx::SqlitePool, id: i64) -> (String, Option<String>) {
        sqlx::query_as("SELECT status, updated_at FROM transactions WHERE transaction_id = ?")
            .bind(id)
            .fetch_one(pool)
            .await
            .unwrap()
    }

    fn presence(verification: BroadcastVerification) -> PresenceReport {
        PresenceReport::from_verification(verification)
    }

    /// The chain index holds it (mined): the answer for everyone.
    fn chain_mined() -> PresenceReport {
        PresenceReport {
            verification: BroadcastVerification::Confirmed,
            evidence: Some(NetworkEvidence::Mined),
            evidence_provider: BROADCAST_PROVIDER_CHAIN,
            chain_index: ChainIndexAnswer::Present(NetworkEvidence::Mined),
            broadcaster_fatal: false,
            network_absent: false,
        }
    }

    /// The broadcaster says SEEN_MULTIPLE_NODES (or holds it), the chain
    /// index says 404, no peer node vouches: `network_absent`.
    fn seen_by_arcade_absent_from_chain() -> PresenceReport {
        PresenceReport {
            verification: BroadcastVerification::Confirmed,
            evidence: Some(NetworkEvidence::Seen),
            evidence_provider: PROVIDER_ARCADE_V2,
            chain_index: ChainIndexAnswer::Absent,
            broadcaster_fatal: false,
            network_absent: true,
        }
    }

    /// Broadcaster JSON-404 AND chain index 404: definitive absence.
    fn absent_everywhere() -> PresenceReport {
        PresenceReport {
            verification: BroadcastVerification::Rejected,
            evidence: None,
            evidence_provider: bsv_wallet_toolbox::BROADCAST_PROVIDER_NETWORK,
            chain_index: ChainIndexAnswer::Absent,
            broadcaster_fatal: false,
            network_absent: true,
        }
    }

    /// The regression: toolbox-written rows use ISO-8601 `…T…+00:00`, which a
    /// bare string compare against `datetime('now')` NEVER matches ('T' > ' ')
    /// (the sweep silently found nothing, forever). Both formats must match,
    /// and the age comes out of both.
    #[tokio::test]
    async fn selects_iso8601_and_space_format_rows() {
        let pool = mem_pool_with(&[
            (
                "aa".repeat(32).leak(),
                "unproven",
                "2020-01-01T00:00:00.123456+00:00",
            ),
            ("bb".repeat(32).leak(), "unproven", "2020-01-02 00:00:00"),
            (
                "cc".repeat(32).leak(),
                "failed",
                "2020-01-01T00:00:00+00:00",
            ), // wrong status
        ])
        .await;
        let rows = select_stale_unproven(&pool, 0).await.unwrap();
        let txids: Vec<String> = rows.iter().map(|r| r.get("txid")).collect();
        assert_eq!(
            txids.len(),
            2,
            "both timestamp formats must be swept: {txids:?}"
        );
        assert!(txids.contains(&"aa".repeat(32)));
        assert!(txids.contains(&"bb".repeat(32)));
        for row in &rows {
            let age: i64 = row.get("age_minutes");
            assert!(age > 60 * 24 * 365 * 5, "years old in minutes: {age}");
        }
    }

    /// The min-age guard still filters: a just-created row (either format)
    /// must NOT be selected with a 5-minute guard, and a 0-second guard is
    /// the no-guard historical behavior.
    #[tokio::test]
    async fn min_age_guard_respected_across_formats() {
        // "now" in both formats via SQLite itself
        let pool = mem_pool_with(&[]).await;
        sqlx::query(
            "INSERT INTO transactions (txid, status, created_at) VALUES
             ('fresh_iso', 'unproven', strftime('%Y-%m-%dT%H:%M:%f+00:00','now')),
             ('fresh_sp',  'unproven', datetime('now')),
             ('old_iso',   'unproven', '2020-01-01T00:00:00+00:00')",
        )
        .execute(&pool)
        .await
        .unwrap();
        let guarded = select_stale_unproven(&pool, 300).await.unwrap();
        let txids: Vec<String> = guarded.iter().map(|r| r.get("txid")).collect();
        assert_eq!(
            txids,
            vec!["old_iso".to_string()],
            "fresh rows must be age-guarded"
        );
        let unguarded = select_stale_unproven(&pool, 0).await.unwrap();
        assert_eq!(unguarded.len(), 3, "0-second guard selects everything");
        let fresh_age: i64 = unguarded
            .iter()
            .find(|r| r.get::<String, _>("txid") == "fresh_iso")
            .unwrap()
            .get("age_minutes");
        assert_eq!(fresh_age, 0);
    }

    // ── THE RELEASE RULE (2026-08-29) — the verdict is corroborated absence ──

    /// Seed one stale `unproven` tx (id 1, txid `ab…`) whose input is output 1
    /// (coin of parent tx 99 `cd…`:0, locked: spent_by = 1) and whose own
    /// phantom change is output 2, with its `unmined` proof request.
    async fn rule_fixture() -> sqlx::SqlitePool {
        rule_fixture_aged("2020-01-01T00:00:00+00:00").await
    }

    async fn rule_fixture_aged(created_at: &str) -> sqlx::SqlitePool {
        let pool = mem_pool_with(&[("ab".repeat(32).leak(), "unproven", created_at)]).await;
        sqlx::query(
            "INSERT INTO transactions (transaction_id, txid, status, created_at) VALUES (99, ?, 'completed', '2019-12-31T00:00:00+00:00')",
        )
        .bind("cd".repeat(32))
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO outputs (transaction_id, spendable, spent_by, satoshis) VALUES
             (99, 0, 1, 5000),
             (1, 1, NULL, 4000)",
        )
        .execute(&pool)
        .await
        .unwrap();
        insert_req(&pool, &"ab".repeat(32), "unmined", 1).await;
        pool
    }

    async fn lock(pool: &sqlx::SqlitePool, output_id: i64) -> (i64, Option<i64>) {
        sqlx::query_as("SELECT spendable, spent_by FROM outputs WHERE output_id = ?")
            .bind(output_id)
            .fetch_one(pool)
            .await
            .unwrap()
    }

    /// RED→GREEN: the old verdict was a single WoC 404 — exactly what a fresh
    /// Arcade/GorillaPool-only tx looks like for minutes. An INCONCLUSIVE
    /// probe must keep the tx and release NOTHING, even under --execute.
    #[tokio::test]
    async fn a_lone_index_miss_is_inconclusive_and_releases_nothing() {
        let pool = rule_fixture().await;
        let report = reconcile_with(
            &pool,
            0,
            30,
            true,
            |_txid| async { presence(BroadcastVerification::Inconclusive) },
            |_src, _vout| async { InputSpend::Unspent },
        )
        .await
        .unwrap();
        assert_eq!(report.checked, 1);
        assert_eq!(report.inconclusive.len(), 1);
        assert!(report.abandoned.is_empty());
        assert!(!report.applied, "nothing to apply");
        assert_eq!(lock(&pool, 1).await, (0, Some(1)), "the input stays locked");
        assert_eq!(lock(&pool, 2).await.0, 1, "its change untouched");
        assert_eq!(tx_state(&pool, 1).await.0, "unproven");
        assert_eq!(req_state(&pool, &"ab".repeat(32)).await.0, "unmined");
    }

    /// A tx ANY source still holds, with no chain-index answer, is kept.
    #[tokio::test]
    async fn a_held_tx_is_kept() {
        let pool = rule_fixture().await;
        let report = reconcile_with(
            &pool,
            0,
            30,
            true,
            |_txid| async { presence(BroadcastVerification::Confirmed) },
            |_src, _vout| async { InputSpend::Unspent },
        )
        .await
        .unwrap();
        assert_eq!(report.kept.len(), 1);
        assert!(report.abandoned.is_empty() && report.inconclusive.is_empty());
        assert_eq!(lock(&pool, 1).await, (0, Some(1)));
    }

    /// DEFINITIVE absence (broadcaster JSON-404 + index 404, nothing holding
    /// it) is the ONE verdict that abandons: inputs restored, phantom change
    /// invalidated, tx failed, its proof request retired, and only under
    /// --execute.
    #[tokio::test]
    async fn a_definitive_absence_abandons_and_restores_under_execute() {
        let pool = rule_fixture().await;
        let dry = reconcile_with(
            &pool,
            0,
            30,
            false,
            |_txid| async { absent_everywhere() },
            |_src, _vout| async { InputSpend::Unspent },
        )
        .await
        .unwrap();
        assert_eq!(dry.abandoned.len(), 1);
        assert!(!dry.applied);
        assert_eq!(
            lock(&pool, 1).await,
            (0, Some(1)),
            "dry run touches nothing"
        );
        assert_eq!(
            req_state(&pool, &"ab".repeat(32)).await,
            ("unmined".into(), 1)
        );

        let wet = reconcile_with(
            &pool,
            0,
            30,
            true,
            |_txid| async { absent_everywhere() },
            |_src, _vout| async { InputSpend::Unspent },
        )
        .await
        .unwrap();
        assert!(wet.applied);
        assert_eq!(
            (wet.failed, wet.restored_count, wet.restored_sats),
            (1, 1, 5000)
        );
        assert_eq!((wet.phantom_count, wet.phantom_sats), (1, 4000));
        assert_eq!(lock(&pool, 1).await, (1, None), "the input is released");
        assert_eq!(lock(&pool, 2).await.0, 0, "the phantom change is dead");
        assert_eq!(tx_state(&pool, 1).await.0, "failed");
        assert_eq!(wet.reqs_retired, 1);
        assert_eq!(
            req_state(&pool, &"ab".repeat(32)).await,
            ("invalid".into(), 2),
            "the proof pass stops asking about it"
        );
    }

    // ── the abandonment-side twin (run B's heal, 2026-08-29): dead however held ──

    /// RED→GREEN: a tx some source still HOLDS, one of whose inputs the chain
    /// says is spent by a DIFFERENT, CONFIRMED tx, can never mine. It is
    /// abandoned: the conflicted input is RELINQUISHED (gone), its phantom
    /// change dies, the tx fails — and the presence probe is not even asked.
    #[tokio::test]
    async fn a_held_tx_whose_input_is_chain_spent_by_another_confirmed_tx_is_dead() {
        let pool = rule_fixture().await;
        let probed = std::cell::Cell::new(false);
        let report = reconcile_with(
            &pool,
            0,
            30,
            true,
            |_txid| {
                probed.set(true);
                async { presence(BroadcastVerification::Confirmed) }
            },
            |_src, _vout| async {
                InputSpend::SpentBy {
                    txid: "98".repeat(32),
                    confirmed: true,
                }
            },
        )
        .await
        .unwrap();
        assert!(!probed.get(), "a chain-dead tx needs no presence probe");
        assert_eq!(report.conflicted.len(), 1);
        assert!(report.kept.is_empty() && report.abandoned.is_empty());
        assert!(report.applied);
        assert_eq!(
            (report.relinquished_count, report.relinquished_sats),
            (1, 5000)
        );
        assert_eq!(
            report.restored_count, 0,
            "a spent-elsewhere coin is never restored"
        );
        assert_eq!(
            lock(&pool, 1).await,
            (0, None),
            "relinquished: unspendable and unlocked"
        );
        assert_eq!(lock(&pool, 2).await.0, 0, "phantom change dead");
        assert_eq!(tx_state(&pool, 1).await.0, "failed");
        assert_eq!(req_state(&pool, &"ab".repeat(32)).await.0, "invalid");
    }

    /// An UNCONFIRMED competitor is not a verdict (a mempool race can still
    /// go either way); the presence probe decides, and a held tx is kept.
    #[tokio::test]
    async fn an_unconfirmed_competitor_does_not_kill_a_held_tx() {
        let pool = rule_fixture().await;
        let report = reconcile_with(
            &pool,
            0,
            30,
            true,
            |_txid| async { presence(BroadcastVerification::Confirmed) },
            |_src, _vout| async {
                InputSpend::SpentBy {
                    txid: "98".repeat(32),
                    confirmed: false,
                }
            },
        )
        .await
        .unwrap();
        assert_eq!(report.kept.len(), 1);
        assert!(report.conflicted.is_empty());
        assert_eq!(lock(&pool, 1).await, (0, Some(1)));
    }

    /// The tx's OWN spend of its input (it mined) is never a conflict.
    #[tokio::test]
    async fn our_own_confirmed_spend_is_not_a_conflict() {
        let pool = rule_fixture().await;
        let report = reconcile_with(
            &pool,
            0,
            30,
            true,
            |_txid| async { presence(BroadcastVerification::Confirmed) },
            |_src, _vout| async {
                InputSpend::SpentBy {
                    txid: "AB".repeat(32), // ours, other case
                    confirmed: true,
                }
            },
        )
        .await
        .unwrap();
        assert_eq!(report.kept.len(), 1);
        assert!(report.conflicted.is_empty());
    }

    /// Abandonment by absence releases per input: UNKNOWN stays locked.
    #[tokio::test]
    async fn a_definitive_absence_keeps_an_unknown_input_locked() {
        let pool = rule_fixture().await;
        let report = reconcile_with(
            &pool,
            0,
            30,
            true,
            |_txid| async { absent_everywhere() },
            |_src, _vout| async { InputSpend::Unknown },
        )
        .await
        .unwrap();
        assert!(report.applied);
        assert_eq!(report.abandoned.len(), 1);
        assert_eq!(report.restored_count, 0);
        assert_eq!(report.kept_locked_count, 1);
        assert_eq!(
            lock(&pool, 1).await,
            (0, Some(1)),
            "an unknown never releases money"
        );
        assert_eq!(lock(&pool, 2).await.0, 0, "the phantom change still dies");
    }

    // ── the absence clock (2026-09-02): a broadcaster's SEEN is not chain evidence ──

    /// RED→GREEN, the 2026-09-02 phantom shape under the DAEMON's sweep:
    /// Arcade answers SEEN_MULTIPLE_NODES for hours, WhatsOnChain 404, the
    /// transaction is older than the absence threshold. The old rule read
    /// the broadcaster's word as "held by a source" and kept it forever;
    /// past the threshold it is a phantom: input restored (verified
    /// unspent), phantom change dead, tx failed, req retired.
    #[tokio::test]
    async fn a_seen_forever_phantom_past_the_threshold_is_retired() {
        let pool = rule_fixture_aged("2026-01-01T00:00:00+00:00").await;
        let dry = reconcile_with(
            &pool,
            0,
            30,
            false,
            |_txid| async { seen_by_arcade_absent_from_chain() },
            |_src, _vout| async { InputSpend::Unspent },
        )
        .await
        .unwrap();
        assert_eq!(dry.absent_past_threshold.len(), 1);
        assert_eq!(dry.absent_past_threshold[0].0, "ab".repeat(32));
        assert!(dry.absent_past_threshold[0].1 >= 30);
        assert!(dry.kept.is_empty() && dry.abandoned.is_empty());
        assert!(!dry.applied);
        assert_eq!(
            lock(&pool, 1).await,
            (0, Some(1)),
            "dry run touches nothing"
        );

        let wet = reconcile_with(
            &pool,
            0,
            30,
            true,
            |_txid| async { seen_by_arcade_absent_from_chain() },
            |_src, _vout| async { InputSpend::Unspent },
        )
        .await
        .unwrap();
        assert!(wet.applied);
        assert_eq!(wet.dead_txids(), vec!["ab".repeat(32)]);
        assert_eq!(
            (wet.failed, wet.restored_count, wet.restored_sats),
            (1, 1, 5000)
        );
        assert_eq!((wet.phantom_count, wet.phantom_sats), (1, 4000));
        assert_eq!(lock(&pool, 1).await, (1, None), "the coin is back");
        assert_eq!(lock(&pool, 2).await.0, 0, "the phantom change is dead");
        assert_eq!(tx_state(&pool, 1).await.0, "failed");
        assert_eq!(req_state(&pool, &"ab".repeat(32)).await.0, "invalid");
    }

    /// The same shape younger than the threshold is on the clock: kept,
    /// nothing written, surfaced with its age.
    #[tokio::test]
    async fn a_seen_but_absent_tx_younger_than_the_threshold_is_on_the_clock() {
        let pool = mem_pool_with(&[]).await;
        sqlx::query(
            "INSERT INTO transactions (transaction_id, txid, status, created_at) VALUES
             (1, ?, 'unproven', strftime('%Y-%m-%dT%H:%M:%f+00:00', 'now', '-5 minutes'))",
        )
        .bind("ab".repeat(32))
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query("INSERT INTO outputs (transaction_id, spendable, spent_by, satoshis) VALUES (1, 1, NULL, 4000)")
            .execute(&pool)
            .await
            .unwrap();
        insert_req(&pool, &"ab".repeat(32), "unmined", 0).await;
        let report = reconcile_with(
            &pool,
            0,
            30,
            true,
            |_txid| async { seen_by_arcade_absent_from_chain() },
            |_src, _vout| async { InputSpend::Unspent },
        )
        .await
        .unwrap();
        assert_eq!(report.absent_on_clock.len(), 1);
        assert_eq!(report.absent_on_clock[0].0, "ab".repeat(32));
        assert!(
            (4..=6).contains(&report.absent_on_clock[0].1),
            "{:?}",
            report.absent_on_clock
        );
        assert!(report.absent_past_threshold.is_empty() && report.kept.is_empty());
        assert!(!report.applied);
        assert_eq!(lock(&pool, 1).await.0, 1, "its change untouched");
        assert_eq!(tx_state(&pool, 1).await.0, "unproven");
        assert_eq!(req_state(&pool, &"ab".repeat(32)).await.0, "unmined");
    }

    // ── the soak wallet's six (2026-09-04) ──

    /// The June shape (e951…, ca29…, 28e3…): this sweep failed them on
    /// 2026-06-29 (the space-form `updated_at` is its `CURRENT_TIMESTAMP`),
    /// their outputs are dead, nothing is locked by them, but their reqs
    /// stayed `unmined` and the proof pass asked three providers about them
    /// every minute for 67 days. They are not sweep candidates (`failed`),
    /// so the OLD sweep never saw them again; the stale-req scan does, and
    /// retires the req only on the probe's definitive absence (Arcade
    /// JSON-404, WhatsOnChain 404).
    #[tokio::test]
    async fn the_june_shape_a_failed_tx_whose_req_the_proof_pass_still_polls() {
        let pool = mem_pool_with(&[]).await;
        let june: [(&str, i64, i64); 3] = [
            ("e9512972dc4f57b6", 2, 14),
            ("ca29779e41865656", 1, 15),
            ("28e3b96e8f26f5db", 1, 21),
        ];
        for (prefix, attempts, id) in june {
            let txid = format!("{prefix}{}", "0".repeat(64 - prefix.len()));
            sqlx::query(
                "INSERT INTO transactions (transaction_id, txid, status, created_at, updated_at) \
                 VALUES (?, ?, 'failed', '2026-06-29T15:42:52.146700+00:00', '2026-06-29 17:34:15')",
            )
            .bind(id)
            .bind(&txid)
            .execute(&pool)
            .await
            .unwrap();
            sqlx::query("INSERT INTO outputs (transaction_id, spendable, spent_by, satoshis) VALUES (?, 0, NULL, 10000)")
                .bind(id)
                .execute(&pool)
                .await
                .unwrap();
            insert_req(&pool, &txid, "unmined", attempts).await;
        }
        let probed = std::cell::RefCell::new(Vec::new());

        let dry = reconcile_with(
            &pool,
            3600,
            30,
            false,
            |txid| {
                probed.borrow_mut().push(txid);
                async { absent_everywhere() }
            },
            |_src, _vout| async { InputSpend::Unknown },
        )
        .await
        .unwrap();
        assert_eq!(
            dry.checked, 0,
            "a failed transaction is not a sweep candidate"
        );
        assert_eq!(dry.stale_reqs_checked, 3);
        assert_eq!(dry.stale_reqs_retired.len(), 3);
        assert!(dry.stale_reqs_known.is_empty() && dry.stale_reqs_kept.is_empty());
        assert!(dry.has_work());
        assert!(!dry.applied, "a dry run writes nothing");
        assert_eq!(
            probed.borrow().len(),
            3,
            "each req is probed, never retired blind"
        );
        for (prefix, attempts, _) in june {
            let txid = format!("{prefix}{}", "0".repeat(64 - prefix.len()));
            assert_eq!(req_state(&pool, &txid).await, ("unmined".into(), attempts));
        }

        let wet = reconcile_with(
            &pool,
            3600,
            30,
            true,
            |_txid| async { absent_everywhere() },
            |_src, _vout| async { InputSpend::Unknown },
        )
        .await
        .unwrap();
        assert!(wet.applied);
        assert_eq!(wet.reqs_retired, 3);
        assert_eq!(
            (wet.failed, wet.restored_count, wet.phantom_count),
            (0, 0, 0)
        );
        for (prefix, attempts, id) in june {
            let txid = format!("{prefix}{}", "0".repeat(64 - prefix.len()));
            assert_eq!(
                req_state(&pool, &txid).await,
                ("invalid".into(), attempts + 1),
                "the proof pass stops asking"
            );
            assert_eq!(
                tx_state(&pool, id).await,
                ("failed".into(), Some("2026-06-29 17:34:15".into())),
                "the transaction row is not touched"
            );
        }

        // The next pass finds nothing left to do.
        let again = reconcile_with(
            &pool,
            3600,
            30,
            true,
            |_txid| async { absent_everywhere() },
            |_src, _vout| async { InputSpend::Unknown },
        )
        .await
        .unwrap();
        assert_eq!(again.stale_reqs_checked, 0);
        assert!(!again.applied);
    }

    /// The September shape (f525…, a053…, ab66…): `unproven` with an
    /// `unmined` req, two days old, spending a completed parent's coin,
    /// mined on chain (WhatsOnChain 290+ confirmations) while Arcade still
    /// answers ACCEPTED_BY_NETWORK. The sweep keeps them (the chain index
    /// holds them) and touches nothing: the proof pass owns the req, and
    /// the toolbox now lets the chain index's `mined` through Arcade's
    /// stale `known` so the proof gets fetched.
    #[tokio::test]
    async fn the_september_shape_a_mined_tx_arcade_still_calls_in_flight_is_kept() {
        let pool = mem_pool_with(&[]).await;
        let ours = format!("f5258b036f7b2694{}", "0".repeat(48));
        sqlx::query(
            "INSERT INTO transactions (transaction_id, txid, status, created_at) VALUES
             (20, ?, 'completed', '2026-06-29T00:00:00+00:00'),
             (22, ?, 'unproven', strftime('%Y-%m-%dT%H:%M:%f+00:00', 'now', '-2 days'))",
        )
        .bind(format!("ec7373a33c77cf6c{}", "0".repeat(48)))
        .bind(&ours)
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO outputs (output_id, transaction_id, spendable, spent_by, satoshis, vout) VALUES
             (65, 20, 0, 22, 30092, 1),
             (70, 22, 1, NULL, 1, 0),
             (71, 22, 0, 23, 29947, 1)",
        )
        .execute(&pool)
        .await
        .unwrap();
        insert_req(&pool, &ours, "unmined", 1).await;
        let spender = ours.clone();
        let report = reconcile_with(
            &pool,
            3600,
            30,
            true,
            |_txid| async { chain_mined() },
            move |_src, _vout| {
                let spender = spender.clone();
                async move {
                    InputSpend::SpentBy {
                        txid: spender,
                        confirmed: true,
                    }
                }
            },
        )
        .await
        .unwrap();
        assert_eq!(report.checked, 1);
        assert_eq!(report.kept, vec![ours.clone()]);
        assert!(report.dead_txids().is_empty() && report.absent_on_clock.is_empty());
        assert_eq!(report.stale_reqs_checked, 0);
        assert!(!report.applied);
        assert_eq!(
            lock(&pool, 65).await,
            (0, Some(22)),
            "its input stays spent by it"
        );
        assert_eq!(
            lock(&pool, 70).await,
            (1, None),
            "its change stays spendable"
        );
        assert_eq!(tx_state(&pool, 22).await.0, "unproven");
        assert_eq!(req_state(&pool, &ours).await, ("unmined".into(), 1));
    }

    /// A stale req whose transaction the chain index KNOWS is never retired
    /// (the `failed` verdict is the suspect one); an undecidable probe keeps
    /// it for the next pass.
    #[tokio::test]
    async fn a_stale_req_the_chain_index_knows_or_cannot_judge_is_kept() {
        let pool = mem_pool_with(&[
            (
                "11".repeat(32).leak(),
                "failed",
                "2026-01-01T00:00:00+00:00",
            ),
            (
                "22".repeat(32).leak(),
                "failed",
                "2026-01-01T00:00:00+00:00",
            ),
        ])
        .await;
        insert_req(&pool, &"11".repeat(32), "unmined", 3).await;
        insert_req(&pool, &"22".repeat(32), "callback", 0).await;
        let report = reconcile_with(
            &pool,
            0,
            30,
            true,
            |txid| async move {
                if txid == "11".repeat(32) {
                    chain_mined()
                } else {
                    presence(BroadcastVerification::Inconclusive)
                }
            },
            |_src, _vout| async { InputSpend::Unknown },
        )
        .await
        .unwrap();
        assert_eq!(report.stale_reqs_checked, 2);
        assert_eq!(report.stale_reqs_known, vec!["11".repeat(32)]);
        assert_eq!(report.stale_reqs_kept, vec!["22".repeat(32)]);
        assert!(report.stale_reqs_retired.is_empty());
        assert!(!report.has_work() && !report.applied);
        assert_eq!(
            req_state(&pool, &"11".repeat(32)).await,
            ("unmined".into(), 3)
        );
        assert_eq!(
            req_state(&pool, &"22".repeat(32)).await,
            ("callback".into(), 0)
        );
    }

    /// A stale req is retired under the SEEN-forever rule too, and a
    /// terminal req (`invalid`, `completed`) or a fresh transaction (inside
    /// the min-age guard) is never a stale-req candidate.
    #[tokio::test]
    async fn stale_req_candidates_are_polled_reqs_of_old_failed_txs_only() {
        let pool = mem_pool_with(&[
            (
                "11".repeat(32).leak(),
                "failed",
                "2026-01-01T00:00:00+00:00",
            ),
            (
                "22".repeat(32).leak(),
                "failed",
                "2026-01-01T00:00:00+00:00",
            ),
            (
                "33".repeat(32).leak(),
                "failed",
                "2026-01-01T00:00:00+00:00",
            ),
            (
                "55".repeat(32).leak(),
                "completed",
                "2026-01-01T00:00:00+00:00",
            ),
        ])
        .await;
        sqlx::query(
            "INSERT INTO transactions (txid, status, created_at) VALUES (?, 'failed', strftime('%Y-%m-%dT%H:%M:%f+00:00','now'))",
        )
        .bind("44".repeat(32))
        .execute(&pool)
        .await
        .unwrap();
        insert_req(&pool, &"11".repeat(32), "unmined", 0).await;
        insert_req(&pool, &"22".repeat(32), "invalid", 4).await;
        insert_req(&pool, &"33".repeat(32), "completed", 0).await;
        insert_req(&pool, &"44".repeat(32), "unmined", 0).await;
        insert_req(&pool, &"55".repeat(32), "unmined", 0).await;
        let report = reconcile_with(
            &pool,
            3600,
            30,
            true,
            |_txid| async { seen_by_arcade_absent_from_chain() },
            |_src, _vout| async { InputSpend::Unknown },
        )
        .await
        .unwrap();
        assert_eq!(
            report.stale_reqs_checked, 1,
            "only the old failed tx's polled req"
        );
        assert_eq!(report.stale_reqs_retired, vec!["11".repeat(32)]);
        assert!(report.applied);
        assert_eq!(report.reqs_retired, 1);
        assert_eq!(
            req_state(&pool, &"11".repeat(32)).await,
            ("invalid".into(), 1)
        );
        assert_eq!(
            req_state(&pool, &"22".repeat(32)).await,
            ("invalid".into(), 4)
        );
        assert_eq!(
            req_state(&pool, &"33".repeat(32)).await,
            ("completed".into(), 0)
        );
        assert_eq!(
            req_state(&pool, &"44".repeat(32)).await,
            ("unmined".into(), 0)
        );
        assert_eq!(
            req_state(&pool, &"55".repeat(32)).await,
            ("unmined".into(), 0)
        );
    }

    /// The pure verdict table.
    #[test]
    fn verdict_table() {
        let ours = "ab".repeat(32);
        let other = InputSpend::SpentBy {
            txid: "98".repeat(32),
            confirmed: true,
        };
        let other_unconf = InputSpend::SpentBy {
            txid: "98".repeat(32),
            confirmed: false,
        };
        let mine = InputSpend::SpentBy {
            txid: ours.to_ascii_uppercase(),
            confirmed: true,
        };
        use BroadcastVerification as V;
        let held = presence(V::Confirmed);
        assert_eq!(
            verdict_for(&ours, std::slice::from_ref(&other), &held, 0, 30),
            Verdict::DeadConflict
        );
        assert_eq!(
            verdict_for(
                &ours,
                &[InputSpend::Unspent, other],
                &absent_everywhere(),
                0,
                30
            ),
            Verdict::DeadConflict
        );
        assert_eq!(
            verdict_for(&ours, &[other_unconf], &held, 0, 30),
            Verdict::Kept
        );
        assert_eq!(
            verdict_for(&ours, &[mine], &presence(V::Inconclusive), 0, 30),
            Verdict::Inconclusive
        );
        assert_eq!(
            verdict_for(&ours, &[], &absent_everywhere(), 0, 30),
            Verdict::DeadAbsent,
            "definitive absence needs no age"
        );
        assert_eq!(
            verdict_for(&ours, &[InputSpend::Unknown], &held, 0, 30),
            Verdict::Kept
        );
        // The absence clock: the broadcaster's SEEN with a chain-index 404.
        let seen = seen_by_arcade_absent_from_chain();
        assert_eq!(
            verdict_for(&ours, &[], &seen, 29, 30),
            Verdict::AbsentOnClock
        );
        assert_eq!(
            verdict_for(&ours, &[], &seen, 30, 30),
            Verdict::DeadAbsentPastThreshold
        );
        assert_eq!(
            verdict_for(&ours, &[], &seen, 100_000, 30),
            Verdict::DeadAbsentPastThreshold
        );
        // The chain index's own answer settles everything, whatever else.
        assert_eq!(
            verdict_for(&ours, &[], &chain_mined(), 100_000, 30),
            Verdict::Kept
        );
        let mut chain_seen = chain_mined();
        chain_seen.chain_index = ChainIndexAnswer::Present(NetworkEvidence::Seen);
        assert_eq!(
            verdict_for(&ours, &[], &chain_seen, 100_000, 30),
            Verdict::Kept
        );
        assert!(Verdict::DeadAbsentPastThreshold.is_dead());
        assert!(!Verdict::AbsentOnClock.is_dead());
    }
}
