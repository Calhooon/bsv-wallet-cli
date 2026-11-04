//! The served doors' per-origin grants in the store (#414,
//! `docs/design/origin-grants.md` section (1), revised under the owner's
//! ruling W1 (a) of 2026-11-03): four tables the CLI owns beside the
//! toolbox's, `origin_grants`, `origin_pending`, `origin_spends` and the
//! probe record `origin_probes` (#539, [`probes`]), and the row functions
//! over them. This module is the only writer of the four tables; it
//! touches none of the toolbox's.
//!
//! Every grant row is one power ([`Power`]): an originator, one of the
//! reference's four kinds and the kind's instance key in its canonical
//! text, unique per (originator, kind, key). Every row is keyed by the
//! originator's hostname, lowercased with its port dropped (the
//! reference's `normalizeOriginator`, ts-stack edf6e03
//! `WalletPermissionsManager.ts:5702-5729`): one grant covers every port
//! of its host. The name as given is kept beside it (`raw_originator`).
//!
//! An expiry is unix seconds and `0` never expires (`expiry > 0 && expiry
//! < now` is expired, `WalletPermissionsManager.ts:2900-2903`); a
//! `spending` row never expires, its ceiling resetting each UTC month.
//!
//! The served doors read these rows per power from the switch on (the
//! design's L5b, `src/server/powers.rs`); `ALLOWED_ORIGINS` is a seed beside
//! them and writes no row.

use anyhow::Result;
use bsv_wallet_toolbox::StorageSqlx;
use sqlx::{Sqlite, SqliteConnection};

mod migrate;
mod power;
pub mod probes;
mod rows;
mod shown;
mod spends;

use migrate::{Case, Moved};
pub use power::Power;
pub use probes::{Probe, ProbeRow};
pub use rows::{
    expired, key_of, month_of, when, when_exact, Abandoned, Answer, Answered, Asking, GrantRow,
    Granting, Kind, Listing, Pending, PendingRow, Reach, Refused, Reply, Standing,
};
use rows::{GrantTuple, PendingTuple};
pub use shown::{GrantShown, GrantState, Shown, ShownJson};
pub use spends::{
    drop_spend, month_tally, open_spend, settle_spend, signed, spent, unspent, Flow, OPEN,
};

/// The four tables and their four indexes, `(type, name)`, as
/// [`ensure`] reads them from `sqlite_master`.
const OBJECTS: [(&str, &str); 8] = [
    ("table", "origin_grants"),
    ("index", "origin_grants_power"),
    ("table", "origin_pending"),
    ("index", "origin_pending_open"),
    ("table", "origin_spends"),
    ("index", "origin_spends_month"),
    ("table", "origin_probes"),
    ("index", "origin_probes_seen"),
];

/// The brief reconnect's window in seconds (#534, the design's W19): the
/// server's `ABANDON_WINDOW`. A row abandoned at `a` is inside its window
/// while `now < a + WINDOW` and gone to every reader from then on, whether
/// or not the server's timer ran.
const WINDOW: i64 = crate::server::powers::ABANDON_WINDOW_SECS;

/// A row past its window is gone: the readers' filter, `now` bound once.
const INSIDE: &str = "(abandoned_at IS NULL OR abandoned_at > ? - ?)";

