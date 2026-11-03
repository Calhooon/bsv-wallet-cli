//! The move of L1's tables to the four kinds (#414, the design's L5a exit
//! test, section (1)'s four cases), the month's tally and the stored
//! words, on the fixture of `super::tests`.

use super::tests::{
    a_second_ensure_writes_nothing, ask, call, count, new_objects, own_objects, toolbox_dump,
    wallet_db, NOV_FIRST, NOW, OCT_LAST,
};
use super::*;
use crate::test_support::Markers;

/// L1's block as PR 449 landed it (`src/store/origin_grants.rs` at
/// `bb9a742`): the shape a store an unreleased beta build touched holds.
const L1_DDL: [&str; 6] = [
    "CREATE TABLE origin_grants (grant_id INTEGER PRIMARY KEY, originator TEXT NOT NULL, \
     raw_originator TEXT NOT NULL, scope TEXT NOT NULL CHECK (scope IN ('wallet', 'spending')), \
     amount INTEGER, granted_at INTEGER NOT NULL, expiry INTEGER NOT NULL DEFAULT 0, \
     revoked_at INTEGER, source TEXT NOT NULL CHECK (source IN ('owner', 'env')))",
    "CREATE UNIQUE INDEX origin_grants_key ON origin_grants (originator, scope)",
    "CREATE TABLE origin_pending (pending_id INTEGER PRIMARY KEY, originator TEXT NOT NULL, \
     raw_originator TEXT NOT NULL, scope TEXT NOT NULL CHECK (scope IN ('wallet', 'spending')), \
     call TEXT NOT NULL, amount INTEGER, renewal INTEGER NOT NULL DEFAULT 0, \
     first_seen INTEGER NOT NULL, answered_at INTEGER, \
     answer TEXT CHECK (answer IN ('grant', 'once', 'deny')))",
    "CREATE UNIQUE INDEX origin_pending_open ON origin_pending (originator) \
     WHERE answered_at IS NULL AND scope = 'wallet'",
    "CREATE TABLE origin_spends (spend_id INTEGER PRIMARY KEY, originator TEXT NOT NULL, \
     month TEXT NOT NULL, txid TEXT, reference TEXT, satoshis INTEGER NOT NULL, \
     at INTEGER NOT NULL, CHECK (txid IS NOT NULL OR reference IS NOT NULL))",
    "CREATE INDEX origin_spends_month ON origin_spends (originator, month)",
];

/// L1's tables written on `storage`, with `rows` run after them.
async fn l1_store(storage: &StorageSqlx, rows: &[&str]) {
    for sql in L1_DDL.iter().chain(rows) {
        sqlx::query(sql).execute(storage.writer()).await.unwrap();
    }
}

/// A store holding L1's tables and their rows, under a capture: every test
/// that moves L1 logs under one, since a callsite first reached with no
/// live subscriber may cache its interest as never.
struct L1 {
    dir: tempfile::TempDir,
    storage: StorageSqlx,
    markers: Markers,
    _guard: tracing::subscriber::DefaultGuard,
}

/// [`L1`] with `rows` run after L1's tables.
async fn l1_captured(rows: &[&str]) -> L1 {
    let (dir, storage) = wallet_db().await;
    let (markers, _guard) = Markers::capture();
    l1_store(&storage, rows).await;
    L1 {
        dir,
        storage,
        markers,
        _guard,
    }
}

/// The grants' columns, in order.
async fn grant_columns(storage: &StorageSqlx) -> Vec<String> {
    sqlx::query_scalar("SELECT name FROM pragma_table_info('origin_grants') ORDER BY cid")
        .fetch_all(storage.pool())
        .await
        .unwrap()
}

/// A wallet row, a ceilinged spending row (revoked), a ceiling-free env
/// spending row, two pending rows (one open, one answered) and a spend.
const L1_ROWS: [&str; 4] = [
    "INSERT INTO origin_grants (originator, raw_originator, scope, amount, granted_at, expiry, \
     revoked_at, source) VALUES \
     ('app.example', 'app.example:8443', 'wallet', NULL, 1793697120, 0, NULL, 'owner'), \
     ('app.example', 'app.example:8443', 'spending', 60000, 1793697120, 0, 1793697180, 'owner'), \
     ('env.example', 'env.example', 'spending', NULL, 1793697120, 0, NULL, 'env')",
    "INSERT INTO origin_pending (originator, raw_originator, scope, call, first_seen) VALUES \
     ('new.example', 'new.example:5173', 'wallet', '/listOutputs', 1793697120)",
    "INSERT INTO origin_pending (originator, raw_originator, scope, call, amount, first_seen, \
     answered_at, answer) VALUES \
     ('app.example', 'app.example', 'spending', '/createAction', 2500, 1793697120, 1793697130, \
     'once')",
    "INSERT INTO origin_spends (originator, month, txid, satoshis, at) VALUES \
     ('app.example', '2026-11', 'aa', 2500, 1793697130)",
];

