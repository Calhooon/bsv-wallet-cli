//! The spend gate against real SQLite storage and the toolbox's mock
//! services: a header service stood in by the mock, nothing on the network.

use super::*;
use crate::tracker_host::tests::{header, header_with_hash};
use bsv_sdk::primitives::PrivateKey;
use bsv_sdk::transaction::MerklePath;
use bsv_sdk::wallet::{CreateActionOptions, CreateActionOutput, WalletInterface};
use bsv_wallet_toolbox::services::mock::MockWalletServices;
use bsv_wallet_toolbox::WalletStorageWriter;
use chrono::Utc;

mod lineage;

const LOCK: &str = "76a914dbc0a7c84983c5bf199b7b2d41b3acf0408ee5aa88ac";

/// A migrated in-memory storage for `identity`: (storage, user, basket).
async fn storage_for(identity: &str) -> (StorageSqlx, i64, i64) {
    let storage = StorageSqlx::in_memory().await.unwrap();
    storage.migrate("spend-gate", identity).await.unwrap();
    storage.make_available().await.unwrap();
    let (user, _) = storage.find_or_insert_user(identity).await.unwrap();
    let basket = storage
        .find_or_create_default_basket(user.user_id)
        .await
        .unwrap()
        .basket_id;
    (storage, user.user_id, basket)
}

/// One coin: a change output of transaction `txid` in the default basket.
/// `made` is an action of ours (`is_outgoing`); `mined_at` gives the
/// toolbox a stored proof at that height (its root is the txid).
async fn coin(
    storage: &StorageSqlx,
    (user, basket): (i64, i64),
    txid: &str,
    made: bool,
    mined_at: Option<u32>,
    satoshis: i64,
    minutes_old: i64,
) -> i64 {
    let when = Utc::now() - chrono::Duration::minutes(minutes_old);
    let status = if mined_at.is_some() {
        "completed"
    } else {
        "unproven"
    };
    let tx_row = sqlx::query(
        "INSERT INTO transactions (user_id, status, reference, is_outgoing, satoshis, version, lock_time, description, txid, raw_tx, created_at, updated_at) \
         VALUES (?, ?, ?, ?, 0, 1, 0, 'gate', ?, X'01000000', ?, ?)",
    )
    .bind(user)
    .bind(status)
    .bind(format!("ref-{}", &txid[..8]))
    .bind(made as i64)
    .bind(txid)
    .bind(when)
    .bind(when)
    .execute(storage.writer())
    .await
    .unwrap()
    .last_insert_rowid();
    if let Some(height) = mined_at {
        sqlx::query(
            "INSERT INTO proven_txs (txid, height, idx, block_hash, merkle_root, merkle_path, raw_tx, created_at, updated_at) \
             VALUES (?, ?, 0, ?, ?, ?, X'01000000', ?, ?)",
        )
        .bind(txid)
        .bind(height as i64)
        .bind("bb".repeat(32))
        .bind(txid)
        .bind(MerklePath::from_coinbase_txid(txid, height).to_binary())
        .bind(when)
        .bind(when)
        .execute(storage.writer())
        .await
        .unwrap();
    }
    sqlx::query(
        "INSERT INTO outputs (user_id, transaction_id, basket_id, vout, satoshis, locking_script, txid, type, spendable, change, provided_by, purpose, output_description, created_at, updated_at) \
         VALUES (?, ?, ?, 0, ?, ?, ?, 'P2PKH', 1, 1, 'storage', 'change', 'c', ?, ?)",
    )
    .bind(user)
    .bind(tx_row)
    .bind(basket)
    .bind(satoshis)
    .bind(hex::decode(LOCK).unwrap())
    .bind(txid)
    .bind(when)
    .bind(when)
    .execute(storage.writer())
    .await
    .unwrap()
    .last_insert_rowid()
}

/// The header service's view: the tip at `tip`, the block at `height`
/// carrying `txid` as its root.
fn headers_with(txid: &str, height: u32, tip: u32) -> MockWalletServices {
    let services = MockWalletServices::builder().build();
    services.set_header_for_height(header(height, txid, 0));
    services.set_tip_header(header_with_hash(tip, &"0c".repeat(32), 0));
    if tip == height {
        services.set_tip_header(header_with_hash(tip, txid, 0));
    }
    services
}

fn opts(spend_depth: u32, own_change: bool) -> GateOptions {
    GateOptions {
        spend_depth,
        own_change,
        allow_pending: AllowPending::Off,
        tracker: crate::tracker_host::TickOptions::default(),
    }
}

fn reason_of(a: &Assessment, txid: &str) -> Option<String> {
    a.held
        .iter()
        .find(|c| c.outpoint.starts_with(txid))
        .map(|c| c.reason.clone())
}

/// R3 (b) at depth 1: a coin whose block is the tip is offered; a coin
/// with no block (a payment received, no proof) is not.
#[tokio::test]
async fn at_depth_one_the_tip_is_offered_and_no_block_is_not() {
    let (storage, user, basket) = storage_for(&("02".to_string() + &"ab".repeat(32))).await;
    let tip = "a1".repeat(32);
    let none = "a2".repeat(32);
    let at_tip = coin(&storage, (user, basket), &tip, false, Some(900), 5_000, 30).await;
    coin(&storage, (user, basket), &none, false, None, 5_000, 4).await;
    let services = headers_with(&tip, 900, 900);

    let a = assess(&storage, &services, &opts(1, true), None)
        .await
        .unwrap();
    println!("depth 1: offered {:?}, held {:?}", a.offered, a.held);
    assert_eq!(a.offered, vec![(at_tip, 5_000)]);
    assert_eq!(
        reason_of(&a, &none).as_deref(),
        Some("depth 1: 0 blocks; received, no proof; offered under allowPending: received")
    );
    let held = &a.held[0];
    assert_eq!(
        (held.word.as_str(), held.origin),
        ("unknown", Origin::Received)
    );
    assert!((3..=5).contains(&held.age_minutes), "{}", held.age_minutes);

    // At depth 2 the tip's coin waits one block, and is named so.
    let a = assess(&storage, &services, &opts(2, true), None)
        .await
        .unwrap();
    assert!(a.offered.is_empty());
    assert_eq!(reason_of(&a, &tip).as_deref(), Some("depth 2: 1 block"));
    // A depth of 0 reads as 1 (the floor).
    let a = assess(&storage, &services, &opts(0, true), None)
        .await
        .unwrap();
    assert_eq!(a.offered, vec![(at_tip, 5_000)]);
}

/// R1 (b): our own change whose creator is pending is offered under
/// `SPEND_OWN_CHANGE=1` and held under `0`.
#[tokio::test]
async fn own_change_is_offered_under_the_rule_and_held_without_it() {
    let (storage, user, basket) = storage_for(&("02".to_string() + &"ab".repeat(32))).await;
    let made = "b1".repeat(32);
    let id = coin(&storage, (user, basket), &made, true, None, 7_000, 2).await;
    let services = headers_with(&"0c".repeat(32), 900, 900);

    let on = assess(&storage, &services, &opts(1, true), None)
        .await
        .unwrap();
    assert_eq!(on.offered, vec![(id, 7_000)]);
    assert!(on.held.is_empty());

    let off = assess(&storage, &services, &opts(1, false), None)
        .await
        .unwrap();
    assert!(off.offered.is_empty());
    assert_eq!(
        reason_of(&off, &made).as_deref(),
        Some("depth 1: 0 blocks; own change, SPEND_OWN_CHANGE=0")
    );
    assert_eq!(off.held[0].origin, Origin::Made);
}

/// R1 (c) refused: a received coin with no proof is held under both
/// host settings; the reason names the door's word that would offer it.
#[tokio::test]
async fn a_received_pending_coin_is_held_under_both_settings() {
    let (storage, user, basket) = storage_for(&("02".to_string() + &"ab".repeat(32))).await;
    let received = "c1".repeat(32);
    coin(&storage, (user, basket), &received, false, None, 9_000, 1).await;
    let services = headers_with(&"0c".repeat(32), 900, 900);
    for own_change in [true, false] {
        let a = assess(&storage, &services, &opts(1, own_change), None)
            .await
            .unwrap();
        assert!(a.offered.is_empty(), "SPEND_OWN_CHANGE={own_change}");
        assert_eq!(
            reason_of(&a, &received).as_deref(),
            Some("depth 1: 0 blocks; received, no proof; offered under allowPending: received")
        );
    }
}

fn pay(satoshis: u64) -> CreateActionArgs {
    CreateActionArgs {
        description: "pay someone".to_string(),
        input_beef: None,
        inputs: None,
        outputs: Some(vec![CreateActionOutput {
            locking_script: hex::decode(LOCK).unwrap(),
            satoshis,
            output_description: "payment".to_string(),
            basket: None,
            custom_instructions: None,
            tags: None,
        }]),
        lock_time: None,
        version: None,
        labels: None,
        options: Some(CreateActionOptions {
            sign_and_process: Some(true),
            accept_delayed_broadcast: Some(true),
            ..Default::default()
        }),
    }
}

/// The refusal through the toolbox's own `createAction`: three coins, each
/// held for its own reason, the toolbox finds no funds, and the refusal
/// lists all three with word, origin, age and reason. The holds are
/// restored after the call. `acceptDelayedBroadcast` is on: the toolbox
/// does not read `spendable` there, and the gate still holds.
#[tokio::test]
async fn the_refusal_lists_every_coin_held_back_with_its_reason() {
    let root = PrivateKey::random();
    let (storage, user, basket) = storage_for(&root.public_key().to_hex()).await;
    let shallow = "d1".repeat(32);
    let received = "d2".repeat(32);
    let made = "d3".repeat(32);
    coin(
        &storage,
        (user, basket),
        &shallow,
        true,
        Some(900),
        6_000,
        60,
    )
    .await;
    coin(&storage, (user, basket), &received, false, None, 6_000, 4).await;
    coin(&storage, (user, basket), &made, true, None, 6_000, 2).await;
    let services = headers_with(&shallow, 900, 900);
    let wallet = Wallet::new(Some(root), storage, services).await.unwrap();

    let Err(err) = create_action(&wallet, pay(1_000), "gate.local", &opts(2, false)).await else {
        panic!("a coin was offered");
    };
    println!("{err}");
    let GateError::Refused(refusal) = err else {
        panic!("not a refusal: {err}");
    };
    assert_eq!(refusal.offered_satoshis, 0);
    let mut reasons: Vec<_> = refusal
        .held
        .iter()
        .map(|c| (c.outpoint[..4].to_string(), c.reason.clone(), c.origin))
        .collect();
    reasons.sort();
    assert_eq!(
        reasons,
        vec![
            (
                "d1d1".to_string(),
                "depth 2: 1 block".to_string(),
                Origin::Made
            ),
            (
                "d2d2".to_string(),
                "depth 2: 0 blocks; received, no proof; offered under allowPending: received"
                    .to_string(),
                Origin::Received
            ),
            (
                "d3d3".to_string(),
                "depth 2: 0 blocks; own change, SPEND_OWN_CHANGE=0".to_string(),
                Origin::Made
            ),
        ]
    );
    let text = refusal.to_string();
    assert!(text.starts_with("Insufficient funds"), "{text}");
    assert!(text.contains("received, 4 min old"), "{text}");
    let change: Vec<i64> = sqlx::query_scalar("SELECT change FROM outputs ORDER BY output_id")
        .fetch_all(wallet.storage().pool())
        .await
        .unwrap();
    assert_eq!(change, vec![1, 1, 1], "every hold restored");
    let spent: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM outputs WHERE spent_by IS NOT NULL")
        .fetch_one(wallet.storage().pool())
        .await
        .unwrap();
    assert_eq!(spent, 0, "nothing allocated");
}

/// A coin a live action of ours consumed is listed `spent by <txid>
/// (unproven)`: an input spent for good, never a hold (#53).
#[tokio::test]
async fn a_consumed_coin_is_listed_as_spent_by_its_spender() {
    let (storage, user, basket) = storage_for(&("02".to_string() + &"ab".repeat(32))).await;
    let parent = "e1".repeat(32);
    let spender = "e2".repeat(32);
    let id = coin(&storage, (user, basket), &parent, true, Some(900), 5_000, 9).await;
    let holder_row = coin(&storage, (user, basket), &spender, true, None, 1, 1).await;
    let spender_tx: i64 =
        sqlx::query_scalar("SELECT transaction_id FROM outputs WHERE output_id = ?")
            .bind(holder_row)
            .fetch_one(storage.pool())
            .await
            .unwrap();
    sqlx::query("UPDATE outputs SET spent_by = ?, spendable = 0 WHERE output_id = ?")
        .bind(spender_tx)
        .bind(id)
        .execute(storage.writer())
        .await
        .unwrap();
    let held = locked(storage.pool()).await.unwrap();
    assert_eq!(held.len(), 1);
    assert_eq!(held[0].reason, format!("spent by {spender} (unproven)"));
    assert_eq!(held[0].word, "locked");
}

/// The holds a 0.15 process left (`change = 0`, listed in
/// `spend_gate_holds`) are change again after the one heal at start, and the
/// list is empty; a coin not listed is untouched, and a store with no table
/// is left as it is.
#[tokio::test]
async fn the_heal_at_start_restores_the_holds_a_015_process_left() {
    let (storage, user, basket) = storage_for(&("02".to_string() + &"ab".repeat(32))).await;
    assert_eq!(heal_holds(&storage).await.unwrap(), 0, "no table, no heal");
    let held = coin(
        &storage,
        (user, basket),
        &"f1".repeat(32),
        true,
        None,
        5_000,
        1,
    )
    .await;
    let not_change = coin(
        &storage,
        (user, basket),
        &"f2".repeat(32),
        true,
        None,
        5_000,
        1,
    )
    .await;
    let w = storage.writer();
    // A 0.15 table, as 0.15 made it, with one hold a crashed call left.
    sqlx::query(
        "CREATE TABLE spend_gate_holds (output_id INTEGER PRIMARY KEY, holder TEXT NOT NULL, held_at INTEGER NOT NULL)",
    )
    .execute(w)
    .await
    .unwrap();
    sqlx::query("UPDATE outputs SET change = 0")
        .execute(w)
        .await
        .unwrap();
    sqlx::query("INSERT INTO spend_gate_holds VALUES (?, '4242:7', 0)")
        .bind(held)
        .execute(w)
        .await
        .unwrap();

    assert_eq!(heal_holds(&storage).await.unwrap(), 1);
    let change = |id: i64| {
        sqlx::query_scalar::<_, i64>("SELECT change FROM outputs WHERE output_id = ?")
            .bind(id)
            .fetch_one(storage.pool())
    };
    assert_eq!(change(held).await.unwrap(), 1, "the hold is healed");
    assert_eq!(
        change(not_change).await.unwrap(),
        0,
        "a coin not listed is not touched"
    );
    let left: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM spend_gate_holds")
        .fetch_one(storage.pool())
        .await
        .unwrap();
    assert_eq!(left, 0);
    assert_eq!(heal_holds(&storage).await.unwrap(), 0, "healed once");
}

