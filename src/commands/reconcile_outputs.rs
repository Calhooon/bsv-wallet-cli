//! `bsv-wallet reconcile-outputs` — the LAST poison class of the LOW fleet's
//! 2026-08-29 run A: inputs of LIVE transactions the DB shows unspent.
//!
//! How it arose: a blind sweep marked a fresh tx `failed` and restored its
//! inputs (`spent_by = NULL, spendable = 1`); the copy a peer's orphan pool
//! held then mined, `sync` brought the row back `completed` — but nothing
//! re-marked its inputs spent, so coin selection re-spent them and every
//! child was Arcade-REJECTED `UTXO_SPENT by <our own mined tx>` (p23's
//! `2ecafa9e…` spent six inputs, five of them already consumed on chain by
//! its own `3b56f161…`, 13 confirmations). Outside `sync --reconcile-spent`
//! (deposit-address scope) and `cleanup-abandoned` (failed/unproven
//! candidates only).
//!
//! Two phases, THE RELEASE RULE's primitives (2026-08-29):
//! (a) **DB consistency, no network:** every live tx (`completed` /
//!     `unproven` / `sending`) with raw bytes names its inputs; any tracked
//!     output it spends that still reads unspent is re-linked
//!     (`spent_by` = that tx, `spendable = 0`).
//! (b) **Chain check, per outpoint:** the remaining `spendable = 1` outputs
//!     are probed with [`InputSpend`]: spent by a CONFIRMED tx ⇒
//!     RELINQUISHED (re-linked instead when that spender is a live tx of
//!     ours the DB simply lacked bytes for); unspent ⇒ kept spendable;
//!     unknown / unconfirmed competitor ⇒ untouched and REPORTED (an unknown
//!     never moves money either way). Bounded per pass (`--max-chain-checks`).

use anyhow::{Context, Result};
use bsv_sdk::transaction::Transaction;
use sqlx::Row;
use std::future::Future;

use crate::commands::cleanup_abandoned::{probe_input_spend, InputSpend};
use crate::commands::receive;
use crate::context::WalletContext;

#[derive(Default, Debug, Clone)]
pub struct OutputsReport {
    /// Live txs whose raw bytes were parsed in phase (a).
    pub live_txs: usize,
    /// Phase (a): outputs re-linked to a live tx of ours that spends them.
    pub relinked_count: u64,
    pub relinked_sats: u64,
    /// Phase (b): spendable outputs chain-checked this pass.
    pub chain_checked: usize,
    /// Phase (b): outputs spent on chain by a confirmed tx not in the DB — gone.
    pub relinquished_count: u64,
    pub relinquished_sats: u64,
    /// Phase (b): confirmed spender turned out to be a live tx of ours — re-linked.
    pub relinked_from_chain_count: u64,
    /// Phase (b): verified unspent — kept spendable.
    pub unspent_count: u64,
    /// Phase (b): unknown / unconfirmed competitor — untouched, reported.
    pub unknown_count: u64,
    /// Outpoints (`txid.vout`) the chain could not vouch for.
    pub unknown: Vec<String>,
    /// Phase (b) stopped at the bound; another pass continues.
    pub more_to_check: bool,
    pub applied: bool,
}

/// One tracked spendable output: the coin and its outpoint.
struct SpendableOutput {
    output_id: i64,
    satoshis: i64,
    source_txid: String,
    vout: u32,
}