/// The move's line for each of [`L1_ROWS`]' grant rows and its pending
/// rows' count, in order.
const L1_LINES: [&str; 4] = [
    "origin grants: the L1 wallet grant for app.example (granted 2026-11-03T09:12Z) names \
             no power under the four kinds and is not carried; the page is asked per power from \
             the switch on",
    "origin grants: the L1 spending grant for app.example (granted 2026-11-03T09:12Z, \
             60000/month, revoked at 2026-11-03T09:13Z) is carried as the row app.example \
             spending []",
    "origin grants: the L1 spending grant for env.example (granted 2026-11-03T09:12Z, \
             source env) has no ceiling and is not carried: a spending row names its month's \
             ceiling",
    "origin grants: 2 L1 pending requests are not carried: no server wrote one, and an \
             answered one answers a scope that no longer exists",
];

#[tokio::test]
async fn an_l1_store_with_no_rows_moves_and_a_second_ensure_sends_nothing() {
    let l1 = l1_captured(&[]).await;
    let storage = l1.storage;
    assert!(!grant_columns(&storage).await.contains(&"kind".to_string()));

    assert!(ensure(&storage).await.unwrap(), "the move wrote");

    assert_eq!(own_objects(&storage).await, new_objects());
    let columns = grant_columns(&storage).await;
    assert!(columns.contains(&"kind".to_string()) && !columns.contains(&"scope".to_string()));
    a_second_ensure_writes_nothing(&storage).await;
}

#[tokio::test]
async fn an_l1_store_keeps_its_ceilinged_spending_row_alone_and_logs_the_rest_by_name() {
    let l1 = l1_captured(&L1_ROWS).await;
    let (storage, markers) = (l1.storage, l1.markers);

    assert!(ensure(&storage).await.unwrap());

    let rows = listing(&storage, true).await.unwrap();
    assert_eq!(rows.pending, []);
    let [carried] = rows.grants.as_slice() else {
        panic!("{:?}", rows.grants);
    };
    assert_eq!(
        (
            carried.originator.as_str(),
            carried.raw_originator.as_str(),
            carried.kind,
            carried.key.as_str(),
            carried.amount,
            carried.granted_at,
            carried.expiry,
            carried.revoked_at,
        ),
        (
            "app.example",
            "app.example:8443",
            Kind::Spending,
            "[]",
            Some(60_000),
            1_793_697_120,
            0,
            Some(1_793_697_180)
        )
    );
    assert_eq!(count(&storage, "origin_spends").await, 1, "the tally stays");
    assert_eq!(
        own_objects(&storage).await,
        new_objects(),
        "the _l1 tables dropped"
    );

    let lines: Vec<String> = markers
        .named("origin_grants_l1")
        .into_iter()
        .map(|m| m.message.unwrap_or_default())
        .collect();
    assert_eq!(lines, L1_LINES);
    let moved = markers.named("origin_grants_moved");
    assert_eq!(moved.len(), 1);
    assert_eq!(moved[0].level, Some(tracing::Level::WARN));
    assert_eq!(
        moved[0].message.as_deref(),
        Some(
            "origin grants: the store's L1 tables moved to the four kinds (1 grant rows carried, \
             2 not carried, 2 pending requests not carried)"
        )
    );
}

#[tokio::test]
async fn two_ensures_on_one_l1_store_both_answer_and_move_it_once() {
    let l1 = l1_captured(&L1_ROWS).await;
    let first = l1.storage;
    // A second handle on the file, as a second process holds it.
    let path = l1.dir.path().join("wallet.db");
    let second = StorageSqlx::open(path.to_str().unwrap()).await.unwrap();

    let (a, b) = tokio::join!(ensure(&first), ensure(&second));

    let mut wrote = [a.unwrap(), b.unwrap()];
    wrote.sort_unstable();
    assert_eq!(wrote, [false, true], "one moved, the other found it moved");
    assert_eq!(l1.markers.named("origin_grants_moved").len(), 1);
    assert_eq!(own_objects(&first).await, new_objects());
    assert_eq!(count(&first, "origin_grants").await, 1, "carried once");
    assert!(!ensure(&second).await.unwrap());
}

