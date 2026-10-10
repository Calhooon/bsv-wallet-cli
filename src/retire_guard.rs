//! The host's retire against the tracker's words (bsv-stack-lean #66, its
//! tracker charter sections 1 and 5).
//!
//! A retire is the host's act: it writes `failed` for a transaction this
//! wallet built and gives back what the chain vouches for. The tracker
//! allows the host that act on one word only, `built`: a template no
//! broadcaster ever took (`abandoned`, "the host's decision on a template
//! never announced, refused once the word left `built`"; #24, never abort a
//! template already broadcast). A transaction a broadcaster accepted
//! (`announced`, `seen`, or a chain word) stays, re-asked on the cadence,
//! until a proof, a node verdict or a competitor's checked proof decides it.
//!
//! [`broadcaster_took`] reads the wallet's own records for an accepting word.
//! Every retire path of the CLI asks it first: the served sweep, the by-hand
//! `reconcile-broadcasts`, the daemon's abandoned-transaction ticker and
//! `cleanup-abandoned`, and the served follow-up after a broadcast. A chain
//! word (a competitor's proof checked against our headers) is not this
//! guard's concern: it is the chain's, and the paths that act on it
//! (`DeadConflict`, the toolbox's competitor step) do not ask.

use anyhow::Result;
use sqlx::SqlitePool;

/// Request statuses the toolbox writes only once a broadcaster accepted the
/// transaction or a status source held it (`unmined`; the legacy `callback`
/// and `unconfirmed`), or a proof landed (`completed`).
const TAKEN_REQ_STATUSES: [&str; 4] = ["unmined", "callback", "unconfirmed", "completed"];

/// Broadcast-memory rows that are an accepting word (`accepted`) or more
/// (`seen`, `mined`), from any provider or the chain index.
const TAKEN_MEMORY_STATUSES: [&str; 3] = ["accepted", "seen", "mined"];

/// Tracker words past `built` (`maps/tracker.json`: the hint tier upward,
/// and `stale`, which was `mined`).
const TAKEN_TRACKER_WORDS: [&str; 4] = ["announced", "seen", "mined", "stale"];

/// Whether a broadcaster ever took `txid`, by the wallet's own records: its
/// proof request past the post, a broadcast-memory row with an accepting
/// word, or the tracker's stored word past `built`. A record the wallet does
/// not hold (no request, no memory table, no tracker table) says nothing.
///
/// The memory alone is not enough: its ladder overwrites a provider's
/// `accepted` with a later `rejected` (toolbox `ladder_step`), so an Arcade
/// that accepted a transaction and then pushed REJECTED leaves only the
/// refusal there. The request keeps `unmined`: a refusal is a hint at the
/// toolbox 0.7.4 and moves no request.
pub async fn broadcaster_took(pool: &SqlitePool, txid: &str) -> Result<bool> {
    let req: Option<String> =
        sqlx::query_scalar("SELECT status FROM proven_tx_reqs WHERE txid = ? LIMIT 1")
            .bind(txid)
            .fetch_optional(pool)
            .await?;
    if req.is_some_and(|s| TAKEN_REQ_STATUSES.contains(&s.as_str())) {
        return Ok(true);
    }
    let completed: Option<i64> = sqlx::query_scalar(
        "SELECT 1 FROM transactions WHERE txid = ? AND status = 'completed' LIMIT 1",
    )
    .bind(txid)
    .fetch_optional(pool)
    .await?;
    if completed.is_some() {
        return Ok(true);
    }
    if table_exists(pool, "broadcast_seen").await? {
        let rows: Vec<String> =
            sqlx::query_scalar("SELECT status FROM broadcast_seen WHERE txid = ?")
                .bind(txid)
                .fetch_all(pool)
                .await?;
        if rows
            .iter()
            .any(|s| TAKEN_MEMORY_STATUSES.contains(&s.as_str()))
        {
            return Ok(true);
        }
    }
    if table_exists(pool, "tracker_states").await? {
        let word: Option<String> =
            sqlx::query_scalar("SELECT word FROM tracker_states WHERE txid = ?")
                .bind(txid)
                .fetch_optional(pool)
                .await?;
        if word.is_some_and(|w| TAKEN_TRACKER_WORDS.contains(&w.as_str())) {
            return Ok(true);
        }
    }
    Ok(false)
}

