//! The row functions of `origin_grants` (#414, the design's L1 exit test),
//! each on a `wallet.db` the toolbox made, in a temporary directory.

use super::*;
use bsv_wallet_toolbox::WalletStorageWriter;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

/// 2026-11-02T00:00:00Z.
pub(super) const NOW: i64 = 1_793_577_600;
/// The last second of October 2026 and the first of November, UTC.
pub(super) const OCT_LAST: i64 = 1_793_491_199;
pub(super) const NOV_FIRST: i64 = 1_793_491_200;

/// A store at `wallet.db` with the toolbox's migrations run, a user and
/// the tracker's tables, as an existing wallet's file holds; the directory
/// lives as long as it.
pub(super) async fn wallet_db() -> (tempfile::TempDir, StorageSqlx) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("wallet.db");
    let storage = StorageSqlx::open(path.to_str().unwrap()).await.unwrap();
    let identity = "02".repeat(33);
    storage.migrate("origin-grants", &identity).await.unwrap();
    storage.make_available().await.unwrap();
    storage.find_or_insert_user(&identity).await.unwrap();
    // The CLI's other tables, as a served wallet's file holds them.
    crate::tracker_host::ensure_tables(&storage).await.unwrap();
    (dir, storage)
}

/// The writer's commits from now on, counted by SQLite's commit hook on
/// the store's one writer connection.
pub(super) async fn commits_on(storage: &StorageSqlx) -> Arc<AtomicU64> {
    let seen = Arc::new(AtomicU64::new(0));
    let hook = Arc::clone(&seen);
    let mut writer = storage.writer().acquire().await.unwrap();
    writer
        .lock_handle()
        .await
        .unwrap()
        .set_commit_hook(move || {
            hook.fetch_add(1, Ordering::SeqCst);
            true
        });
    seen
}

/// Every `sqlite_master` row and every row of every table, but the
/// grants' own, as text: a dump of the toolbox's side of the file.
pub(super) async fn toolbox_dump(storage: &StorageSqlx) -> Vec<String> {
    let master: Vec<(String, String, String, i64, Option<String>)> = sqlx::query_as(
        "SELECT type, name, tbl_name, rootpage, sql FROM sqlite_master \
         WHERE tbl_name NOT LIKE 'origin\\_%' ESCAPE '\\' ORDER BY type, name",
    )
    .fetch_all(storage.pool())
    .await
    .unwrap();
    let mut dump: Vec<String> = master.iter().map(|m| format!("{m:?}")).collect();
    for (kind, table, ..) in &master {
        if kind != "table" {
            continue;
        }
        let columns: Vec<String> =
            sqlx::query_scalar("SELECT name FROM pragma_table_info(?) ORDER BY cid")
                .bind(table)
                .fetch_all(storage.pool())
                .await
                .unwrap();
        let row = columns
            .iter()
            .map(|c| format!("quote(\"{c}\")"))
            .collect::<Vec<_>>()
            .join(" || '|' || ");
        let rows: Vec<String> =
            sqlx::query_scalar(&format!("SELECT {row} FROM \"{table}\" ORDER BY 1"))
                .fetch_all(storage.pool())
                .await
                .unwrap();
        dump.push(format!("{table}: {rows:?}"));
    }
    dump
}

/// The grants' own objects in `sqlite_master`, `(type, name)`, sorted.
pub(super) async fn own_objects(storage: &StorageSqlx) -> Vec<(String, String)> {
    sqlx::query_as(
        "SELECT type, name FROM sqlite_master \
         WHERE tbl_name LIKE 'origin\\_%' ESCAPE '\\' ORDER BY type, name",
    )
    .fetch_all(storage.pool())
    .await
    .unwrap()
}

pub(super) fn refusal(e: &anyhow::Error) -> Refused {
    e.downcast_ref::<Refused>()
        .unwrap_or_else(|| panic!("not a refusal: {e:#}"))
        .clone()
}

pub(super) fn owner(
    originator: &str,
    scope: Scope,
    amount: Option<i64>,
    expiry: i64,
) -> Granting<'_> {
    Granting {
        originator,
        scope,
        amount,
        expiry,
        source: Source::Owner,
    }
}

pub(super) fn wallet_call<'a>(originator: &'a str, call: &'a str) -> Asking<'a> {
    Asking {
        originator,
        scope: Scope::Wallet,
        call,
        amount: None,
        renewal: false,
    }
}