pub async fn reconcile_outputs_with<I, IF>(
    pool: &sqlx::SqlitePool,
    execute: bool,
    max_chain_checks: usize,
    input_spend: I,
) -> Result<OutputsReport>
where
    I: Fn(String, u32) -> IF,
    IF: Future<Output = InputSpend>,
{
    let mut report = OutputsReport::default();

    // ── (a) DB consistency: a live tx's inputs are spent, whatever a sweep did ──
    let live = sqlx::query(
        "SELECT transaction_id, txid, raw_tx FROM transactions \
         WHERE status IN ('completed', 'unproven', 'sending') AND raw_tx IS NOT NULL",
    )
    .fetch_all(pool)
    .await?;
    report.live_txs = live.len();
    for row in &live {
        let tx_id: i64 = row.get("transaction_id");
        let raw: Vec<u8> = row.get("raw_tx");
        let Ok(tx) = Transaction::from_binary(&raw) else {
            continue; // unparseable bytes name no inputs — leave the row alone
        };
        for input in &tx.inputs {
            let Ok(src) = input.get_source_txid() else {
                continue;
            };
            if src.chars().all(|c| c == '0') {
                continue;
            }
            let hit = sqlx::query(
                "SELECT o.output_id, o.satoshis FROM outputs o \
                 JOIN transactions p ON p.transaction_id = o.transaction_id \
                 WHERE lower(p.txid) = ? AND o.vout = ? AND o.spent_by IS NULL",
            )
            .bind(src.to_ascii_lowercase())
            .bind(i64::from(input.source_output_index))
            .fetch_optional(pool)
            .await?;
            if let Some(o) = hit {
                let output_id: i64 = o.get("output_id");
                let sats: i64 = o.get("satoshis");
                report.relinked_count += 1;
                report.relinked_sats += sats.max(0) as u64;
                if execute {
                    sqlx::query(
                        "UPDATE outputs SET spendable = 0, spent_by = ?, \
                         updated_at = CURRENT_TIMESTAMP WHERE output_id = ? AND spent_by IS NULL",
                    )
                    .bind(tx_id)
                    .bind(output_id)
                    .execute(pool)
                    .await?;
                }
            }
        }
    }

    // ── (b) chain check of what still reads spendable ──
    let candidates: Vec<SpendableOutput> = sqlx::query(
        "SELECT o.output_id, o.satoshis, o.vout, p.txid AS src FROM outputs o \
         JOIN transactions p ON p.transaction_id = o.transaction_id \
         WHERE o.spendable = 1 AND o.spent_by IS NULL \
           AND p.status IN ('completed', 'unproven') \
         ORDER BY o.output_id ASC LIMIT ?",
    )
    .bind((max_chain_checks + 1) as i64)
    .fetch_all(pool)
    .await?
    .iter()
    .map(|r| SpendableOutput {
        output_id: r.get("output_id"),
        satoshis: r.get::<i64, _>("satoshis"),
        source_txid: r.get::<String, _>("src"),
        vout: r.get::<i64, _>("vout") as u32,
    })
    .collect();
    report.more_to_check = candidates.len() > max_chain_checks;
    for c in candidates.iter().take(max_chain_checks) {
        report.chain_checked += 1;
        match input_spend(c.source_txid.clone(), c.vout).await {
            InputSpend::Unspent => report.unspent_count += 1,
            InputSpend::SpentBy {
                txid,
                confirmed: true,
            } => {
                // A live tx of ours the DB lacked bytes for? Re-link, don't discard.
                let ours: Option<(i64,)> = sqlx::query_as(
                    "SELECT transaction_id FROM transactions WHERE lower(txid) = ? \
                     AND status IN ('completed', 'unproven', 'sending')",
                )
                .bind(txid.to_ascii_lowercase())
                .fetch_optional(pool)
                .await?;
                match ours {
                    Some((tx_id,)) => {
                        report.relinked_from_chain_count += 1;
                        if execute {
                            sqlx::query(
                                "UPDATE outputs SET spendable = 0, spent_by = ?, \
                                 updated_at = CURRENT_TIMESTAMP WHERE output_id = ?",
                            )
                            .bind(tx_id)
                            .bind(c.output_id)
                            .execute(pool)
                            .await?;
                        }
                    }
                    None => {
                        report.relinquished_count += 1;
                        report.relinquished_sats += c.satoshis.max(0) as u64;
                        if execute {
                            sqlx::query(
                                "UPDATE outputs SET spendable = 0, spent_by = NULL, \
                                 updated_at = CURRENT_TIMESTAMP WHERE output_id = ?",
                            )
                            .bind(c.output_id)
                            .execute(pool)
                            .await?;
                        }
                    }
                }
            }
            InputSpend::SpentBy {
                confirmed: false, ..
            }
            | InputSpend::Unknown => {
                report.unknown_count += 1;
                report.unknown.push(format!("{}.{}", c.source_txid, c.vout));
            }
        }
    }
    report.applied = execute;
    Ok(report)
}

