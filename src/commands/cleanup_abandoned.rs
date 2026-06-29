use anyhow::{Context, Result};
use bsv_wallet_toolbox::Chain;
use sqlx::Row;

use crate::commands::receive;
use crate::context::WalletContext;

/// Summary of one reconcile pass over abandoned transactions.
#[derive(Default, Debug, Clone)]
pub struct ReconcileReport {
    /// `unproven` txs inspected (after the min-age filter).
    pub checked: usize,
    /// txids still tracked by the network (kept spendable).
    pub kept: Vec<String>,
    /// txids missing on chain (HTTP 404) — the abandoned set.
    pub abandoned: Vec<String>,
    /// Whether `execute` actually applied the cleanup.
    pub applied: bool,
    /// Transactions transitioned `unproven` -> `failed`.
    pub failed: u64,
    /// Inputs of abandoned txs restored to spendable.
    pub restored_count: u64,
    pub restored_sats: u64,
    /// Phantom outputs of abandoned txs invalidated (spendable=0).
    pub phantom_count: u64,
    pub phantom_sats: u64,
}

/// Core reconcile, shared by the CLI `cleanup-abandoned` command and the daemon's
/// periodic ticker.
///
/// Scans `status='unproven'` transactions that are at least `min_age_secs` old
/// (so a freshly-broadcast tx that has not yet propagated to WhatsOnChain is
/// never mis-classified), checks each against WoC, and — when `execute` — fails
/// the ones missing on chain: restore their inputs, invalidate their own phantom
/// outputs, and mark them `failed` (which excludes them from coin selection,
/// preventing a never-landed tx's change from funding — and orphaning — a new
/// transaction).
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
    let mut report = ReconcileReport::default();

    // `datetime('now', '-N seconds')`: only consider txs at least N seconds old.
    // min_age_secs == 0 -> '-0 seconds' == now, i.e. no effective age guard
    // (the CLI command's historical behavior).
    let age_modifier = format!("-{} seconds", min_age_secs.max(0));
    let rows = sqlx::query(
        "SELECT transaction_id, txid FROM transactions \
         WHERE status='unproven' AND created_at <= datetime('now', ?)",
    )
    .bind(&age_modifier)
    .fetch_all(pool)
    .await?;

    report.checked = rows.len();
    if rows.is_empty() {
        return Ok(report);
    }

    let base = receive::woc_base(chain);
    let client = reqwest::Client::new();
    let mut to_fail: Vec<(i64, String)> = Vec::new();

    for row in &rows {
        let tx_id: i64 = row.get("transaction_id");
        let txid: String = row.get("txid");
        let resp = client
            .get(format!("{}/tx/{}", base, txid))
            .send()
            .await
            .with_context(|| format!("WoC tx fetch failed for {}", txid))?;
        if resp.status().as_u16() == 404 {
            to_fail.push((tx_id, txid));
        } else {
            report.kept.push(txid);
        }
    }
    report.abandoned = to_fail.iter().map(|(_, t)| t.clone()).collect();

    if to_fail.is_empty() || !execute {
        return Ok(report);
    }

    let ids: Vec<i64> = to_fail.iter().map(|(id, _)| *id).collect();
    let restored = restore_inputs(pool, &ids).await?;
    let phantoms = remove_phantom_outputs(pool, &ids).await?;
    let failed = mark_failed(pool, &ids).await?;

    report.applied = true;
    report.failed = failed;
    report.restored_count = restored.0;
    report.restored_sats = restored.1;
    report.phantom_count = phantoms.0;
    report.phantom_sats = phantoms.1;
    Ok(report)
}

pub async fn run(ctx: &WalletContext, db_path: &str, execute: bool) -> Result<()> {
    let pool = sqlx::SqlitePool::connect(&format!("sqlite:{}", db_path))
        .await
        .with_context(|| format!("failed to open {} (is the daemon running?)", db_path))?;

    // Operator-initiated: no age guard (inspect every unproven tx).
    let report = reconcile(&pool, ctx.chain, 0, execute).await?;

    if report.checked == 0 {
        println!("No unproven transactions found.");
        return Ok(());
    }
    println!(
        "Found {} unproven transaction(s); checked against WoC.",
        report.checked
    );
    println!(
        "  Missing on chain: {}    Still tracked by network: {}",
        report.abandoned.len(),
        report.kept.len()
    );
    for txid in &report.kept {
        println!("    keep: {}", txid);
    }
    for txid in &report.abandoned {
        println!("    fail: {}", txid);
    }

    if report.abandoned.is_empty() {
        println!("Nothing to clean up.");
        return Ok(());
    }
    if !execute {
        println!();
        println!("Dry run. Re-run with --execute to apply.");
        return Ok(());
    }

    println!();
    println!("Applied:");
    println!("  Transactions marked failed: {}", report.failed);
    println!(
        "  Inputs restored to spendable: {} ({} sats)",
        report.restored_count, report.restored_sats
    );
    println!(
        "  Phantom outputs unspendable: {} ({} sats)",
        report.phantom_count, report.phantom_sats
    );
    println!();
    println!(
        "Net balance delta: {:+} sats. Restart the daemon to refresh its in-memory view.",
        report.restored_sats as i64 - report.phantom_sats as i64
    );

    Ok(())
}

async fn restore_inputs(pool: &sqlx::SqlitePool, ids: &[i64]) -> Result<(u64, u64)> {
    let mut count = 0u64;
    let mut sats = 0u64;
    let mut tx = pool.begin().await?;
    for id in ids {
        let restored = sqlx::query("SELECT output_id, satoshis FROM outputs WHERE spent_by = ?")
            .bind(id)
            .fetch_all(&mut *tx)
            .await?;
        for r in &restored {
            count += 1;
            sats += r.get::<i64, _>("satoshis") as u64;
        }
        sqlx::query(
            "UPDATE outputs SET spendable = 1, spent_by = NULL, \
             updated_at = CURRENT_TIMESTAMP WHERE spent_by = ?",
        )
        .bind(id)
        .execute(&mut *tx)
        .await?;
    }
    tx.commit().await?;
    Ok((count, sats))
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