pub(super) fn spend_call(originator: &str, amount: i64) -> Asking<'_> {
    Asking {
        originator,
        scope: Scope::Spending,
        call: "/createAction",
        amount: Some(amount),
        renewal: false,
    }
}

pub(super) fn reply(answer: Answer, amount: Option<i64>) -> Reply {
    Reply {
        answer,
        expiry: 0,
        amount,
    }
}

pub(super) async fn count(storage: &StorageSqlx, table: &str) -> i64 {
    sqlx::query_scalar(&format!("SELECT COUNT(*) FROM {table}"))
        .fetch_one(storage.pool())
        .await
        .unwrap()
}

#[tokio::test]
async fn an_existing_wallet_db_gains_the_three_tables_and_nothing_else_moves() {
    let (_dir, storage) = wallet_db().await;
    let before = toolbox_dump(&storage).await;
    assert!(own_objects(&storage).await.is_empty());

    assert!(ensure(&storage).await.unwrap(), "the first ensure writes");

    assert_eq!(
        toolbox_dump(&storage).await,
        before,
        "the toolbox's side moved"
    );
    let mut expected: Vec<(String, String)> = OBJECTS
        .iter()
        .map(|(t, n)| ((*t).to_string(), (*n).to_string()))
        .collect();
    expected.sort();
    assert_eq!(own_objects(&storage).await, expected);
}

#[tokio::test]
async fn a_second_ensure_sends_nothing_to_the_writer() {
    let (_dir, storage) = wallet_db().await;
    ensure(&storage).await.unwrap();
    let commits = commits_on(&storage).await;
    let schema = own_objects(&storage).await;

    assert!(!ensure(&storage).await.unwrap(), "the second ensure wrote");

    assert_eq!(commits.load(Ordering::SeqCst), 0, "the writer committed");
    assert_eq!(own_objects(&storage).await, schema);
    // The hook counts: a write through the writer is seen.
    granted(&storage, "app.example").await;
    assert!(commits.load(Ordering::SeqCst) > 0);
}

#[tokio::test]
async fn a_store_missing_an_index_gains_it_and_keeps_its_rows() {
    let (_dir, storage) = wallet_db().await;
    granted(&storage, "app.example").await;
    sqlx::query("DROP INDEX origin_pending_open")
        .execute(storage.writer())
        .await
        .unwrap();

    assert!(
        ensure(&storage).await.unwrap(),
        "the missing index was not seen"
    );

    assert_eq!(own_objects(&storage).await.len(), OBJECTS.len());
    assert_eq!(count(&storage, "origin_grants").await, 1);
}

#[tokio::test]
async fn lookup_reads_none_live_expired_and_revoked_by_the_host() {
    let (_dir, storage) = wallet_db().await;
    let read = |name: &'static str, now| {
        let storage = &storage;
        async move { lookup(storage, name, Scope::Wallet, now).await.unwrap() }
    };
    assert_eq!(read("app.example", NOW).await, Standing::None);

    let row = grant(
        &storage,
        &owner("App.Example:8443", Scope::Wallet, None, NOW + 10),
        NOW,
    )
    .await
    .unwrap();
    assert_eq!(
        (row.originator.as_str(), row.raw_originator.as_str()),
        ("app.example", "app.example:8443")
    );
    // Every port of the host reads the one row.
    for name in ["app.example", "app.example:8080", "APP.EXAMPLE:8443"] {
        assert!(matches!(read(name, NOW).await, Standing::Live(_)), "{name}");
    }
    assert_eq!(read("other.example", NOW).await, Standing::None);
    assert_eq!(
        lookup(&storage, "app.example", Scope::Spending, NOW)
            .await
            .unwrap(),
        Standing::None
    );

    revoke(&storage, "app.example", None, NOW).await.unwrap();
    // Revoked is read before the expiry.
    assert!(matches!(
        read("app.example", NOW + 11).await,
        Standing::Revoked(_)
    ));
}