/// The block [`ensure`] runs in one transaction when any of [`OBJECTS`]
/// is missing (`IF NOT EXISTS` throughout), the design's section (1).
const DDL: [&str; 8] = [
    "CREATE TABLE IF NOT EXISTS origin_grants (\
        grant_id INTEGER PRIMARY KEY, \
        originator TEXT NOT NULL, \
        raw_originator TEXT NOT NULL, \
        kind TEXT NOT NULL CHECK (kind IN ('protocol', 'basket', 'certificate', 'spending')), \
        key TEXT NOT NULL, \
        amount INTEGER, \
        granted_at INTEGER NOT NULL, \
        expiry INTEGER NOT NULL DEFAULT 0, \
        revoked_at INTEGER, \
        CHECK ((kind = 'spending') = (amount IS NOT NULL)), \
        CHECK (kind <> 'spending' OR (key = '[]' AND expiry = 0)))",
    "CREATE UNIQUE INDEX IF NOT EXISTS origin_grants_power \
        ON origin_grants (originator, kind, key)",
    "CREATE TABLE IF NOT EXISTS origin_pending (\
        pending_id INTEGER PRIMARY KEY AUTOINCREMENT, \
        originator TEXT NOT NULL, \
        raw_originator TEXT NOT NULL, \
        kind TEXT NOT NULL CHECK (kind IN ('protocol', 'basket', 'certificate', 'spending')), \
        key TEXT NOT NULL, \
        usage TEXT, \
        call TEXT NOT NULL, \
        reason TEXT, \
        amount INTEGER, \
        renewal INTEGER NOT NULL DEFAULT 0, \
        first_seen INTEGER NOT NULL, \
        answered_at INTEGER, \
        answer TEXT CHECK (answer IN ('grant', 'once', 'deny')), \
        abandoned_at INTEGER, \
        CHECK ((kind = 'spending') = (amount IS NOT NULL)), \
        CHECK (abandoned_at IS NULL OR (answered_at IS NULL AND kind <> 'spending')))",
    "CREATE UNIQUE INDEX IF NOT EXISTS origin_pending_open \
        ON origin_pending (originator, kind, key) \
        WHERE answered_at IS NULL AND kind <> 'spending'",
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
    "CREATE TABLE IF NOT EXISTS origin_probes (\
        probe_id INTEGER PRIMARY KEY, \
        originator TEXT NOT NULL, \
        raw_originator TEXT NOT NULL, \
        door TEXT NOT NULL, \
        power TEXT NOT NULL DEFAULT '', \
        count INTEGER NOT NULL CHECK (count > 0), \
        first_seen INTEGER NOT NULL, \
        last_seen INTEGER NOT NULL)",
    "CREATE UNIQUE INDEX IF NOT EXISTS origin_probes_seen \
        ON origin_probes (originator, door, power)",
];

/// The tables and indexes, in the shape of the four kinds. The shape is
/// read on the reader first: a store holding all six objects with the
/// grants' `kind` column sends nothing to the writer and answers `false`.
/// Otherwise the case is decided again on the writer after `BEGIN
/// IMMEDIATE`, under its lock, so two processes that both read an old
/// shape move it once: a store another process moved in between answers
/// `false` and commits nothing; a store with no grants table, or the new
/// one missing an object, gains it; L1's tables are moved ([`migrate`]).
/// Answers `true` when it wrote. The toolbox's tables are never touched.
///
/// # Errors
///
/// The store's error: the reader's, or the writer's (a busy store is the
/// toolbox's typed `StoreBusy`, [`super::is_store_busy`]).
pub async fn ensure(storage: &StorageSqlx) -> Result<bool> {
    let mut reader = storage.pool().acquire().await?;
    if shape(&mut reader).await? == Case::Current {
        return Ok(false);
    }
    drop(reader);
    let mut tx = storage.write_tx().await?;
    let moved = match shape(&mut tx).await? {
        Case::Current => {
            tx.rollback().await?;
            return Ok(false);
        }
        Case::L1 { pending } => Some(move_l1(&mut tx, pending).await?),
        Case::Unsequenced => {
            sequence_pending(&mut tx).await?;
            None
        }
        Case::Unwindowed => {
            window_pending(&mut tx).await?;
            None
        }
        Case::Missing => {
            create(&mut tx).await?;
            None
        }
    };
    tx.commit().await?;
    if let Some(moved) = moved {
        moved.log();
    }
    Ok(true)
}

/// The store's shape read on `conn` (the reader's for the fast path, the
/// transaction's after `BEGIN IMMEDIATE` for the decision): the objects
/// present and whether the grants table has `kind`.
async fn shape(conn: &mut SqliteConnection) -> Result<Case> {
    let present: Vec<(String, String)> = sqlx::query_as(
        "SELECT type, name FROM sqlite_master WHERE tbl_name IN \
         ('origin_grants', 'origin_pending', 'origin_spends', 'origin_probes')",
    )
    .fetch_all(&mut *conn)
    .await?;
    let kinded: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM pragma_table_info('origin_grants') WHERE name = 'kind'",
    )
    .fetch_one(&mut *conn)
    .await?;
    let sequenced: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = 'origin_pending' \
         AND sql LIKE '%AUTOINCREMENT%'",
    )
    .fetch_one(&mut *conn)
    .await?;
    let windowed: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM pragma_table_info('origin_pending') WHERE name = 'abandoned_at'",
    )
    .fetch_one(&mut *conn)
    .await?;
    Ok(Case::of(&present, kinded > 0, sequenced > 0, windowed > 0))
}

