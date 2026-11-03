//! The served doors' per-origin grants in the store (#414, the design's
//! row L1, `docs/design/origin-grants.md` section (1)): three tables the
//! CLI owns beside the toolbox's, `origin_grants`, `origin_pending` and
//! `origin_spends`, and the row functions over them. This module is the
//! only writer of the three tables; it touches none of the toolbox's.
//!
//! Every row is keyed by the originator's hostname, lowercased with its
//! port dropped (the reference's `normalizeOriginator`, ts-stack edf6e03
//! `WalletPermissionsManager.ts:5702-5729`): one grant covers every port of
//! its host. The name as given is kept beside it (`raw_originator`).
//!
//! An expiry is unix seconds and `0` never expires (`expiry > 0 && expiry
//! < now` is expired, `WalletPermissionsManager.ts:2900-2903`); a
//! `spending` row never expires, its ceiling resetting each UTC month.
//!
//! Nothing reads these rows at a door yet: the gate still reads
//! `ALLOWED_ORIGINS` (the design's L5 is the switch).

use anyhow::Result;
use bsv_wallet_toolbox::StorageSqlx;
use sqlx::{Sqlite, SqliteConnection};

mod rows;
mod shown;

pub use rows::{
    expired, key_of, month_of, unlisted_text, when, Answer, Answered, Asking, Fold, FoldLine,
    Folded, GrantRow, Granting, Listing, Pending, PendingRow, Refused, Reply, Scope, Source,
    Standing, SEED_NOTICE,
};
use rows::{GrantTuple, PendingTuple};
pub use shown::{Shown, ShownJson};

/// The three tables and their three indexes, `(type, name)`, as
/// [`ensure`] reads them from `sqlite_master`.
const OBJECTS: [(&str, &str); 6] = [
    ("table", "origin_grants"),
    ("index", "origin_grants_key"),
    ("table", "origin_pending"),
    ("index", "origin_pending_open"),
    ("table", "origin_spends"),
    ("index", "origin_spends_month"),
];

/// The block [`ensure`] runs in one transaction when any of [`OBJECTS`]
/// is missing (`IF NOT EXISTS` throughout), the design's section (1).
const DDL: [&str; 6] = [
    "CREATE TABLE IF NOT EXISTS origin_grants (\
        grant_id INTEGER PRIMARY KEY, \
        originator TEXT NOT NULL, \
        raw_originator TEXT NOT NULL, \
        scope TEXT NOT NULL CHECK (scope IN ('wallet', 'spending')), \
        amount INTEGER, \
        granted_at INTEGER NOT NULL, \
        expiry INTEGER NOT NULL DEFAULT 0, \
        revoked_at INTEGER, \
        source TEXT NOT NULL CHECK (source IN ('owner', 'env')))",
    "CREATE UNIQUE INDEX IF NOT EXISTS origin_grants_key ON origin_grants (originator, scope)",
    "CREATE TABLE IF NOT EXISTS origin_pending (\
        pending_id INTEGER PRIMARY KEY, \
        originator TEXT NOT NULL, \
        raw_originator TEXT NOT NULL, \
        scope TEXT NOT NULL CHECK (scope IN ('wallet', 'spending')), \
        call TEXT NOT NULL, \
        amount INTEGER, \
        renewal INTEGER NOT NULL DEFAULT 0, \
        first_seen INTEGER NOT NULL, \
        answered_at INTEGER, \
        answer TEXT CHECK (answer IN ('grant', 'once', 'deny')))",
    "CREATE UNIQUE INDEX IF NOT EXISTS origin_pending_open ON origin_pending (originator) \
        WHERE answered_at IS NULL AND scope = 'wallet'",
    "CREATE TABLE IF NOT EXISTS origin_spends (\
        spend_id INTEGER PRIMARY KEY, \
        originator TEXT NOT NULL, \
        month TEXT NOT NULL, \
        txid TEXT, \
        reference TEXT, \
        satoshis INTEGER NOT NULL, \
        at INTEGER NOT NULL, \
        CHECK (txid IS NOT NULL OR reference IS NOT NULL))",
    "CREATE INDEX IF NOT EXISTS origin_spends_month ON origin_spends (originator, month)",
];