#[tokio::test]
async fn the_expiry_reads_at_its_boundary_and_zero_never_expires() {
    // The reference's `expiry > 0 && expiry < now`.
    assert!(!expired(0, i64::MAX));
    assert!(!expired(NOW, NOW - 1));
    assert!(!expired(NOW, NOW), "live through its expiry's second");
    assert!(expired(NOW, NOW + 1));

    let (_dir, storage) = wallet_db().await;
    grant(
        &storage,
        &owner("a.example", Scope::Wallet, None, NOW),
        NOW - 60,
    )
    .await
    .unwrap();
    grant(
        &storage,
        &owner("b.example", Scope::Wallet, None, 0),
        NOW - 60,
    )
    .await
    .unwrap();
    let at = |name, now| {
        let storage = &storage;
        async move { lookup(storage, name, Scope::Wallet, now).await.unwrap() }
    };
    assert!(matches!(at("a.example", NOW).await, Standing::Live(_)));
    assert!(matches!(
        at("a.example", NOW + 1).await,
        Standing::Expired(_)
    ));
    assert!(matches!(at("b.example", i64::MAX).await, Standing::Live(_)));
}

#[tokio::test]
async fn a_wallet_call_joins_the_open_row_and_never_duplicates_it() {
    let (_dir, storage) = wallet_db().await;
    let first = open_or_join(
        &storage,
        &wallet_call("new.example:5173", "/listOutputs"),
        NOW,
    )
    .await
    .unwrap();
    assert!(!first.joined);
    assert_eq!(first.row.raw_originator, "new.example:5173");
    assert_eq!(first.row.call, "/listOutputs");
    assert_eq!(first.row.answered_at, None);

    // Another call, another port of the host: the same row.
    let second = open_or_join(
        &storage,
        &wallet_call("new.example", "/getVersion"),
        NOW + 5,
    )
    .await
    .unwrap();
    assert!(second.joined);
    assert_eq!(second.row, first.row);
    assert_eq!(count(&storage, "origin_pending").await, 1);

    // The partial unique index refuses a second open wallet row outright.
    let dup = sqlx::query(
        "INSERT INTO origin_pending (originator, raw_originator, scope, call, first_seen) \
         VALUES ('new.example', 'new.example', 'wallet', '/x', 0)",
    )
    .execute(storage.writer())
    .await;
    assert!(dup.is_err());

    // Answered, the row is history: the next call opens a new one.
    answer(
        &storage,
        first.row.pending_id,
        &reply(Answer::Deny, None),
        NOW + 9,
    )
    .await
    .unwrap();
    let third = open_or_join(
        &storage,
        &wallet_call("new.example", "/listOutputs"),
        NOW + 10,
    )
    .await
    .unwrap();
    assert!(!third.joined);
    assert_ne!(third.row.pending_id, first.row.pending_id);
}

#[tokio::test]
async fn each_spending_call_writes_its_own_row() {
    let (_dir, storage) = wallet_db().await;
    let a = open_or_join(&storage, &spend_call("app.example", 2500), NOW)
        .await
        .unwrap();
    let b = open_or_join(&storage, &spend_call("app.example", 2500), NOW)
        .await
        .unwrap();
    assert!(!a.joined && !b.joined);
    assert_ne!(a.row.pending_id, b.row.pending_id);
    assert_eq!(a.row.amount, Some(2500));

    let mut no_amount = spend_call("app.example", 1);
    no_amount.amount = None;
    let e = open_or_join(&storage, &no_amount, NOW).await.unwrap_err();
    assert_eq!(refusal(&e), Refused::AmountRequired);
    let mut wallet_amount = wallet_call("app.example", "/listOutputs");
    wallet_amount.amount = Some(1);
    let e = open_or_join(&storage, &wallet_amount, NOW)
        .await
        .unwrap_err();
    assert_eq!(refusal(&e), Refused::AmountOnWallet);
    assert_eq!(count(&storage, "origin_pending").await, 2);
}

#[tokio::test]
async fn a_revoked_origin_is_refused_at_once_with_no_pending_row() {
    let (_dir, storage) = wallet_db().await;
    granted(&storage, "old.example").await;
    revoke(&storage, "old.example", None, NOW + 1)
        .await
        .unwrap();

    let e = open_or_join(
        &storage,
        &wallet_call("old.example:8080", "/listOutputs"),
        NOW + 2,
    )
    .await
    .unwrap_err();

    assert_eq!(
        refusal(&e),
        Refused::Revoked {
            originator: "old.example".to_string(),
            at: NOW + 1
        }
    );
    assert_eq!(
        e.to_string(),
        "the grant row old.example was revoked by the owner at 2026-11-02T00:00Z"
    );
    assert_eq!(count(&storage, "origin_pending").await, 0);
}