/// Move L1's shape inside the caller's transaction ([`migrate`]): rename
/// the two tables aside and drop their indexes, run the block, carry each
/// ceilinged spending row, drop the old tables. `origin_spends` keeps its
/// shape and its rows.
async fn move_l1(conn: &mut SqliteConnection, pending: bool) -> Result<Moved> {
    let rows: Vec<migrate::L1Tuple> = sqlx::query_as(
        "SELECT originator, scope, amount, granted_at, revoked_at, source FROM origin_grants \
         ORDER BY grant_id",
    )
    .fetch_all(&mut *conn)
    .await?;
    let count: i64 = if pending {
        sqlx::query_scalar("SELECT COUNT(*) FROM origin_pending")
            .fetch_one(&mut *conn)
            .await?
    } else {
        0
    };
    let mut aside = vec![
        "ALTER TABLE origin_grants RENAME TO origin_grants_l1",
        "DROP INDEX IF EXISTS origin_grants_key",
    ];
    if pending {
        aside.push("ALTER TABLE origin_pending RENAME TO origin_pending_l1");
        aside.push("DROP INDEX IF EXISTS origin_pending_open");
    }
    for sql in aside {
        sqlx::query(sql).execute(&mut *conn).await?;
    }
    create(conn).await?;
    for sql in [
        "INSERT INTO origin_grants \
            (originator, raw_originator, kind, key, amount, granted_at, expiry, revoked_at) \
         SELECT originator, raw_originator, 'spending', '[]', amount, granted_at, 0, revoked_at \
         FROM origin_grants_l1 WHERE scope = 'spending' AND amount IS NOT NULL ORDER BY grant_id",
        "DROP TABLE origin_grants_l1",
        "DROP TABLE IF EXISTS origin_pending_l1",
    ] {
        sqlx::query(sql).execute(&mut *conn).await?;
    }
    Ok(Moved::of(rows, count))
}

