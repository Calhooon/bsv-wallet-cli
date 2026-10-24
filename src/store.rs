//! The store's two doors, as this crate uses them (bsv-wallet-cli #44,
//! toolbox 0.11.0).
//!
//! `StorageSqlx` opens two pools over the one file: `pool()` is the READER
//! (several connections, each `PRAGMA query_only = ON`, so a write sent
//! there fails loudly), and `writer()` is exactly one connection. Every
//! transaction here opens on `write_tx()` (`BEGIN IMMEDIATE` on the writer:
//! the write lock before the first read, so the loser waits instead of
//! answering 517), and every single-statement write runs on a connection of
//! `writer()`, acquired through [`writer`].
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

/// One store per file per process (F9 (b) of the #44 CLI review): the file
/// opened once, made available, and answered as two handles of the one
/// store, the first for the `Wallet` (which takes its store by value), the
/// second for the `Monitor` and the chain feed (which take an `Arc`). A
/// `StorageSqlx` clone shares the reader, the one-connection writer, the
/// in-flight reservations and every other piece of state (toolbox 0.11.0),
/// so every writer in the process queues on the one fair writer pool, never
/// on SQLite's busy handler between two pools.
pub async fn open_shared(
    db: &str,
) -> Result<(
    bsv_wallet_toolbox::StorageSqlx,
    std::sync::Arc<bsv_wallet_toolbox::StorageSqlx>,
)> {
    use bsv_wallet_toolbox::WalletStorageWriter;
    let store = bsv_wallet_toolbox::StorageSqlx::open(db).await?;
    store.make_available().await?;
    let monitors = std::sync::Arc::new(store.clone());
    Ok((store, monitors))
}

/// One connection of the store's writer, for this crate's single-statement
/// writes: a writer not acquired within its ceiling is the toolbox's typed
/// `StoreBusy { WriterNotAcquired }`, as `write_tx` answers. The acquire is
/// where the pool's timeout is told apart: a `PoolTimedOut` from any other
/// pool (the reader's 30 s acquire among them) is a plain storage fault.
pub async fn writer(
    storage: &bsv_wallet_toolbox::StorageSqlx,
) -> Result<sqlx::pool::PoolConnection<sqlx::Sqlite>> {
    storage.writer().acquire().await.map_err(|e| match e {
        sqlx::Error::PoolTimedOut => anyhow::Error::new(bsv_wallet_toolbox::Error::StoreBusy {
            reason: bsv_wallet_toolbox::StoreBusyReason::WriterNotAcquired,
        }),
        other => anyhow::Error::new(other),
    })
}