/// A 0.15 hold table with one hold on `held` and every coin `change = 0`,
/// as a crashed 0.15 call left it.
async fn a_015_hold(storage: &StorageSqlx, held: i64) {
    let w = storage.writer();
    sqlx::query(
        "CREATE TABLE IF NOT EXISTS spend_gate_holds (output_id INTEGER PRIMARY KEY, holder TEXT NOT NULL, held_at INTEGER NOT NULL)",
    )
    .execute(w)
    .await
    .unwrap();
    sqlx::query("UPDATE outputs SET change = 0")
        .execute(w)
        .await
        .unwrap();
    sqlx::query("INSERT INTO spend_gate_holds VALUES (?, '4242:7', 0)")
        .bind(held)
        .execute(w)
        .await
        .unwrap();
}

async fn change_of(storage: &StorageSqlx, id: i64) -> i64 {
    sqlx::query_scalar("SELECT change FROM outputs WHERE output_id = ?")
        .bind(id)
        .fetch_one(storage.pool())
        .await
        .unwrap()
}

/// F6 of the #44 CLI review: the spending commands' heal reads first. With
/// no table, or an empty one, it never asks for the writer: it answers at
/// once while another process holds the file's write lock. With a hold it
/// heals, once.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_commands_heal_reads_first_and_heals_only_a_hold() {
    use sqlx::{ConnectOptions, Connection};
    let dir = tempfile::TempDir::new().unwrap();
    let path = dir.path().join("wallet.db");
    let identity = "02".to_string() + &"ab".repeat(32);
    let storage = StorageSqlx::open(path.to_str().unwrap()).await.unwrap();
    storage.migrate("heal", &identity).await.unwrap();
    storage.make_available().await.unwrap();
    let (user, _) = storage.find_or_insert_user(&identity).await.unwrap();
    let basket = storage
        .find_or_create_default_basket(user.user_id)
        .await
        .unwrap()
        .basket_id;
    let held = coin(
        &storage,
        (user.user_id, basket),
        &"f3".repeat(32),
        true,
        None,
        5_000,
        1,
    )
    .await;
    sqlx::query(
        "CREATE TABLE spend_gate_holds (output_id INTEGER PRIMARY KEY, holder TEXT NOT NULL, held_at INTEGER NOT NULL)",
    )
    .execute(storage.writer())
    .await
    .unwrap();

    let mut other = sqlx::sqlite::SqliteConnectOptions::new()
        .filename(&path)
        .connect()
        .await
        .unwrap();
    sqlx::query("BEGIN IMMEDIATE")
        .execute(&mut other)
        .await
        .unwrap();
    let started = std::time::Instant::now();
    assert_eq!(heal_if_held(&storage).await.unwrap(), 0);
    assert!(
        started.elapsed() < std::time::Duration::from_secs(2),
        "an empty table is read, the writer never asked: {:?}",
        started.elapsed()
    );
    sqlx::query("ROLLBACK").execute(&mut other).await.unwrap();
    other.close().await.unwrap();

    a_015_hold(&storage, held).await;
    heal_before_spend(&storage).await;
    assert_eq!(change_of(&storage, held).await, 1, "the hold is healed");
    assert_eq!(heal_if_held(&storage).await.unwrap(), 0, "healed once");
}

/// F6 of the #44 CLI review: a heal that failed at the served start is
/// owed, and the next served request tries it again: while it still fails
/// (a fault on the outputs table) it stays owed and the request is
/// answered; once it succeeds the coin is change again and nothing is owed.
#[tokio::test]
async fn a_served_request_retries_the_heal_a_failed_start_owed() {
    use tower::ServiceExt;
    let root = PrivateKey::random();
    let (storage, user, basket) = storage_for(&root.public_key().to_hex()).await;
    let held = coin(
        &storage,
        (user, basket),
        &"f4".repeat(32),
        true,
        None,
        5_000,
        1,
    )
    .await;
    a_015_hold(&storage, held).await;
    sqlx::query(
        "CREATE TRIGGER fault_heal BEFORE UPDATE ON outputs \
         BEGIN SELECT RAISE(ABORT, 'injected heal fault'); END",
    )
    .execute(storage.writer())
    .await
    .unwrap();
    let services = bsv_wallet_toolbox::Services::with_options(
        bsv_wallet_toolbox::Chain::Main,
        bsv_wallet_toolbox::ServicesOptions::mainnet(),
    )
    .unwrap();
    let wallet =
        crate::server::make_wallet_state(Wallet::new(Some(root), storage, services).await.unwrap());
    assert!(
        heal_if_held(wallet.storage()).await.is_err(),
        "the start's heal fails"
    );
    let owed = crate::server::HealOwed::owed();
    let app = crate::server::make_router_owing(
        wallet.clone(),
        crate::server::ServerConfig::default(),
        owed.clone(),
    );
    let request = || {
        axum::http::Request::get("/isAuthenticated")
            .body(axum::body::Body::empty())
            .unwrap()
    };

    let response = app.clone().oneshot(request()).await.unwrap();
    assert_eq!(response.status(), axum::http::StatusCode::OK);
    assert!(owed.is_owed(), "still failing: still owed");
    assert_eq!(change_of(wallet.storage(), held).await, 0);

    sqlx::query("DROP TRIGGER fault_heal")
        .execute(wallet.storage().writer())
        .await
        .unwrap();
    let response = app.clone().oneshot(request()).await.unwrap();
    assert_eq!(response.status(), axum::http::StatusCode::OK);
    assert!(!owed.is_owed(), "healed: nothing owed");
    assert_eq!(change_of(wallet.storage(), held).await, 1);
}

/// R1 (d): `allowPending: received` offers a received coin whose creator is
/// `pending`, and nothing else changes: our own change is offered either
/// way (R1 (b)), a mined coin at the tip (one block deep) is offered at a
/// spend depth of 0 and of 1 either way, and a mined coin whose block is
/// above the tip (0 blocks deep) is held either way, failing closed.
#[tokio::test]
async fn the_door_word_offers_a_received_pending_coin_and_nothing_else() {
    let (storage, user, basket) = storage_for(&("02".to_string() + &"ab".repeat(32))).await;
    let received = "a5".repeat(32);
    let made = "a6".repeat(32);
    let at_tip = "a7".repeat(32);
    let above = "a8".repeat(32);
    let r = coin(&storage, (user, basket), &received, false, None, 4_000, 3).await;
    let m = coin(&storage, (user, basket), &made, true, None, 3_000, 2).await;
    let t = coin(
        &storage,
        (user, basket),
        &at_tip,
        false,
        Some(900),
        2_000,
        9,
    )
    .await;
    coin(&storage, (user, basket), &above, false, Some(901), 1_000, 9).await;
    let services = MockWalletServices::builder().build();
    services.set_header_for_height(header(900, &at_tip, 0));
    services.set_header_for_height(header(901, &above, 0));
    services.set_tip_header(header_with_hash(900, &at_tip, 0));

    for depth in [0, 1] {
        for allow in [false, true] {
            let gate = opts(depth, true).with_allow_received(allow);
            let mut a = assess(&storage, &services, &gate, None).await.unwrap();
            a.offered.sort();
            let mut want = vec![(m, 3_000), (t, 2_000)];
            if allow {
                want.push((r, 4_000));
            }
            want.sort();
            assert_eq!(a.offered, want, "depth {depth}, allowPending {allow}");
            // The snapshot reads no header above its tip: the guard fails
            // closed on it.
            assert_eq!(
                reason_of(&a, &above).as_deref(),
                Some("headers unavailable: Unavailable(901)"),
                "depth {depth}, allowPending {allow}"
            );
            assert_eq!(
                reason_of(&a, &received).is_some(),
                !allow,
                "depth {depth}, allowPending {allow}"
            );
        }
    }
}

/// The door's word: only `"received"` turns it on.
#[test]
fn only_the_word_received_opens_the_door() {
    assert!(allow_pending(Some("received")));
    for off in [None, Some(""), Some("Received"), Some("all"), Some("true")] {
        assert!(!allow_pending(off), "{off:?}");
    }
    assert_eq!(
        GateOptions::from_config(&crate::config::Config::default()).allow_pending,
        AllowPending::Off
    );
    assert_eq!(GateOptions::default().allow_pending, AllowPending::Off);
}

/// A 0.15 store's `spend_gate_actions`, as 0.15 made it: from 0.16 the
/// table is read, never created or written.
async fn a_015_rule_table(storage: &StorageSqlx) {
    sqlx::query(
        "CREATE TABLE spend_gate_actions (txid TEXT PRIMARY KEY, rule TEXT NOT NULL, \
         recorded_at INTEGER NOT NULL, reference TEXT)",
    )
    .execute(storage.writer())
    .await
    .unwrap();
    sqlx::query(
        "CREATE UNIQUE INDEX spend_gate_actions_reference ON spend_gate_actions (reference)",
    )
    .execute(storage.writer())
    .await
    .unwrap();
}

/// A 0.15 record of the word keeps answering: an action recorded under
/// R1 (d) by its txid is shown as `underRule` on that action only, and the
/// read writes nothing (no table is made on a store without one).
#[tokio::test]
async fn a_015_record_of_the_word_keeps_answering() {
    let (storage, _, _) = storage_for(&("02".to_string() + &"ab".repeat(32))).await;
    assert!(action_rules(&storage).await.unwrap().by_txid.is_empty());
    assert!(
        !crate::store::has_column(storage.pool(), "spend_gate_actions", "txid")
            .await
            .unwrap(),
        "a read makes no table"
    );
    a_015_rule_table(&storage).await;
    let under = "b7".repeat(32);
    sqlx::query("INSERT INTO spend_gate_actions (txid, rule, recorded_at) VALUES (?, ?, 0)")
        .bind(&under)
        .bind(RECEIVED_PENDING)
        .execute(storage.writer())
        .await
        .unwrap();
    let rules = action_rules(&storage).await.unwrap();
    assert_eq!(
        rules.by_txid.get(&under).map(String::as_str),
        Some("received-pending")
    );
    let shown = crate::server::handlers::with_rules(
        serde_json::json!({"totalActions": 2, "actions": [
            {"txid": under, "status": "unproven"},
            {"txid": "b8".repeat(32), "status": "completed"},
        ]}),
        &rules,
    );
    assert_eq!(shown["actions"][0]["underRule"], "received-pending");
    assert!(shown["actions"][1].get("underRule").is_none());
}

/// Delivers a `frozen` chain event for `txid.0` at `cursor`.
async fn freeze(storage: &StorageSqlx, txid: &str, cursor: u64) {
    let services = MockWalletServices::builder().build();
    crate::tracker_host::deliver_chain_event(
        storage,
        &services,
        &crate::tracker_host::tests::FixedClock(1_700_000_000),
        &crate::tracker_host::tests::opts(),
        cursor,
        bsv_tracker::ChainEvent::Frozen {
            outpoint: bsv_tracker::Outpoint {
                txid: txid.to_string(),
                vout: 0,
            },
        },
    )
    .await
    .unwrap();
}

/// R6: the feed's `frozen` event is recorded, and the gate holds the coin,
/// mined or not, rule or no rule, and never clears it.
#[tokio::test]
async fn a_frozen_outpoint_is_held_under_every_rule() {
    let (storage, user, basket) = storage_for(&("02".to_string() + &"ab".repeat(32))).await;
    let mined = "c7".repeat(32);
    let received = "c8".repeat(32);
    coin(&storage, (user, basket), &mined, false, Some(900), 5_000, 9).await;
    coin(&storage, (user, basket), &received, false, None, 5_000, 9).await;
    freeze(&storage, &mined, 7).await;
    freeze(&storage, &received, 8).await;
    // Delivered again (at least once): one row each, the cursor unchanged.
    freeze(&storage, &mined, 7).await;
    assert_eq!(
        crate::tracker_host::frozen_count(&storage).await.unwrap(),
        2
    );
    let services = headers_with(&mined, 900, 900);
    let a = assess(
        &storage,
        &services,
        &opts(1, true).with_allow_received(true),
        None,
    )
    .await
    .unwrap();
    assert!(a.offered.is_empty(), "{:?}", a.offered);
    for txid in [&mined, &received] {
        assert_eq!(
            reason_of(&a, txid).as_deref(),
            Some("frozen: never offered")
        );
    }
}

/// #165's three coins, each mined at its own height under a deep tip: a lost
/// one (the store's `spendable = 0` with no spender, the relinquished
/// record), one a live action of ours consumed, and a spendable one.
struct LostCoins {
    storage: StorageSqlx,
    services: MockWalletServices,
    spender: String,
    /// (txid, output id) of the lost, the consumed and the spendable coin.
    lost: (String, i64),
    consumed: (String, i64),
    spendable: (String, i64),
}

async fn lost_coins() -> LostCoins {
    let (storage, user, basket) = storage_for(&("02".to_string() + &"ab".repeat(32))).await;
    let [lost, consumed, spendable, spender] = ["c9", "ca", "cb", "cc"].map(|b| b.repeat(32));
    let lost_id = coin(&storage, (user, basket), &lost, true, Some(900), 5_000, 9).await;
    let consumed_id = coin(
        &storage,
        (user, basket),
        &consumed,
        true,
        Some(901),
        5_000,
        9,
    )
    .await;
    let spendable_id = coin(
        &storage,
        (user, basket),
        &spendable,
        true,
        Some(902),
        5_000,
        9,
    )
    .await;
    let holder_row = coin(&storage, (user, basket), &spender, true, None, 1, 1).await;
    sqlx::query("UPDATE outputs SET spendable = 0 WHERE output_id = ?")
        .bind(lost_id)
        .execute(storage.writer())
        .await
        .unwrap();
    sqlx::query(
        "UPDATE outputs SET spendable = 0, spent_by = \
         (SELECT transaction_id FROM outputs WHERE output_id = ?) WHERE output_id = ?",
    )
    .bind(holder_row)
    .bind(consumed_id)
    .execute(storage.writer())
    .await
    .unwrap();
    // Each creator's root carried at its height: only the coin's record can
    // hold the lost one.
    let services = headers_with(&lost, 900, 905);
    services.set_header_for_height(header(901, &consumed, 0));
    services.set_header_for_height(header(902, &spendable, 0));
    LostCoins {
        storage,
        services,
        spender,
        lost: (lost, lost_id),
        consumed: (consumed, consumed_id),
        spendable: (spendable, spendable_id),
    }
}

