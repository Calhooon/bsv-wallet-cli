//! The row functions of `origin_grants` over a power of each kind (#414,
//! the design's L5a exit test), each on a `wallet.db` the toolbox made, in
//! a temporary directory.

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

/// The generator's public key: a verifier.
pub(super) const G: &str = "0279be667ef9dcbbac55a06295ce870b07029bfcdb2dce28d959f2815b16f81798";
/// A certificate type, 32 bytes in base64.
pub(super) const TYPE: &str = "AGfk/WrT1eBDXpz3mcw386Zww2HmqcIn3uY6x4Af1eo=";

/// A protocol power at level 2, counterparty `self`.
pub(super) fn protocol(name: &str) -> Power {
    Power::protocol(false, 2, name, None).unwrap()
}

/// A basket power.
pub(super) fn basket(name: &str) -> Power {
    Power::basket(name).unwrap()
}

/// A certificate power of [`TYPE`] from [`G`] over `fields`.
pub(super) fn certificate(fields: &[&str]) -> Power {
    Power::certificate(false, TYPE, G, fields).unwrap()
}

/// One power of each kind, a label among the protocols, each with a
/// usage of its calls and the spending ceiling a grant of it names.
pub(super) fn every_kind() -> Vec<(Power, Option<&'static str>, Option<i64>)> {
    vec![
        (protocol("todo list"), Some("signing"), None),
        (Power::label("invoices").unwrap(), Some("apply"), None),
        (basket("todo items"), Some("listing"), None),
        (certificate(&["email", "name"]), Some("disclosure"), None),
        (Power::Spending, None, Some(60_000)),
    ]
}

pub(super) fn owner<'a>(
    originator: &'a str,
    power: &'a Power,
    amount: Option<i64>,
    expiry: i64,
) -> Granting<'a> {
    Granting {
        originator,
        power,
        amount,
        expiry,
    }
}

/// [`every_kind`] but spending: the powers a call asks one open row of.
pub(super) fn asked_kinds() -> Vec<(Power, Option<&'static str>, Option<i64>)> {
    let mut kinds = every_kind();
    kinds.retain(|(p, ..)| p.kind() != Kind::Spending);
    kinds
}

/// A call of `power` at `usage` from `originator` to `call`; a spending
/// power's call asks 2500 satoshis.
pub(super) fn call<'a>(
    originator: &'a str,
    power: &'a Power,
    usage: Option<&'a str>,
    call: &'a str,
) -> Asking<'a> {
    Asking {
        originator,
        power,
        usage,
        call,
        reason: None,
        amount: (power.kind() == Kind::Spending).then_some(2500),
        renewal: false,
    }
}

pub(super) fn spend_call(originator: &str, amount: i64) -> Asking<'_> {
    Asking {
        originator,
        power: &Power::Spending,
        usage: None,
        call: "/createAction",
        reason: Some("Buy a coffee"),
        amount: Some(amount),
        renewal: false,
    }
}

/// A `listOutputs` call of the basket `todo items`.
pub(super) fn listing_call(originator: &str) -> Asking<'_> {
    static TODO: std::sync::LazyLock<Power> = std::sync::LazyLock::new(|| basket("todo items"));
    call(originator, &TODO, Some("listing"), "/listOutputs")
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

