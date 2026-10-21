//! The store's two doors, as this crate uses them (bsv-wallet-cli #44,
//! toolbox 0.11.0).
//!
//! `StorageSqlx` opens two pools over the one file: `pool()` is the READER
//! (several connections, each `PRAGMA query_only = ON`, so a write sent
//! there fails loudly), and `writer()` is exactly one connection. Every
//! transaction here opens on `write_tx()` (`BEGIN IMMEDIATE` on the writer:
//! the write lock before the first read, so the loser waits instead of
//! answering 517), and every single-statement write runs on `writer()`.
//! Both pools carry the store's pragmas (`busy_timeout` 5000 among them).
//! No write transaction is held across a service or header await.
//!
//! A store fault that is transient, the store busy, is told apart by its
//! type ([`is_store_busy`]), never by its words.

use anyhow::Result;
use sqlx::SqlitePool;

/// The table `table` has the column `column`, read on the reader. The
/// schema helpers ask this first and send their DDL to the writer only
/// when something is missing, so a read path never queues for the writer.
pub async fn has_column(reader: &SqlitePool, table: &str, column: &str) -> Result<bool> {
    let n: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM pragma_table_info(?) WHERE name = ?")
        .bind(table)
        .bind(column)
        .fetch_one(reader)
        .await?;
    Ok(n > 0)
}

/// Is `e` the store busy: SQLite's `SQLITE_BUSY` (primary code 5, 517
/// included) or the writer not acquired within its ceiling? Read by type:
/// a `sqlx::Error` in the chain (its SQLite code, or the pool's acquire
/// timeout), or the toolbox's typed `StoreBusy` (from `write_tx`). Never by
/// the error's words.
pub fn is_store_busy(e: &anyhow::Error) -> bool {
    e.chain().any(|cause| {
        if let Some(sqlx_error) = cause.downcast_ref::<sqlx::Error>() {
            return sqlx_is_busy(sqlx_error);
        }
        matches!(
            cause.downcast_ref::<bsv_wallet_toolbox::Error>(),
            Some(bsv_wallet_toolbox::Error::StoreBusy { .. })
        )
    })
}

/// A `sqlx::Error` that is the store busy: SQLite code 5 by its primary
/// code, or a pool's acquire timeout (the writer's ceiling: the reader's
/// pool is never waited on long enough to time out in this crate).
pub fn sqlx_is_busy(e: &sqlx::Error) -> bool {
    match e {
        sqlx::Error::PoolTimedOut => true,
        sqlx::Error::Database(db) => db
            .code()
            .and_then(|code| code.parse::<i64>().ok())
            .is_some_and(|code| code & 0xff == 5),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_the_typed_busy_is_busy() {
        assert!(is_store_busy(&anyhow::Error::new(
            sqlx::Error::PoolTimedOut
        )));
        assert!(is_store_busy(&anyhow::Error::new(
            bsv_wallet_toolbox::Error::StoreBusy {
                reason: bsv_wallet_toolbox::StoreBusyReason::WriterNotAcquired,
            }
        )));
        assert!(is_store_busy(
            &anyhow::Error::new(sqlx::Error::PoolTimedOut).context("spend gate: hold")
        ));
        // The words alone are no signal.
        assert!(!is_store_busy(&anyhow::anyhow!(
            "Store busy: database is locked (code: 517)"
        )));
        assert!(!is_store_busy(&anyhow::Error::new(
            sqlx::Error::RowNotFound
        )));
    }
}
