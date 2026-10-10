//! The host's spend guard against an immediate-refusal hint
//! (bsv-stack-lean #66; its tracker charter section 4, `Reask.spend`:
//! confirm the word before the coin is used).
//!
//! From the toolbox 0.7.4 a transaction whose immediate post drew no
//! accepting word keeps its word: an internalized one stays `unproven`, and
//! its outputs stay `spendable`, so the toolbox's coin selection (change
//! outputs of `completed` and `unproven` transactions) would spend a coin
//! whose transaction no broadcaster took and no status source holds. A
//! spend of it is posted behind a parent the network may never accept. A
//! `createAction` refused on its post stays `sending`, which the selection
//! never reads; the guard covers it all the same.
//!
//! The guard holds such a coin before every spend this CLI makes (the
//! served `/createAction`, `send`, `drain`, `split`, `gift-send`) and on
//! each served pass: it marks the coin not spendable and records the hold
//! in its own table, `spend_guard_holds`. A hold is released, and the coin
//! spendable again, once a status source holds its transaction (the
//! toolbox promotes the request to `unmined`, or a proof completes it). A
//! hold whose transaction is retired (`failed`) is dropped without
//! restoring: the retire's word stands. The guard writes no transaction or
//! request word; it changes only what it held, and only back.
//!
//! The hint is the toolbox's `immediateBroadcastHint` note on the
//! request's history, with the request still `unsent` (or `sending`,
//! `unprocessed`): no status source has held the transaction since.

use anyhow::Result;
use sqlx::{Row, SqlitePool};

use crate::server::post_word::{last_note, IMMEDIATE_HINT};

/// What one run of the guard did.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct GuardReport {
    /// Transactions whose coins were held this run (txids).
    pub held: Vec<String>,
    /// Transactions whose coins were released this run: a status source
    /// holds them now (txids).
    pub released: Vec<String>,
    /// Holds dropped without restoring: the transaction was retired.
    pub dropped: Vec<String>,
}

impl GuardReport {
    /// Nothing moved.
    pub fn is_quiet(&self) -> bool {
        self.held.is_empty() && self.released.is_empty() && self.dropped.is_empty()
    }
}

async fn ensure_schema(pool: &SqlitePool) -> Result<()> {
    sqlx::query(
        "CREATE TABLE IF NOT EXISTS spend_guard_holds (\
            output_id INTEGER PRIMARY KEY, \
            txid TEXT NOT NULL, \
            held_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP)",
    )
    .execute(pool)
    .await?;
    Ok(())
}

/// Release what a status source now holds, then hold every spendable coin
/// of a transaction whose last word is an immediate-refusal hint.
pub async fn run(pool: &SqlitePool) -> Result<GuardReport> {
    ensure_schema(pool).await?;
    let mut report = GuardReport::default();

    // 1. Release.
    let held: Vec<(String, Option<String>, Option<String>)> = sqlx::query_as(
        "SELECT DISTINCT h.txid, t.status, r.status FROM spend_guard_holds h \
         LEFT JOIN transactions t ON t.txid = h.txid \
         LEFT JOIN proven_tx_reqs r ON r.txid = h.txid",
    )
    .fetch_all(pool)
    .await?;
    for (txid, tx_status, req_status) in held {
        let tx_status = tx_status.unwrap_or_default();
        let req_status = req_status.unwrap_or_default();
        if tx_status == "failed" || matches!(req_status.as_str(), "invalid" | "doubleSpend") {
            sqlx::query("DELETE FROM spend_guard_holds WHERE txid = ?")
                .bind(&txid)
                .execute(pool)
                .await?;
            report.dropped.push(txid);
        } else if tx_status == "completed" || matches!(req_status.as_str(), "unmined" | "completed")
        {
            let mut tx = pool.begin().await?;
            sqlx::query(
                "UPDATE outputs SET spendable = 1 WHERE spendable = 0 AND spent_by IS NULL \
                 AND output_id IN (SELECT output_id FROM spend_guard_holds WHERE txid = ?)",
            )
            .bind(&txid)
            .execute(&mut *tx)
            .await?;
            sqlx::query("DELETE FROM spend_guard_holds WHERE txid = ?")
                .bind(&txid)
                .execute(&mut *tx)
                .await?;
            tx.commit().await?;
            tracing::info!(txid = %txid, "spend guard: a status source holds it; its coins are selectable again");
            report.released.push(txid);
        }
    }

    // 2. Hold.
    let rows = sqlx::query(
        "SELECT o.output_id, t.txid, r.history FROM outputs o \
         JOIN transactions t ON o.transaction_id = t.transaction_id \
         JOIN proven_tx_reqs r ON r.txid = t.txid \
         WHERE o.spendable = 1 AND o.spent_by IS NULL \
           AND t.status IN ('unproven', 'sending') \
           AND r.status IN ('unsent', 'sending', 'unprocessed') \
           AND r.history LIKE ?",
    )
    .bind(format!("%{IMMEDIATE_HINT}%"))
    .fetch_all(pool)
    .await?;
    for row in rows {
        let output_id: i64 = row.get("output_id");
        let txid: String = row.get("txid");
        let history: String = row.get("history");
        if last_note(&history, IMMEDIATE_HINT).is_none() {
            continue;
        }
        let mut tx = pool.begin().await?;
        sqlx::query("INSERT OR IGNORE INTO spend_guard_holds (output_id, txid) VALUES (?, ?)")
            .bind(output_id)
            .bind(&txid)
            .execute(&mut *tx)
            .await?;
        sqlx::query(
            "UPDATE outputs SET spendable = 0 WHERE output_id = ? AND spendable = 1 AND spent_by IS NULL",
        )
        .bind(output_id)
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        if !report.held.contains(&txid) {
            tracing::warn!(
                txid = %txid,
                "spend guard: its immediate post drew no accepting word and no status source holds it; its coins are not selected until one does"
            );
            report.held.push(txid);
        }
    }
    Ok(report)
}