/// The owner's grant of `power` to `name` at [`NOW`], never expiring.
pub(super) async fn granted(storage: &StorageSqlx, name: &str, power: &Power) -> GrantRow {
    let amount = (power.kind() == Kind::Spending).then_some(60_000);
    grant(storage, &owner(name, power, amount, 0), NOW)
        .await
        .unwrap()
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

/// The `answered_at` stamped on pending row `id`.
pub(super) async fn answered_at(storage: &StorageSqlx, id: i64) -> Option<i64> {
    sqlx::query_scalar("SELECT answered_at FROM origin_pending WHERE pending_id = ?")
        .bind(id)
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
    assert_eq!(own_objects(&storage).await, new_objects());
}

/// The new shape's objects, `(type, name)`, sorted.
pub(super) fn new_objects() -> Vec<(String, String)> {
    let mut objects: Vec<(String, String)> = OBJECTS
        .iter()
        .map(|(t, n)| ((*t).to_string(), (*n).to_string()))
        .collect();
    objects.sort();
    objects
}

/// A second `ensure` on `storage` answers `false`, the writer commits
/// nothing and the schema stands.
pub(super) async fn a_second_ensure_writes_nothing(storage: &StorageSqlx) -> Arc<AtomicU64> {
    let commits = commits_on(storage).await;
    let schema = own_objects(storage).await;
    assert!(!ensure(storage).await.unwrap(), "the second ensure wrote");
    assert_eq!(commits.load(Ordering::SeqCst), 0, "the writer committed");
    assert_eq!(own_objects(storage).await, schema);
    commits
}

#[tokio::test]
async fn a_second_ensure_sends_nothing_to_the_writer() {
    let (_dir, storage) = wallet_db().await;
    ensure(&storage).await.unwrap();
    let commits = a_second_ensure_writes_nothing(&storage).await;
    // The hook counts: a write through the writer is seen.
    granted(&storage, "app.example", &basket("todo items")).await;
    assert!(commits.load(Ordering::SeqCst) > 0);
}

#[tokio::test]
async fn a_store_missing_an_index_gains_it_and_keeps_its_rows() {
    let (_dir, storage) = wallet_db().await;
    granted(&storage, "app.example", &basket("todo items")).await;
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
async fn each_kind_is_granted_looked_up_and_revoked_by_its_power() {
    let (_dir, storage) = wallet_db().await;
    let other = basket("other basket");
    for (power, _, amount) in every_kind() {
        let read = |name: &'static str, p: &Power, now| {
            let (storage, p) = (&storage, p.clone());
            async move { lookup(storage, name, &p, now).await.unwrap() }
        };
        assert_eq!(read("app.example", &power, NOW).await, Standing::None);
        let row = grant(&storage, &owner("App.Example:8443", &power, amount, 0), NOW)
            .await
            .unwrap();
        assert_eq!(
            (row.originator.as_str(), row.raw_originator.as_str()),
            ("app.example", "app.example:8443")
        );
        assert_eq!(
            (row.kind, row.key.clone(), row.amount),
            (power.kind(), power.key(), amount)
        );
        // Every port of the host reads the one row; another host none.
        for name in ["app.example", "app.example:8080", "APP.EXAMPLE:8443"] {
            assert!(
                matches!(read(name, &power, NOW).await, Standing::Live(_)),
                "{name}"
            );
        }
        assert_eq!(read("other.example", &power, NOW).await, Standing::None);
        // The row grants its power alone.
        assert_eq!(read("app.example", &other, NOW).await, Standing::None);

        let reach = Reach::Power(power.clone());
        let stamped = revoke(&storage, "app.example", &reach, NOW + 1)
            .await
            .unwrap();
        assert_eq!(stamped.len(), 1, "{power:?}");
        assert!(matches!(
            read("app.example", &power, NOW + 2).await,
            Standing::Revoked(_)
        ));
    }
    assert_eq!(count(&storage, "origin_grants").await, 5);
}

#[tokio::test]
async fn a_certificate_row_covers_the_calls_whose_fields_it_holds() {
    let (_dir, storage) = wallet_db().await;
    granted(&storage, "app.example", &certificate(&["email", "name"])).await;
    let read = |fields: &'static [&'static str]| {
        let storage = &storage;
        async move {
            lookup(storage, "app.example", &certificate(fields), NOW)
                .await
                .unwrap()
        }
    };
    assert!(matches!(read(&["email"]).await, Standing::Live(_)));
    assert!(matches!(read(&["name", "email"]).await, Standing::Live(_)));
    assert_eq!(read(&["phone"]).await, Standing::None);
    assert_eq!(read(&["email", "phone"]).await, Standing::None);

    // A live covering row admits though a narrower one is revoked.
    granted(&storage, "app.example", &certificate(&["email"])).await;
    let narrow = Reach::Power(certificate(&["email"]));
    revoke(&storage, "app.example", &narrow, NOW).await.unwrap();
    assert!(matches!(read(&["email"]).await, Standing::Live(_)));
    // With every covering row revoked, the call is refused.
    let wide = Reach::Power(certificate(&["email", "name"]));
    revoke(&storage, "app.example", &wide, NOW).await.unwrap();
    assert!(matches!(read(&["email"]).await, Standing::Revoked(_)));
}