/// The tables and indexes, created when any one is missing. Reads
/// `sqlite_master` on the reader first: a store holding all six sends
/// nothing to the writer and answers `false`. Otherwise the whole block
/// runs in one `BEGIN IMMEDIATE` transaction on the writer, so a store
/// interrupted between a table and its index gains the index, and answers
/// `true`. The toolbox's tables are never touched.
///
/// # Errors
///
/// The store's error: the reader's, or the writer's (a busy store is the
/// toolbox's typed `StoreBusy`, [`super::is_store_busy`]).
pub async fn ensure(storage: &StorageSqlx) -> Result<bool> {
    let present: Vec<(String, String)> = sqlx::query_as(
        "SELECT type, name FROM sqlite_master WHERE tbl_name IN \
         ('origin_grants', 'origin_pending', 'origin_spends')",
    )
    .fetch_all(storage.pool())
    .await?;
    if OBJECTS
        .iter()
        .all(|(t, n)| present.iter().any(|(pt, pn)| pt == t && pn == n))
    {
        return Ok(false);
    }
    let mut tx = storage.write_tx().await?;
    for sql in DDL {
        sqlx::query(sql).execute(&mut *tx).await?;
    }
    tx.commit().await?;
    Ok(true)
}

const GRANT_COLUMNS: &str = "grant_id, originator, raw_originator, scope, amount, granted_at, \
                             expiry, revoked_at, source";

const PENDING_COLUMNS: &str = "pending_id, originator, raw_originator, scope, call, amount, \
                               renewal, first_seen, answered_at, answer";

async fn grant_row<'e, E: sqlx::Executor<'e, Database = Sqlite>>(
    executor: E,
    key: &str,
    scope: Scope,
) -> Result<Option<GrantRow>> {
    let sql =
        format!("SELECT {GRANT_COLUMNS} FROM origin_grants WHERE originator = ? AND scope = ?");
    let row: Option<GrantTuple> = sqlx::query_as(&sql)
        .bind(key)
        .bind(scope.as_str())
        .fetch_optional(executor)
        .await?;
    row.map(GrantRow::from_tuple).transpose()
}

async fn pending_row(conn: &mut SqliteConnection, pending_id: i64) -> Result<Option<PendingRow>> {
    let sql = format!("SELECT {PENDING_COLUMNS} FROM origin_pending WHERE pending_id = ?");
    let row: Option<PendingTuple> = sqlx::query_as(&sql)
        .bind(pending_id)
        .fetch_optional(conn)
        .await?;
    row.map(PendingRow::from_tuple).transpose()
}

/// The `scope` row of `originator` (any name of its host) read at `now`:
/// one indexed read on the reader.
///
/// # Errors
///
/// [`Refused::NotAnOriginator`], or the store's error.
pub async fn lookup(
    storage: &StorageSqlx,
    originator: &str,
    scope: Scope,
    now: i64,
) -> Result<Standing> {
    ensure(storage).await?;
    let (key, _) = key_of(originator)?;
    Ok(Standing::of(
        grant_row(storage.pool(), &key, scope).await?,
        now,
    ))
}

/// Write `g` as the one row of its (originator, scope), replacing a row
/// there: `granted_at`, `expiry`, `amount` and `source` set, `revoked_at`
/// cleared.
async fn upsert(conn: &mut SqliteConnection, g: &Granting<'_>, now: i64) -> Result<GrantRow> {
    g.check()?;
    let (key, raw) = key_of(g.originator)?;
    sqlx::query(
        "INSERT INTO origin_grants \
            (originator, raw_originator, scope, amount, granted_at, expiry, revoked_at, source) \
         VALUES (?, ?, ?, ?, ?, ?, NULL, ?) \
         ON CONFLICT (originator, scope) DO UPDATE SET \
            raw_originator = excluded.raw_originator, amount = excluded.amount, \
            granted_at = excluded.granted_at, expiry = excluded.expiry, revoked_at = NULL, \
            source = excluded.source",
    )
    .bind(&key)
    .bind(&raw)
    .bind(g.scope.as_str())
    .bind(g.amount)
    .bind(now)
    .bind(g.expiry)
    .bind(g.source.as_str())
    .execute(&mut *conn)
    .await?;
    grant_row(&mut *conn, &key, g.scope)
        .await?
        .ok_or_else(|| anyhow::anyhow!("the grant row {key} was not written"))
}

