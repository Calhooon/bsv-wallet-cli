//! The probe record (#539, the design's section "The probe record"): every
//! answer the gate gives with no row (the open doors, a protocol at
//! security level 0, W15 (i)) is counted per origin in `origin_probes`, a
//! row of record, never a grant: the gate never reads it, `origin-revoke`
//! never touches it.
//!
//! One row per (originator, door, power key) holds the count and the first
//! and last time seen, so a page polling a door writes one row, not one a
//! call. An originator keeps at most [`PER_ORIGINATOR`] rows: past it the
//! rows seen longest ago fold into its one remainder row (door
//! [`REMAINDER`]), their counts summed and their times spanned, so no page
//! grows the table past the cap whatever it calls.

use std::collections::BTreeMap;

use anyhow::Result;
use bsv_wallet_toolbox::StorageSqlx;
use sqlx::SqliteConnection;

use super::{ensure, key_of};

/// The rows an originator keeps, its remainder row among them: the six
/// open doors and the level-0 protocols a page uses, with room to spare,
/// in one screen of `bsv-wallet origins`.
pub const PER_ORIGINATOR: usize = 32;

/// The remainder row's door: the folded rows' counts and times.
pub const REMAINDER: &str = "*";

const PROBE_COLUMNS: &str = "originator, raw_originator, door, power, count, first_seen, last_seen";

/// One answer with no row, as the gate saw it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Probe {
    /// The originator the gate admitted (any name of its host).
    pub originator: String,
    /// The door, without its slash (`getVersion`, `createSignature`).
    pub door: String,
    /// A level-0 protocol's key (`[false,0,"name",""]`); `None` at an open
    /// door.
    pub power: Option<String>,
    /// Unix seconds.
    pub at: i64,
}

/// One row of the record.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProbeRow {
    /// The originator's hostname, the rows' key.
    pub originator: String,
    /// The name as last given.
    pub raw_originator: String,
    /// The door, or [`REMAINDER`].
    pub door: String,
    /// The level-0 protocol's key, `None` at an open door and on the
    /// remainder.
    pub power: Option<String>,
    /// The answers counted.
    pub count: i64,
    /// Unix seconds.
    pub first_seen: i64,
    /// Unix seconds.
    pub last_seen: i64,
}

type ProbeTuple = (String, String, String, String, i64, i64, i64);

impl ProbeRow {
    fn from_tuple(t: ProbeTuple) -> Self {
        let (originator, raw_originator, door, power, count, first_seen, last_seen) = t;
        Self {
            originator,
            raw_originator,
            door,
            power: (!power.is_empty()).then_some(power),
            count,
            first_seen,
            last_seen,
        }
    }
}

/// The probes of one batch, per (originator's key, door, power): the raw
/// name last given, the count, the first and last times.
type Tally = BTreeMap<(String, String, String), (String, i64, i64, i64)>;

fn tally(probes: &[Probe]) -> Result<Tally> {
    let mut seen = Tally::new();
    for p in probes {
        let (key, raw) = key_of(&p.originator)?;
        let power = p.power.clone().unwrap_or_default();
        let entry =
            seen.entry((key, p.door.clone(), power))
                .or_insert((raw.clone(), 0, p.at, p.at));
        entry.0 = raw;
        entry.1 += 1;
        entry.2 = entry.2.min(p.at);
        entry.3 = entry.3.max(p.at);
    }
    Ok(seen)
}

/// Add `count` answers seen from `first` to `last` to a row.
async fn add(
    conn: &mut SqliteConnection,
    (originator, door, power): (&str, &str, &str),
    raw: &str,
    (count, first, last): (i64, i64, i64),
) -> Result<()> {
    sqlx::query(
        "INSERT INTO origin_probes \
            (originator, raw_originator, door, power, count, first_seen, last_seen) \
         VALUES (?, ?, ?, ?, ?, ?, ?) \
         ON CONFLICT (originator, door, power) DO UPDATE SET \
            raw_originator = excluded.raw_originator, count = count + excluded.count, \
            first_seen = MIN(first_seen, excluded.first_seen), \
            last_seen = MAX(last_seen, excluded.last_seen)",
    )
    .bind(originator)
    .bind(raw)
    .bind(door)
    .bind(power)
    .bind(count)
    .bind(first)
    .bind(last)
    .execute(&mut *conn)
    .await?;
    Ok(())
}

/// Fold `originator`'s rows past the cap into its remainder: the rows
/// seen longest ago (by `last_seen`, then the older row) are the ones folded.
async fn fold(conn: &mut SqliteConnection, originator: &str) -> Result<()> {
    let rows: Vec<(i64, i64, i64, i64)> = sqlx::query_as(
        "SELECT probe_id, count, first_seen, last_seen FROM origin_probes \
         WHERE originator = ? AND door <> ? ORDER BY last_seen DESC, probe_id DESC",
    )
    .bind(originator)
    .bind(REMAINDER)
    .fetch_all(&mut *conn)
    .await?;
    let Some(past) = rows.get(PER_ORIGINATOR - 1..).filter(|p| !p.is_empty()) else {
        return Ok(());
    };
    let count = past.iter().map(|r| r.1).sum();
    let first = past.iter().map(|r| r.2).min().unwrap_or_default();
    let last = past.iter().map(|r| r.3).max().unwrap_or_default();
    add(
        conn,
        (originator, REMAINDER, ""),
        originator,
        (count, first, last),
    )
    .await?;
    for (probe_id, ..) in past {
        sqlx::query("DELETE FROM origin_probes WHERE probe_id = ?")
            .bind(probe_id)
            .execute(&mut *conn)
            .await?;
    }
    Ok(())
}

/// Count `probes` in one transaction: each (originator, door, power) row
/// gains its answers, then each originator touched is folded to the cap.
///
/// # Errors
///
/// A probe's originator not in the reference's form, or the store's error.
pub async fn record(storage: &StorageSqlx, probes: &[Probe]) -> Result<()> {
    ensure(storage).await?;
    let seen = tally(probes)?;
    let mut tx = storage.write_tx().await?;
    for ((originator, door, power), (raw, count, first, last)) in &seen {
        add(
            &mut tx,
            (originator, door, power),
            raw,
            (*count, *first, *last),
        )
        .await?;
    }
    let mut folded: Vec<&str> = seen.keys().map(|k| k.0.as_str()).collect();
    folded.dedup();
    for originator in folded {
        fold(&mut tx, originator).await?;
    }
    tx.commit().await?;
    Ok(())
}

/// Every row of the record, by originator, its remainder last, then door
/// and power. Read on the reader.
///
/// # Errors
///
/// The store's error.
pub async fn probes(storage: &StorageSqlx) -> Result<Vec<ProbeRow>> {
    ensure(storage).await?;
    let sql = format!(
        "SELECT {PROBE_COLUMNS} FROM origin_probes \
         ORDER BY originator, door = '{REMAINDER}', door, power"
    );
    let rows: Vec<ProbeTuple> = sqlx::query_as(&sql).fetch_all(storage.pool()).await?;
    Ok(rows.into_iter().map(ProbeRow::from_tuple).collect())
}

#[cfg(test)]
mod tests;