#[tokio::test]
async fn a_row_is_answered_once_and_the_second_answer_is_refused() {
    let (_dir, storage) = wallet_db().await;
    let asked = open_or_join(
        &storage,
        &wallet_call("new.example:5173", "/listOutputs"),
        NOW,
    )
    .await
    .unwrap();
    let id = asked.row.pending_id;

    let won = answer(&storage, id, &reply(Answer::Grant, None), NOW + 30)
        .await
        .unwrap();

    assert_eq!(won.pending.answered_at, Some(NOW + 30));
    assert_eq!(won.pending.answer, Some(Answer::Grant));
    let granted = won.grant.unwrap();
    assert_eq!(
        (
            granted.originator.as_str(),
            granted.raw_originator.as_str(),
            granted.scope,
            granted.source,
            granted.expiry,
            granted.amount,
        ),
        (
            "new.example",
            "new.example:5173",
            Scope::Wallet,
            Source::Owner,
            0,
            None
        )
    );
    for second in [Answer::Deny, Answer::Grant] {
        let e = answer(&storage, id, &reply(second, None), NOW + 31)
            .await
            .unwrap_err();
        assert_eq!(
            refusal(&e),
            Refused::AlreadyAnswered {
                pending_id: id,
                answer: Some(Answer::Grant),
                at: NOW + 30
            }
        );
    }
    let e = answer(&storage, id, &reply(Answer::Deny, None), NOW + 31)
        .await
        .unwrap_err();
    assert_eq!(
        e.to_string(),
        format!("pending request #{id} was already answered grant at 2026-11-02T00:00Z")
    );
    let e = answer(&storage, id + 1, &reply(Answer::Deny, None), NOW)
        .await
        .unwrap_err();
    assert_eq!(refusal(&e), Refused::NoSuchPending(id + 1));
    // The row is kept: the history of the owner's prompts.
    assert_eq!(count(&storage, "origin_pending").await, 1);
    assert_eq!(count(&storage, "origin_grants").await, 1);
}

/// The owner's `answer` to row `id` at [`NOW`].
pub(super) async fn say(
    storage: &StorageSqlx,
    id: i64,
    said: Answer,
    amount: Option<i64>,
) -> Result<Answered> {
    answer(storage, id, &reply(said, amount), NOW).await
}

/// A call's pending row, opened or joined at [`NOW`].
pub(super) async fn ask(storage: &StorageSqlx, asking: &Asking<'_>) -> PendingRow {
    open_or_join(storage, asking, NOW).await.unwrap().row
}

#[tokio::test]
async fn once_answers_a_spend_alone() {
    let (_dir, storage) = wallet_db().await;
    let wallet = ask(&storage, &wallet_call("app.example", "/listOutputs")).await;
    let e = say(&storage, wallet.pending_id, Answer::Once, None)
        .await
        .unwrap_err();
    assert_eq!(refusal(&e), Refused::OnceOnWallet(wallet.pending_id));
    let open = answered_at(&storage, wallet.pending_id).await;
    assert_eq!(open, None, "a refused answer stamped the row");

    let spend = ask(&storage, &spend_call("app.example", 2500)).await;
    let once = say(&storage, spend.pending_id, Answer::Once, None)
        .await
        .unwrap();
    assert_eq!(
        (once.pending.answer, once.grant),
        (Some(Answer::Once), None)
    );
    assert_eq!(count(&storage, "origin_grants").await, 0);
}

#[tokio::test]
async fn a_spending_grant_names_its_ceiling_and_a_deny_writes_no_grant() {
    let (_dir, storage) = wallet_db().await;
    let spend = ask(&storage, &spend_call("app.example", 2500)).await;
    let e = say(&storage, spend.pending_id, Answer::Grant, None)
        .await
        .unwrap_err();
    assert_eq!(refusal(&e), Refused::AmountRequired);
    let ceiling = say(&storage, spend.pending_id, Answer::Grant, Some(60_000))
        .await
        .unwrap();
    assert_eq!(ceiling.grant.unwrap().amount, Some(60_000));

    let wallet = ask(&storage, &wallet_call("app.example", "/listOutputs")).await;
    let denied = say(&storage, wallet.pending_id, Answer::Deny, None)
        .await
        .unwrap();
    assert_eq!(
        (denied.pending.answer, denied.grant),
        (Some(Answer::Deny), None)
    );
    let standing = lookup(&storage, "app.example", Scope::Wallet, NOW).await;
    assert_eq!(standing.unwrap(), Standing::None);
}