/// #165, the offer: the lost coin is held `lost` before its creator is
/// read, though its creator is mined at depth (its block, 900, is never
/// asked of the header service; the spendable coin's, 902, is); the
/// consumed coin is the lock's (no candidate of the offer); the spendable
/// coin is offered.
#[tokio::test]
async fn the_offer_holds_a_lost_coin_by_its_record() {
    let w = lost_coins().await;
    let a = assess(&w.storage, &w.services, &opts(1, false), None)
        .await
        .unwrap();
    let offered = |id: i64| a.offered.iter().any(|(o, _)| *o == id);
    let held = a.held.iter().find(|c| c.output_id == w.lost.1).unwrap();
    assert_eq!(
        (held.word.as_str(), held.reason.as_str()),
        ("lost", "lost: the store marks it not spendable")
    );
    assert!(held.headers_fault.is_none());
    assert!(!offered(w.lost.1), "{a:?}");
    assert!(
        a.held.iter().all(|c| c.output_id != w.consumed.1) && !offered(w.consumed.1),
        "a consumed coin is no candidate: {a:?}"
    );
    assert!(offered(w.spendable.1), "{a:?}");
    let asked: Vec<String> = w
        .services
        .call_history()
        .into_iter()
        .filter(|c| c.method == "get_header_for_height")
        .flat_map(|c| c.args)
        .collect();
    assert_eq!(
        asked,
        vec!["902".to_string()],
        "only the spendable coin's block"
    );
}

/// #165, a named input: the lost coin is held `lost`, the consumed coin
/// `spent by` its spender (and passes as the respend's own, `reserved_to`
/// that spender), the spendable coin passes.
#[tokio::test]
async fn a_named_lost_coin_is_held_by_its_record_a_consumed_one_by_its_lock() {
    let w = lost_coins().await;
    let named = |coin: &(String, i64), reserved_to: Option<i64>| {
        let coins = [(coin.0.clone(), 0u32)];
        let (storage, services) = (&w.storage, &w.services);
        async move {
            assess_named(
                storage,
                services,
                &opts(1, false),
                &coins,
                &[],
                reserved_to,
                false,
            )
            .await
            .unwrap()
            .into_iter()
            .map(|c| (c.word, c.reason))
            .collect::<Vec<_>>()
        }
    };
    assert_eq!(
        named(&w.lost, None).await,
        vec![(
            "lost".to_string(),
            "lost: the store marks it not spendable".to_string()
        )]
    );
    assert_eq!(
        named(&w.consumed, None).await,
        vec![(
            "locked".to_string(),
            format!("spent by {} (unproven)", w.spender)
        )]
    );
    let reserved: i64 = sqlx::query_scalar("SELECT spent_by FROM outputs WHERE output_id = ?")
        .bind(w.consumed.1)
        .fetch_one(w.storage.pool())
        .await
        .unwrap();
    assert!(named(&w.consumed, Some(reserved)).await.is_empty());
    assert!(named(&w.spendable, None).await.is_empty());
}

/// Marks a coin lost: the store's `spendable = 0`, no spender.
async fn lose(storage: &StorageSqlx, id: i64) {
    sqlx::query("UPDATE outputs SET spendable = 0 WHERE output_id = ?")
        .bind(id)
        .execute(storage.writer())
        .await
        .unwrap();
}

/// A received coin whose transaction the toolbox marked failed (0.13.0
/// `mark_internalized_tx_failed`): the transaction `failed`, the coin
/// `spendable = 0`, no spender.
async fn fail(storage: &StorageSqlx, txid: &str, id: i64) {
    set_status(storage, txid, "failed").await;
    lose(storage, id).await;
}

/// A transaction's stored status.
async fn set_status(storage: &StorageSqlx, txid: &str, status: &str) {
    sqlx::query("UPDATE transactions SET status = ? WHERE txid = ?")
        .bind(status)
        .bind(txid)
        .execute(storage.writer())
        .await
        .unwrap();
}

/// The coin's `spent_by`.
async fn spent_by(storage: &StorageSqlx, id: i64) -> Option<i64> {
    sqlx::query_scalar("SELECT spent_by FROM outputs WHERE output_id = ?")
        .bind(id)
        .fetch_one(storage.pool())
        .await
        .unwrap()
}

/// A template under `acceptDelayedBroadcast`, where the toolbox's
/// allocation does not read `spendable`: the gate's hold is the only guard
/// against a lost coin (#165).
fn delayed() -> CreateActionArgs {
    let mut args = pay(1_000);
    args.options = Some(CreateActionOptions {
        sign_and_process: Some(false),
        accept_delayed_broadcast: Some(true),
        ..Default::default()
    });
    args
}

/// A wallet whose mined coin of 50,000 sats (block 800) the store marks
/// lost; `beside`, a spendable mined coin of that many sats (block 801).
/// (wallet, the lost coin's (txid, id), the spendable coin's id.)
async fn a_lost_coin_beside(
    beside: Option<u64>,
) -> (
    Wallet<StorageSqlx, MockWalletServices>,
    (String, i64),
    Option<i64>,
) {
    let root = PrivateKey::random();
    let (storage, user, basket) = storage_for(&root.public_key().to_hex()).await;
    let (lost, lost_id) = mined_coin(&storage, (user, basket), 0xd5, 50_000, 800).await;
    lose(&storage, lost_id).await;
    let services = headers_with(&lost, 800, 900);
    let mut kept = None;
    if let Some(satoshis) = beside {
        let (txid, id) = mined_coin(&storage, (user, basket), 0xd7, satoshis, 801).await;
        services.set_header_for_height(header(801, &txid, 0));
        kept = Some(id);
    }
    let wallet = Wallet::new(Some(root), storage, services).await.unwrap();
    (wallet, (lost, lost_id), kept)
}

/// #165 under `acceptDelayedBroadcast`, only a lost coin could cover the
/// payment: the refusal holds it `lost` and nothing is allocated.
#[tokio::test]
async fn under_delayed_broadcast_a_lost_coin_alone_is_refused_lost() {
    let (wallet, (lost, lost_id), _) = a_lost_coin_beside(None).await;
    let Err(GateError::Refused(refusal)) =
        create_action(&wallet, delayed(), "gate.local", &opts(1, true)).await
    else {
        panic!("the lost coin was offered");
    };
    let held: Vec<_> = refusal
        .held
        .iter()
        .map(|c| (c.outpoint.clone(), c.word.as_str(), c.reason.as_str()))
        .collect();
    assert_eq!(
        held,
        vec![(
            format!("{lost}.0"),
            "lost",
            "lost: the store marks it not spendable"
        )]
    );
    assert_eq!(
        spent_by(wallet.storage(), lost_id).await,
        None,
        "nothing allocated"
    );
}

/// #165 under `acceptDelayedBroadcast`, a lost coin beside a spendable one,
/// the lost one the larger (read first, covering alone): the spendable coin
/// funds the action, the lost one is untouched.
#[tokio::test]
async fn under_delayed_broadcast_the_spendable_coin_funds_beside_a_lost_one() {
    let (wallet, (_, lost_id), kept_id) = a_lost_coin_beside(Some(40_000)).await;
    let result = create_action(&wallet, delayed(), "gate.local", &opts(1, true))
        .await
        .unwrap();
    assert!(result.signable_transaction.is_some());
    assert_eq!(
        spent_by(wallet.storage(), lost_id).await,
        None,
        "the lost coin untouched"
    );
    assert!(
        spent_by(wallet.storage(), kept_id.unwrap()).await.is_some(),
        "the spendable coin funds it"
    );
}

/// The scoped word naming a lost received coin (#165, #63): `assess` holds
/// it `lost`, never offered under the word that would reach it.
#[tokio::test]
async fn the_scoped_word_does_not_offer_a_named_lost_coin() {
    let (storage, user, basket) = storage_for(&("02".to_string() + &"cd".repeat(32))).await;
    let lost = "b5".repeat(32);
    let id = coin(&storage, (user, basket), &lost, false, None, 4_000, 3).await;
    lose(&storage, id).await;
    let services = headers_with(&"0c".repeat(32), 900, 900);
    let gate =
        opts(1, true).with_allow_pending(AllowPending::ReceivedOnly(vec![(lost.clone(), 0)]));
    let a = assess(&storage, &services, &gate, None).await.unwrap();
    assert!(a.offered.is_empty(), "{a:?}");
    assert_eq!(
        reason_of(&a, &lost).as_deref(),
        Some("lost: the store marks it not spendable")
    );
}

/// A named input over `txid`'s output 0, as a caller names one.
fn named_input(txid: &str) -> bsv_sdk::wallet::CreateActionInput {
    bsv_sdk::wallet::CreateActionInput {
        outpoint: bsv_sdk::wallet::Outpoint::from_string(&format!("{txid}.0")).unwrap(),
        input_description: "named".to_string(),
        unlocking_script: None,
        unlocking_script_length: Some(107),
        sequence_number: None,
    }
}

/// Explicit inputs under the guard: a coin the caller names that the gate
/// would hold refuses the action before the toolbox runs, with the same
/// `held[]` listing; under the door's word the same received coin passes
/// the gate.
#[tokio::test]
async fn a_named_input_the_gate_would_hold_is_refused() {
    let root = PrivateKey::random();
    let (storage, user, basket) = storage_for(&root.public_key().to_hex()).await;
    let received = "d7".repeat(32);
    coin(&storage, (user, basket), &received, false, None, 8_000, 5).await;
    let services = headers_with(&"0c".repeat(32), 900, 900);
    let wallet = Wallet::new(Some(root), storage, services).await.unwrap();
    let mut args = pay(1_000);
    args.inputs = Some(vec![named_input(&received)]);

    let Err(GateError::Refused(refusal)) =
        create_action(&wallet, args.clone(), "gate.local", &opts(1, true)).await
    else {
        panic!("the named coin passed the gate");
    };
    assert!(refusal.is_named_input());
    assert_eq!(refusal.held.len(), 1);
    assert_eq!(refusal.held[0].outpoint, format!("{received}.0"));
    assert_eq!(
        refusal.held[0].reason,
        "depth 1: 0 blocks; received, no proof; offered under allowPending: received"
    );
    assert!(
        refusal.to_string().starts_with("Access denied: "),
        "{refusal}"
    );

    let held = assess_named(
        wallet.storage(),
        wallet.services(),
        &opts(1, true).with_allow_received(true),
        &[(received.clone(), 0)],
        &[],
        None,
        false,
    )
    .await
    .unwrap();
    assert!(held.is_empty(), "{held:?}");
}

/// #166, createAction's inputs: an outpoint that is not a coin of this
/// wallet is refused at the gate, the caller's 400 (`Scope`, the word the
/// scoped `allowPending` uses), before a creator is read, the toolbox asked
/// or anything stored; beside a coin the gate would offer too. The caller's
/// own input, its transaction carried whole in `inputBEEF`, is not a coin of
/// ours and is left to the toolbox.
#[tokio::test]
async fn a_named_input_the_wallet_does_not_hold_is_refused_at_the_gate() {
    let root = PrivateKey::random();
    let (storage, user, basket) = storage_for(&root.public_key().to_hex()).await;
    let (deep, _) = mined_coin(&storage, (user, basket), 0xd1, 50_000, 800).await;
    let services = MockWalletServices::builder().build();
    services.set_header_for_height(header(800, &deep, 0));
    services.set_tip_header(header_with_hash(901, &"0d".repeat(32), 0));
    let wallet = Wallet::new(Some(root), storage, services).await.unwrap();
    let absent = "d9".repeat(32);
    let before = stored_rows(wallet.storage()).await;
    for named in [
        vec![named_input(&absent)],
        vec![named_input(&deep), named_input(&absent)],
    ] {
        let mut args = pay(1_000);
        args.inputs = Some(named);
        assert_eq!(
            refused_before_the_door(&wallet, args, &opts(1, true)).await,
            format!(
                "Invalid argument: inputs names {absent}.0, which is not a coin this wallet could spend: {NOT_HELD}"
            )
        );
    }
    assert_eq!(
        stored_rows(wallet.storage()).await,
        before,
        "nothing stored"
    );
}

/// #166: the caller's own input, its transaction carried whole in its
/// `inputBEEF`, is not a coin of ours and is left to the toolbox; a txid-only
/// entry, or a BEEF that does not parse, carries nothing, and the same
/// outpoint is refused.
#[tokio::test]
async fn a_named_input_the_callers_beef_carries_is_left_to_the_toolbox() {
    let root = PrivateKey::random();
    let (storage, _, _) = storage_for(&root.public_key().to_hex()).await;
    let wallet = Wallet::new(Some(root), storage, MockWalletServices::builder().build())
        .await
        .unwrap();
    let mut puzzle = bsv_sdk::transaction::Transaction::new();
    puzzle
        .add_input(bsv_sdk::transaction::TransactionInput::new(
            "e4".repeat(32),
            0,
        ))
        .unwrap();
    puzzle
        .add_output(bsv_sdk::transaction::TransactionOutput::new(
            5_000,
            bsv_sdk::script::LockingScript::from_hex("5187").unwrap(),
        ))
        .unwrap();
    let mut beef = bsv_sdk::transaction::Beef::new();
    let carried_txid = beef.merge_transaction(puzzle).txid();
    let carried = carried_txids(Some(&beef.to_binary()));
    assert_eq!(carried, vec![carried_txid.clone()]);
    let mut only_txid = bsv_sdk::transaction::Beef::new();
    only_txid.merge_txid_only(carried_txid.clone());
    assert!(carried_txids(Some(&only_txid.to_binary())).is_empty());
    assert!(carried_txids(Some(&[0, 1, 2])).is_empty());
    let held = assess_named(
        wallet.storage(),
        wallet.services(),
        &opts(1, true),
        &[(carried_txid.clone(), 0)],
        &carried,
        None,
        false,
    )
    .await
    .unwrap();
    assert!(held.is_empty(), "{held:?}");
    assert!(matches!(
        assess_named(
            wallet.storage(),
            wallet.services(),
            &opts(1, true),
            &[(carried_txid, 0)],
            &[],
            None,
            false
        )
        .await,
        Err(GateError::Scope(_))
    ));
}