#[tokio::test]
async fn the_expiry_reads_at_its_boundary_and_zero_never_expires() {
    // The reference's `expiry > 0 && expiry < now`.
    assert!(!expired(0, i64::MAX));
    assert!(!expired(NOW, NOW - 1));
    assert!(!expired(NOW, NOW), "live through its expiry's second");
    assert!(expired(NOW, NOW + 1));

    let (_dir, storage) = wallet_db().await;
    let power = protocol("todo list");
    let dated = owner("a.example", &power, None, NOW);
    grant(&storage, &dated, NOW - 60).await.unwrap();
    grant(&storage, &owner("b.example", &power, None, 0), NOW - 60)
        .await
        .unwrap();
    let at = |name, now| {
        let (storage, power) = (&storage, &power);
        async move { lookup(storage, name, power, now).await.unwrap() }
    };
    assert!(matches!(at("a.example", NOW).await, Standing::Live(_)));
    assert!(matches!(
        at("a.example", NOW + 1).await,
        Standing::Expired(_)
    ));
    assert!(matches!(at("b.example", i64::MAX).await, Standing::Live(_)));
}

#[tokio::test]
async fn open_or_join_joins_per_originator_kind_and_key() {
    let (_dir, storage) = wallet_db().await;
    for (power, usage, _) in asked_kinds() {
        let first = open_or_join(
            &storage,
            &call("new.example:5173", &power, usage, "/x"),
            NOW,
        )
        .await
        .unwrap();
        assert!(!first.joined);
        assert_eq!(
            (
                first.row.kind,
                first.row.key.clone(),
                first.row.usage.as_deref()
            ),
            (power.kind(), power.key(), usage)
        );
        assert_eq!(first.row.raw_originator, "new.example:5173");
        assert_eq!(first.row.answered_at, None);
        // Another call of another usage, from another port: the same row,
        // keeping the usage of the call that wrote it (W16).
        let again = call("new.example", &power, Some("other"), "/y");
        let second = open_or_join(&storage, &again, NOW + 5).await.unwrap();
        assert!(second.joined, "{power:?}");
        assert_eq!(second.row, first.row);
    }
    // One open row per power: four powers, four rows.
    assert_eq!(count(&storage, "origin_pending").await, 4);
    // Another basket of the same originator is another power.
    let other = basket("other basket");
    let row = ask(
        &storage,
        &call("new.example", &other, Some("listing"), "/x"),
    )
    .await;
    assert_eq!(count(&storage, "origin_pending").await, 5);

    // The partial unique index refuses a second open row of one power.
    let dup = sqlx::query(
        "INSERT INTO origin_pending (originator, raw_originator, kind, key, call, first_seen) \
         VALUES ('new.example', 'new.example', 'basket', '[\"other basket\"]', '/x', 0)",
    )
    .execute(storage.writer())
    .await;
    assert!(dup.is_err());

    // Answered, the row is history: the next call opens a new one.
    say(&storage, row.pending_id, Answer::Deny, None)
        .await
        .unwrap();
    let third = open_or_join(&storage, &call("new.example", &other, None, "/x"), NOW + 10)
        .await
        .unwrap();
    assert!(!third.joined);
    assert_ne!(third.row.pending_id, row.pending_id);
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
    assert_eq!(
        (
            a.row.kind,
            a.row.key.as_str(),
            a.row.amount,
            a.row.usage.clone()
        ),
        (Kind::Spending, "[]", Some(2500), None)
    );
    assert_eq!(a.row.reason.as_deref(), Some("Buy a coffee"));

    let mut no_amount = spend_call("app.example", 1);
    no_amount.amount = None;
    let e = open_or_join(&storage, &no_amount, NOW).await.unwrap_err();
    assert_eq!(refusal(&e), Refused::AmountRequired);
    let mut basket_amount = listing_call("app.example");
    basket_amount.amount = Some(1);
    let e = open_or_join(&storage, &basket_amount, NOW)
        .await
        .unwrap_err();
    assert_eq!(refusal(&e), Refused::AmountOnKind(Kind::Basket));
    assert_eq!(count(&storage, "origin_pending").await, 2);
}