#[tokio::test]
async fn the_move_leaves_the_toolboxs_side_byte_identical() {
    let l1 = l1_captured(&L1_ROWS).await;
    let storage = l1.storage;
    let before = toolbox_dump(&storage).await;

    ensure(&storage).await.unwrap();
    // A fresh store's block and a partial one's leave it too.
    sqlx::query("DROP INDEX origin_grants_power")
        .execute(storage.writer())
        .await
        .unwrap();
    assert!(
        ensure(&storage).await.unwrap(),
        "the missing index was not seen"
    );

    assert_eq!(
        toolbox_dump(&storage).await,
        before,
        "the toolbox's side moved"
    );
    assert_eq!(own_objects(&storage).await, new_objects());
}

#[tokio::test]
async fn an_interrupted_move_leaves_l1_and_the_next_ensure_moves_it() {
    let l1 = l1_captured(&L1_ROWS).await;
    let storage = l1.storage;
    // The move's transaction, interrupted before its commit.
    let mut tx = storage.write_tx().await.unwrap();
    move_l1(&mut tx, true).await.unwrap();
    drop(tx);
    assert!(!grant_columns(&storage).await.contains(&"kind".to_string()));
    assert_eq!(
        count(&storage, "origin_pending").await,
        2,
        "L1's rows stand"
    );

    assert!(ensure(&storage).await.unwrap());
    assert_eq!(count(&storage, "origin_grants").await, 1);
}

async fn spent(storage: &StorageSqlx, originator: &str, txid: Option<&str>, sats: i64, at: i64) {
    sqlx::query(
        "INSERT INTO origin_spends (originator, month, txid, reference, satoshis, at) \
         VALUES (?, ?, ?, ?, ?, ?)",
    )
    .bind(originator)
    .bind(month_of(at).unwrap())
    .bind(txid)
    .bind(txid.is_none().then_some("cmVm"))
    .bind(sats)
    .bind(at)
    .execute(storage.writer())
    .await
    .unwrap();
}

#[tokio::test]
async fn the_months_tally_turns_at_the_month_boundary() {
    assert_eq!(month_of(OCT_LAST).unwrap(), "2026-10");
    assert_eq!(month_of(NOV_FIRST).unwrap(), "2026-11");

    let (_dir, storage) = wallet_db().await;
    assert_eq!(month_tally(&storage, "app.example", NOW).await.unwrap(), 0);
    spent(&storage, "app.example", Some("aa"), 1000, OCT_LAST).await;
    spent(&storage, "app.example", None, 300, NOV_FIRST).await;
    spent(&storage, "app.example", Some("bb"), 20, NOW).await;
    spent(&storage, "other.example", Some("cc"), 7, NOW).await;

    let tally = |name, at| {
        let storage = &storage;
        async move { month_tally(storage, name, at).await.unwrap() }
    };
    assert_eq!(tally("app.example", OCT_LAST).await, 1000);
    assert_eq!(tally("app.example:8443", NOV_FIRST).await, 320);
    assert_eq!(tally("other.example", NOV_FIRST - 1).await, 0);

    // A spend row names its txid (signed) or its reference (deferred).
    let neither = sqlx::query(
        "INSERT INTO origin_spends (originator, month, satoshis, at) \
         VALUES ('app.example', '2026-11', 1, 0)",
    )
    .execute(storage.writer())
    .await;
    assert!(neither.is_err());
}

#[tokio::test]
async fn the_tables_refuse_a_row_off_its_kinds_rules() {
    let (_dir, storage) = wallet_db().await;
    ensure(&storage).await.unwrap();
    for values in [
        "('a', 'a', 'wallet', '[]', NULL, 0, 0)",
        "('a', 'a', 'spending', '[]', NULL, 0, 0)",
        "('a', 'a', 'spending', '[\"x\"]', 5, 0, 0)",
        "('a', 'a', 'spending', '[]', 5, 0, 9)",
        "('a', 'a', 'basket', '[\"x\"]', 5, 0, 0)",
    ] {
        let sql = format!(
            "INSERT INTO origin_grants (originator, raw_originator, kind, key, amount, \
             granted_at, expiry) VALUES {values}"
        );
        let refused = sqlx::query(&sql).execute(storage.writer()).await;
        assert!(refused.is_err(), "{values}");
    }
    assert_eq!(count(&storage, "origin_grants").await, 0);
}

