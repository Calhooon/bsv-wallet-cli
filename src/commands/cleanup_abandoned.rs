use anyhow::{Context, Result};
use sqlx::Row;

use crate::commands::receive;
use crate::context::WalletContext;

pub async fn run(ctx: &WalletContext, db_path: &str, execute: bool) -> Result<()> {
    let pool = sqlx::SqlitePool::connect(&format!("sqlite:{}", db_path))
        .await
        .with_context(|| format!("failed to open {} (is the daemon running?)", db_path))?;

    let rows = sqlx::query("SELECT transaction_id, txid FROM transactions WHERE status='unproven'")
        .fetch_all(&pool)
        .await?;

    if rows.is_empty() {
        println!("No unproven transactions found.");
        return Ok(());
    }
    println!("Found {} unproven transaction(s); checking WoC...", rows.len());

    let base = receive::woc_base(ctx.chain);
    let client = reqwest::Client::new();
    let mut to_fail: Vec<(i64, String)> = Vec::new();
    let mut still_in_mempool: Vec<String> = Vec::new();

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
            still_in_mempool.push(txid);
        }
    }

    println!(
        "  Missing on chain: {}    Still tracked by network: {}",
        to_fail.len(),
        still_in_mempool.len()
    );
    for txid in &still_in_mempool {
        println!("    keep: {}", txid);
    }
    for (_, txid) in &to_fail {
        println!("    fail: {}", txid);
    }

    if to_fail.is_empty() {
        println!("Nothing to clean up.");
        return Ok(());
    }

    if !execute {
        println!();
        println!("Dry run. Re-run with --execute to apply.");
        return Ok(());
    }

    let ids: Vec<i64> = to_fail.iter().map(|(id, _)| *id).collect();

    let restored = restore_inputs(&pool, &ids).await?;
    let phantoms = remove_phantom_outputs(&pool, &ids).await?;
    let failed = mark_failed(&pool, &ids).await?;

    println!();
    println!("Applied:");
    println!("  Transactions marked failed: {}", failed);
    println!("  Inputs restored to spendable: {} ({} sats)", restored.0, restored.1);
    println!("  Phantom outputs unspendable: {} ({} sats)", phantoms.0, phantoms.1);
    println!();
    println!(
        "Net balance delta: {:+} sats. Restart the daemon to refresh its in-memory view.",
        restored.1 as i64 - phantoms.1 as i64
    );

    Ok(())
}

async fn restore_inputs(pool: &sqlx::SqlitePool, ids: &[i64]) -> Result<(u64, u64)> {
    let mut count = 0u64;
    let mut sats = 0u64;
    let mut tx = pool.begin().await?;
    for id in ids {
        let restored = sqlx::query(
            "SELECT output_id, satoshis FROM outputs WHERE spent_by = ?",
        )
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