#[tokio::test]
async fn a_revoked_power_is_refused_at_once_and_another_power_is_asked() {
    let (_dir, storage) = wallet_db().await;
    let todo = basket("todo items");
    granted(&storage, "old.example", &todo).await;
    revoke(&storage, "old.example", &Reach::Every, NOW + 1)
        .await
        .unwrap();

    let e = open_or_join(&storage, &listing_call("old.example:8080"), NOW + 2)
        .await
        .unwrap_err();

    assert_eq!(
        refusal(&e),
        Refused::Revoked {
            originator: "old.example".to_string(),
            kind: Kind::Basket,
            key: todo.key(),
            at: NOW + 1
        }
    );
    assert_eq!(
        e.to_string(),
        "the grant row old.example basket [\"todo items\"] was revoked by the owner at \
         2026-11-02T00:00Z"
    );
    assert_eq!(count(&storage, "origin_pending").await, 0);
    // The revoke reaches its own row alone: another power is asked.
    let other = protocol("todo list");
    ask(
        &storage,
        &call("old.example", &other, Some("signing"), "/createSignature"),
    )
    .await;
    assert_eq!(count(&storage, "origin_pending").await, 1);
}

#[tokio::test]
async fn a_row_is_answered_once_and_the_second_answer_is_refused() {
    let (_dir, storage) = wallet_db().await;
    let asked = ask(&storage, &listing_call("new.example:5173")).await;
    let id = asked.pending_id;

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
            granted.kind,
            granted.key.as_str(),
            granted.expiry,
            granted.amount,
        ),
        (
            "new.example",
            "new.example:5173",
            Kind::Basket,
            r#"["todo items"]"#,
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
    let e = say(&storage, id, Answer::Deny, None).await.unwrap_err();
    assert_eq!(
        e.to_string(),
        format!("pending request #{id} was already answered grant at 2026-11-02T00:00Z")
    );
    let e = say(&storage, id + 1, Answer::Deny, None).await.unwrap_err();
    assert_eq!(refusal(&e), Refused::NoSuchPending(id + 1));
    // The row is kept: the history of the owner's answers.
    assert_eq!(count(&storage, "origin_pending").await, 1);
    assert_eq!(count(&storage, "origin_grants").await, 1);
}

#[tokio::test]
async fn each_kind_is_answered_into_its_own_power() {
    let (_dir, storage) = wallet_db().await;
    for (power, usage, amount) in asked_kinds() {
        let row = ask(&storage, &call("app.example", &power, usage, "/x")).await;
        let answered = say(&storage, row.pending_id, Answer::Grant, amount)
            .await
            .unwrap();
        let g = answered.grant.unwrap();
        assert_eq!(
            (g.kind, g.key, g.amount),
            (power.kind(), power.key(), amount)
        );
        let standing = lookup(&storage, "app.example", &power, NOW).await.unwrap();
        assert!(matches!(standing, Standing::Live(_)), "{power:?}");
    }
    assert_eq!(count(&storage, "origin_grants").await, 4);
}

