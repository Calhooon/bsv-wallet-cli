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
//!
//! The report MEASURES (#53): the default basket's spendable sum (what
//! `balance` reads) before the pass and after it, and the rows the pass
//! turned off, per basket, from the UPDATEs that actually changed a row.
//! A protocol output in another basket turns off without the balance
//! moving; a row phase (a) finds already off is linked and named apart.

use anyhow::Result;
use bsv_sdk::transaction::Transaction;
use sqlx::Row;
use std::future::Future;

use crate::commands::balance::{
    balance_line, default_basket_sats, tally, tally_lines, BasketTally, BASKET_NAME_SQL,
};
use crate::commands::cleanup_abandoned::{probe_input_spend, stored_locking_script, InputSpend};
use crate::context::WalletContext;
use crate::explorers::Explorers;

#[derive(Default, Debug, Clone)]
pub struct OutputsReport {
    /// Live txs whose raw bytes were parsed in phase (a).
    pub live_txs: usize,
    /// Phase (a): outputs re-linked to a live tx of ours that spends them
    /// and turned off (they read `spendable = 1`).
    pub relinked_count: u64,
    pub relinked_sats: u64,
    /// Phase (a): outputs a live tx of ours spends that read `spent_by` NULL
    /// but were already `spendable = 0`: linked, never in the sats above.
    pub already_off_count: u64,
    pub already_off_sats: u64,
    /// Phase (b): spendable outputs chain-checked this pass.
    pub chain_checked: usize,
    /// Phase (b): outputs spent on chain by a confirmed tx not in the DB — gone.
    pub relinquished_count: u64,
    pub relinquished_sats: u64,
    /// Phase (b): confirmed spender turned out to be a live tx of ours — re-linked.
    pub relinked_from_chain_count: u64,
    pub relinked_from_chain_sats: u64,
    /// Phase (b): verified unspent — kept spendable.
    pub unspent_count: u64,
    /// Phase (b): unknown / unconfirmed competitor — untouched, reported.
    pub unknown_count: u64,
    /// Outpoints (`txid.vout`) the chain could not vouch for.
    pub unknown: Vec<String>,
    /// Phase (b) stopped at the bound; another pass continues.
    pub more_to_check: bool,
    pub applied: bool,
    /// The default basket's spendable sum (what `balance` reads) before the pass.
    pub balance_before: u64,
    /// The same read after the pass; `None` on a dry run.
    pub balance_after: Option<u64>,
    /// Rows the pass turned off (`spendable` 1 -> 0), per basket; on a dry
    /// run, the rows it would.
    pub turned_off: Vec<BasketTally>,
}