/// Grant: the upsert of the (originator, scope) row, the owner's direct
/// grant (a revoked row is granted again only here and by [`answer`]).
///
/// # Errors
///
/// [`Refused`]: the originator's form, a `wallet` amount, a `spending`
/// expiry, an owner's `spending` grant with no amount, a negative number;
/// or the store's error.
pub async fn grant(storage: &StorageSqlx, granting: &Granting<'_>, now: i64) -> Result<GrantRow> {
    ensure(storage).await?;
    let mut tx = storage.write_tx().await?;
    let row = upsert(&mut tx, granting, now).await?;
    tx.commit().await?;
    Ok(row)
}

/// Revoke: stamp `revoked_at` on the originator's rows not yet revoked,
/// of `scope` or (`None`) both; nothing is deleted. Answers the rows
/// stamped; none when the originator holds no such row.
///
/// # Errors
///
/// [`Refused::NotAnOriginator`], or the store's error.
pub async fn revoke(
    storage: &StorageSqlx,
    originator: &str,
    scope: Option<Scope>,
    now: i64,
) -> Result<Vec<GrantRow>> {
    ensure(storage).await?;
    let (key, _) = key_of(originator)?;
    let sql = format!(
        "UPDATE origin_grants SET revoked_at = ? \
         WHERE originator = ? AND revoked_at IS NULL AND (? IS NULL OR scope = ?) \
         RETURNING {GRANT_COLUMNS}"
    );
    let scope = scope.map(Scope::as_str);
    let mut tx = storage.write_tx().await?;
    let rows: Vec<GrantTuple> = sqlx::query_as(&sql)
        .bind(now)
        .bind(&key)
        .bind(scope)
        .bind(scope)
        .fetch_all(&mut *tx)
        .await?;
    tx.commit().await?;
    let mut rows: Vec<GrantRow> = rows
        .into_iter()
        .map(GrantRow::from_tuple)
        .collect::<Result<_>>()?;
    rows.sort_by_key(|r| r.grant_id);
    Ok(rows)
}

