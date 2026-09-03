use anyhow::{Context, Result};
use bsv_wallet_toolbox::Chain;
use sqlx::Row;
use std::future::Future;

use crate::broadcast_verify::{BroadcastVerification, BroadcastVerifier};
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

/// Summary of one reconcile pass over abandoned transactions.
#[derive(Default, Debug, Clone)]
pub struct ReconcileReport {
    /// `unproven` txs inspected (after the min-age filter).
    pub checked: usize,
    /// txids some source still holds (kept spendable).
    pub kept: Vec<String>,
    /// txids whose absence was NOT definitive (a lone index miss, a source
    /// fault, probing disabled) — kept, because an unknown never releases
    /// money. Surfaced so an operator sees what the sweep could not decide.
    pub inconclusive: Vec<String>,
    /// txids DEFINITIVELY absent (broadcaster + chain index both 404, no
    /// source holding them) — abandoned.
    pub abandoned: Vec<String>,
    /// txids DEFINITIVELY DEAD BY CONFLICT: an input of theirs is chain-spent
    /// by a DIFFERENT, CONFIRMED txid, so the bytes can never mine however
    /// many sources still hold them (the run-B heal's sharp edge: 11
    /// phantom-parent outputs counted as balance behind one "held" tx).
    /// Abandoned like the absent set.
    pub conflicted: Vec<String>,
    /// Whether `execute` actually applied the cleanup.
    pub applied: bool,
    /// Transactions transitioned `unproven`/`sending` -> `failed`.
    pub failed: u64,
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
///    WhatsOnChain): held by any source ⇒ kept; a lone index miss / fault /
///    probing disabled ⇒ `Inconclusive`, kept; DEFINITIVE absence (broadcaster
///    JSON-404 AND index 404) ⇒ abandoned.
///
/// Abandoning is per-input verified, never blind: an input the chain says is
/// UNSPENT is restored; one spent by another confirmed tx is RELINQUISHED
/// (unspendable, unlocked — it is gone); one the chain cannot vouch for stays
/// LOCKED. The tx's own outputs go unspendable and the tx `failed`.
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
        execute,
        |txid: String| {
            let v = verifier.clone();
            async move { v.verify(&txid).await }
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

/// The pure decision for one candidate given its two chain answers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    Kept,
    Inconclusive,
    DeadAbsent,
    DeadConflict,
}

/// THE two-question rule, as a value: a confirmed spend of any input by a
/// DIFFERENT txid is dead however held; otherwise the presence probe decides.
pub fn verdict_for(
    our_txid: &str,
    inputs: &[InputSpend],
    presence: BroadcastVerification,
) -> Verdict {
    let conflict = inputs.iter().any(|i| {
        matches!(i, InputSpend::SpentBy { txid, confirmed: true }
                     if !txid.eq_ignore_ascii_case(our_txid))
    });
    if conflict {
        return Verdict::DeadConflict;
    }
    match presence {
        BroadcastVerification::Confirmed => Verdict::Kept,
        BroadcastVerification::Inconclusive => Verdict::Inconclusive,
        BroadcastVerification::Rejected => Verdict::DeadAbsent,
    }
}

/// [`reconcile`] with both chain probes injected — the seam the cells drive
/// (the real probes hit four networks; the RULE is what is under test).
pub async fn reconcile_with<P, PF, I, IF>(
    pool: &sqlx::SqlitePool,
    min_age_secs: i64,
    execute: bool,
    probe: P,
    input_spend: I,
) -> Result<ReconcileReport>
where
    P: Fn(String) -> PF,
    PF: Future<Output = BroadcastVerification>,
    I: Fn(String, u32) -> IF,
    IF: Future<Output = InputSpend>,
{
    let mut report = ReconcileReport::default();
    let rows = select_stale_unproven(pool, min_age_secs).await?;
    report.checked = rows.len();
    if rows.is_empty() {
        return Ok(report);
    }

    // (tx_id, txid, per-input answers) for every tx to abandon.
    let mut dead: Vec<DeadCandidate> = Vec::new();
    for row in &rows {
        let tx_id: i64 = row.get("transaction_id");
        let txid: String = row.get("txid");
        let inputs = tracked_inputs(pool, tx_id).await?;
        let mut answers: Vec<(TrackedInput, InputSpend)> = Vec::with_capacity(inputs.len());
        for i in inputs {
            let a = input_spend(i.source_txid.clone(), i.vout).await;
            answers.push((i, a));
        }
        let spends: Vec<InputSpend> = answers.iter().map(|(_, a)| a.clone()).collect();
        // The conflict question is decided from the inputs alone; the
        // presence probe is only asked when no input says "dead".
        let presence = if verdict_for(&txid, &spends, BroadcastVerification::Inconclusive)
            == Verdict::DeadConflict
        {
            BroadcastVerification::Inconclusive
        } else {
            probe(txid.clone()).await
        };
        match verdict_for(&txid, &spends, presence) {
            Verdict::Kept => report.kept.push(txid),
            Verdict::Inconclusive => report.inconclusive.push(txid),
            Verdict::DeadAbsent => {
                report.abandoned.push(txid.clone());
                dead.push((tx_id, txid, answers));
            }
            Verdict::DeadConflict => {
                report.conflicted.push(txid.clone());
                dead.push((tx_id, txid, answers));
            }
        }
    }

    if dead.is_empty() || !execute {
        return Ok(report);
    }

    let ids: Vec<i64> = dead.iter().map(|(id, _, _)| *id).collect();
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
    let phantoms = remove_phantom_outputs(pool, &ids).await?;
    let failed = mark_failed(pool, &ids).await?;
    report.applied = true;
    report.failed = failed;
    report.phantom_count = phantoms.0;
    report.phantom_sats = phantoms.1;
    Ok(report)
}

/// `unproven` transactions at least `min_age_secs` old.
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
    Ok(sqlx::query(
        "SELECT transaction_id, txid FROM transactions \
         WHERE (status='unproven' AND datetime(created_at) <= datetime('now', ?)) \
            OR (status='sending' AND datetime(created_at) <= datetime('now', ?) \
                AND datetime(created_at) <= datetime('now', '-600 seconds'))",
    )
    .bind(&age_modifier)
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

    if report.checked == 0 {
        println!("No unproven (or stale sending) transactions found.");
        return Ok(());
    }
    println!(
        "Found {} unproven/stale-sending transaction(s); probed every broadcast source.",
        report.checked
    );
    println!(
        "  Definitively absent: {}    Dead by conflict: {}    Held by a source: {}    Undecidable (kept): {}",
        report.abandoned.len(),
        report.conflicted.len(),
        report.kept.len(),
        report.inconclusive.len()
    );
    for txid in &report.kept {
        println!("    keep: {}", txid);
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
    for txid in &report.conflicted {
        println!(
            "    fail (an input is chain-spent by another confirmed tx — dead however held): {}",
            txid
        );
    }

    if report.abandoned.is_empty() && report.conflicted.is_empty() {
        println!("Nothing to clean up.");
        return Ok(());
    }

    // Everything this wallet built on an abandoned transaction is a phantom
    // too (the poisoned chain, 2026-09-02): the toolbox walks the unproven
    // descendants under THE RELEASE RULE. On a dry run it only lists them.
    let mut descendants_retired = 0usize;
    for txid in report.abandoned.iter().chain(report.conflicted.iter()) {
        let poison = ctx
            .wallet
            .storage()
            .retire_poisoned_chain_from(ctx.wallet.services(), txid, "invalid", execute)
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

async fn mark_failed(pool: &sqlx::SqlitePool, ids: &[i64]) -> Result<u64> {
    let mut tx = pool.begin().await?;
    let mut count = 0u64;
    for id in ids {
        let res = sqlx::query(
            "UPDATE transactions SET status = 'failed', \
             updated_at = CURRENT_TIMESTAMP WHERE transaction_id = ? AND status = 'unproven'",
        )
        .bind(id)
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
    use sqlx::Row;

    async fn mem_pool_with(rows: &[(&str, &str, &str)]) -> sqlx::SqlitePool {
        let pool = sqlx::SqlitePool::connect("sqlite::memory:").await.unwrap();
        sqlx::query(
            "CREATE TABLE transactions (
                transaction_id INTEGER PRIMARY KEY AUTOINCREMENT,
                txid TEXT NOT NULL, status TEXT NOT NULL, created_at TEXT NOT NULL,
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

    /// The regression: toolbox-written rows use ISO-8601 `…T…+00:00`, which a
    /// bare string compare against `datetime('now')` NEVER matches ('T' > ' ')
    /// — the sweep silently found nothing, forever. Both formats must match.
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
    }

    // ── THE RELEASE RULE (2026-08-29) — the verdict is corroborated absence ──

    /// Seed one stale `unproven` tx (id 1) whose input is output 1 (locked,
    /// spent_by = 1) and whose own phantom change is output 2.
    /// Seed one stale `unproven` tx (id 1, txid `ab…`) whose input is output 1
    /// (coin of parent tx 99 `cd…`:0, locked: spent_by = 1) and whose own
    /// phantom change is output 2.
    async fn rule_fixture() -> sqlx::SqlitePool {
        let pool = mem_pool_with(&[(
            "ab".repeat(32).leak(),
            "unproven",
            "2020-01-01T00:00:00+00:00",
        )])
        .await;
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
            true,
            |_txid| async { BroadcastVerification::Inconclusive },
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
        let status: String =
            sqlx::query_scalar("SELECT status FROM transactions WHERE transaction_id = 1")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(status, "unproven");
    }

    /// A tx ANY source still holds is kept.
    #[tokio::test]
    async fn a_held_tx_is_kept() {
        let pool = rule_fixture().await;
        let report = reconcile_with(
            &pool,
            0,
            true,
            |_txid| async { BroadcastVerification::Confirmed },
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
    /// invalidated, tx failed — and only under --execute.
    #[tokio::test]
    async fn a_definitive_absence_abandons_and_restores_under_execute() {
        let pool = rule_fixture().await;
        let dry = reconcile_with(
            &pool,
            0,
            false,
            |_txid| async { BroadcastVerification::Rejected },
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

        let wet = reconcile_with(
            &pool,
            0,
            true,
            |_txid| async { BroadcastVerification::Rejected },
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
        let status: String =
            sqlx::query_scalar("SELECT status FROM transactions WHERE transaction_id = 1")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(status, "failed");
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
            true,
            |_txid| {
                probed.set(true);
                async { BroadcastVerification::Confirmed }
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
        let status: String =
            sqlx::query_scalar("SELECT status FROM transactions WHERE transaction_id = 1")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(status, "failed");
    }

    /// An UNCONFIRMED competitor is not a verdict (a mempool race can still
    /// go either way); the presence probe decides, and a held tx is kept.
    #[tokio::test]
    async fn an_unconfirmed_competitor_does_not_kill_a_held_tx() {
        let pool = rule_fixture().await;
        let report = reconcile_with(
            &pool,
            0,
            true,
            |_txid| async { BroadcastVerification::Confirmed },
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
            true,
            |_txid| async { BroadcastVerification::Confirmed },
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
            true,
            |_txid| async { BroadcastVerification::Rejected },
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
        assert_eq!(
            verdict_for(&ours, std::slice::from_ref(&other), V::Confirmed),
            Verdict::DeadConflict
        );
        assert_eq!(
            verdict_for(&ours, &[InputSpend::Unspent, other], V::Rejected),
            Verdict::DeadConflict
        );
        assert_eq!(
            verdict_for(&ours, &[other_unconf], V::Confirmed),
            Verdict::Kept
        );
        assert_eq!(
            verdict_for(&ours, &[mine], V::Inconclusive),
            Verdict::Inconclusive
        );
        assert_eq!(verdict_for(&ours, &[], V::Rejected), Verdict::DeadAbsent);
        assert_eq!(
            verdict_for(&ours, &[InputSpend::Unknown], V::Confirmed),
            Verdict::Kept
        );
    }
}
