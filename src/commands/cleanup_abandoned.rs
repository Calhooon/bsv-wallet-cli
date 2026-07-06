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

    let rows = select_stale_unproven(pool, min_age_secs).await?;

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
    Ok(sqlx::query(
        "SELECT transaction_id, txid FROM transactions \
         WHERE status='unproven' AND datetime(created_at) <= datetime('now', ?)",
    )
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

#[cfg(test)]
mod tests {
    use super::*;
    use sqlx::Row;

    async fn mem_pool_with(rows: &[(&str, &str, &str)]) -> sqlx::SqlitePool {
        let pool = sqlx::SqlitePool::connect("sqlite::memory:").await.unwrap();
        sqlx::query(
            "CREATE TABLE transactions (
                transaction_id INTEGER PRIMARY KEY AUTOINCREMENT,
                txid TEXT NOT NULL, status TEXT NOT NULL, created_at TEXT NOT NULL)",
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
            ("aa".repeat(32).leak(), "unproven", "2020-01-01T00:00:00.123456+00:00"),
            ("bb".repeat(32).leak(), "unproven", "2020-01-02 00:00:00"),
            ("cc".repeat(32).leak(), "failed", "2020-01-01T00:00:00+00:00"), // wrong status
        ])
        .await;
        let rows = select_stale_unproven(&pool, 0).await.unwrap();
        let txids: Vec<String> = rows.iter().map(|r| r.get("txid")).collect();
        assert_eq!(txids.len(), 2, "both timestamp formats must be swept: {txids:?}");
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
        assert_eq!(txids, vec!["old_iso".to_string()], "fresh rows must be age-guarded");
        let unguarded = select_stale_unproven(&pool, 0).await.unwrap();
        assert_eq!(unguarded.len(), 3, "0-second guard selects everything");
    }
}
