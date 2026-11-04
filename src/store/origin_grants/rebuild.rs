//! The rebuilds of `origin_pending` in place (the design's section (1)):
//! L5a's table gains ids never reused (`AUTOINCREMENT`), and L5b's gains
//! `abandoned_at` with its table `CHECK` (#534). SQLite alters neither a key
//! nor a `CHECK`, so each renames the table aside, runs the block, carries
//! every row and the sequence, and drops the old table, inside the
//! transaction `super::ensure` decided it in. Behind the store's boundary,
//! as the parent's statements.

use anyhow::Result;
use sqlx::SqliteConnection;

use super::{create, CARRIED_COLUMNS};

/// Rebuild L5a's `origin_pending` as the block writes it, its ids never
/// reused (`AUTOINCREMENT`), inside the caller's transaction
/// ([`rebuild_pending`]). From the switch (L5b) a row whose caller left is
/// deleted; without the sequence SQLite would give its id to the next
/// request, and an owner answering `#N` from a listing read before the
/// deletion would answer another page's request.
pub(super) async fn sequence_pending(conn: &mut SqliteConnection) -> Result<()> {
    let carried = rebuild_pending(conn, "origin_pending_unsequenced").await?;
    tracing::info!(
        marker = "origin_grants_sequenced",
        carried,
        "origin grants: the pending requests' ids are never reused from now on ({carried} rows \
         carried)"
    );
    Ok(())
}

/// Rebuild L5b's `origin_pending` with the column `abandoned_at` and its
/// table `CHECK` (#534, the design's L8), inside the caller's transaction
/// ([`rebuild_pending`]): SQLite's `ADD COLUMN` cannot add a table
/// `CHECK`, so a moved store and a fresh one hold the same constraint in
/// the same place. Every row is carried with `abandoned_at` NULL, and the
/// table's sequence with it, so no deleted id is given again.
pub(super) async fn window_pending(conn: &mut SqliteConnection) -> Result<()> {
    let carried = rebuild_pending(conn, "origin_pending_l5b").await?;
    tracing::info!(
        marker = "origin_grants_windowed",
        carried,
        "origin grants: a pending request waits for its page's brief reconnect from now on \
         ({carried} rows carried)"
    );
    Ok(())
}

/// Rename `origin_pending` to `aside`, drop its index, run the block, carry
/// every row with its id (`abandoned_at` NULL), carry `aside`'s sequence
/// when it had one, and drop `aside`. Answers the rows carried.
async fn rebuild_pending(conn: &mut SqliteConnection, aside: &str) -> Result<u64> {
    for sql in [
        format!("ALTER TABLE origin_pending RENAME TO {aside}"),
        "DROP INDEX IF EXISTS origin_pending_open".to_string(),
    ] {
        sqlx::query(&sql).execute(&mut *conn).await?;
    }
    create(conn).await?;
    let carried = sqlx::query(&format!(
        "INSERT INTO origin_pending ({CARRIED_COLUMNS}) \
         SELECT {CARRIED_COLUMNS} FROM {aside} ORDER BY pending_id"
    ))
    .execute(&mut *conn)
    .await?
    .rows_affected();
    // The old table's sequence passes the ids it deleted at the top; the
    // carried rows alone would set the new one to the highest kept.
    let seq: Option<i64> = sqlx::query_scalar("SELECT seq FROM sqlite_sequence WHERE name = ?")
        .bind(aside)
        .fetch_optional(&mut *conn)
        .await?;
    if let Some(seq) = seq {
        let raised = sqlx::query(
            "UPDATE sqlite_sequence SET seq = MAX(seq, ?) WHERE name = 'origin_pending'",
        )
        .bind(seq)
        .execute(&mut *conn)
        .await?
        .rows_affected();
        if raised == 0 {
            sqlx::query("INSERT INTO sqlite_sequence (name, seq) VALUES ('origin_pending', ?)")
                .bind(seq)
                .execute(&mut *conn)
                .await?;
        }
    }
    sqlx::query(&format!("DROP TABLE {aside}"))
        .execute(&mut *conn)
        .await?;
    Ok(carried)
}