/// Rebuild L5a's `origin_pending` as the block writes it, its ids never
/// reused (`AUTOINCREMENT`), inside the caller's transaction
/// ([`rebuild_pending`]). From the switch (L5b) a row whose caller left is
/// deleted; without the sequence SQLite would give its id to the next
/// request, and an owner answering `#N` from a listing read before the
/// deletion would answer another page's request.
async fn sequence_pending(conn: &mut SqliteConnection) -> Result<()> {
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
async fn window_pending(conn: &mut SqliteConnection) -> Result<()> {
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

/// The block, on the transaction's connection.
async fn create(conn: &mut SqliteConnection) -> Result<()> {
    for sql in DDL {
        sqlx::query(sql).execute(&mut *conn).await?;
    }
    Ok(())
}

const GRANT_COLUMNS: &str = "grant_id, originator, raw_originator, kind, key, amount, \
                             granted_at, expiry, revoked_at";

const PENDING_COLUMNS: &str = "pending_id, originator, raw_originator, kind, key, usage, call, \
                               reason, amount, renewal, first_seen, answered_at, answer, \
                               abandoned_at";

/// The pending columns every earlier shape holds, which a rebuild carries.
const CARRIED_COLUMNS: &str = "pending_id, originator, raw_originator, kind, key, usage, call, \
                               reason, amount, renewal, first_seen, answered_at, answer";

/// The originator's rows that cover `power`: its one (kind, key) row; for
/// a certificate, the rows of the same `privileged`, type and verifier
/// whose fields hold every field asked (the subset kept here).
async fn covering<'e, E: sqlx::Executor<'e, Database = Sqlite>>(
    executor: E,
    key: &str,
    power: &Power,
) -> Result<Vec<GrantRow>> {
    let rows: Vec<GrantTuple> = if power.kind() == Kind::Certificate {
        let sql = format!(
            "SELECT {GRANT_COLUMNS} FROM origin_grants \
             WHERE originator = ? AND kind = 'certificate' ORDER BY grant_id"
        );
        sqlx::query_as(&sql).bind(key).fetch_all(executor).await?
    } else {
        let sql = format!(
            "SELECT {GRANT_COLUMNS} FROM origin_grants WHERE originator = ? AND kind = ? AND key = ?"
        );
        sqlx::query_as(&sql)
            .bind(key)
            .bind(power.kind().as_str())
            .bind(power.key())
            .fetch_all(executor)
            .await?
    };
    let rows = rows
        .into_iter()
        .map(GrantRow::from_tuple)
        .collect::<Result<Vec<_>>>()?;
    Ok(rows
        .into_iter()
        .filter(|r| r.power().is_ok_and(|p| p.covers(power)))
        .collect())
}

async fn pending_row(conn: &mut SqliteConnection, pending_id: i64) -> Result<Option<PendingRow>> {
    let sql = format!("SELECT {PENDING_COLUMNS} FROM origin_pending WHERE pending_id = ?");
    let row: Option<PendingTuple> = sqlx::query_as(&sql)
        .bind(pending_id)
        .fetch_optional(conn)
        .await?;
    row.map(PendingRow::from_tuple).transpose()
}

/// Whether `row` is past its window at `now` (#534): gone to every
/// reader.
fn past_window(row: &PendingRow, now: i64) -> bool {
    row.withdrawn_at().is_some_and(|end| now >= end)
}

/// The pending row `pending_id` at `now`, open, abandoned inside its
/// window, or answered; none when a disconnect, a window's end or a
/// restart deleted it, or its window has passed (#534): what a held call
/// reads at each wake.
///
/// # Errors
///
/// The store's error.
pub async fn pending(
    storage: &StorageSqlx,
    pending_id: i64,
    now: i64,
) -> Result<Option<PendingRow>> {
    ensure(storage).await?;
    let mut reader = storage.pool().acquire().await?;
    let row = pending_row(&mut reader, pending_id).await?;
    Ok(row.filter(|r| !past_window(r, now)))
}

/// How `originator` (any name of its host) stands for `power` at `now`
/// (section (1)'s lookup): an indexed read on the reader.
///
/// # Errors
///
/// [`Refused::NotAnOriginator`], or the store's error.
pub async fn lookup(
    storage: &StorageSqlx,
    originator: &str,
    power: &Power,
    now: i64,
) -> Result<Standing> {
    ensure(storage).await?;
    let (key, _) = key_of(originator)?;
    Ok(Standing::of(
        covering(storage.pool(), &key, power).await?,
        now,
    ))
}

/// Write `g` as the one row of its (originator, kind, key), replacing a
/// row there: `granted_at`, `expiry` and `amount` set, `revoked_at`
/// cleared.
async fn upsert(conn: &mut SqliteConnection, g: &Granting<'_>, now: i64) -> Result<GrantRow> {
    g.check()?;
    let (originator, raw) = key_of(g.originator)?;
    let (kind, key) = (g.power.kind().as_str(), g.power.key());
    let sql = format!(
        "INSERT INTO origin_grants \
            (originator, raw_originator, kind, key, amount, granted_at, expiry, revoked_at) \
         VALUES (?, ?, ?, ?, ?, ?, ?, NULL) \
         ON CONFLICT (originator, kind, key) DO UPDATE SET \
            raw_originator = excluded.raw_originator, amount = excluded.amount, \
            granted_at = excluded.granted_at, expiry = excluded.expiry, revoked_at = NULL \
         RETURNING {GRANT_COLUMNS}"
    );
    let row: GrantTuple = sqlx::query_as(&sql)
        .bind(&originator)
        .bind(&raw)
        .bind(kind)
        .bind(&key)
        .bind(g.amount)
        .bind(now)
        .bind(g.expiry)
        .fetch_one(&mut *conn)
        .await?;
    GrantRow::from_tuple(row)
}

/// Grant: the upsert of the (originator, kind, key) row, the owner's
/// direct grant (a revoked row is granted again only here and by
/// [`answer`]).
///
/// # Errors
///
/// [`Refused`]: the originator's form, an amount on a kind other than
/// spending, a `spending` expiry, a `spending` grant with no amount, a
/// negative number; or the store's error.
pub async fn grant(storage: &StorageSqlx, granting: &Granting<'_>, now: i64) -> Result<GrantRow> {
    ensure(storage).await?;
    let mut tx = storage.write_tx().await?;
    let row = upsert(&mut tx, granting, now).await?;
    tx.commit().await?;
    Ok(row)
}

/// Revoke: stamp `revoked_at` on the originator's live rows the reach
/// names (every kind, one kind, or one power's row); nothing is deleted.
/// Answers the rows stamped; none when the originator holds no such row.
///
/// # Errors
///
/// [`Refused::NotAnOriginator`], or the store's error.
pub async fn revoke(
    storage: &StorageSqlx,
    originator: &str,
    reach: &Reach,
    now: i64,
) -> Result<Vec<GrantRow>> {
    ensure(storage).await?;
    let (key, _) = key_of(originator)?;
    let sql = format!(
        "UPDATE origin_grants SET revoked_at = ?1 \
         WHERE originator = ?2 AND revoked_at IS NULL \
           AND (?3 IS NULL OR kind = ?3) AND (?4 IS NULL OR key = ?4) \
         RETURNING {GRANT_COLUMNS}"
    );
    let (kind, power_key) = match reach {
        Reach::Every => (None, None),
        Reach::Kind(kind) => (Some(kind.as_str()), None),
        Reach::Power(p) => (Some(p.kind().as_str()), Some(p.key())),
    };
    let mut tx = storage.write_tx().await?;
    let rows: Vec<GrantTuple> = sqlx::query_as(&sql)
        .bind(now)
        .bind(&key)
        .bind(kind)
        .bind(power_key)
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

/// The amount's place on a call: a spending call names its satoshis, a
/// call of another kind none.
fn amount_placed(asking: &Asking<'_>) -> Result<(), Refused> {
    match (asking.power.kind(), asking.amount) {
        (_, Some(a)) if a < 0 => Err(Refused::Negative),
        (Kind::Spending, None) => Err(Refused::AmountRequired),
        (Kind::Spending, Some(_)) | (_, None) => Ok(()),
        (kind, Some(_)) => Err(Refused::AmountOnKind(kind)),
    }
}

/// Open or join: a call joins the open row of its (originator, kind, key)
/// (one per power, never duplicated; a call of another usage joins it,
/// the design's W16), else writes one; a spending call writes its own row
/// (the reference's per-request spending key). A revoked row covering the
/// power refuses at once and writes nothing. An abandoned row of the power
/// (#534) is resumed inside its window, its mark cleared in the same
/// transaction (`resumed`), and deleted past it before a new row is
/// written.
///
/// # Errors
///
/// [`Refused`]: the originator's form, the revoked row, the amount's
/// place; or the store's error.
pub async fn open_or_join(storage: &StorageSqlx, asking: &Asking<'_>, now: i64) -> Result<Pending> {
    ensure(storage).await?;
    amount_placed(asking)?;
    let (key, raw) = key_of(asking.originator)?;
    let (kind, power_key) = (asking.power.kind(), asking.power.key());
    let mut tx = storage.write_tx().await?;
    if let Standing::Revoked(row) = Standing::of(covering(&mut *tx, &key, asking.power).await?, now)
    {
        return Err(Refused::Revoked {
            originator: key,
            kind: row.kind,
            key: row.key,
            at: row.revoked_at.unwrap_or_default(),
        }
        .into());
    }
    let open: Option<(i64, Option<i64>)> = if kind == Kind::Spending {
        None
    } else {
        sqlx::query_as(
            "SELECT pending_id, abandoned_at FROM origin_pending \
             WHERE originator = ? AND kind = ? AND key = ? AND answered_at IS NULL",
        )
        .bind(&key)
        .bind(kind.as_str())
        .bind(&power_key)
        .fetch_optional(&mut *tx)
        .await?
    };
    let (open, resumed) = match open {
        Some((id, None)) => (Some(id), false),
        Some((id, Some(at))) if now < at.saturating_add(WINDOW) => {
            sqlx::query("UPDATE origin_pending SET abandoned_at = NULL WHERE pending_id = ?")
                .bind(id)
                .execute(&mut *tx)
                .await?;
            (Some(id), true)
        }
        Some((id, Some(_))) => {
            // Past its window: gone, though no timer deleted it.
            sqlx::query("DELETE FROM origin_pending WHERE pending_id = ?")
                .bind(id)
                .execute(&mut *tx)
                .await?;
            (None, false)
        }
        None => (None, false),
    };
    let joined = open.is_some();
    let id = match open {
        Some(id) => id,
        None => sqlx::query(
            "INSERT INTO origin_pending (originator, raw_originator, kind, key, usage, call, \
                reason, amount, renewal, first_seen) \
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(&key)
        .bind(&raw)
        .bind(kind.as_str())
        .bind(&power_key)
        .bind(asking.usage)
        .bind(asking.call)
        .bind(asking.reason)
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
    Ok(Pending {
        row,
        joined,
        resumed,
    })
}

/// Answer once: stamp `answered_at` and `answer` on an open row
/// (`UPDATE .. WHERE answered_at IS NULL AND abandoned_at IS NULL`, so two
/// places answering race to one winner, and the answer and a disconnect's
/// mark), and on `grant` write the row's power ([`grant`]'s upsert) in the
/// same transaction. The row is kept: the table is the history of the
/// owner's answers. A row abandoned inside its window is refused (#534, the
/// captain's word of 2026-11-04): no answer reaches a page that has left;
/// one past its window is no row.
///
/// # Errors
///
/// [`Refused`]: no such row, the row already answered (the second answer),
/// the row waiting for its page to reconnect ([`Refused::Abandoned`]),
/// `once` on a row of another kind than spending, a grant's amount or
/// expiry out of place (a spending `grant` names its ceiling and no
/// expiry), a stored key that does not read back; or the
/// store's error.
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
        .filter(|r| !past_window(r, now))
        .ok_or(Refused::NoSuchPending(pending_id))?;
    if let Some(withdrawn_at) = row.withdrawn_at() {
        return Err(Refused::Abandoned {
            pending_id,
            withdrawn_at,
        }
        .into());
    }
    if let Some(at) = row.answered_at {
        return Err(Refused::AlreadyAnswered {
            pending_id,
            answer: row.answer,
            at,
        }
        .into());
    }
    if reply.answer == Answer::Once && row.kind != Kind::Spending {
        return Err(Refused::OnceOnKind {
            pending_id,
            kind: row.kind,
        }
        .into());
    }
    let grant = if reply.answer == Answer::Grant {
        let power = Power::parse(row.kind, &row.key)?;
        let granting = Granting {
            originator: &row.raw_originator,
            power: &power,
            amount: reply.amount,
            expiry: reply.expiry,
        };
        Some(upsert(&mut tx, &granting, now).await?)
    } else {
        None
    };
    sqlx::query(
        "UPDATE origin_pending SET answered_at = ?, answer = ? \
         WHERE pending_id = ? AND answered_at IS NULL AND abandoned_at IS NULL",
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
/// [`Refused::NotAnOriginator`], or the store's error.
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
    let marks = vec!["?"; ids.len()].join(", ");
    let delete = format!(
        "DELETE FROM origin_pending \
         WHERE originator = ? AND pending_id IN ({marks}) AND answered_at IS NULL \
           AND kind = 'spending' \
         RETURNING {PENDING_COLUMNS}"
    );
    let mark = format!(
        "UPDATE origin_pending SET abandoned_at = ? \
         WHERE originator = ? AND pending_id IN ({marks}) AND answered_at IS NULL \
           AND abandoned_at IS NULL AND kind <> 'spending' \
         RETURNING {PENDING_COLUMNS}"
    );
    let mut deleting = sqlx::query_as::<_, PendingTuple>(&delete).bind(&key);
    let mut marking = sqlx::query_as::<_, PendingTuple>(&mark)
        .bind(now)
        .bind(&key);
    for id in ids {
        deleting = deleting.bind(id);
        marking = marking.bind(id);
    }
    let mut tx = storage.write_tx().await?;
    let deleted = deleting.fetch_all(&mut *tx).await?;
    let marked = marking.fetch_all(&mut *tx).await?;
    tx.commit().await?;
    let done = Abandoned {
        deleted: sorted(deleted)?,
        marked: sorted(marked)?,
    };
    if !done.deleted.is_empty() {
        withdrawn(&key, &done.deleted);
    }
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
/// [`Refused::NotAnOriginator`], or the store's error.
pub async fn expire(
    storage: &StorageSqlx,
    originator: &str,
    ids: &[i64],
    abandoned_at: i64,
) -> Result<Vec<PendingRow>> {
    ensure(storage).await?;
    let (key, _) = key_of(originator)?;
    if ids.is_empty() {
        return Ok(Vec::new());
    }
    let marks = vec!["?"; ids.len()].join(", ");
    let sql = format!(
        "DELETE FROM origin_pending \
         WHERE originator = ? AND pending_id IN ({marks}) AND answered_at IS NULL \
           AND abandoned_at = ? \
         RETURNING {PENDING_COLUMNS}"
    );
    let mut query = sqlx::query_as::<_, PendingTuple>(&sql).bind(&key);
    for id in ids {
        query = query.bind(id);
    }
    let mut tx = storage.write_tx().await?;
    let rows = query.bind(abandoned_at).fetch_all(&mut *tx).await?;
    tx.commit().await?;
    let rows = sorted(rows)?;
    if !rows.is_empty() {
        withdrawn(&key, &rows);
    }
    Ok(rows)
}

/// Rows read back, by id.
fn sorted(rows: Vec<PendingTuple>) -> Result<Vec<PendingRow>> {
    let mut rows: Vec<PendingRow> = rows
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

/// The deleted rows of a disconnect or a window's end, by name.
fn withdrawn(key: &str, rows: &[PendingRow]) {
    tracing::info!(
        marker = "origin_grants_abandoned",
        originator = %key,
        rows = %named(rows),
        "browser gate: abandoned"
    );
}

/// The restart's sweep (section (2)): delete every open pending row, an
/// abandoned one too (#534), since no caller survives a restart; logged by
/// name with the count. Answers the count.
///
/// # Errors
///
/// The store's error.
pub async fn sweep_open(storage: &StorageSqlx) -> Result<u64> {
    ensure(storage).await?;
    let mut tx = storage.write_tx().await?;
    let swept = sqlx::query("DELETE FROM origin_pending WHERE answered_at IS NULL")
        .execute(&mut *tx)
        .await?
        .rows_affected();
    tx.commit().await?;
    tracing::info!(
        marker = "origin_grants_swept",
        swept,
        "origin grants: {swept} open requests from a previous run withdrawn; their callers are gone"
    );
    Ok(swept)
}

/// The listing `bsv-wallet origins` prints (section (4)): every grant row,
/// by originator, kind and key, and the pending rows by id, the open ones
/// (an abandoned one inside its window among them, #534) alone unless
/// `all` asks for the answered ones too; a row past its window at `now` is
/// never listed. Read on the reader.
///
/// # Errors
///
/// The store's error.
pub async fn listing(storage: &StorageSqlx, all: bool, now: i64) -> Result<Listing> {
    listed(storage, "? OR answered_at IS NULL", i64::from(all), now).await
}

/// The listing the owner's door answers (`GET /origins`, section (3)):
/// every grant row as [`listing`] reads them, and the open pending rows
/// with the last `answered` answered ones (by their answer's time, then
/// id), by id; a row past its window at `now` is never listed. Read on the
/// reader.
///
/// # Errors
///
/// The store's error.
pub async fn owners_listing(storage: &StorageSqlx, answered: i64, now: i64) -> Result<Listing> {
    let recent = "answered_at IS NULL OR pending_id IN (SELECT pending_id FROM origin_pending \
                  WHERE answered_at IS NOT NULL ORDER BY answered_at DESC, pending_id DESC LIMIT ?)";
    listed(storage, recent, answered, now).await
}

/// Every grant row, the pending rows `pending_where` (one bound number)
/// selects inside their window at `now`, by id, and every probe row
/// ([`probes::probes`]).
async fn listed(
    storage: &StorageSqlx,
    pending_where: &str,
    bound: i64,
    now: i64,
) -> Result<Listing> {
    ensure(storage).await?;
    let sql = format!("SELECT {GRANT_COLUMNS} FROM origin_grants ORDER BY originator, kind, key");
    let grants: Vec<GrantTuple> = sqlx::query_as(&sql).fetch_all(storage.pool()).await?;
    let sql = format!(
        "SELECT {PENDING_COLUMNS} FROM origin_pending \
         WHERE ({pending_where}) AND {INSIDE} ORDER BY pending_id"
    );
    let pending: Vec<PendingTuple> = sqlx::query_as(&sql)
        .bind(bound)
        .bind(now)
        .bind(WINDOW)
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
        probes: probes::probes(storage).await?,
    })
}

#[cfg(test)]
mod migrate_tests;
#[cfg(test)]
mod pending_tests;
#[cfg(test)]
mod spends_tests;
#[cfg(test)]
mod tests;
#[cfg(test)]
mod window_tests;