/// A mined coin the toolbox can carry in a BEEF: a parseable transaction
/// whose stored proof's root is its txid at `height`. (txid, output id).
async fn mined_coin(
    storage: &StorageSqlx,
    (user, basket): (i64, i64),
    seed: u8,
    satoshis: u64,
    height: u32,
) -> (String, i64) {
    let lock = bsv_sdk::script::LockingScript::from_hex(LOCK).unwrap();
    let mut parent = bsv_sdk::transaction::Transaction::new();
    parent
        .add_input(bsv_sdk::transaction::TransactionInput::new(
            hex::encode([seed; 32]),
            0,
        ))
        .unwrap();
    parent
        .add_output(bsv_sdk::transaction::TransactionOutput::new(satoshis, lock))
        .unwrap();
    let txid = parent.id();
    let id = coin(
        storage,
        (user, basket),
        &txid,
        false,
        Some(height),
        satoshis as i64,
        9,
    )
    .await;
    for table in ["transactions", "proven_txs"] {
        sqlx::query(&format!("UPDATE {table} SET raw_tx = ? WHERE txid = ?"))
            .bind(parent.to_binary())
            .bind(&txid)
            .execute(storage.writer())
            .await
            .unwrap();
    }
    (txid, id)
}

/// The offer (#44, bsv-low #828): the toolbox allocates from the coins the
/// gate offers only. Two coins it could carry: the larger, which its own
/// preference would take first, is held at depth 2 (its block is the tip);
/// the action takes the offered one, the held coin is untouched, and the
/// gate wrote no hold (no `change` flipped, no `spend_gate_holds` table).
#[tokio::test]
async fn the_offer_allocates_only_the_offered_coins_and_writes_no_hold() {
    let root = PrivateKey::random();
    let (storage, user, basket) = storage_for(&root.public_key().to_hex()).await;
    let (deep, deep_id) = mined_coin(&storage, (user, basket), 0xa1, 50_000, 800).await;
    let (tip, tip_id) = mined_coin(&storage, (user, basket), 0xa2, 90_000, 900).await;
    let services = MockWalletServices::builder().build();
    services.set_header_for_height(header(800, &deep, 0));
    services.set_header_for_height(header(900, &tip, 0));
    services.set_tip_header(header_with_hash(900, &tip, 0));
    let wallet = Wallet::new(Some(root), storage, services).await.unwrap();
    let gate = opts(2, true);
    let a = assess(wallet.storage(), wallet.services(), &gate, None)
        .await
        .unwrap();
    assert_eq!(a.offer(), vec![deep_id]);
    assert_eq!(reason_of(&a, &tip).as_deref(), Some("depth 2: 1 block"));

    let mut args = pay(1_000);
    args.options = Some(CreateActionOptions {
        sign_and_process: Some(false),
        ..Default::default()
    });
    let result = create_action(&wallet, args, "gate.local", &gate)
        .await
        .unwrap();
    assert!(result.signable_transaction.is_some());
    let spent: Vec<(i64, Option<i64>, i64)> =
        sqlx::query_as("SELECT output_id, spent_by, change FROM outputs WHERE output_id IN (?, ?) ORDER BY output_id")
            .bind(deep_id)
            .bind(tip_id)
            .fetch_all(wallet.storage().pool())
            .await
            .unwrap();
    assert!(spent[0].1.is_some(), "the offered coin funds it: {spent:?}");
    assert_eq!(
        (spent[1].1, spent[1].2),
        (None, 1),
        "the held coin untouched"
    );
    assert!(
        !crate::store::has_column(wallet.storage().pool(), "spend_gate_holds", "output_id")
            .await
            .unwrap(),
        "no hold table: the gate wrote no hold"
    );
}

/// F7 of the #44 CLI review: the lazy read offers the larger coin alone
/// (it covers the action), and the toolbox cannot carry it (its parent is
/// not stored): `Unspendable coin:`. The gate reads every coin and runs the
/// call once more, and the smaller coin, unread the first time, funds it.
/// With no other coin the answer stays the toolbox's 409.
#[tokio::test]
async fn an_unspendable_coin_in_the_lazy_offer_falls_through_to_every_coin() {
    let root = PrivateKey::random();
    let (storage, user, basket) = storage_for(&root.public_key().to_hex()).await;
    let lock = || bsv_sdk::script::LockingScript::from_hex(LOCK).unwrap();
    // Two coins of our own change, both unproven: the big one's parent is
    // not stored, the small one's parent is mined and stored.
    let (parent, parent_id) = mined_coin(&storage, (user, basket), 0xe0, 40_000, 800).await;
    sqlx::query("UPDATE outputs SET spendable = 0, change = 0 WHERE output_id = ?")
        .bind(parent_id)
        .execute(storage.writer())
        .await
        .unwrap();
    let mut ids = Vec::new();
    for (spends, satoshis) in [("e1".repeat(32), 90_000u64), (parent.clone(), 30_000)] {
        let mut creator = bsv_sdk::transaction::Transaction::new();
        creator
            .add_input(bsv_sdk::transaction::TransactionInput::new(spends, 0))
            .unwrap();
        creator
            .add_output(bsv_sdk::transaction::TransactionOutput::new(
                satoshis,
                lock(),
            ))
            .unwrap();
        let id = coin(
            &storage,
            (user, basket),
            &creator.id(),
            true,
            None,
            satoshis as i64,
            1,
        )
        .await;
        sqlx::query("UPDATE transactions SET raw_tx = ? WHERE txid = ?")
            .bind(creator.to_binary())
            .bind(creator.id())
            .execute(storage.writer())
            .await
            .unwrap();
        ids.push(id);
    }
    let (big_id, small_id) = (ids[0], ids[1]);
    let services = headers_with(&parent, 800, 900);
    let wallet = Wallet::new(Some(root), storage, services).await.unwrap();
    let gate = opts(1, true);
    let lazy = assess(wallet.storage(), wallet.services(), &gate, Some(11_000))
        .await
        .unwrap();
    assert_eq!(
        lazy.offer(),
        vec![big_id],
        "the lazy read offers the big coin"
    );
    assert_eq!(lazy.unread, vec![small_id]);
    let template = || {
        let mut args = pay(1_000);
        args.options = Some(CreateActionOptions {
            sign_and_process: Some(false),
            ..Default::default()
        });
        args
    };
    let direct = wallet
        .create_action_offering(template(), "gate.local", &lazy.offer())
        .await
        .expect_err("the big coin alone cannot be carried");
    assert!(crate::toolbox_words::is_unspendable(&direct), "{direct}");

    let result = create_action(&wallet, template(), "gate.local", &gate)
        .await
        .unwrap();
    assert!(result.signable_transaction.is_some());
    let spent: Option<i64> = sqlx::query_scalar("SELECT spent_by FROM outputs WHERE output_id = ?")
        .bind(small_id)
        .fetch_one(wallet.storage().pool())
        .await
        .unwrap();
    assert!(spent.is_some(), "the coin read the second time funds it");

    // No other coin: the toolbox's own word, 409.
    let Err(GateError::Wallet(e)) = create_action(&wallet, template(), "gate.local", &gate).await
    else {
        panic!("the big coin alone is unspendable");
    };
    assert!(crate::toolbox_words::is_unspendable(&e), "{e}");
}

/// F9 (c) of the #44 CLI review: one offer names at most [`OFFER_CAP`]
/// coins, the largest by value, in the order read; the rest are unread. At
/// the cap nothing moves.
#[test]
fn the_offer_is_capped_at_the_largest_coins() {
    let mut at = Assessment {
        offered: (0..OFFER_CAP as i64)
            .map(|i| (i, 1_000 + i as u64))
            .collect(),
        ..Default::default()
    };
    cap_offer(&mut at, OFFER_CAP);
    assert_eq!(at.offered.len(), OFFER_CAP);
    assert!(at.unread.is_empty());

    // One past the cap: the smallest coin (id 7, 1 sat) goes unread.
    let mut over = Assessment {
        offered: (0..=OFFER_CAP as i64)
            .map(|i| (i, if i == 7 { 1 } else { 1_000 + i as u64 }))
            .collect(),
        ..Default::default()
    };
    cap_offer(&mut over, OFFER_CAP);
    assert_eq!(over.offered.len(), OFFER_CAP);
    assert_eq!(over.unread, vec![7]);
    assert_eq!(over.offered[6].0, 6);
    assert_eq!(over.offered[7].0, 8, "the order read is kept");
}

/// D3 of the #44 delta review: the coins past the offer's cap are neither
/// offered nor held back, so the refusal names them, their count and their
/// satoshis. The cap is lowered to 2 here; the production cap is
/// [`OFFER_CAP`].
#[tokio::test]
async fn a_refusal_names_the_coins_past_the_offers_cap() {
    let mut assessment = Assessment {
        offered: vec![(1, 500), (2, 40), (3, 900), (4, 60), (5, 700)],
        ..Default::default()
    };
    cap_offer(&mut assessment, 2);
    assert_eq!(assessment.offer(), vec![3, 5], "the largest two, in order");
    assert_eq!(assessment.unread, vec![1, 2, 4]);
    assert_eq!(
        assessment.capped,
        Some(Capped {
            coins: 3,
            satoshis: 600,
            cap: 2
        })
    );

    let (storage, _, _) = storage_for(&PrivateKey::random().public_key().to_hex()).await;
    let toolbox =
        bsv_sdk::Error::WalletError("Insufficient funds: need 5000, have 1600".to_string());
    let GateError::Refused(refused) = refusal(&storage, &assessment, &toolbox, 5_000).await else {
        panic!("a shortfall is the refusal");
    };
    assert_eq!(refused.capped, assessment.capped);
    assert_eq!(
        refused.to_string(),
        "Insufficient funds: no offerable coin covers the action \
         (Insufficient funds: need 5000, have 1600; 1600 sats offered, 0 coin(s) held back; \
         3 coins (600 sats) beyond the offer's cap of 2, the largest offered)"
    );
    assert_eq!(refused.to_json()["message"], refused.to_string());

    // Under the cap nothing is capped and the refusal's words are as before.
    let mut under = Assessment {
        offered: vec![(1, 500), (2, 40)],
        ..Default::default()
    };
    cap_offer(&mut under, 2);
    assert_eq!(under.capped, None);
    let GateError::Refused(plain) = refusal(&storage, &under, &toolbox, 5_000).await else {
        panic!("a shortfall is the refusal");
    };
    assert!(plain
        .to_string()
        .ends_with("540 sats offered, 0 coin(s) held back)"));
}

/// F9 (c): the cap's reason, on the toolbox's own statement. An offer of
/// [`OFFER_CAP`] ids (one real coin among them), with as many again bound
/// beside it, still runs; an offer past SQLite's bound-variable limit
/// (32766) does not.
#[tokio::test]
async fn an_offer_at_the_cap_runs_and_one_past_sqlites_limit_does_not() {
    let root = PrivateKey::random();
    let (storage, user, basket) = storage_for(&root.public_key().to_hex()).await;
    let (deep, deep_id) = mined_coin(&storage, (user, basket), 0xf1, 50_000, 800).await;
    let services = MockWalletServices::builder().build();
    services.set_header_for_height(header(800, &deep, 0));
    services.set_tip_header(header_with_hash(900, &"0f".repeat(32), 0));
    let wallet = Wallet::new(Some(root), storage, services).await.unwrap();
    let template = || {
        let mut args = pay(1_000);
        args.options = Some(CreateActionOptions {
            sign_and_process: Some(false),
            ..Default::default()
        });
        args
    };
    let ids = |n: usize| -> Vec<i64> {
        std::iter::once(deep_id)
            .chain((1..n as i64).map(|i| 1_000_000 + i))
            .collect()
    };
    let result = wallet
        .create_action_offering(template(), "gate.local", &ids(2 * OFFER_CAP - 64))
        .await
        .expect("twice the cap, less a margin, is under the limit");
    let reference = reference_string(&result.signable_transaction.unwrap().reference);
    wallet
        .abort_action(bsv_sdk::wallet::AbortActionArgs { reference }, "gate.local")
        .await
        .unwrap();
    let err = wallet
        .create_action_offering(template(), "gate.local", &ids(32_767))
        .await
        .expect_err("past SQLite's bound-variable limit");
    println!("{err}");
    assert!(err.to_string().contains("too many SQL variables"), "{err}");
}

fn listed(wallet_list: serde_json::Value, rules: &ActionRules) -> Vec<(String, Option<String>)> {
    let shown = crate::server::handlers::with_rules(wallet_list, rules);
    shown["actions"]
        .as_array()
        .unwrap()
        .iter()
        .map(|a| {
            (
                a["status"].as_str().unwrap().to_string(),
                a.get("underRule")
                    .and_then(|r| r.as_str())
                    .map(String::from),
            )
        })
        .collect()
}

fn list_args() -> bsv_sdk::wallet::ListActionsArgs {
    bsv_sdk::wallet::ListActionsArgs {
        labels: vec![],
        label_query_mode: None,
        include_labels: None,
        include_inputs: None,
        include_input_source_locking_scripts: None,
        include_input_unlocking_scripts: None,
        include_output_locking_scripts: None,
        include_outputs: None,
        limit: Some(100),
        offset: None,
        seek_permission: None,
    }
}

