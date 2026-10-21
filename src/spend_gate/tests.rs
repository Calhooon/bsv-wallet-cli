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
        allow_received: false,
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

/// A coin an action of ours holds is listed `locked by <txid>`.
#[tokio::test]
async fn a_locked_coin_is_listed_by_its_holder() {
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
    assert_eq!(held[0].reason, format!("locked by {spender}"));
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
    assert!(!GateOptions::from_env().allow_received);
    assert!(!GateOptions::default().allow_received);
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
    args.inputs = Some(vec![bsv_sdk::wallet::CreateActionInput {
        outpoint: bsv_sdk::wallet::Outpoint::from_string(&format!("{received}.0")).unwrap(),
        input_description: "named".to_string(),
        unlocking_script: None,
        unlocking_script_length: Some(107),
        sequence_number: None,
    }]);

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
        None,
    )
    .await
    .unwrap();
    assert!(held.is_empty(), "{held:?}");
    // Not one of this wallet's coins: left to the toolbox.
    let foreign = assess_named(
        wallet.storage(),
        wallet.services(),
        &opts(1, true),
        &[("d9".repeat(32), 0)],
        None,
    )
    .await
    .unwrap();
    assert!(foreign.is_empty());
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
            output_id: 1,
        }],
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

/// Calhooon/bsv-wallet-cli #44, the witness: on one wallet file, a writer
/// shaped like the daemon's monitor (a transaction that reads a row, then
/// writes, then commits, on its own store's writer through `write_tx`, as
/// the daemon's monitor holds its own store) runs while `createAction`
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