/// Is `e` the store busy: SQLite's `SQLITE_BUSY` (primary code 5, 517
/// included) or the writer not acquired within its ceiling? Read by type:
/// a `sqlx::Error` in the chain with SQLite's busy code, or the toolbox's
/// typed `StoreBusy` (from `write_tx`, or [`writer`]'s acquire). A pool's
/// `PoolTimedOut` alone is not: it does not say which pool timed out, and
/// the reader's is a plain storage fault. Never by the error's words.
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
/// code. The writer's acquire timeout is typed at the acquire ([`writer`],
/// `write_tx`); a bare `PoolTimedOut` is not busy.
pub fn sqlx_is_busy(e: &sqlx::Error) -> bool {
    match e {
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
        // A bare pool timeout does not say which pool: not busy.
        assert!(!is_store_busy(&anyhow::Error::new(
            sqlx::Error::PoolTimedOut
        )));
        assert!(is_store_busy(&anyhow::Error::new(
            bsv_wallet_toolbox::Error::StoreBusy {
                reason: bsv_wallet_toolbox::StoreBusyReason::WriterNotAcquired,
            }
        )));
        assert!(is_store_busy(
            &anyhow::Error::new(bsv_wallet_toolbox::Error::StoreBusy {
                reason: bsv_wallet_toolbox::StoreBusyReason::WriterNotAcquired,
            })
            .context("spend gate: hold")
        ));
        // The words alone are no signal.
        assert!(!is_store_busy(&anyhow::anyhow!(
            "Store busy: database is locked (code: 517)"
        )));
        assert!(!is_store_busy(&anyhow::Error::new(
            sqlx::Error::RowNotFound
        )));
    }

    /// F9 (a) of the #44 CLI review: the writer not acquired within the
    /// store's ceiling is busy, typed at the acquire ([`writer`]); a
    /// reader's acquire timeout is a plain storage fault.
    #[tokio::test]
    async fn only_the_writers_acquire_timeout_is_busy() {
        let storage = bsv_wallet_toolbox::StorageSqlx::in_memory().await.unwrap();
        let held = storage.writer().acquire().await.unwrap();
        let err = writer(&storage)
            .await
            .expect_err("the one writer is held past the ceiling");
        assert!(is_store_busy(&err), "{err}");
        assert!(matches!(
            err.downcast_ref::<bsv_wallet_toolbox::Error>(),
            Some(bsv_wallet_toolbox::Error::StoreBusy {
                reason: bsv_wallet_toolbox::StoreBusyReason::WriterNotAcquired
            })
        ));
        drop(held);
        writer(&storage).await.expect("released, acquired");

        // A reader-shaped pool whose acquire times out.
        let reader = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .acquire_timeout(std::time::Duration::from_millis(50))
            .connect("sqlite::memory:")
            .await
            .unwrap();
        let _held = reader.acquire().await.unwrap();
        let Err(read) = sqlx::query("SELECT 1").fetch_one(&reader).await else {
            panic!("the reader is held");
        };
        assert!(matches!(read, sqlx::Error::PoolTimedOut), "{read}");
        assert!(!is_store_busy(&anyhow::Error::new(read)));
    }

    /// The production half of a source file: everything before its first
    /// `#[cfg(test)]` (a test module opens its own in-memory stores).
    fn production(src: &str) -> &str {
        src.split("#[cfg(test)]").next().unwrap_or(src)
    }

    /// The ways a source file can open the wallet file itself.
    fn direct_opens(src: &str) -> usize {
        let src = production(src);
        [
            "StorageSqlx::open(",
            "SqlitePool::connect(",
            ".connect_with(",
        ]
        .iter()
        .map(|door| src.matches(door).count())
        .sum()
    }

    /// D1 of the #44 delta review: each command that runs a Monitor or a
    /// server beside a Wallet opens its file once. The daemon (and
    /// `serve-fleet --daemon`, per seat) opens it only through
    /// `store::open_shared`; `serve` and `serve-fleet` take it from
    /// `WalletContext::load_with`, the one open in `context.rs`. A second
    /// open for the Wallet beside the Monitor's store passes every
    /// behavioural test, so the sources are read here.
    #[test]
    fn the_daemon_tenant_opens_its_file_only_through_open_shared() {
        let tenant = production(include_str!("commands/daemon_tenant.rs"));
        assert_eq!(tenant.matches("StorageSqlx::open(").count(), 0);
        assert_eq!(tenant.matches("store::open_shared(").count(), 1);
        assert_eq!(direct_opens(tenant), 0);
        assert_eq!(tenant.matches("WalletContext").count(), 0);

        let daemon = production(include_str!("commands/daemon.rs"));
        assert_eq!(direct_opens(daemon), 0);
        assert_eq!(daemon.matches("WalletContext").count(), 0);
        assert_eq!(daemon.matches("daemon_tenant::start(").count(), 1);

        let serve = production(include_str!("commands/serve.rs"));
        assert_eq!(direct_opens(serve), 0);
        assert_eq!(serve.matches("open_shared(").count(), 0);

        // Per seat: the daemon's composition or a context, never both.
        let fleet = production(include_str!("commands/serve_fleet.rs"));
        assert_eq!(direct_opens(fleet), 0);
        assert_eq!(fleet.matches("open_shared(").count(), 0);
        assert_eq!(fleet.matches("daemon_tenant::start(").count(), 1);
        assert_eq!(fleet.matches("WalletContext::load_with(").count(), 1);

        let context = production(include_str!("context.rs"));
        assert_eq!(context.matches("StorageSqlx::open(").count(), 1);
        assert_eq!(direct_opens(context), 1);

        let shared = production(include_str!("store.rs"));
        assert_eq!(shared.matches("StorageSqlx::open(").count(), 1);
    }

    /// D2 of the #44 delta review: no command opens the wallet file beside
    /// its context's store (`reconcile-outputs` once kept its own pool). A
    /// command opens the file itself only when it carries no Wallet at all:
    /// `init` (it creates the file), `tick` (a Monitor alone) and `ui` (the
    /// read-only inspector), each exactly once. Every file in
    /// `src/commands/` is read, so a new command is held to the rule too.
    #[test]
    fn no_command_opens_the_wallet_file_beside_its_store() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/commands");
        let alone = ["init.rs", "tick.rs", "ui.rs"];
        let mut read = 0;
        for entry in std::fs::read_dir(&dir).unwrap() {
            let path = entry.unwrap().path();
            if path.extension().and_then(|e| e.to_str()) != Some("rs") {
                continue;
            }
            let name = path.file_name().unwrap().to_str().unwrap().to_string();
            let src = std::fs::read_to_string(&path).unwrap();
            read += 1;
            if alone.contains(&name.as_str()) {
                assert_eq!(direct_opens(&src), 1, "{name} opens its file once");
                let prod = production(&src);
                for wallet in ["WalletContext", "Wallet::new(", "open_shared("] {
                    assert!(!prod.contains(wallet), "{name} carries no Wallet");
                }
            } else {
                assert_eq!(direct_opens(&src), 0, "{name} opens the wallet file itself");
            }
        }
        assert!(
            read > 20,
            "read {read} command files from {}",
            dir.display()
        );
        let reconcile = production(include_str!("commands/reconcile_outputs.rs"));
        assert_eq!(direct_opens(reconcile), 0);
        assert_eq!(
            reconcile.matches("ctx.wallet.storage().writer()").count(),
            1
        );
    }

    /// A real SQLITE_BUSY: two connections on one file, the second asking
    /// for the write lock the first holds, with no busy timeout.
    #[tokio::test]
    async fn sqlites_own_busy_is_busy() {
        use sqlx::{ConnectOptions, Connection};
        let dir = tempfile::TempDir::new().unwrap();
        let options = crate::test_support::no_wait_file(&dir.path().join("busy.db"));
        let mut holder =
            crate::test_support::busy_holder(&options, "CREATE TABLE t (x INTEGER)").await;
        sqlx::query("BEGIN IMMEDIATE")
            .execute(&mut holder)
            .await
            .unwrap();
        let mut other = options.connect().await.unwrap();
        let err = sqlx::query("BEGIN IMMEDIATE")
            .execute(&mut other)
            .await
            .expect_err("the write lock is held");
        println!("{err}");
        assert!(sqlx_is_busy(&err));
        assert!(is_store_busy(
            &anyhow::Error::new(err).context("spend gate")
        ));
        let write = sqlx::query("INSERT INTO t VALUES (1)")
            .execute(&mut other)
            .await
            .expect_err("still locked");
        assert!(
            sqlx_is_busy(&write),
            "a write under the held lock is busy too"
        );
        sqlx::query("ROLLBACK").execute(&mut holder).await.unwrap();
        // A constraint (2067) is not busy.
        sqlx::query("CREATE UNIQUE INDEX tx ON t (x)")
            .execute(&mut holder)
            .await
            .unwrap();
        sqlx::query("INSERT INTO t VALUES (1)")
            .execute(&mut holder)
            .await
            .unwrap();
        let unique = sqlx::query("INSERT INTO t VALUES (1)")
            .execute(&mut holder)
            .await
            .expect_err("a duplicate");
        assert!(!sqlx_is_busy(&unique), "{unique}");
        holder.close().await.unwrap();
    }
}