/// A deferred-signing template under the door's word, through the
/// toolbox's own `createAction`: no txid yet, the word is the template's
/// label (written by the toolbox with it), the listing shows it as
/// `underRule` on the template, and aborting the template leaves the label
/// (the record of an act is never deleted).
#[tokio::test]
async fn a_template_under_the_word_carries_its_label_and_keeps_it_aborted() {
    let root = PrivateKey::random();
    let (storage, user, basket) = storage_for(&root.public_key().to_hex()).await;
    // A parseable creator with a mined parent (its BUMP the BEEF's root),
    // so the toolbox can carry it in the template's BEEF; the parent's coin
    // is spent, never a candidate.
    let lock = || bsv_sdk::script::LockingScript::from_hex(LOCK).unwrap();
    let mut parent = bsv_sdk::transaction::Transaction::new();
    parent
        .add_input(bsv_sdk::transaction::TransactionInput::new(
            "aa".repeat(32),
            0,
        ))
        .unwrap();
    parent
        .add_output(bsv_sdk::transaction::TransactionOutput::new(9_000, lock()))
        .unwrap();
    let parent_id = parent.id();
    coin(
        &storage,
        (user, basket),
        &parent_id,
        false,
        Some(800),
        9_000,
        9,
    )
    .await;
    for table in ["transactions", "proven_txs"] {
        sqlx::query(&format!("UPDATE {table} SET raw_tx = ? WHERE txid = ?"))
            .bind(parent.to_binary())
            .bind(&parent_id)
            .execute(storage.writer())
            .await
            .unwrap();
    }
    sqlx::query("UPDATE outputs SET spendable = 0 WHERE txid = ?")
        .bind(&parent_id)
        .execute(storage.writer())
        .await
        .unwrap();
    let mut creator = bsv_sdk::transaction::Transaction::new();
    creator
        .add_input(bsv_sdk::transaction::TransactionInput::new(
            parent_id.clone(),
            0,
        ))
        .unwrap();
    creator
        .add_output(bsv_sdk::transaction::TransactionOutput::new(8_000, lock()))
        .unwrap();
    let received = creator.id();
    coin(&storage, (user, basket), &received, false, None, 8_000, 5).await;
    sqlx::query("UPDATE transactions SET raw_tx = ? WHERE txid = ?")
        .bind(creator.to_binary())
        .bind(&received)
        .execute(storage.writer())
        .await
        .unwrap();
    let services = headers_with(&parent_id, 800, 900);
    let wallet = Wallet::new(Some(root), storage, services).await.unwrap();
    let mut args = pay(1_000);
    args.options = Some(CreateActionOptions {
        sign_and_process: Some(false),
        ..Default::default()
    });

    let result = create_action(
        &wallet,
        args,
        "gate.local",
        &opts(1, true).with_allow_received(true),
    )
    .await
    .unwrap();
    assert!(result.txid.is_none());
    let reference = reference_string(&result.signable_transaction.unwrap().reference);
    assert_eq!(
        labels_of(wallet.storage(), &reference).await,
        vec![RECEIVED_PENDING_LABEL]
    );
    assert!(rules_of(wallet.storage()).await.unwrap().is_empty());

    let list = wallet
        .list_actions(list_args(), "gate.local")
        .await
        .unwrap();
    let rules = action_rules(wallet.storage()).await.unwrap();
    let shown = listed(serde_json::to_value(&list).unwrap(), &rules);
    assert!(
        shown.contains(&("unsigned".to_string(), Some("received-pending".to_string()))),
        "{shown:?}"
    );

    wallet
        .abort_action(
            bsv_sdk::wallet::AbortActionArgs {
                reference: reference.clone(),
            },
            "gate.local",
        )
        .await
        .unwrap();
    assert_eq!(
        labels_of(wallet.storage(), &reference).await,
        vec![RECEIVED_PENDING_LABEL]
    );
}

/// F4 of the #44 CLI review: a caller's own copy of the rule's label, in
/// any case or padding, without `allowPending`, is stripped before the
/// door: the action is stored without it, the listing shows no
/// `underRule`, and no rule is recorded, so nothing (a respend's
/// `allow_received` among them) reads it as granted. Under the door's word
/// the label is written once.
#[tokio::test]
async fn a_forged_rule_label_is_stripped_and_grants_nothing() {
    let root = PrivateKey::random();
    let (storage, user, basket) = storage_for(&root.public_key().to_hex()).await;
    let (deep, _) = mined_coin(&storage, (user, basket), 0xd1, 50_000, 800).await;
    let (other, _) = mined_coin(&storage, (user, basket), 0xd2, 50_000, 800).await;
    let services = MockWalletServices::builder().build();
    services.set_header_for_height(header(800, &deep, 0));
    services.set_tip_header(header_with_hash(900, &other, 0));
    let wallet = Wallet::new(Some(root), storage, services).await.unwrap();
    let forged = || {
        let mut args = pay(1_000);
        args.labels = Some(vec![
            RECEIVED_PENDING_LABEL.to_string(),
            " Spend-Gate:Received-Pending ".to_string(),
            "mine".to_string(),
        ]);
        args.options = Some(CreateActionOptions {
            sign_and_process: Some(false),
            ..Default::default()
        });
        args
    };

    let result = create_action(&wallet, forged(), "gate.local", &opts(1, true))
        .await
        .unwrap();
    let reference = reference_string(&result.signable_transaction.unwrap().reference);
    assert_eq!(labels_of(wallet.storage(), &reference).await, vec!["mine"]);
    let rules = action_rules(wallet.storage()).await.unwrap();
    assert!(rules.by_txid.is_empty() && rules.by_template.is_empty());
    let list = wallet
        .list_actions(list_args(), "gate.local")
        .await
        .unwrap();
    let shown = listed(serde_json::to_value(&list).unwrap(), &rules);
    assert!(
        shown.contains(&("unsigned".to_string(), None))
            && shown.iter().all(|(_, rule)| rule.is_none()),
        "{shown:?}"
    );

    // Under the door's word the label is the gate's, written once.
    let result = create_action(
        &wallet,
        forged(),
        "gate.local",
        &opts(1, true).with_allow_received(true),
    )
    .await
    .unwrap();
    let reference = reference_string(&result.signable_transaction.unwrap().reference);
    assert_eq!(
        labels_of(wallet.storage(), &reference).await,
        vec!["mine", RECEIVED_PENDING_LABEL]
    );

    let mut labels = Some(vec![
        "x".to_string(),
        "SPEND-GATE:RECEIVED-PENDING".to_string(),
    ]);
    strip_rule_label(&mut labels);
    assert_eq!(labels, Some(vec!["x".to_string()]));
    let mut none = None;
    strip_rule_label(&mut none);
    assert_eq!(none, None);
}

/// The labels the toolbox stored on the action whose reference is
/// `reference`.
async fn labels_of(storage: &StorageSqlx, reference: &str) -> Vec<String> {
    sqlx::query_scalar(
        "SELECT l.label FROM transactions t \
         JOIN tx_labels_map m ON m.transaction_id = t.transaction_id AND m.is_deleted = 0 \
         JOIN tx_labels l ON l.tx_label_id = m.tx_label_id AND l.is_deleted = 0 \
         WHERE t.reference = ? ORDER BY l.label",
    )
    .bind(reference)
    .fetch_all(storage.pool())
    .await
    .unwrap()
}

/// A 0.15 template recorded by reference keeps answering: the listing
/// shows the rule on the template before signing and on the signed action
/// after, with nothing written at the sign (the reference names its txid at
/// the read). A template of the same shape under no rule makes the shape
/// name no rule, never a wrong one; a labelled template is named by its
/// label first.
#[tokio::test]
async fn a_015_template_record_answers_before_and_after_its_signing() {
    let (storage, user, _) = storage_for(&("02".to_string() + &"ab".repeat(32))).await;
    a_015_rule_table(&storage).await;
    let pool = storage.writer();
    let template = |reference: &'static str| {
        sqlx::query(
            "INSERT INTO transactions (user_id, status, reference, is_outgoing, satoshis, version, lock_time, description, created_at, updated_at) \
             VALUES (?, 'unsigned', ?, 1, -1000, 1, 0, 'pay someone', ?, ?)",
        )
        .bind(user)
        .bind(reference)
        .bind(Utc::now())
        .bind(Utc::now())
    };
    let record = |reference: &'static str| {
        sqlx::query(
            "INSERT INTO spend_gate_actions (txid, rule, recorded_at, reference) VALUES (NULL, ?, 0, ?)",
        )
        .bind(RECEIVED_PENDING)
        .bind(reference)
    };
    template("tmpl-ref").execute(pool).await.unwrap();
    record("tmpl-ref").execute(pool).await.unwrap();
    let zero = "00".repeat(32);
    let before = serde_json::json!({"totalActions": 1, "actions": [
        {"txid": zero, "status": "unsigned", "description": "pay someone", "satoshis": -1000},
    ]});
    let rules = action_rules(&storage).await.unwrap();
    assert_eq!(
        listed(before.clone(), &rules),
        vec![("unsigned".to_string(), Some("received-pending".to_string()))]
    );

    let signed = "e2".repeat(32);
    sqlx::query(
        "UPDATE transactions SET txid = ?, status = 'unproven' WHERE reference = 'tmpl-ref'",
    )
    .bind(signed.to_ascii_uppercase())
    .execute(pool)
    .await
    .unwrap();
    let row: (Option<String>, Option<String>) =
        sqlx::query_as("SELECT txid, reference FROM spend_gate_actions")
            .fetch_one(storage.pool())
            .await
            .unwrap();
    assert_eq!(
        row,
        (None, Some("tmpl-ref".to_string())),
        "nothing written at the sign"
    );
    let rules = action_rules(&storage).await.unwrap();
    assert!(rules.by_template.is_empty());
    let after = serde_json::json!({"totalActions": 1, "actions": [
        {"txid": signed, "status": "unproven", "description": "pay someone", "satoshis": -1000},
    ]});
    assert_eq!(
        listed(after, &rules),
        vec![("unproven".to_string(), Some("received-pending".to_string()))]
    );

    // Two unsigned templates of one shape, one under the word: neither is
    // named, the listing cannot tell them apart.
    template("tmpl-a").execute(pool).await.unwrap();
    template("tmpl-b").execute(pool).await.unwrap();
    record("tmpl-a").execute(pool).await.unwrap();
    let rules = action_rules(&storage).await.unwrap();
    assert_eq!(
        listed(before.clone(), &rules),
        vec![("unsigned".to_string(), None)]
    );
    // Both labelled: the shape names the rule.
    for reference in ["tmpl-a", "tmpl-b"] {
        let id: i64 =
            sqlx::query_scalar("SELECT transaction_id FROM transactions WHERE reference = ?")
                .bind(reference)
                .fetch_one(storage.pool())
                .await
                .unwrap();
        let mut conn = storage.writer().acquire().await.unwrap();
        label_in(&mut conn, user, id, RECEIVED_PENDING)
            .await
            .unwrap();
    }
    let rules = action_rules(&storage).await.unwrap();
    assert_eq!(
        listed(before, &rules),
        vec![("unsigned".to_string(), Some("received-pending".to_string()))]
    );
}

/// The writer connection's changes and commits, counted by SQLite's own
/// hooks: (rows changed, transactions committed).
async fn count_writes(
    storage: &StorageSqlx,
) -> std::sync::Arc<(std::sync::atomic::AtomicU64, std::sync::atomic::AtomicU64)> {
    use std::sync::atomic::Ordering;
    let counts = std::sync::Arc::new((
        std::sync::atomic::AtomicU64::new(0),
        std::sync::atomic::AtomicU64::new(0),
    ));
    let mut conn = storage.writer().acquire().await.unwrap();
    let mut handle = conn.lock_handle().await.unwrap();
    let changes = counts.clone();
    handle.set_update_hook(move |_| {
        changes.0.fetch_add(1, Ordering::SeqCst);
    });
    let commits = counts.clone();
    handle.set_commit_hook(move || {
        commits.1.fetch_add(1, Ordering::SeqCst);
        true
    });
    counts
}

tokio::task_local! {
    /// Tests only: called as the toolbox's door returns inside the gate.
    pub(super) static AT_THE_DOOR: std::sync::Arc<dyn Fn() + Send + Sync>;
}

/// `call` run with the [`AT_THE_DOOR`] hook set: its answer, and whether the
/// toolbox's door was reached.
async fn at_the_door<T>(call: impl std::future::Future<Output = T>) -> (T, bool) {
    let door = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let reached = door.clone();
    let hook =
        std::sync::Arc::new(move || reached.store(true, std::sync::atomic::Ordering::SeqCst));
    let result = AT_THE_DOOR.scope(hook, call).await;
    (result, door.load(std::sync::atomic::Ordering::SeqCst))
}

/// The gate's `createAction` under `opts`, expected to refuse `args` as the
/// caller's error before the door: the toolbox not asked (the
/// [`AT_THE_DOOR`] hook not called) and nothing read from the services.
/// Returns the refusal's message.
async fn refused_before_the_door(
    wallet: &Wallet<StorageSqlx, MockWalletServices>,
    args: CreateActionArgs,
    opts: &GateOptions,
) -> String {
    let (result, asked) = at_the_door(create_action(wallet, args, "gate.local", opts)).await;
    let Err(GateError::Scope(message)) = result else {
        panic!("passed the gate: {result:?}");
    };
    assert!(!asked, "the toolbox was asked: {message}");
    assert_eq!(wallet.services().total_calls(), 0, "a read: {message}");
    message
}

/// The transactions and the outputs the store holds: a refusal stores
/// neither.
async fn stored_rows(storage: &StorageSqlx) -> (i64, i64) {
    let mut counts = [0; 2];
    for (n, table) in counts.iter_mut().zip(["transactions", "outputs"]) {
        *n = sqlx::query_scalar(&format!("SELECT COUNT(*) FROM {table}"))
            .fetch_one(storage.pool())
            .await
            .unwrap();
    }
    (counts[0], counts[1])
}

