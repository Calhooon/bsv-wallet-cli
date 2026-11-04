//! The month's tally in `origin_spends` (#414, the design's section (1),
//! W7): the satoshis an originator's admitted spends hold in a UTC month,
//! one row per spend keyed per flow, and the writers the served doors
//! call (`createAction`'s open row, settled or dropped, `signAction`'s
//! txid, `abortAction`'s delete).

use anyhow::Result;
use bsv_wallet_toolbox::StorageSqlx;

use super::{ensure, key_of, month_of, Refused};

/// The month's tally: the satoshis `origin_spends` holds for the
/// originator (any name of its host) in the UTC month of `now`.
///
/// # Errors
///
/// [`Refused::NotAnOriginator`], a time outside chrono's range, or the
/// store's error.
pub async fn month_tally(storage: &StorageSqlx, originator: &str, now: i64) -> Result<i64> {
    ensure(storage).await?;
    let (key, _) = key_of(originator)?;
    let total: i64 = sqlx::query_scalar(
        "SELECT COALESCE(SUM(satoshis), 0) FROM origin_spends WHERE originator = ? AND month = ?",
    )
    .bind(key)
    .bind(month_of(now)?)
    .fetch_one(storage.pool())
    .await?;
    Ok(total)
}

/// A spend's handle in `origin_spends`, per flow (section (1)): the txid
/// on the signed path, the reference as the door serves it on the deferred
/// one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Flow<'a> {
    /// `signAndProcess` true (`noSend` or not): the action's txid, hex.
    Signed(&'a str),
    /// `signAndProcess` false: the template's reference.
    Deferred(&'a str),
}

/// The reference an open spend row holds until the toolbox answers (a
/// base64 reference never holds a parenthesis): the row counts in the
/// tally from the moment it is written.
pub const OPEN: &str = "(open)";

/// Tally an admitted spend: one `origin_spends` row of `satoshis` for the
/// originator (any name of its host) in the UTC month of `now`, its flow's
/// key already known.
///
/// # Errors
///
/// As [`open_spend`] and [`settle_spend`].
pub async fn spent(
    storage: &StorageSqlx,
    originator: &str,
    flow: Flow<'_>,
    satoshis: i64,
    now: i64,
) -> Result<()> {
    let spend_id = open_spend(storage, originator, satoshis, now).await?;
    settle_spend(storage, spend_id, flow).await
}

/// The spend's row written before the toolbox's call (section (1)): open,
/// holding [`OPEN`] for its reference, counted in the month's tally at
/// once. Answers its `spend_id`.
///
/// # Errors
///
/// [`Refused::NotAnOriginator`], [`Refused::Negative`], a time outside
/// chrono's range, or the store's error.
pub async fn open_spend(
    storage: &StorageSqlx,
    originator: &str,
    satoshis: i64,
    now: i64,
) -> Result<i64> {
    ensure(storage).await?;
    if satoshis < 0 {
        return Err(Refused::Negative.into());
    }
    let (key, _) = key_of(originator)?;
    let month = month_of(now)?;
    let mut tx = storage.write_tx().await?;
    let spend_id = sqlx::query(
        "INSERT INTO origin_spends (originator, month, txid, reference, satoshis, at) \
         VALUES (?, ?, NULL, ?, ?, ?)",
    )
    .bind(&key)
    .bind(month)
    .bind(OPEN)
    .bind(satoshis)
    .bind(now)
    .execute(&mut *tx)
    .await?
    .last_insert_rowid();
    tx.commit().await?;
    Ok(spend_id)
}

/// The open row `spend_id` given its flow's key once the toolbox returned
/// the action: the txid on the signed path, the reference on the deferred.
///
/// # Errors
///
/// The store's error.
pub async fn settle_spend(storage: &StorageSqlx, spend_id: i64, flow: Flow<'_>) -> Result<()> {
    ensure(storage).await?;
    let (txid, reference) = match flow {
        Flow::Signed(txid) => (Some(txid), None),
        Flow::Deferred(reference) => (None, Some(reference)),
    };
    let mut tx = storage.write_tx().await?;
    sqlx::query("UPDATE origin_spends SET txid = ?, reference = ? WHERE spend_id = ?")
        .bind(txid)
        .bind(reference)
        .bind(spend_id)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(())
}

/// The row `spend_id` deleted: the toolbox's call it opened for failed
/// (section (1): a failed call writes none). Answers the rows deleted.
///
/// # Errors
///
/// The store's error.
pub async fn drop_spend(storage: &StorageSqlx, spend_id: i64) -> Result<u64> {
    ensure(storage).await?;
    let mut tx = storage.write_tx().await?;
    let deleted = sqlx::query("DELETE FROM origin_spends WHERE spend_id = ?")
        .bind(spend_id)
        .execute(&mut *tx)
        .await?
        .rows_affected();
    tx.commit().await?;
    Ok(deleted)
}

/// `signAction`'s answer on the deferred path: the txid filled on the
/// spend rows of `reference` that hold none. Answers the rows filled.
///
/// # Errors
///
/// The store's error.
pub async fn signed(storage: &StorageSqlx, reference: &str, txid: &str) -> Result<u64> {
    ensure(storage).await?;
    let mut tx = storage.write_tx().await?;
    let filled =
        sqlx::query("UPDATE origin_spends SET txid = ? WHERE reference = ? AND txid IS NULL")
            .bind(txid)
            .bind(reference)
            .execute(&mut *tx)
            .await?
            .rows_affected();
    tx.commit().await?;
    Ok(filled)
}

/// `abortAction`'s success: the originator's spend rows `name` names, as
/// the toolbox resolves it (a reference, else a txid), deleted, so an
/// aborted template and an aborted `noSend` action both leave the tally.
/// Answers the rows deleted.
///
/// # Errors
///
/// [`Refused::NotAnOriginator`], or the store's error.
pub async fn unspent(storage: &StorageSqlx, originator: &str, name: &str) -> Result<u64> {
    ensure(storage).await?;
    let (key, _) = key_of(originator)?;
    let mut tx = storage.write_tx().await?;
    let deleted = sqlx::query(
        "DELETE FROM origin_spends WHERE originator = ?2 AND (reference = ?1 OR txid = ?1)",
    )
    .bind(name)
    .bind(&key)
    .execute(&mut *tx)
    .await?
    .rows_affected();
    tx.commit().await?;
    Ok(deleted)
}