/// One tracked spendable output: the coin and its outpoint.
struct SpendableOutput {
    output_id: i64,
    satoshis: i64,
    basket: String,
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
    let mut report = OutputsReport {
        balance_before: default_basket_sats(pool).await?,
        ..OutputsReport::default()
    };

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
            let hit = sqlx::query(&format!(
                "SELECT o.output_id, o.satoshis, o.spendable, {BASKET_NAME_SQL} AS basket \
                 FROM outputs o JOIN transactions p ON p.transaction_id = o.transaction_id \
                 WHERE lower(p.txid) = ? AND o.vout = ? AND o.spent_by IS NULL"
            ))
            .bind(src.to_ascii_lowercase())
            .bind(i64::from(input.source_output_index))
            .fetch_optional(pool)
            .await?;
            let Some(o) = hit else {
                continue;
            };
            let output_id: i64 = o.get("output_id");
            let sats: i64 = o.get("satoshis");
            let basket: String = o.get("basket");
            // Was it on? The dry run reads it; the execute run asks the
            // UPDATE that turns it off, so a row another writer moved
            // meanwhile is counted as it actually went.
            let turned_off = if execute {
                sqlx::query(
                    "UPDATE outputs SET spendable = 0, spent_by = ?, \
                     updated_at = CURRENT_TIMESTAMP \
                     WHERE output_id = ? AND spent_by IS NULL AND spendable = 1",
                )
                .bind(tx_id)
                .bind(output_id)
                .execute(pool)
                .await?
                .rows_affected()
                    == 1
            } else {
                o.get::<i64, _>("spendable") == 1
            };
            if turned_off {
                report.relinked_count += 1;
                report.relinked_sats += sats.max(0) as u64;
                tally(&mut report.turned_off, &basket, sats);
                continue;
            }
            // Already off: it still gets its spent_by link.
            let linked = !execute
                || sqlx::query(
                    "UPDATE outputs SET spendable = 0, spent_by = ?, \
                     updated_at = CURRENT_TIMESTAMP WHERE output_id = ? AND spent_by IS NULL",
                )
                .bind(tx_id)
                .bind(output_id)
                .execute(pool)
                .await?
                .rows_affected()
                    == 1;
            if linked {
                report.already_off_count += 1;
                report.already_off_sats += sats.max(0) as u64;
            }
        }
    }

    // ── (b) chain check of what still reads spendable ──
    let candidates: Vec<SpendableOutput> = sqlx::query(&format!(
        "SELECT o.output_id, o.satoshis, o.vout, p.txid AS src, {BASKET_NAME_SQL} AS basket \
         FROM outputs o JOIN transactions p ON p.transaction_id = o.transaction_id \
         WHERE o.spendable = 1 AND o.spent_by IS NULL \
           AND p.status IN ('completed', 'unproven') \
         ORDER BY o.output_id ASC LIMIT ?"
    ))
    .bind((max_chain_checks + 1) as i64)
    .fetch_all(pool)
    .await?
    .iter()
    .map(|r| SpendableOutput {
        output_id: r.get("output_id"),
        satoshis: r.get::<i64, _>("satoshis"),
        basket: r.get::<String, _>("basket"),
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
                // Every candidate read `spendable = 1`; the UPDATE is
                // guarded on it, so a row counts only if this pass turned it off.
                let turned_off = !execute
                    || sqlx::query(
                        "UPDATE outputs SET spendable = 0, spent_by = ?, \
                         updated_at = CURRENT_TIMESTAMP \
                         WHERE output_id = ? AND spendable = 1 AND spent_by IS NULL",
                    )
                    .bind(ours.map(|(tx_id,)| tx_id))
                    .bind(c.output_id)
                    .execute(pool)
                    .await?
                    .rows_affected()
                        == 1;
                if !turned_off {
                    continue;
                }
                tally(&mut report.turned_off, &c.basket, c.satoshis);
                if ours.is_some() {
                    report.relinked_from_chain_count += 1;
                    report.relinked_from_chain_sats += c.satoshis.max(0) as u64;
                } else {
                    report.relinquished_count += 1;
                    report.relinquished_sats += c.satoshis.max(0) as u64;
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
    if execute {
        report.balance_after = Some(default_basket_sats(pool).await?);
    }
    Ok(report)
}

pub async fn run(ctx: &WalletContext, execute: bool, max_chain_checks: usize) -> Result<()> {
    // The context's store, never a second open of the file (D2 of the #44
    // delta review): its writer, the one connection every write in this
    // process queues on. Every statement below is a single autocommit and
    // no connection is held across a probe's network await.
    let pool = ctx.wallet.storage().writer().clone();
    let explorers = Explorers::from_env(ctx.chain);
    let report = reconcile_outputs_with(&pool, execute, max_chain_checks, |src, vout| {
        let explorers = explorers.clone();
        let pool = &pool;
        async move {
            let script = stored_locking_script(pool, &src, vout).await;
            probe_input_spend(
                &explorers,
                ctx.wallet.services(),
                &src,
                vout,
                script.as_deref(),
            )
            .await
        }
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
        "  already off: {} rows, {} sats{}",
        report.already_off_count,
        report.already_off_sats,
        if execute {
            " — linked to their spender"
        } else {
            " — would link to their spender"
        }
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
        "  spent by a live tx of ours the DB lacked bytes for{}: {} ({} sats)",
        if execute {
            " — re-linked"
        } else {
            " — would re-link"
        },
        report.relinked_from_chain_count,
        report.relinked_from_chain_sats
    );
    println!(
        "  unknown / unconfirmed competitor (untouched — an unknown never moves money): {}",
        report.unknown_count
    );
    for op in &report.unknown {
        println!("    unknown: {}", op);
    }
    println!();
    println!(
        "{}",
        balance_line(report.balance_before, report.balance_after)
    );
    let verb = if execute {
        "Turned off"
    } else {
        "Would turn off"
    };
    for line in tally_lines(verb, &report.turned_off) {
        println!("{}", line);
    }
    if !execute {
        println!();
        println!("Dry run. Re-run with --execute to apply.");
        return Ok(());
    }
    println!("Restart the daemon to refresh its in-memory view.");
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
                basket_id INTEGER, updated_at TEXT)",
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "CREATE TABLE output_baskets (
                basket_id INTEGER PRIMARY KEY, name TEXT NOT NULL,
                is_deleted INTEGER NOT NULL DEFAULT 0)",
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO output_baskets (basket_id, name) VALUES (1, 'default'), (2, 'protocol')",
        )
        .execute(&pool)
        .await
        .unwrap();
        // Parent tx 1 (`cd…`, completed) with output vout 0 = output 1, shown
        // UNSPENT, in the default basket.
        let parent_txid = "cd".repeat(32);
        sqlx::query(
            "INSERT INTO transactions (transaction_id, txid, status) VALUES (1, ?, 'completed')",
        )
        .bind(&parent_txid)
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query("INSERT INTO outputs (output_id, transaction_id, spendable, spent_by, satoshis, vout, basket_id) VALUES (1, 1, 1, NULL, 5000, 0, 1)")
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
            "spent by the live child"
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

    /// The measured move of the default basket's balance; `None` on a dry run.
    fn delta(r: &OutputsReport) -> Option<i64> {
        r.balance_after.map(|a| a as i64 - r.balance_before as i64)
    }

    /// A stranger's confirmed spend of every probed coin.
    async fn spent_by_a_stranger(_s: String, _v: u32) -> InputSpend {
        InputSpend::SpentBy {
            txid: "98".repeat(32),
            confirmed: true,
        }
    }

    /// #53 (p3, p7, p35): a protocol output in another basket, spent on chain
    /// by a counterparty's confirmed tx, is relinquished, and the balance
    /// does not move: the measured delta is 0 and the row shows under its
    /// own basket.
    #[tokio::test]
    async fn a_relinquished_output_in_another_basket_leaves_the_balance_where_it_was() {
        let (pool, _, _) = fixture().await;
        sqlx::query("UPDATE outputs SET basket_id = 2 WHERE output_id = 1")
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("INSERT INTO outputs (transaction_id, spendable, spent_by, satoshis, vout, basket_id) VALUES (1, 1, NULL, 700, 1, 1)")
            .execute(&pool)
            .await
            .unwrap();
        let r = reconcile_outputs_with(&pool, true, 100, |s, v| async move {
            if v == 0 {
                spent_by_a_stranger(s, v).await
            } else {
                InputSpend::Unspent
            }
        })
        .await
        .unwrap();
        assert_eq!((r.relinquished_count, r.relinquished_sats), (1, 5000));
        assert_eq!((r.balance_before, r.balance_after), (700, Some(700)));
        assert_eq!(delta(&r), Some(0));
        assert_eq!(
            r.turned_off,
            vec![BasketTally {
                basket: "protocol".into(),
                rows: 1,
                sats: 5000
            }]
        );
        assert_eq!(
            tally_lines("Turned off", &r.turned_off),
            vec!["Turned off: 1 rows, 5000 sats in protocol".to_string()]
        );
    }

    /// #53: phase (a) counts only rows that were on. A row its live spender
    /// consumed that already reads `spendable = 0` is "already off", never in
    /// the re-linked sats, and still gets its `spent_by` link.
    #[tokio::test]
    async fn a_phase_a_row_already_off_is_named_apart_and_still_linked() {
        let (pool, child_txid, child_raw) = fixture().await;
        sqlx::query("UPDATE outputs SET spendable = 0 WHERE output_id = 1")
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("INSERT INTO transactions (transaction_id, txid, status, raw_tx) VALUES (2, ?, 'completed', ?)")
            .bind(&child_txid)
            .bind(&child_raw)
            .execute(&pool)
            .await
            .unwrap();
        let dry = reconcile_outputs_with(&pool, false, 100, |_s, _v| async { InputSpend::Unspent })
            .await
            .unwrap();
        assert_eq!((dry.relinked_count, dry.relinked_sats), (0, 0));
        assert_eq!((dry.already_off_count, dry.already_off_sats), (1, 5000));
        assert!(dry.turned_off.is_empty());
        assert_eq!(dry.balance_after, None, "a dry run reads the balance once");
        let wet = reconcile_outputs_with(&pool, true, 100, |_s, _v| async { InputSpend::Unspent })
            .await
            .unwrap();
        assert_eq!((wet.relinked_count, wet.relinked_sats), (0, 0));
        assert_eq!((wet.already_off_count, wet.already_off_sats), (1, 5000));
        assert!(wet.turned_off.is_empty());
        assert_eq!(delta(&wet), Some(0));
        assert_eq!(lock(&pool, 1).await, (0, Some(2)), "linked to its spender");
    }

    /// #53: the measured delta is the default-basket rows the pass turned
    /// off, and nothing else (a phase (a) re-link and a phase (b)
    /// relinquish, beside a protocol-basket relinquish).
    #[tokio::test]
    async fn the_measured_delta_equals_the_default_basket_rows_turned_off() {
        let (pool, child_txid, child_raw) = fixture().await;
        sqlx::query("INSERT INTO transactions (transaction_id, txid, status, raw_tx) VALUES (2, ?, 'completed', ?)")
            .bind(&child_txid)
            .bind(&child_raw)
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("INSERT INTO outputs (transaction_id, spendable, spent_by, satoshis, vout, basket_id) VALUES (1, 1, NULL, 1200, 1, 1), (1, 1, NULL, 300, 2, 2), (1, 1, NULL, 50, 3, 1)")
            .execute(&pool)
            .await
            .unwrap();
        let r = reconcile_outputs_with(&pool, true, 100, |s, v| async move {
            if v == 3 {
                InputSpend::Unspent
            } else {
                spent_by_a_stranger(s, v).await
            }
        })
        .await
        .unwrap();
        assert_eq!(r.balance_before, 5000 + 1200 + 50);
        let default_off = r.turned_off.iter().find(|t| t.basket == "default").unwrap();
        assert_eq!((default_off.rows, default_off.sats), (2, 6200));
        assert_eq!(delta(&r), Some(-(default_off.sats as i64)));
        assert_eq!(r.balance_after, Some(50));
        assert_eq!(
            balance_line(r.balance_before, r.balance_after),
            "Balance (default basket): 6250 -> 50 (-6200 sats)"
        );
    }
}