pub async fn run(
    ctx: &WalletContext,
    db_path: &str,
    execute: bool,
    max_chain_checks: usize,
) -> Result<()> {
    let pool = sqlx::SqlitePool::connect(&format!("sqlite:{}", db_path))
        .await
        .with_context(|| format!("failed to open {} (is the daemon running?)", db_path))?;
    let client = reqwest::Client::new();
    let base = receive::woc_base(ctx.chain);
    let report = reconcile_outputs_with(&pool, execute, max_chain_checks, |src, vout| {
        let c = client.clone();
        async move { probe_input_spend(&c, base, &src, vout).await }
    })
    .await?;

    println!(
        "Phase (a) DB consistency: {} live tx(s) parsed; {} output(s) ({} sats) spent by a live tx of ours but shown unspent{}",
        report.live_txs,
        report.relinked_count,
        report.relinked_sats,
        if execute { " — re-linked" } else { " — would re-link" }
    );
    println!(
        "Phase (b) chain check: {} spendable output(s) probed{}",
        report.chain_checked,
        if report.more_to_check {
            " (bound hit — run again to continue)"
        } else {
            ""
        }
    );
    println!("  verified unspent (kept): {}", report.unspent_count);
    println!(
        "  spent on chain by a confirmed tx not ours{}: {} ({} sats)",
        if execute {
            " — relinquished"
        } else {
            " — would relinquish"
        },
        report.relinquished_count,
        report.relinquished_sats
    );
    println!(
        "  spent by a live tx of ours the DB lacked bytes for{}: {}",
        if execute {
            " — re-linked"
        } else {
            " — would re-link"
        },
        report.relinked_from_chain_count
    );
    println!(
        "  unknown / unconfirmed competitor (untouched — an unknown never moves money): {}",
        report.unknown_count
    );
    for op in &report.unknown {
        println!("    unknown: {}", op);
    }
    if !execute {
        println!();
        println!("Dry run. Re-run with --execute to apply.");
        return Ok(());
    }
    println!();
    println!(
        "Net balance delta: {:+} sats. Restart the daemon to refresh its in-memory view.",
        -(report.relinked_sats as i64) - report.relinquished_sats as i64
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use bsv_sdk::script::{LockingScript, UnlockingScript};
    use bsv_sdk::transaction::{TransactionInput, TransactionOutput};

    async fn fixture() -> (sqlx::SqlitePool, String, Vec<u8>) {
        let pool = sqlx::SqlitePool::connect("sqlite::memory:").await.unwrap();
        sqlx::query(
            "CREATE TABLE transactions (
                transaction_id INTEGER PRIMARY KEY AUTOINCREMENT,
                txid TEXT NOT NULL, status TEXT NOT NULL, raw_tx BLOB, created_at TEXT)",
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
        // Parent tx 1 (`cd…`, completed) with output vout 0 = output 1, shown UNSPENT.
        let parent_txid = "cd".repeat(32);
        sqlx::query(
            "INSERT INTO transactions (transaction_id, txid, status) VALUES (1, ?, 'completed')",
        )
        .bind(&parent_txid)
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query("INSERT INTO outputs (output_id, transaction_id, spendable, spent_by, satoshis, vout) VALUES (1, 1, 1, NULL, 5000, 0)")
            .execute(&pool)
            .await
            .unwrap();
        // A real child tx spending parent:0.
        let mut child = Transaction::new();
        child.version = 1;
        let mut cin = TransactionInput::new(parent_txid.clone(), 0);
        cin.unlocking_script = Some(UnlockingScript::from_hex("00").unwrap());
        child.inputs.push(cin);
        child.outputs.push(TransactionOutput {
            satoshis: Some(4_000),
            locking_script: LockingScript::from_hex(
                "76a914000000000000000000000000000000000000000088ac",
            )
            .unwrap(),
            change: true,
        });
        (pool, child.id(), child.to_binary())
    }

    async fn lock(pool: &sqlx::SqlitePool, output_id: i64) -> (i64, Option<i64>) {
        sqlx::query_as("SELECT spendable, spent_by FROM outputs WHERE output_id = ?")
            .bind(output_id)
            .fetch_one(pool)
            .await
            .unwrap()
    }

    /// THE p23 SHAPE, red→green: the child is LIVE (`completed`, mined) yet
    /// its input still reads spendable — phase (a) re-links it from the
    /// child's own bytes, no network asked.
    #[tokio::test]
    async fn phase_a_relinks_an_input_of_a_live_tx_the_db_shows_unspent() {
        let (pool, child_txid, child_raw) = fixture().await;
        sqlx::query("INSERT INTO transactions (transaction_id, txid, status, raw_tx) VALUES (2, ?, 'completed', ?)")
            .bind(&child_txid)
            .bind(&child_raw)
            .execute(&pool)
            .await
            .unwrap();
        let probed = std::cell::Cell::new(0u32);
        let dry = reconcile_outputs_with(&pool, false, 100, |_s, _v| {
            probed.set(probed.get() + 1);
            async { InputSpend::Unspent }
        })
        .await
        .unwrap();
        assert_eq!((dry.relinked_count, dry.relinked_sats), (1, 5000));
        assert_eq!(lock(&pool, 1).await, (1, None), "dry run touches nothing");
        let wet = reconcile_outputs_with(&pool, true, 100, |_s, _v| {
            probed.set(probed.get() + 1);
            async { InputSpend::Unspent }
        })
        .await
        .unwrap();
        assert!(wet.applied);
        assert_eq!(wet.relinked_count, 1);
        assert_eq!(
            lock(&pool, 1).await,
            (0, Some(2)),
            "locked by the live child"
        );
        assert_eq!(
            wet.chain_checked, 0,
            "a re-linked coin is not chain-checked"
        );
        assert_eq!(
            probed.get(),
            1,
            "only the dry run's phase (b) probed it, once"
        );
    }

    /// Phase (b): a coin the chain says a CONFIRMED stranger spent is gone.
    #[tokio::test]
    async fn phase_b_relinquishes_a_coin_spent_by_a_confirmed_stranger() {
        let (pool, _, _) = fixture().await;
        let r = reconcile_outputs_with(&pool, true, 100, |_s, _v| async {
            InputSpend::SpentBy {
                txid: "98".repeat(32),
                confirmed: true,
            }
        })
        .await
        .unwrap();
        assert_eq!((r.relinquished_count, r.relinquished_sats), (1, 5000));
        assert_eq!(
            lock(&pool, 1).await,
            (0, None),
            "unspendable, unlocked: gone"
        );
    }

    /// Phase (b): the confirmed spender is a LIVE tx of ours whose bytes the
    /// DB lacks — re-linked, never discarded.
    #[tokio::test]
    async fn phase_b_relinks_when_the_confirmed_spender_is_ours() {
        let (pool, child_txid, _) = fixture().await;
        sqlx::query(
            "INSERT INTO transactions (transaction_id, txid, status) VALUES (2, ?, 'completed')",
        )
        .bind(&child_txid)
        .execute(&pool)
        .await
        .unwrap();
        let spender = child_txid.to_ascii_uppercase();
        let r = reconcile_outputs_with(&pool, true, 100, move |_s, _v| {
            let t = spender.clone();
            async move {
                InputSpend::SpentBy {
                    txid: t,
                    confirmed: true,
                }
            }
        })
        .await
        .unwrap();
        assert_eq!(r.relinked_from_chain_count, 1);
        assert_eq!(r.relinquished_count, 0);
        assert_eq!(lock(&pool, 1).await, (0, Some(2)));
    }

    /// Phase (b): unspent stays spendable; unknown and an unconfirmed
    /// competitor are untouched and reported.
    #[tokio::test]
    async fn phase_b_keeps_unspent_and_never_moves_an_unknown() {
        let (pool, _, _) = fixture().await;
        let r = reconcile_outputs_with(&pool, true, 100, |_s, _v| async { InputSpend::Unspent })
            .await
            .unwrap();
        assert_eq!(r.unspent_count, 1);
        assert_eq!(lock(&pool, 1).await, (1, None));
        let r = reconcile_outputs_with(&pool, true, 100, |_s, _v| async { InputSpend::Unknown })
            .await
            .unwrap();
        assert_eq!(r.unknown_count, 1);
        assert_eq!(r.unknown, vec![format!("{}.0", "cd".repeat(32))]);
        assert_eq!(
            lock(&pool, 1).await,
            (1, None),
            "an unknown never moves money"
        );
        let r = reconcile_outputs_with(&pool, true, 100, |_s, _v| async {
            InputSpend::SpentBy {
                txid: "98".repeat(32),
                confirmed: false,
            }
        })
        .await
        .unwrap();
        assert_eq!(r.unknown_count, 1);
        assert_eq!(lock(&pool, 1).await, (1, None));
    }

    /// The chain-check bound: at most `max` probes per pass, and the report
    /// says more remain.
    #[tokio::test]
    async fn phase_b_is_bounded_per_pass() {
        let (pool, _, _) = fixture().await;
        for vout in 1..=4 {
            sqlx::query("INSERT INTO outputs (transaction_id, spendable, spent_by, satoshis, vout) VALUES (1, 1, NULL, 10, ?)")
                .bind(vout)
                .execute(&pool)
                .await
                .unwrap();
        }
        let r = reconcile_outputs_with(&pool, false, 2, |_s, _v| async { InputSpend::Unspent })
            .await
            .unwrap();
        assert_eq!(r.chain_checked, 2);
        assert!(r.more_to_check);
    }
}