#[tokio::test]
async fn once_answers_a_spend_alone() {
    let (_dir, storage) = wallet_db().await;
    for (power, usage, _) in asked_kinds() {
        let row = ask(&storage, &call("app.example", &power, usage, "/x")).await;
        let e = say(&storage, row.pending_id, Answer::Once, None)
            .await
            .unwrap_err();
        let kind = power.kind();
        let id = row.pending_id;
        assert_eq!(
            refusal(&e),
            Refused::OnceOnKind {
                pending_id: id,
                kind
            }
        );
        assert_eq!(
            e.to_string(),
            format!(
                "pending request #{id} asks for a {} power; once answers a spending request alone",
                kind.as_str()
            )
        );
        assert_eq!(
            answered_at(&storage, id).await,
            None,
            "a refused answer stamped"
        );
    }

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
async fn a_spend_is_answered_once_or_deny_until_l6_and_a_deny_writes_no_grant() {
    let (_dir, storage) = wallet_db().await;
    // The switch (#414 L5b): a spending row's `grant` is refused by name
    // until the ceilings land (L6), with or without an amount; the row
    // stays open for `once` or `deny`.
    let spend = ask(&storage, &spend_call("app.example", 2500)).await;
    for amount in [None, Some(60_000)] {
        let e = say(&storage, spend.pending_id, Answer::Grant, amount)
            .await
            .unwrap_err();
        assert_eq!(refusal(&e), Refused::GrantOnSpending(spend.pending_id));
        assert!(
            e.to_string().contains("answered once or deny, never grant"),
            "{e}"
        );
    }
    let denied = say(&storage, spend.pending_id, Answer::Deny, None).await;
    assert_eq!(denied.unwrap().pending.answer, Some(Answer::Deny));

    let listing = ask(&storage, &listing_call("app.example")).await;
    let e = say(&storage, listing.pending_id, Answer::Grant, Some(5))
        .await
        .unwrap_err();
    assert_eq!(refusal(&e), Refused::AmountOnKind(Kind::Basket));
    let denied = say(&storage, listing.pending_id, Answer::Deny, None)
        .await
        .unwrap();
    assert_eq!(
        (denied.pending.answer, denied.grant),
        (Some(Answer::Deny), None)
    );
    let standing = lookup(&storage, "app.example", &basket("todo items"), NOW).await;
    assert_eq!(standing.unwrap(), Standing::None);
}

#[tokio::test]
async fn a_grant_is_one_row_per_power_and_a_new_grant_replaces_it() {
    let (_dir, storage) = wallet_db().await;
    let ceiling = |amount| owner("app.example", &Power::Spending, Some(amount), 0);
    let first = grant(&storage, &ceiling(100), NOW).await.unwrap();
    let spending = Reach::Kind(Kind::Spending);
    revoke(&storage, "app.example", &spending, NOW + 1)
        .await
        .unwrap();

    let mut moved = ceiling(500);
    moved.originator = "app.example:9";
    let again = grant(&storage, &moved, NOW + 2).await.unwrap();

    assert_eq!(
        again.grant_id, first.grant_id,
        "one row per (originator, kind, key)"
    );
    assert_eq!(
        (again.amount, again.granted_at, again.revoked_at),
        (Some(500), NOW + 2, None)
    );
    assert_eq!(again.raw_originator, "app.example:9");
    // Two baskets are two rows.
    granted(&storage, "app.example", &basket("one")).await;
    granted(&storage, "app.example", &basket("two")).await;
    granted(&storage, "app.example", &basket("one")).await;
    assert_eq!(count(&storage, "origin_grants").await, 3);
}

#[tokio::test]
async fn a_grant_out_of_its_rules_is_refused_by_name() {
    let (_dir, storage) = wallet_db().await;
    let todo = basket("todo items");
    let cases = [
        (&todo, Some(1), 0, Refused::AmountOnKind(Kind::Basket)),
        (&Power::Spending, Some(1), NOW, Refused::ExpiryOnSpending),
        (&Power::Spending, None, 0, Refused::AmountRequired),
        (&todo, None, -1, Refused::Negative),
        (&Power::Spending, Some(-1), 0, Refused::Negative),
    ];
    for (power, amount, expiry, refused) in cases {
        let granting = owner("app.example", power, amount, expiry);
        let e = grant(&storage, &granting, NOW).await.unwrap_err();
        assert_eq!(refusal(&e), refused, "{granting:?}");
    }
    let schemed = owner("https://app.example", &todo, None, 0);
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
async fn a_revoke_reaches_a_power_a_kind_or_every_row_and_deletes_nothing() {
    let (_dir, storage) = wallet_db().await;
    for (power, ..) in every_kind() {
        granted(&storage, "app.example", &power).await;
    }
    granted(&storage, "app.example", &basket("other basket")).await;

    let one = Reach::Power(basket("todo items"));
    let stamped = revoke(&storage, "app.example", &one, NOW + 1)
        .await
        .unwrap();
    assert_eq!(stamped.len(), 1);
    assert_eq!(
        (stamped[0].key.as_str(), stamped[0].revoked_at),
        (r#"["todo items"]"#, Some(NOW + 1))
    );
    let kind = Reach::Kind(Kind::Basket);
    let stamped = revoke(&storage, "app.example:8080", &kind, NOW + 2)
        .await
        .unwrap();
    assert_eq!(stamped.len(), 1, "the other basket, the revoked one passed");
    assert_eq!(stamped[0].key, r#"["other basket"]"#);
    let rest = revoke(&storage, "app.example", &Reach::Every, NOW + 3)
        .await
        .unwrap();
    let kinds: Vec<Kind> = rest.iter().map(|r| r.kind).collect();
    assert_eq!(
        kinds,
        [
            Kind::Protocol,
            Kind::Protocol,
            Kind::Certificate,
            Kind::Spending
        ]
    );
    assert!(revoke(&storage, "app.example", &Reach::Every, NOW + 4)
        .await
        .unwrap()
        .is_empty());
    assert_eq!(count(&storage, "origin_grants").await, 6);
}