#[test]
fn the_stored_words_read_back() {
    for kind in Kind::ALL {
        assert_eq!(Kind::parse(kind.as_str()).unwrap(), kind);
        assert_eq!(Kind::named(kind.as_str()).unwrap(), kind);
    }
    for answer in [Answer::Grant, Answer::Once, Answer::Deny] {
        assert_eq!(Answer::parse(answer.as_str()).unwrap(), answer);
    }
    assert!(Kind::parse("wallet").is_err(), "the withdrawn scope");
    let e = Kind::named("wallet").unwrap_err().to_string();
    assert_eq!(
        e,
        "\"wallet\" is not a kind (protocol, basket, certificate, spending)"
    );
    assert_eq!(
        key_of(" App.Example:8443 ").unwrap(),
        ("app.example".to_string(), "app.example:8443".to_string())
    );
    assert_eq!(key_of("[::1]:3000").unwrap().0, "[::1]");
}

/// L5a's block as it landed (`origin_pending` keyed by a bare `INTEGER
/// PRIMARY KEY`): its other objects are the current block's.
const L5A_PENDING: &str = "CREATE TABLE origin_pending (pending_id INTEGER PRIMARY KEY, \
     originator TEXT NOT NULL, raw_originator TEXT NOT NULL, \
     kind TEXT NOT NULL CHECK (kind IN ('protocol', 'basket', 'certificate', 'spending')), \
     key TEXT NOT NULL, usage TEXT, call TEXT NOT NULL, reason TEXT, amount INTEGER, \
     renewal INTEGER NOT NULL DEFAULT 0, first_seen INTEGER NOT NULL, answered_at INTEGER, \
     answer TEXT CHECK (answer IN ('grant', 'once', 'deny')), \
     CHECK ((kind = 'spending') = (amount IS NOT NULL)))";

/// An open basket row of `originator` (any name of its host).
async fn opened(storage: &StorageSqlx, originator: &str, basket: &str) -> i64 {
    let power = Power::basket(basket).unwrap();
    let row = ask(storage, &call(originator, &power, Some("listing"), "/x")).await;
    row.pending_id
}

/// #414 L5b: a row whose caller left is deleted, and its id is never given
/// to another request, so an owner answering `#N` from a listing read
/// before the deletion cannot answer another page's request.
#[tokio::test]
async fn a_withdrawn_requests_id_is_never_given_again() {
    let (_dir, storage) = wallet_db().await;
    let gone = opened(&storage, "gone.example", "browser gate").await;
    assert_eq!(
        abandon(&storage, "gone.example", &[gone])
            .await
            .unwrap()
            .len(),
        1
    );
    let next = opened(&storage, "evil.example", "browser gate").await;
    assert!(
        next > gone,
        "the deleted top id {gone} is given again as {next}"
    );
    assert_eq!(sweep_open(&storage).await.unwrap(), 1);
    let after = opened(&storage, "evil.example", "browser gate").await;
    assert!(
        after > next,
        "the swept id {next} is given again as {after}"
    );
}

/// An L5a store (every object, `origin_pending` without `AUTOINCREMENT`)
/// is rebuilt once with its rows and their ids, and its next id passes
/// them; a second ensure sends nothing to the writer.
#[tokio::test]
async fn an_l5a_store_gains_ids_never_reused_with_its_rows_carried() {
    let (_dir, storage) = wallet_db().await;
    let (markers, _guard) = Markers::capture();
    ensure(&storage).await.unwrap();
    for sql in [
        "DROP TABLE origin_pending",
        L5A_PENDING,
        "CREATE UNIQUE INDEX origin_pending_open ON origin_pending (originator, kind, key) \
         WHERE answered_at IS NULL AND kind <> 'spending'",
        "INSERT INTO origin_pending (pending_id, originator, raw_originator, kind, key, usage, \
         call, first_seen, answered_at, answer) VALUES \
         (7, 'app.example', 'app.example', 'basket', '[\"todo items\"]', 'listing', \
          '/listOutputs', 1793697120, 1793697130, 'deny')",
    ] {
        sqlx::query(sql).execute(storage.writer()).await.unwrap();
    }
    assert!(ensure(&storage).await.unwrap(), "the rebuild wrote");
    assert_eq!(own_objects(&storage).await, new_objects());
    let kept = listing(&storage, true).await.unwrap().pending;
    assert_eq!(
        kept.iter()
            .map(|r| (r.pending_id, r.answer))
            .collect::<Vec<_>>(),
        [(7, Some(Answer::Deny))]
    );
    assert!(opened(&storage, "app.example", "next basket").await > 7);
    assert_eq!(markers.named("origin_grants_sequenced").len(), 1);
    a_second_ensure_writes_nothing(&storage).await;
}