/// No write after the door (#44): through the gate, a `createAction` under
/// the door's word (its label the toolbox's to write) and a plain one, each
/// funded, count the writer's row changes and commits as the toolbox's door
/// returns and as the gate returns: equal. The writer is the store's one
/// connection, so every write of this process is counted.
#[tokio::test]
async fn the_gate_writes_nothing_after_the_door() {
    use std::sync::atomic::Ordering;
    let root = PrivateKey::random();
    let (storage, user, basket) = storage_for(&root.public_key().to_hex()).await;
    let (deep, _) = mined_coin(&storage, (user, basket), 0xb1, 50_000, 800).await;
    let (deep2, _) = mined_coin(&storage, (user, basket), 0xb2, 50_000, 801).await;
    let services = MockWalletServices::builder().build();
    services.set_header_for_height(header(800, &deep, 0));
    services.set_header_for_height(header(801, &deep2, 0));
    services.set_tip_header(header_with_hash(901, &"0d".repeat(32), 0));
    let wallet = Wallet::new(Some(root), storage, services).await.unwrap();
    // The tables a first read makes, made now: the count is the action's.
    crate::tracker_host::frozen_count(wallet.storage())
        .await
        .unwrap();
    let counts = count_writes(wallet.storage()).await;

    for allow in [true, false] {
        let mut args = pay(1_000);
        args.options = Some(CreateActionOptions {
            sign_and_process: Some(false),
            ..Default::default()
        });
        let at_door = std::sync::Arc::new(std::sync::Mutex::new(None));
        let seen = {
            let (counts, at_door) = (counts.clone(), at_door.clone());
            std::sync::Arc::new(move || {
                *at_door.lock().unwrap() = Some((
                    counts.0.load(Ordering::SeqCst),
                    counts.1.load(Ordering::SeqCst),
                ));
            }) as std::sync::Arc<dyn Fn() + Send + Sync>
        };
        let before = (
            counts.0.load(Ordering::SeqCst),
            counts.1.load(Ordering::SeqCst),
        );
        let result = AT_THE_DOOR
            .scope(
                seen,
                create_action(
                    &wallet,
                    args,
                    "gate.local",
                    &opts(1, true).with_allow_received(allow),
                ),
            )
            .await
            .unwrap();
        let after = (
            counts.0.load(Ordering::SeqCst),
            counts.1.load(Ordering::SeqCst),
        );
        let door = at_door.lock().unwrap().expect("the door was reached");
        println!("allowPending {allow}: before {before:?}, at the door {door:?}, after {after:?}");
        assert!(door.1 > before.1, "the door committed the action");
        assert_eq!(after, door, "allowPending {allow}: a write after the door");
        let reference = reference_string(&result.signable_transaction.unwrap().reference);
        assert_eq!(
            labels_of(wallet.storage(), &reference)
                .await
                .contains(&RECEIVED_PENDING_LABEL.to_string()),
            allow
        );
    }
}

/// F4 of the stack's coin read: the refusal as one JSON object, the
/// route's `402` body (`code`, `message`, `offeredSatoshis`, `held`), which
/// `send`, `drain`, `split` and `gift-send` print under `--json`.
#[test]
fn the_refusal_as_json_is_the_routes_body() {
    let refusal = Refusal {
        toolbox: "Insufficient funds: need 30005, have 0".to_string(),
        offered_satoshis: 0,
        held: vec![HeldCoin {
            outpoint: format!("{}.0", "9b".repeat(32)),
            satoshis: 3_000,
            word: "unknown".to_string(),
            origin: Origin::Received,
            age_minutes: 3,
            reason: "depth 1: 0 blocks; received, no proof".to_string(),
            headers_fault: None,
            output_id: 1,
        }],
        capped: None,
        headers_fault: None,
    };
    let json = refusal.to_json();
    println!("{json}");
    assert_eq!(json["code"], "INSUFFICIENT_FUNDS");
    assert_eq!(json["offeredSatoshis"], 0);
    assert_eq!(
        json["held"][0]["outpoint"],
        format!("{}.0", "9b".repeat(32))
    );
    assert_eq!(json["held"][0]["satoshis"], 3_000);
    assert_eq!(json["held"][0]["origin"], "received");
    assert_eq!(json["held"][0]["ageMinutes"], 3);
    assert_eq!(json["message"], refusal.to_string());
    let named = Refusal {
        toolbox: NAMED_INPUT_REFUSED.to_string(),
        ..refusal
    };
    assert_eq!(named.to_json()["code"], "INPUT_NOT_OFFERED");
}

/// The store busy by its type, for `--json` and the route alike; the
/// toolbox's words in a plain error are not it.
#[test]
fn the_gate_error_is_the_store_busy_by_its_type_only() {
    let busy = [
        GateError::Wallet(bsv_sdk::Error::WalletBusy {
            detail: "Store busy: sqlite code 5".to_string(),
            retry_after_ms: None,
        }),
        GateError::Gate(anyhow::Error::new(bsv_wallet_toolbox::Error::StoreBusy {
            reason: bsv_wallet_toolbox::StoreBusyReason::WriterNotAcquired,
        })),
    ];
    for e in &busy {
        assert!(e.is_store_busy(), "{e}");
    }
    let not = [
        GateError::Wallet(bsv_sdk::Error::WalletError(
            "Store busy: sqlite code 5".to_string(),
        )),
        GateError::Gate(anyhow::anyhow!("Store busy: sqlite code 517")),
        GateError::Gate(anyhow::Error::new(sqlx::Error::RowNotFound)),
        // A bare pool timeout: the reader's, as far as it says.
        GateError::Gate(anyhow::Error::new(sqlx::Error::PoolTimedOut)),
    ];
    for e in &not {
        assert!(!e.is_store_busy(), "{e}");
    }
    assert_eq!(
        busy_json("m"),
        serde_json::json!({"code": "STORE_BUSY", "message": "m"})
    );
}

/// Calhooon/bsv-wallet-cli #44, the witness: on one wallet file, a writer
/// shaped like the daemon's monitor (a transaction that reads a row, then
/// writes, then commits, on its own store's writer through `write_tx`, as
/// a second process on the file holds its own store: a CLI command beside
/// the daemon, or a 0.15 daemon's monitor) runs while `createAction`
/// takes the served handler's path (the spend guard, then the gate's
/// `createAction`; a template, aborted after) for 300 rounds. Every
/// `createAction` and every abort answers without a store error, and no
/// writer transaction fails. On toolbox 0.10.0 (e0ff758) the toolbox's
/// deferred `BEGIN` read under a WAL snapshot and wrote after the writer
/// committed: SQLite answered 517 (`SQLITE_BUSY_SNAPSHOT`), which
/// `busy_timeout` never waits out. From 0.11.0 every transaction opens
/// `BEGIN IMMEDIATE`, and the loser waits.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn create_action_beside_a_monitor_shaped_writer_answers_no_store_error() {
    const ROUNDS: usize = 300;
    let dir = tempfile::TempDir::new().unwrap();
    let path = dir.path().join("wallet.db");
    let root = PrivateKey::random();
    let storage = StorageSqlx::open(path.to_str().unwrap()).await.unwrap();
    storage
        .migrate("store-busy", &root.public_key().to_hex())
        .await
        .unwrap();
    storage.make_available().await.unwrap();
    let (user, _) = storage
        .find_or_insert_user(&root.public_key().to_hex())
        .await
        .unwrap();
    let basket = storage
        .find_or_create_default_basket(user.user_id)
        .await
        .unwrap()
        .basket_id;

    // One mined, parseable coin (its BUMP the header's root at 800).
    let lock = bsv_sdk::script::LockingScript::from_hex(LOCK).unwrap();
    let mut parent = bsv_sdk::transaction::Transaction::new();
    parent
        .add_input(bsv_sdk::transaction::TransactionInput::new(
            "aa".repeat(32),
            0,
        ))
        .unwrap();
    parent
        .add_output(bsv_sdk::transaction::TransactionOutput::new(100_000, lock))
        .unwrap();
    let parent_id = parent.id();
    coin(
        &storage,
        (user.user_id, basket),
        &parent_id,
        false,
        Some(800),
        100_000,
        9,
    )
    .await;
    for table in ["transactions", "proven_txs"] {
        sqlx::query(&format!("UPDATE {table} SET raw_tx = ? WHERE txid = ?"))
            .bind(parent.to_binary())
            .bind(&parent_id)
            .execute(storage.writer())
            .await
            .unwrap();
    }
    let services = headers_with(&parent_id, 800, 900);
    let wallet = Wallet::new(Some(root), storage, services).await.unwrap();

    // The monitor-shaped writer: read, then write, then commit, until told,
    // on the store's writer as every writer of this crate is.
    let storage = std::sync::Arc::new(StorageSqlx::open(path.to_str().unwrap()).await.unwrap());
    let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let writer = {
        let stop = stop.clone();
        let storage = storage.clone();
        tokio::spawn(async move {
            let (mut commits, mut failures) = (0usize, 0usize);
            while !stop.load(std::sync::atomic::Ordering::Relaxed) {
                let round = async {
                    let mut tx = storage.write_tx().await?;
                    let (n,): (i64,) = sqlx::query_as("SELECT COUNT(*) FROM transactions")
                        .fetch_one(&mut *tx)
                        .await?;
                    sqlx::query(
                        "INSERT INTO monitor_events (event, details) VALUES ('store-busy', ?)",
                    )
                    .bind(n.to_string())
                    .execute(&mut *tx)
                    .await?;
                    tx.commit().await?;
                    anyhow::Ok(())
                };
                match round.await {
                    Ok(()) => commits += 1,
                    Err(_) => failures += 1,
                }
            }
            (commits, failures)
        })
    };

    let mut store_error = None;
    for round in 1..=ROUNDS {
        let mut args = pay(1_000);
        args.options = Some(CreateActionOptions {
            sign_and_process: Some(false),
            ..Default::default()
        });
        let answer = match crate::spend_guard::run(wallet.storage()).await {
            Err(e) => Err(format!("Storage error: spend guard: {e}")),
            Ok(_) => create_action(&wallet, args, "store-busy.local", &opts(1, true))
                .await
                .map_err(|e| e.to_string()),
        };
        match answer {
            Ok(result) => {
                let reference = reference_string(&result.signable_transaction.unwrap().reference);
                // The abort is a write too, and answers no store error
                // either: the coin is free for the next round.
                if let Err(e) = wallet
                    .abort_action(
                        bsv_sdk::wallet::AbortActionArgs { reference },
                        "store-busy.local",
                    )
                    .await
                {
                    store_error = Some((round, format!("abortAction: {e}")));
                    break;
                }
            }
            Err(e) => {
                store_error = Some((round, e));
                break;
            }
        }
    }
    stop.store(true, std::sync::atomic::Ordering::Relaxed);
    let (commits, failures) = writer.await.unwrap();
    println!("writer: {commits} commits, {failures} failed transactions");
    if let Some((round, e)) = store_error {
        panic!("createAction round {round} of {ROUNDS} answered a store error: {e}");
    }
    assert_eq!(failures, 0, "a monitor-shaped transaction failed");
    assert!(commits > 0, "the writer ran beside the calls");
}

/// One store per file per process (F9 (b) of the #44 CLI review), the
/// daemon's shape: the file opened once by `store::open_shared`, a toolbox
/// `Monitor` over the one handle and the served `Wallet` over the other, as
/// `daemon_tenant::start` builds them. The writer the monitor's handle holds
/// is the wallet's writer (one connection, none idle). Then 100 rounds of
/// the monitor's pass beside the served handler's path (the spend guard,
/// the gate's `createAction`, the abort): no store error, one writer.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_daemons_monitor_and_its_served_create_action_share_one_writer() {
    const ROUNDS: usize = 100;
    let dir = tempfile::TempDir::new().unwrap();
    let path = dir.path().join("wallet.db");
    let root = PrivateKey::random();
    {
        // `init`'s step: the schema, then the file closed.
        let made = StorageSqlx::open(path.to_str().unwrap()).await.unwrap();
        made.migrate("one-store", &root.public_key().to_hex())
            .await
            .unwrap();
    }
    let (storage, monitor_store) = crate::store::open_shared(path.to_str().unwrap())
        .await
        .unwrap();
    let (user, _) = storage
        .find_or_insert_user(&root.public_key().to_hex())
        .await
        .unwrap();
    let basket = storage
        .find_or_create_default_basket(user.user_id)
        .await
        .unwrap()
        .basket_id;
    let lock = bsv_sdk::script::LockingScript::from_hex(LOCK).unwrap();
    let mut parent = bsv_sdk::transaction::Transaction::new();
    parent
        .add_input(bsv_sdk::transaction::TransactionInput::new(
            "ab".repeat(32),
            0,
        ))
        .unwrap();
    parent
        .add_output(bsv_sdk::transaction::TransactionOutput::new(100_000, lock))
        .unwrap();
    let parent_id = parent.id();
    coin(
        &storage,
        (user.user_id, basket),
        &parent_id,
        false,
        Some(800),
        100_000,
        9,
    )
    .await;
    for table in ["transactions", "proven_txs"] {
        sqlx::query(&format!("UPDATE {table} SET raw_tx = ? WHERE txid = ?"))
            .bind(parent.to_binary())
            .bind(&parent_id)
            .execute(storage.writer())
            .await
            .unwrap();
    }
    let wallet = Wallet::new(Some(root), storage, headers_with(&parent_id, 800, 900))
        .await
        .unwrap();
    let monitor = bsv_wallet_toolbox::Monitor::new(
        monitor_store.clone(),
        std::sync::Arc::new(MockWalletServices::new()),
    );

    {
        let held = monitor_store.writer().acquire().await.unwrap();
        assert_eq!(wallet.storage().writer().size(), 1);
        assert_eq!(
            wallet.storage().writer().num_idle(),
            0,
            "the writer the monitor's handle holds is the served wallet's writer"
        );
        drop(held);
    }

    for round in 1..=ROUNDS {
        let mut args = pay(1_000);
        args.options = Some(CreateActionOptions {
            sign_and_process: Some(false),
            ..Default::default()
        });
        let served = async {
            crate::spend_guard::run(wallet.storage())
                .await
                .map_err(|e| format!("Storage error: spend guard: {e}"))?;
            create_action(&wallet, args, "one-store.local", &opts(1, true))
                .await
                .map_err(|e| e.to_string())
        };
        let (answer, pass) = tokio::join!(served, monitor.run_once());
        if let Err(e) = pass {
            panic!("round {round}: the monitor's pass beside createAction: {e:?}");
        }
        let result = answer
            .unwrap_or_else(|e| panic!("round {round}: createAction beside the monitor: {e}"));
        let reference = reference_string(&result.signable_transaction.unwrap().reference);
        wallet
            .abort_action(
                bsv_sdk::wallet::AbortActionArgs { reference },
                "one-store.local",
            )
            .await
            .unwrap_or_else(|e| panic!("round {round}: abortAction: {e}"));
    }
    assert_eq!(wallet.storage().writer().size(), 1, "one writer connection");
}