/// The first of `txids` a broadcaster took ([`broadcaster_took`]), if any.
pub async fn first_taken<'a>(
    pool: &SqlitePool,
    txids: impl IntoIterator<Item = &'a str>,
) -> Result<Option<String>> {
    for txid in txids {
        if broadcaster_took(pool, txid).await? {
            return Ok(Some(txid.to_string()));
        }
    }
    Ok(None)
}

/// The transaction an `abortAction` of `reference` (a createAction
/// reference, or a txid) would fail, when a broadcaster took it: the abort
/// is then refused (the tracker's `abandoned` is refused once the word left
/// `built`; #24, never abort a template already broadcast). The toolbox
/// honours the abort of a broadcast transaction whenever the wallet holds
/// no chain evidence for it, an accepting word included (its 0.3.60
/// broadcast abort); this is the host's narrower rule.
pub async fn abort_refused_for(pool: &SqlitePool, reference: &str) -> Result<Option<String>> {
    let by_reference: Option<Option<String>> =
        sqlx::query_scalar("SELECT txid FROM transactions WHERE reference = ? LIMIT 1")
            .bind(reference)
            .fetch_optional(pool)
            .await?;
    let txid = match by_reference {
        Some(txid) => txid,
        None if reference.len() == 64 && reference.chars().all(|c| c.is_ascii_hexdigit()) => {
            Some(reference.to_ascii_lowercase())
        }
        None => None,
    };
    let Some(txid) = txid else {
        return Ok(None);
    };
    if broadcaster_took(pool, &txid).await? {
        Ok(Some(txid))
    } else {
        Ok(None)
    }
}

async fn table_exists(pool: &SqlitePool, name: &str) -> Result<bool> {
    let found: Option<i64> =
        sqlx::query_scalar("SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = ?")
            .bind(name)
            .fetch_optional(pool)
            .await?;
    Ok(found.is_some())
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn pool() -> SqlitePool {
        let pool = sqlx::SqlitePool::connect("sqlite::memory:").await.unwrap();
        sqlx::query("CREATE TABLE proven_tx_reqs (txid TEXT, status TEXT)")
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("CREATE TABLE transactions (txid TEXT, status TEXT)")
            .execute(&pool)
            .await
            .unwrap();
        pool
    }

    #[tokio::test]
    async fn a_request_past_the_post_or_an_accepting_row_is_taken_and_a_refusal_alone_is_not() {
        let pool = pool().await;
        let (a, b, c, d) = (
            "a".repeat(64),
            "b".repeat(64),
            "c".repeat(64),
            "d".repeat(64),
        );
        for (txid, req, tx) in [
            (&a, "unmined", "unproven"),
            (&b, "unsent", "sending"),
            (&c, "unsent", "unproven"),
            (&d, "sending", "sending"),
        ] {
            sqlx::query("INSERT INTO proven_tx_reqs VALUES (?, ?)")
                .bind(txid)
                .bind(req)
                .execute(&pool)
                .await
                .unwrap();
            sqlx::query("INSERT INTO transactions VALUES (?, ?)")
                .bind(txid)
                .bind(tx)
                .execute(&pool)
                .await
                .unwrap();
        }
        assert!(broadcaster_took(&pool, &a).await.unwrap(), "unmined");
        assert!(!broadcaster_took(&pool, &b).await.unwrap(), "no record");

        sqlx::query("CREATE TABLE broadcast_seen (txid TEXT, provider TEXT, status TEXT)")
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("INSERT INTO broadcast_seen VALUES (?, 'arcade', 'rejected'), (?, 'arcade', 'accepted')")
            .bind(&b)
            .bind(&c)
            .execute(&pool)
            .await
            .unwrap();
        assert!(!broadcaster_took(&pool, &b).await.unwrap(), "a refusal row");
        assert!(
            broadcaster_took(&pool, &c).await.unwrap(),
            "an accepting row"
        );

        sqlx::query("CREATE TABLE tracker_states (txid TEXT, word TEXT)")
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("INSERT INTO tracker_states VALUES (?, 'announced')")
            .bind(&d)
            .execute(&pool)
            .await
            .unwrap();
        assert!(
            broadcaster_took(&pool, &d).await.unwrap(),
            "the tracker's word"
        );
        assert_eq!(
            first_taken(&pool, [b.as_str(), d.as_str()]).await.unwrap(),
            Some(d.clone())
        );
    }
}