/// Open or join: a `wallet` call joins its originator's open `wallet` row
/// (one per originator, never duplicated), else writes one; a `spending`
/// call writes its own row (the reference's per-request spending key).
/// A revoked row of the scope refuses at once and writes nothing.
///
/// # Errors
///
/// [`Refused`]: the originator's form, the revoked row, the amount's
/// place; or the store's error.
pub async fn open_or_join(storage: &StorageSqlx, asking: &Asking<'_>, now: i64) -> Result<Pending> {
    ensure(storage).await?;
    match (asking.scope, asking.amount) {
        (_, Some(a)) if a < 0 => return Err(Refused::Negative.into()),
        (Scope::Wallet, Some(_)) => return Err(Refused::AmountOnWallet.into()),
        (Scope::Spending, None) => return Err(Refused::AmountRequired.into()),
        _ => {}
    }
    let (key, raw) = key_of(asking.originator)?;
    let mut tx = storage.write_tx().await?;
    if let Some(Some(at)) = grant_row(&mut *tx, &key, asking.scope)
        .await?
        .map(|g| g.revoked_at)
    {
        return Err(Refused::Revoked {
            originator: key,
            at,
        }
        .into());
    }
    let open: Option<i64> = if asking.scope == Scope::Wallet {
        sqlx::query_scalar(
            "SELECT pending_id FROM origin_pending \
             WHERE originator = ? AND scope = 'wallet' AND answered_at IS NULL",
        )
        .bind(&key)
        .fetch_optional(&mut *tx)
        .await?
    } else {
        None
    };
    let joined = open.is_some();
    let id = match open {
        Some(id) => id,
        None => sqlx::query(
            "INSERT INTO origin_pending \
                (originator, raw_originator, scope, call, amount, renewal, first_seen) \
             VALUES (?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(&key)
        .bind(&raw)
        .bind(asking.scope.as_str())
        .bind(asking.call)
        .bind(asking.amount)
        .bind(asking.renewal)
        .bind(now)
        .execute(&mut *tx)
        .await?
        .last_insert_rowid(),
    };
    let row = pending_row(&mut tx, id).await?;
    tx.commit().await?;
    let row = row.ok_or(Refused::NoSuchPending(id))?;
    Ok(Pending { row, joined })
}

/// Answer once: stamp `answered_at` and `answer` on an open row
/// (`UPDATE .. WHERE answered_at IS NULL`, so two places answering race to
/// one winner), and on `grant` write the row's grant ([`grant`]'s upsert,
/// `source` `owner`) in the same transaction. The row is kept: the table
/// is the history of the owner's prompts.
///
/// # Errors
///
/// [`Refused`]: no such row, the row already answered (the second answer),
/// `once` on a `wallet` row, a grant's amount or expiry out of place; or
/// the store's error.
pub async fn answer(
    storage: &StorageSqlx,
    pending_id: i64,
    reply: &Reply,
    now: i64,
) -> Result<Answered> {
    ensure(storage).await?;
    let mut tx = storage.write_tx().await?;
    let row = pending_row(&mut tx, pending_id)
        .await?
        .ok_or(Refused::NoSuchPending(pending_id))?;
    if let Some(at) = row.answered_at {
        return Err(Refused::AlreadyAnswered {
            pending_id,
            answer: row.answer,
            at,
        }
        .into());
    }
    if reply.answer == Answer::Once && row.scope == Scope::Wallet {
        return Err(Refused::OnceOnWallet(pending_id).into());
    }
    let grant = if reply.answer == Answer::Grant {
        let granting = Granting {
            originator: &row.raw_originator,
            scope: row.scope,
            amount: reply.amount,
            expiry: reply.expiry,
            source: Source::Owner,
        };
        Some(upsert(&mut tx, &granting, now).await?)
    } else {
        None
    };
    sqlx::query(
        "UPDATE origin_pending SET answered_at = ?, answer = ? \
         WHERE pending_id = ? AND answered_at IS NULL",
    )
    .bind(now)
    .bind(reply.answer.as_str())
    .bind(pending_id)
    .execute(&mut *tx)
    .await?;
    let pending = pending_row(&mut tx, pending_id)
        .await?
        .ok_or(Refused::NoSuchPending(pending_id))?;
    tx.commit().await?;
    Ok(Answered { pending, grant })
}

/// Seed `key`'s env rows: the `wallet` row, and a ceiling-free `spending`
/// row unless one is already there (the owner's is kept).
async fn seed_env(conn: &mut SqliteConnection, key: &str, raw: &str, now: i64) -> Result<()> {
    let wallet = Granting {
        originator: raw,
        scope: Scope::Wallet,
        amount: None,
        expiry: 0,
        source: Source::Env,
    };
    upsert(&mut *conn, &wallet, now).await?;
    sqlx::query(
        "INSERT INTO origin_grants \
            (originator, raw_originator, scope, amount, granted_at, expiry, source) \
         VALUES (?, ?, 'spending', NULL, ?, 0, 'env') \
         ON CONFLICT (originator, scope) DO NOTHING",
    )
    .bind(key)
    .bind(raw)
    .bind(now)
    .execute(&mut *conn)
    .await?;
    Ok(())
}

/// Fold one entry inside the fold's transaction.
async fn fold_entry(conn: &mut SqliteConnection, entry: &str, now: i64) -> Result<FoldLine> {
    let (key, raw) = key_of(entry)?;
    let folded = match Standing::of(grant_row(&mut *conn, &key, Scope::Wallet).await?, now) {
        Standing::None => {
            seed_env(conn, &key, &raw, now).await?;
            Folded::Granted
        }
        Standing::Revoked(GrantRow {
            revoked_at: Some(at),
            ..
        }) => Folded::Revoked(at),
        Standing::Expired(g) => Folded::Expired(g.expiry),
        Standing::Live(_) | Standing::Revoked(_) => Folded::AlreadyGranted,
    };
    Ok(FoldLine {
        entry: raw,
        originator: key,
        folded,
    })
}

/// The keys of the env rows not revoked, and whether any env row is held.
async fn env_rows(conn: &mut SqliteConnection) -> Result<(Vec<String>, bool)> {
    let live: Vec<String> = sqlx::query_scalar(
        "SELECT DISTINCT originator FROM origin_grants \
         WHERE source = 'env' AND revoked_at IS NULL ORDER BY originator",
    )
    .fetch_all(&mut *conn)
    .await?;
    let held: bool =
        sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM origin_grants WHERE source = 'env')")
            .fetch_one(&mut *conn)
            .await?;
    Ok((live, held))
}