/// What a store holds that an action could change: every output's state,
/// the transactions, labels and the sequence counters.
async fn store_state(storage: &StorageSqlx) -> Vec<String> {
    let mut state: Vec<String> = sqlx::query_scalar(
        "SELECT output_id || ':' || spendable || ':' || change || ':' || COALESCE(spent_by, '-') FROM outputs ORDER BY output_id",
    )
    .fetch_all(storage.pool())
    .await
    .unwrap();
    for table in [
        "transactions",
        "tx_labels",
        "tx_labels_map",
        "proven_tx_reqs",
    ] {
        let n: i64 = sqlx::query_scalar(&format!("SELECT COUNT(*) FROM {table}"))
            .fetch_one(storage.pool())
            .await
            .unwrap();
        state.push(format!("{table}={n}"));
    }
    state.extend(
        sqlx::query_scalar::<_, String>(
            "SELECT name || '=' || seq FROM sqlite_sequence ORDER BY name",
        )
        .fetch_all(storage.pool())
        .await
        .unwrap(),
    );
    state
}

/// A busy before the door (#44): another process holds the file's write
/// lock past the store's ceiling while the served handler's path runs (the
/// spend guard's first write, then the gate's). Each answers 503
/// STORE_BUSY with `Retry-After: 1`, nothing is posted and the store is
/// unchanged; with the lock released, the same call succeeds.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_busy_before_the_door_answers_503_with_nothing_posted_and_the_coins_unchanged() {
    use axum::response::IntoResponse;
    use sqlx::{ConnectOptions, Connection};
    let dir = tempfile::TempDir::new().unwrap();
    let path = dir.path().join("wallet.db");
    let root = PrivateKey::random();
    let storage = StorageSqlx::open(path.to_str().unwrap()).await.unwrap();
    storage
        .migrate("busy-before", &root.public_key().to_hex())
        .await
        .unwrap();
    storage.make_available().await.unwrap();
    let (user, _) = storage
        .find_or_insert_user(&root.public_key().to_hex())
        .await
        .unwrap();
    let basket = storage
        .find_or_create_default_basket(user.user_id)
        .await
        .unwrap()
        .basket_id;
    let (deep, _) = mined_coin(&storage, (user.user_id, basket), 0xc1, 50_000, 800).await;
    let services = MockWalletServices::builder().post_beef_success().build();
    services.set_header_for_height(header(800, &deep, 0));
    services.set_tip_header(header_with_hash(900, &"0e".repeat(32), 0));
    let wallet = Wallet::new(Some(root), storage, services).await.unwrap();
    let before = store_state(wallet.storage()).await;

    // Another process on the file holds the write lock.
    let mut other = sqlx::sqlite::SqliteConnectOptions::new()
        .filename(&path)
        .connect()
        .await
        .unwrap();
    sqlx::query("BEGIN IMMEDIATE")
        .execute(&mut other)
        .await
        .unwrap();

    let answers = [
        crate::server::handlers::AppError::from_pre_door(
            crate::spend_guard::run(wallet.storage())
                .await
                .expect_err("the guard's first write waits out the ceiling"),
            "spend guard",
        ),
        crate::server::handlers::AppError::from_gate_error(
            create_action(&wallet, pay(1_000), "busy.local", &opts(1, true))
                .await
                .expect_err("the gate's first write waits out the ceiling"),
        ),
    ];
    for answer in answers {
        let response = answer.into_response();
        assert_eq!(
            response.status(),
            axum::http::StatusCode::SERVICE_UNAVAILABLE
        );
        assert_eq!(
            response
                .headers()
                .get(axum::http::header::RETRY_AFTER)
                .unwrap(),
            "1"
        );
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let body: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        println!("{body}");
        assert_eq!(body["code"], "STORE_BUSY");
        assert_eq!(body.as_object().unwrap().len(), 2, "{body}");
    }
    assert_eq!(
        wallet.services().call_count("post_beef"),
        0,
        "nothing posted"
    );
    assert_eq!(
        store_state(wallet.storage()).await,
        before,
        "the store unchanged"
    );

    // The lock released, the same call goes through the door.
    sqlx::query("ROLLBACK").execute(&mut other).await.unwrap();
    other.close().await.unwrap();
    crate::spend_guard::run(wallet.storage()).await.unwrap();
    let mut args = pay(1_000);
    args.options = Some(CreateActionOptions {
        sign_and_process: Some(false),
        ..Default::default()
    });
    create_action(&wallet, args, "busy.local", &opts(1, true))
        .await
        .unwrap();
}

/// #53: the spent-by reason names the spender's state, never a hold.
#[test]
fn the_spent_by_reason_names_the_spenders_state() {
    let t = "e2".repeat(32);
    assert_eq!(
        spent_by_reason(&t, None),
        format!("spent by {t} (unproven)")
    );
    assert_eq!(
        spent_by_reason(&t, Some("unproven")),
        format!("spent by {t} (unproven)")
    );
    assert_eq!(
        spent_by_reason(&t, Some("sending")),
        format!("spent by {t} (unproven)")
    );
    assert_eq!(
        spent_by_reason(&t, Some("completed")),
        format!("spent by {t}")
    );
    assert_eq!(
        spent_by_reason(&t, Some("failed")),
        format!("spent by {t} (failed)")
    );
}

// ---------------------------------------------------------------------------
// The word's scope (#63)
// ---------------------------------------------------------------------------

/// The scoped word offers only the received coins it names: a second
/// received unproven coin is held with the scope named in its reason, and
/// own change keeps its own rule.
#[tokio::test]
async fn the_scoped_word_offers_only_the_named_received_coins() {
    let (storage, user, basket) = storage_for(&("02".to_string() + &"cd".repeat(32))).await;
    let named = "b1".repeat(32);
    let other = "b2".repeat(32);
    let made = "b3".repeat(32);
    let n = coin(&storage, (user, basket), &named, false, None, 4_000, 3).await;
    coin(&storage, (user, basket), &other, false, None, 5_000, 3).await;
    let m = coin(&storage, (user, basket), &made, true, None, 3_000, 2).await;
    let services = headers_with(&"0c".repeat(32), 900, 900);
    let scoped = AllowPending::ReceivedOnly(vec![(named.clone(), 0)]);

    let gate = opts(1, true).with_allow_pending(scoped.clone());
    let mut a = assess(&storage, &services, &gate, None).await.unwrap();
    a.offered.sort();
    assert_eq!(a.offered, vec![(n, 4_000), (m, 3_000)]);
    assert_eq!(
        reason_of(&a, &other).as_deref(),
        Some("depth 1: 0 blocks; received, no proof; not named under allowPending: received")
    );
    assert_eq!(reason_of(&a, &named), None);

    // Own change under SPEND_OWN_CHANGE=0 keeps its reason; the scope says
    // nothing about it.
    let gate = opts(1, false).with_allow_pending(scoped.clone());
    let a = assess(&storage, &services, &gate, None).await.unwrap();
    assert_eq!(a.offered, vec![(n, 4_000)]);
    assert_eq!(
        reason_of(&a, &made).as_deref(),
        Some("depth 1: 0 blocks; own change, SPEND_OWN_CHANGE=0")
    );

    // The scope reaches a coin by vout: the same txid at another vout is
    // not named.
    let gate =
        opts(1, true).with_allow_pending(AllowPending::ReceivedOnly(vec![(named.clone(), 1)]));
    let a = assess(&storage, &services, &gate, None).await.unwrap();
    assert_eq!(
        reason_of(&a, &named).as_deref(),
        Some("depth 1: 0 blocks; received, no proof; not named under allowPending: received")
    );
}

/// A scoped word is the word: the action carries R1 (d)'s label. A named
/// coin already mined is offered as any mined coin (the word is moot for
/// it, not wrong), so a scope naming it passes the door.
#[tokio::test]
async fn a_scoped_word_writes_the_label_and_a_mined_named_coin_passes() {
    let root = PrivateKey::random();
    let (storage, user, basket) = storage_for(&root.public_key().to_hex()).await;
    let (mined, _) = mined_coin(&storage, (user, basket), 0xb4, 50_000, 900).await;
    let services = MockWalletServices::builder().build();
    services.set_header_for_height(header(900, &mined, 0));
    services.set_tip_header(header_with_hash(900, &mined, 0));
    let wallet = Wallet::new(Some(root), storage, services).await.unwrap();
    let gate =
        opts(1, true).with_allow_pending(AllowPending::ReceivedOnly(vec![(mined.clone(), 0)]));
    let mut args = pay(1_000);
    args.options = Some(CreateActionOptions {
        sign_and_process: Some(false),
        ..Default::default()
    });
    let result = create_action(&wallet, args, "gate.local", &gate)
        .await
        .unwrap();
    let reference = reference_string(&result.signable_transaction.unwrap().reference);
    assert_eq!(
        labels_of(wallet.storage(), &reference).await,
        vec![RECEIVED_PENDING_LABEL.to_string()]
    );
}

/// The scope table's coins, each received unless named: a received coin
/// (`c1`), own change (`c2`), a coin another action spent (`c3`), a nosend
/// (`c4`), a lost coin of a completed transaction (`c5`), a frozen coin
/// (`c6`), a failed one (`c7`), a lost coin of an unproven transaction (`c8`)
/// and a failed and frozen one (`ca`).
async fn scope_table(storage: &StorageSqlx, owner: (i64, i64)) -> [String; 9] {
    let coins = ["c1", "c2", "c3", "c4", "c5", "c6", "c7", "c8", "ca"].map(|b| b.repeat(32));
    let [received, made, spent, nosend, lost, frozen, failed, lost_unproven, failed_frozen] =
        &coins;
    coin(storage, owner, received, false, None, 8_000, 5).await;
    coin(storage, owner, made, true, None, 8_000, 5).await;
    coin(storage, owner, spent, false, None, 8_000, 5).await;
    sqlx::query("UPDATE outputs SET spent_by = transaction_id WHERE txid = ?")
        .bind(spent)
        .execute(storage.writer())
        .await
        .unwrap();
    let id = coin(storage, owner, lost, false, Some(900), 8_000, 5).await;
    lose(storage, id).await;
    let id = coin(storage, owner, failed, false, None, 8_000, 5).await;
    fail(storage, failed, id).await;
    let id = coin(storage, owner, lost_unproven, false, None, 8_000, 5).await;
    lose(storage, id).await;
    coin(storage, owner, frozen, false, None, 8_000, 5).await;
    freeze(storage, frozen, 9).await;
    let id = coin(storage, owner, failed_frozen, false, None, 8_000, 5).await;
    fail(storage, failed_frozen, id).await;
    freeze(storage, failed_frozen, 10).await;
    coin(storage, owner, nosend, false, None, 8_000, 5).await;
    set_status(storage, nosend, "nosend").await;
    coins
}

/// A scoped word naming a coin that is not a received coin this wallet
/// could spend under it is the caller's error before the door, naming the
/// coin and why: not ours, own change, consumed by another action, a
/// nosend, and a coin its own record holds as a consumed one is (#165): a
/// lost coin of a completed or an unproven transaction (the store's
/// `spendable = 0`, no spender) and a frozen one. The order is the Lean's
/// `coinWord` (#197): frozen, spent by, lost, the transaction's status; a
/// received coin whose transaction failed (the toolbox's
/// `mark_internalized_tx_failed`: `spendable = 0`, no spender) is refused
/// with its transaction's word, not lost, and a failed and frozen one reads
/// frozen. Nothing is read for the action, the toolbox is not asked, nothing
/// is stored and nothing allocated.
#[tokio::test]
async fn a_named_coin_that_is_not_a_received_spendable_coin_is_refused_before_the_door() {
    let root = PrivateKey::random();
    let (storage, user, basket) = storage_for(&root.public_key().to_hex()).await;
    let services = headers_with(&"0c".repeat(32), 900, 900);
    let [received, made, spent, nosend, lost, frozen, failed, lost_unproven, failed_frozen] =
        scope_table(&storage, (user, basket)).await;
    let wallet = Wallet::new(Some(root), storage, services).await.unwrap();
    let before = stored_rows(wallet.storage()).await;

    let cases: Vec<(String, &str)> = vec![
        ("c9".repeat(32), NOT_HELD),
        (
            made.clone(),
            "own change of an action of this wallet, not a received coin",
        ),
        (spent.clone(), "spent by "),
        (nosend.clone(), "its transaction is nosend"),
        (lost.clone(), "lost: the store marks it not spendable"),
        (frozen.clone(), "frozen: never offered"),
        (failed.clone(), "its transaction is failed"),
        (
            lost_unproven.clone(),
            "lost: the store marks it not spendable",
        ),
        (failed_frozen.clone(), "frozen: never offered"),
    ];
    for (txid, why) in cases {
        let gate = opts(1, true).with_allow_pending(AllowPending::ReceivedOnly(vec![
            (received.clone(), 0),
            (txid.clone(), 0),
        ]));
        let message = refused_before_the_door(&wallet, pay(1_000), &gate).await;
        assert!(
            message.starts_with(&format!(
                "Invalid argument: allowPending: received names {txid}:0, which is not a received coin this wallet could spend under it: {why}"
            )),
            "{message}"
        );
    }
    assert_eq!(
        stored_rows(wallet.storage()).await,
        before,
        "nothing stored"
    );
    let spent_count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM outputs WHERE spent_by IS NOT NULL")
            .fetch_one(wallet.storage().pool())
            .await
            .unwrap();
    assert_eq!(spent_count, 1, "nothing allocated");
}

