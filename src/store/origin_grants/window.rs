//! The disconnect, the brief reconnect's window and the shutdown over the
//! pending rows (#414 L5b, W4 (a); #534, W19; the design's section (2)):
//! [`abandon`] marks a snapshot's rows (its spending rows deleted at once),
//! [`expire`] deletes the rows still abandoned with their mark at the
//! window's end, [`withdraw`] deletes a held call's row at the shutdown, and
//! [`abandoned`] names the row a call would resume. Behind the store's
//! boundary, as the parent's statements.

use anyhow::Result;
use bsv_wallet_toolbox::StorageSqlx;
use sqlx::SqliteConnection;

use super::{
    ensure, key_of, Abandoned, Kind, PendingRow, PendingTuple, Power, PENDING_COLUMNS, WINDOW,
};

/// The disconnect (section (2), W4 (a), and #534's window): among `ids`,
/// the snapshot its held call took, the open rows of `originator` (any
/// name of its host) are, in one write transaction, deleted when they are
/// spending rows (a spending row is one call's, so no retry can join it)
/// and marked `abandoned_at = now` otherwise. A row already abandoned keeps
/// its earlier mark (a second disconnect never lengthens a window), and an
/// answered row, or an open row the list does not name, is kept. Logged at
/// info by name: `browser gate: abandoned` for the deleted rows,
/// `browser gate: disconnected` for the marked ones with the window's end.
///
/// # Errors
///
/// [`super::Refused::NotAnOriginator`], or the store's error.
pub async fn abandon(
    storage: &StorageSqlx,
    originator: &str,
    ids: &[i64],
    now: i64,
) -> Result<Abandoned> {
    ensure(storage).await?;
    let (key, _) = key_of(originator)?;
    if ids.is_empty() {
        return Ok(Abandoned::default());
    }
    let mut tx = storage.write_tx().await?;
    let deleted = among(
        &mut tx,
        "DELETE FROM origin_pending",
        "AND kind = 'spending'",
        (None, &key, ids, None),
    )
    .await?;
    let marked = among(
        &mut tx,
        "UPDATE origin_pending SET abandoned_at = ?",
        "AND abandoned_at IS NULL AND kind <> 'spending'",
        (Some(now), &key, ids, None),
    )
    .await?;
    tx.commit().await?;
    let done = Abandoned { deleted, marked };
    withdrawn(&key, &done.deleted);
    if !done.marked.is_empty() {
        tracing::info!(
            marker = "origin_grants_disconnected",
            originator = %key,
            rows = %named(&done.marked),
            until = now.saturating_add(WINDOW),
            "browser gate: disconnected"
        );
    }
    Ok(done)
}

/// The window's end (#534): delete, in one write transaction, the rows of
/// `originator` among `ids` still abandoned with the mark `abandoned_at`
/// (a row a call rejoined, its mark cleared, or a row marked again later is
/// left to its own timer). Logged at info by name (`browser gate:
/// abandoned`). Answers the rows deleted.
///
/// # Errors
///
/// [`super::Refused::NotAnOriginator`], or the store's error.
pub async fn expire(
    storage: &StorageSqlx,
    originator: &str,
    ids: &[i64],
    abandoned_at: i64,
) -> Result<Vec<PendingRow>> {
    deleted(storage, originator, ids, Some(abandoned_at)).await
}

/// The shutdown's withdrawal (section (2)): delete the open rows of
/// `originator` among `ids`, abandoned or not, of every kind; an answered
/// row is kept. Logged at info by name (`browser gate: abandoned`).
/// Answers the rows deleted.
///
/// # Errors
///
/// [`super::Refused::NotAnOriginator`], or the store's error.
pub async fn withdraw(
    storage: &StorageSqlx,
    originator: &str,
    ids: &[i64],
) -> Result<Vec<PendingRow>> {
    deleted(storage, originator, ids, None).await
}

/// Delete the open rows of `originator` among `ids`, those abandoned with
/// `mark` alone when one is named, in one write transaction; logged.
async fn deleted(
    storage: &StorageSqlx,
    originator: &str,
    ids: &[i64],
    mark: Option<i64>,
) -> Result<Vec<PendingRow>> {
    ensure(storage).await?;
    let (key, _) = key_of(originator)?;
    if ids.is_empty() {
        return Ok(Vec::new());
    }
    let tail = if mark.is_some() {
        "AND abandoned_at = ?"
    } else {
        ""
    };
    let mut tx = storage.write_tx().await?;
    let rows = among(
        &mut tx,
        "DELETE FROM origin_pending",
        tail,
        (None, &key, ids, mark),
    )
    .await?;
    tx.commit().await?;
    withdrawn(&key, &rows);
    Ok(rows)
}

/// The abandoned row of `power` that `originator` (any name of its host)
/// would resume at `now` (#534): its id inside its window, else none. Read
/// on the reader, before the call asks a place under the cap.
///
/// # Errors
///
/// [`super::Refused::NotAnOriginator`], or the store's error.
pub async fn abandoned(
    storage: &StorageSqlx,
    originator: &str,
    power: &Power,
    now: i64,
) -> Result<Option<i64>> {
    ensure(storage).await?;
    let (key, _) = key_of(originator)?;
    if power.kind() == Kind::Spending {
        return Ok(None);
    }
    Ok(sqlx::query_scalar(
        "SELECT pending_id FROM origin_pending \
         WHERE originator = ? AND kind = ? AND key = ? AND answered_at IS NULL \
           AND abandoned_at IS NOT NULL AND abandoned_at > ? - ?",
    )
    .bind(&key)
    .bind(power.kind().as_str())
    .bind(power.key())
    .bind(now)
    .bind(WINDOW)
    .fetch_optional(storage.pool())
    .await?)
}

/// `head` over the open rows of the originator `key` among `ids` with
/// `tail` added to the condition, returning the rows by id. The binds, in
/// order: `head`'s number when given, the key, the ids, `tail`'s number
/// when given.
async fn among(
    conn: &mut SqliteConnection,
    head: &str,
    tail: &str,
    (first, key, ids, last): (Option<i64>, &str, &[i64], Option<i64>),
) -> Result<Vec<PendingRow>> {
    let marks = vec!["?"; ids.len()].join(", ");
    let sql = format!(
        "{head} WHERE originator = ? AND pending_id IN ({marks}) AND answered_at IS NULL {tail} \
         RETURNING {PENDING_COLUMNS}"
    );
    let mut query = sqlx::query_as::<_, PendingTuple>(&sql);
    if let Some(n) = first {
        query = query.bind(n);
    }
    query = query.bind(key);
    for id in ids {
        query = query.bind(id);
    }
    if let Some(n) = last {
        query = query.bind(n);
    }
    let mut rows: Vec<PendingRow> = query
        .fetch_all(&mut *conn)
        .await?
        .into_iter()
        .map(PendingRow::from_tuple)
        .collect::<Result<_>>()?;
    rows.sort_by_key(|r| r.pending_id);
    Ok(rows)
}

/// The rows by id, kind and key, for a log line.
fn named(rows: &[PendingRow]) -> String {
    let named: Vec<String> = rows
        .iter()
        .map(|r| format!("#{} {} {}", r.pending_id, r.kind.as_str(), r.key))
        .collect();
    named.join(", ")
}

/// The deleted rows of a disconnect, a window's end or a shutdown, by name;
/// nothing when none was deleted.
fn withdrawn(key: &str, rows: &[PendingRow]) {
    if rows.is_empty() {
        return;
    }
    tracing::info!(
        marker = "origin_grants_abandoned",
        originator = %key,
        rows = %named(rows),
        "browser gate: abandoned"
    );
}