/// The owner's `wallet` grant of `name` at [`NOW`], never expiring.
pub(super) async fn granted(storage: &StorageSqlx, name: &str) -> GrantRow {
    let granting = owner(name, Scope::Wallet, None, 0);
    grant(storage, &granting, NOW).await.unwrap()
}

/// The `answered_at` stamped on pending row `id`.
pub(super) async fn answered_at(storage: &StorageSqlx, id: i64) -> Option<i64> {
    sqlx::query_scalar("SELECT answered_at FROM origin_pending WHERE pending_id = ?")
        .bind(id)
        .fetch_one(storage.pool())
        .await
        .unwrap()
}

#[tokio::test]
async fn a_grant_is_one_row_per_scope_and_a_new_grant_replaces_it() {
    let (_dir, storage) = wallet_db().await;
    let ceiling = |amount| owner("app.example", Scope::Spending, Some(amount), 0);
    let first = grant(&storage, &ceiling(100), NOW).await.unwrap();
    let spending = Some(Scope::Spending);
    revoke(&storage, "app.example", spending, NOW + 1)
        .await
        .unwrap();

    let mut moved = ceiling(500);
    moved.originator = "app.example:9";
    let again = grant(&storage, &moved, NOW + 2).await.unwrap();

    assert_eq!(
        again.grant_id, first.grant_id,
        "one row per (originator, scope)"
    );
    assert_eq!(
        (again.amount, again.granted_at, again.revoked_at),
        (Some(500), NOW + 2, None)
    );
    assert_eq!(again.raw_originator, "app.example:9");
    assert_eq!(count(&storage, "origin_grants").await, 1);
}

#[tokio::test]
async fn a_grant_out_of_its_rules_is_refused_by_name() {
    let (_dir, storage) = wallet_db().await;
    let cases = [
        (Scope::Wallet, Some(1), 0, Refused::AmountOnWallet),
        (Scope::Spending, Some(1), NOW, Refused::ExpiryOnSpending),
        (Scope::Spending, None, 0, Refused::AmountRequired),
        (Scope::Wallet, None, -1, Refused::Negative),
        (Scope::Spending, Some(-1), 0, Refused::Negative),
    ];
    for (scope, amount, expiry, refused) in cases {
        let granting = owner("app.example", scope, amount, expiry);
        let e = grant(&storage, &granting, NOW).await.unwrap_err();
        assert_eq!(refusal(&e), refused, "{granting:?}");
    }
    let schemed = owner("https://app.example", Scope::Wallet, None, 0);
    let e = grant(&storage, &schemed, NOW).await.unwrap_err();
    assert!(matches!(refusal(&e), Refused::NotAnOriginator { .. }));
    let text = e.to_string();
    assert!(
        text.starts_with("\"https://app.example\" is not an originator"),
        "{text}"
    );
    assert_eq!(count(&storage, "origin_grants").await, 0);
}

#[tokio::test]
async fn a_revoke_stamps_and_deletes_nothing() {
    let (_dir, storage) = wallet_db().await;
    granted(&storage, "app.example").await;
    grant(
        &storage,
        &owner("app.example", Scope::Spending, Some(9), 0),
        NOW,
    )
    .await
    .unwrap();

    let wallet_only = revoke(&storage, "app.example", Some(Scope::Wallet), NOW + 1)
        .await
        .unwrap();
    assert_eq!(wallet_only.len(), 1);
    assert_eq!(wallet_only[0].revoked_at, Some(NOW + 1));
    let rest = revoke(&storage, "app.example:8080", None, NOW + 2)
        .await
        .unwrap();
    assert_eq!(
        rest.iter()
            .map(|r| (r.scope, r.revoked_at))
            .collect::<Vec<_>>(),
        vec![(Scope::Spending, Some(NOW + 2))]
    );
    assert!(revoke(&storage, "app.example", None, NOW + 3)
        .await
        .unwrap()
        .is_empty());
    assert_eq!(count(&storage, "origin_grants").await, 2);
}