/// The fold of `ALLOWED_ORIGINS` (section (6)), in one transaction: each
/// entry whose host holds no `wallet` row becomes a `wallet` row and a
/// ceiling-free `spending` row, `source` `env`, `expiry` `0`; a row of any
/// state already there is left as it is (a revoked one is never granted
/// again). Each entry's line, each env row whose entry left the list, and
/// the seed notice are logged at info by name. A second fold writes
/// nothing.
///
/// # Errors
///
/// [`Refused::NotAnOriginator`] for an entry (nothing is written), or the
/// store's error.
pub async fn fold(storage: &StorageSqlx, entries: &[String], now: i64) -> Result<Fold> {
    ensure(storage).await?;
    let mut tx = storage.write_tx().await?;
    let mut lines = Vec::with_capacity(entries.len());
    for entry in entries {
        lines.push(fold_entry(&mut tx, entry, now).await?);
    }
    let (env, seeded) = env_rows(&mut tx).await?;
    tx.commit().await?;
    let unlisted = env
        .into_iter()
        .filter(|k| lines.iter().all(|l| &l.originator != k))
        .collect();
    let fold = Fold {
        lines,
        unlisted,
        seeded,
    };
    fold.log();
    Ok(fold)
}

/// The listing `bsv-wallet origins` prints (section (4)): every grant row,
/// by originator and then scope (`wallet` first), and the pending rows by
/// id, the open ones alone unless `all` asks for the answered ones too.
/// Read on the reader.
///
/// # Errors
///
/// The store's error.
pub async fn listing(storage: &StorageSqlx, all: bool) -> Result<Listing> {
    listed(storage, "? OR answered_at IS NULL", i64::from(all)).await
}

/// The listing the owner's door answers (`GET /origins`, section (3)):
/// every grant row as [`listing`] reads them, and the open pending rows
/// with the last `answered` answered ones (by their answer's time, then
/// id), by id. Read on the reader.
///
/// # Errors
///
/// The store's error.
pub async fn owners_listing(storage: &StorageSqlx, answered: i64) -> Result<Listing> {
    let recent = "answered_at IS NULL OR pending_id IN (SELECT pending_id FROM origin_pending \
                  WHERE answered_at IS NOT NULL ORDER BY answered_at DESC, pending_id DESC LIMIT ?)";
    listed(storage, recent, answered).await
}

/// Every grant row, and the pending rows `pending_where` (one bound
/// number) selects, by id.
async fn listed(storage: &StorageSqlx, pending_where: &str, bound: i64) -> Result<Listing> {
    ensure(storage).await?;
    let sql = format!(
        "SELECT {GRANT_COLUMNS} FROM origin_grants ORDER BY originator, scope = 'spending'"
    );
    let grants: Vec<GrantTuple> = sqlx::query_as(&sql).fetch_all(storage.pool()).await?;
    let sql = format!(
        "SELECT {PENDING_COLUMNS} FROM origin_pending WHERE {pending_where} ORDER BY pending_id"
    );
    let pending: Vec<PendingTuple> = sqlx::query_as(&sql)
        .bind(bound)
        .fetch_all(storage.pool())
        .await?;
    Ok(Listing {
        grants: grants
            .into_iter()
            .map(GrantRow::from_tuple)
            .collect::<Result<_>>()?,
        pending: pending
            .into_iter()
            .map(PendingRow::from_tuple)
            .collect::<Result<_>>()?,
    })
}

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

#[cfg(test)]
mod fold_tests;
#[cfg(test)]
mod tests;