/// The served `allowPending` and the commands' flags, parsed: the string
/// `"received"` is every received coin, any other string off; an object
/// names the scope (sorted, no repeats, `txid.vout` taken as the listings
/// print it, an empty list off); a bad shape or outpoint is the caller's
/// error; `--coin` without the word is the caller's error.
#[test]
fn the_word_is_parsed_from_the_route_and_the_flags() {
    use serde_json::json;
    let t1 = "a1".repeat(32);
    let t2 = "A2".repeat(32);
    assert_eq!(AllowPending::from_json(None).unwrap(), AllowPending::Off);
    assert_eq!(
        AllowPending::from_json(Some(&json!("received"))).unwrap(),
        AllowPending::Received
    );
    for off in [json!("all"), json!(true), json!(1), json!(["received"])] {
        assert_eq!(
            AllowPending::from_json(Some(&off)).unwrap(),
            AllowPending::Off,
            "{off}"
        );
    }
    assert_eq!(
        AllowPending::from_json(Some(
            &json!({"received": [format!("{t2}:1"), format!("{t1}.0"), format!("{t2}:1")]})
        ))
        .unwrap(),
        AllowPending::ReceivedOnly(vec![(t1.clone(), 0), (t2.to_ascii_lowercase(), 1)])
    );
    assert_eq!(
        AllowPending::from_json(Some(&json!({"received": []}))).unwrap(),
        AllowPending::Off
    );
    for bad in [
        json!({"received": "received"}),
        json!({"all": [format!("{t1}:0")]}),
        json!({"received": [1]}),
        json!({"received": [format!("{t1}:0")], "more": 1}),
    ] {
        let err = AllowPending::from_json(Some(&bad)).unwrap_err();
        assert!(
            err.starts_with("allowPending: an object is"),
            "{bad}: {err}"
        );
    }
    let err = AllowPending::from_json(Some(&json!({"received": ["abc:0"]}))).unwrap_err();
    assert!(err.contains("not an outpoint"), "{err}");
    let err = AllowPending::from_json(Some(&json!({"received": [format!("{t1}:x")]}))).unwrap_err();
    assert!(err.contains("vout is not a number"), "{err}");

    let coin = format!("{t1}:0");
    assert_eq!(
        AllowPending::from_cli(None, &[]).unwrap(),
        AllowPending::Off
    );
    assert_eq!(
        AllowPending::from_cli(Some("received"), &[]).unwrap(),
        AllowPending::Received
    );
    assert_eq!(
        AllowPending::from_cli(Some("received"), std::slice::from_ref(&coin)).unwrap(),
        AllowPending::ReceivedOnly(vec![(t1.clone(), 0)])
    );
    assert!(AllowPending::from_cli(None, &[coin])
        .unwrap_err()
        .contains("--allow-pending received"));

    let word = AllowPending::ReceivedOnly(vec![(t1.clone(), 0)]);
    assert!(word.is_on());
    assert!(word.reaches(&t1, 0));
    assert!(!word.reaches(&t1, 1));
    assert!(!AllowPending::Off.is_on());
    assert!(AllowPending::Received.reaches(&t2, 7));
}

// ---------------------------------------------------------------------------
// The header service's fault (#59)
// ---------------------------------------------------------------------------

/// A mined coin the gate could carry, and a header service that does not
/// answer for its tip.
async fn a_coin_under_an_outage(
    satoshis: u64,
) -> (Wallet<StorageSqlx, MockWalletServices>, String) {
    let root = PrivateKey::random();
    let (storage, user, basket) = storage_for(&root.public_key().to_hex()).await;
    let (mined, _) = mined_coin(&storage, (user, basket), 0xe1, satoshis, 900).await;
    let services = MockWalletServices::builder().build();
    services.set_header_for_height(header(900, &mined, 0));
    services.set_tip_unavailable();
    let wallet = Wallet::new(Some(root), storage, services).await.unwrap();
    (wallet, mined)
}

/// The header service does not answer: the coin is held on its fault, in
/// the service's own words; the gate reads once more after the pause and,
/// the coin having covered the action, answers "could not verify now"
/// (`HEADERS_UNAVAILABLE`), never the funds' verdict. The tip was read
/// twice and nothing was allocated.
#[tokio::test]
async fn a_header_service_fault_is_read_once_more_and_is_could_not_verify_now() {
    let (wallet, mined) = a_coin_under_an_outage(50_000).await;
    let started = std::time::Instant::now();
    let Err(GateError::Refused(refusal)) =
        create_action(&wallet, pay(1_000), "gate.local", &opts(1, true)).await
    else {
        panic!("the coin was offered with no header");
    };
    assert!(started.elapsed() >= HEADER_RETRY_PAUSE, "no pause");
    assert_eq!(wallet.services().call_count("get_chain_tip_header"), 2);
    assert_eq!(refusal.code(), "HEADERS_UNAVAILABLE");
    assert!(refusal.is_headers_fault());
    assert_eq!(refusal.held.len(), 1);
    let coin = &refusal.held[0];
    assert_eq!(coin.outpoint, format!("{mined}.0"));
    assert_eq!(
        coin.headers_fault.as_deref(),
        Some("Service error: mock: the chain tip header is unavailable")
    );
    assert_eq!(
        coin.reason,
        "headers unavailable: Service error: mock: the chain tip header is unavailable"
    );
    assert_eq!(
        refusal.headers_fault.as_deref(),
        Some("Service error: mock: the chain tip header is unavailable")
    );
    let text = refusal.to_string();
    assert!(
        text.starts_with("Could not verify now: the header service did not answer (Service error: mock: the chain tip header is unavailable); 1 coin(s), 50000 sats, held on it, no verdict ("),
        "{text}"
    );
    assert!(!text.contains("HeaderError("), "{text}");
    assert_eq!(refusal.to_json()["code"], "HEADERS_UNAVAILABLE");
    assert_eq!(refusal.to_json()["message"], text);
    let spent: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM outputs WHERE spent_by IS NOT NULL")
        .fetch_one(wallet.storage().pool())
        .await
        .unwrap();
    assert_eq!(spent, 0, "nothing allocated");
}

/// A shortfall the faulted coins could not have covered is the funds'
/// refusal whatever the service said: the coin is held on the fault (and
/// read once more), but 500 sats never pay 1,000.
#[tokio::test]
async fn a_shortfall_the_faulted_coins_could_not_cover_stays_insufficient_funds() {
    let (wallet, _) = a_coin_under_an_outage(500).await;
    let Err(GateError::Refused(refusal)) =
        create_action(&wallet, pay(1_000), "gate.local", &opts(1, true)).await
    else {
        panic!("the coin was offered with no header");
    };
    assert_eq!(wallet.services().call_count("get_chain_tip_header"), 2);
    assert_eq!(refusal.code(), "INSUFFICIENT_FUNDS");
    assert!(refusal.held[0].headers_fault.is_some());
    assert!(
        refusal.to_string().starts_with("Insufficient funds"),
        "{refusal}"
    );
}

/// The service answers on the re-read: the coin is offered and the
/// assessment says it was read twice. Without the door's retry
/// (`assess`), one read and the hold.
#[tokio::test]
async fn the_re_read_offers_the_coin_when_the_service_answers_again() {
    let (wallet, mined) = a_coin_under_an_outage(50_000).await;
    let gate = opts(1, true);
    let once = assess(wallet.storage(), wallet.services(), &gate, None)
        .await
        .unwrap();
    assert!(!once.headers_retried);
    assert!(once.offered.is_empty());
    assert!(once.held[0].headers_fault.is_some());
    assert_eq!(wallet.services().call_count("get_chain_tip_header"), 1);

    let recover = async {
        tokio::time::sleep(Duration::from_millis(300)).await;
        wallet
            .services()
            .set_tip_header(header_with_hash(900, &mined, 0));
    };
    let (twice, ()) = tokio::join!(
        assess_retrying(wallet.storage(), wallet.services(), &gate, None, true),
        recover
    );
    let twice = twice.unwrap();
    assert!(twice.headers_retried);
    assert_eq!(twice.offered.len(), 1);
    assert!(twice.held.is_empty(), "{:?}", twice.held);
    assert_eq!(wallet.services().call_count("get_chain_tip_header"), 3);
}

/// A named input held on the service's fault alone is "could not verify
/// now" (read once more first); a named input held by a rule beside it is
/// the refusal, as every named input must be offerable.
#[tokio::test]
async fn a_named_input_held_on_the_fault_alone_is_could_not_verify_now() {
    let (wallet, mined) = a_coin_under_an_outage(50_000).await;
    let user: i64 = sqlx::query_scalar("SELECT user_id FROM users")
        .fetch_one(wallet.storage().pool())
        .await
        .unwrap();
    let basket: i64 =
        sqlx::query_scalar("SELECT basket_id FROM output_baskets WHERE name = 'default'")
            .fetch_one(wallet.storage().pool())
            .await
            .unwrap();
    let received = "e2".repeat(32);
    coin(
        wallet.storage(),
        (user, basket),
        &received,
        false,
        None,
        8_000,
        5,
    )
    .await;

    let mut args = pay(1_000);
    args.inputs = Some(vec![named_input(&mined)]);
    let Err(GateError::Refused(refusal)) =
        create_action(&wallet, args, "gate.local", &opts(1, true)).await
    else {
        panic!("the named coin passed with no header");
    };
    assert_eq!(wallet.services().call_count("get_chain_tip_header"), 2);
    assert_eq!(refusal.code(), "HEADERS_UNAVAILABLE");
    assert!(refusal.is_named_input());
    assert!(
        refusal.to_string().starts_with("Could not verify now: the header service did not answer (Service error: mock: the chain tip header is unavailable); a named input is held on it, no verdict (1 coin(s), 50000 sats)"),
        "{refusal}"
    );

    let mut args = pay(1_000);
    args.inputs = Some(vec![named_input(&mined), named_input(&received)]);
    let Err(GateError::Refused(refusal)) =
        create_action(&wallet, args, "gate.local", &opts(1, true)).await
    else {
        panic!("the named coins passed");
    };
    assert_eq!(refusal.code(), "INPUT_NOT_OFFERED");
    assert_eq!(refusal.held.len(), 2);
}

/// A coin of `txid` whose creator a reorg took: the toolbox's record as the
/// daemon's demotion leaves it (`unproven`, no stored proof) and the host's
/// tracker row, which replays `stale` against the header service here (its
/// header at 900 carries another root).
async fn stale_coin(storage: &StorageSqlx, ub: (i64, i64), txid: &str, made: bool) -> i64 {
    let id = coin(storage, ub, txid, made, None, 6_000, 5).await;
    crate::tracker_host::tests::stale_row(storage, txid, 900).await;
    id
}

/// Ruling 7a (`lean/Wallet.lean` `doorWord`, `stale_ranks_as_seen`): a
/// `stale` creator ranks as `seen` at the door. Our own change is offered
/// under `SPEND_OWN_CHANGE=1` and held without it; a received coin is held
/// with no word, offered under the plain word and under a scoped word that
/// names it, and held "not named" under one that does not. The reasons say
/// the word: the block left the chain and the transaction is confirming.
#[tokio::test]
async fn a_stale_creator_is_offered_under_the_rule_a_pending_one_is() {
    let (storage, user, basket) = storage_for(&("02".to_string() + &"ab".repeat(32))).await;
    let (made, named, other) = ("f1".repeat(32), "f2".repeat(32), "f3".repeat(32));
    let m = stale_coin(&storage, (user, basket), &made, true).await;
    let n = stale_coin(&storage, (user, basket), &named, false).await;
    let o = stale_coin(&storage, (user, basket), &other, false).await;
    let services = headers_with(&"0c".repeat(32), 900, 900);
    let held = |hint: &str| format!("{STALE}; received, confirming again; {hint}");
    let scoped = AllowPending::ReceivedOnly(vec![(named.clone(), 0)]);
    let cases = [
        (
            opts(1, true),
            vec![m],
            vec![(&named, held(ALLOW_PENDING_HINT))],
        ),
        (
            opts(1, false),
            vec![],
            vec![(&made, format!("{STALE}; own change, SPEND_OWN_CHANGE=0"))],
        ),
        (
            opts(1, true).with_allow_received(true),
            vec![m, n, o],
            vec![],
        ),
        (
            opts(1, true).with_allow_pending(scoped),
            vec![m, n],
            vec![(&other, held(NOT_NAMED_HINT))],
        ),
    ];
    for (gate, offered, reasons) in cases {
        let a = assess(&storage, &services, &gate, None).await.unwrap();
        println!("{gate:?}: offered {:?}, held {:?}", a.offered, a.held);
        let mut ids = a.offer();
        ids.sort_unstable();
        assert_eq!(ids, offered, "{gate:?}");
        for (txid, reason) in reasons {
            assert_eq!(reason_of(&a, txid), Some(reason), "{gate:?}");
        }
        assert!(a.held.iter().all(|c| c.word == "stale"), "{gate:?}");
    }
}

/// The door closes on `rejected`, `conflicted` and `abandoned` only (ruling
/// 7a; `dead_creator_never_offered`): never offered under any rule or word,
/// for either origin, the reason byte for byte as before.
#[test]
fn a_dead_creator_is_never_offered_under_any_rule() {
    let txid = "f4".repeat(32);
    let gates = [
        opts(1, true),
        opts(1, true).with_allow_received(true),
        opts(1, true).with_allow_pending(AllowPending::ReceivedOnly(vec![(txid.clone(), 0)])),
    ];
    for word in ["rejected", "conflicted", "abandoned"] {
        let reading = CreatorReading {
            word,
            depth: 0,
            suspect: false,
            door: false,
            fault: None,
            headers_fault: false,
        };
        for (gate, origin) in gates
            .iter()
            .flat_map(|g| [(g, Origin::Made), (g, Origin::Received)])
        {
            assert_eq!(
                hold_reason(&reading, origin, (&txid, 0), gate),
                Some(format!("{word}: never offered")),
                "{word}, {origin:?}, {gate:?}"
            );
        }
    }
}

/// The toolbox allocates a stale creator's coin the gate offers (#165's
/// lesson: under `acceptDelayedBroadcast` the toolbox reads the store, not
/// the word). The `serve` shape, where no monitor demotes the action: still
/// `completed` with its stored proof, which our header at 900 no longer
/// carries; the tracker reads it `stale`, and the action's word offers the
/// received coin, the wallet's only one, which funds the action (with the
/// gate holding it, the action is refused for funds).
#[tokio::test]
async fn the_toolbox_allocates_a_stale_coin_the_gate_offers() {
    let root = PrivateKey::random();
    let (storage, user, basket) = storage_for(&root.public_key().to_hex()).await;
    let (txid, id) = mined_coin(&storage, (user, basket), 0xf5, 50_000, 900).await;
    crate::tracker_host::tests::stale_row(&storage, &txid, 900).await;
    let services = headers_with(&"0c".repeat(32), 900, 905);
    let wallet = Wallet::new(Some(root), storage, services).await.unwrap();
    let gate = opts(1, true).with_allow_received(true);
    let result = create_action(&wallet, delayed(), "gate.local", &gate)
        .await
        .unwrap();
    assert!(result.signable_transaction.is_some());
    assert!(
        spent_by(wallet.storage(), id).await.is_some(),
        "the stale creator's coin funds the action"
    );
}
