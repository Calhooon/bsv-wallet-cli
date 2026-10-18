//! The spend gate against real SQLite storage and the toolbox's mock
//! services: a header service stood in by the mock, nothing on the network.

use super::*;
use crate::tracker_host::tests::{header, header_with_hash};
use bsv_sdk::primitives::PrivateKey;
use bsv_sdk::transaction::MerklePath;
use bsv_sdk::wallet::{CreateActionOptions, CreateActionOutput};
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
    .execute(storage.pool())
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
        .execute(storage.pool())
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
    .execute(storage.pool())
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
        Some("depth 1: 0 blocks; received, no proof")
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
/// settings.
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
            Some("depth 1: 0 blocks; received, no proof")
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

    let err = create_action(&wallet, pay(1_000), "gate.local", &opts(2, false))
        .await
        .err()
        .expect("refused");
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
                "depth 2: 0 blocks; received, no proof".to_string(),
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
        .execute(storage.pool())
        .await
        .unwrap();
    let held = locked(storage.pool()).await.unwrap();
    assert_eq!(held.len(), 1);
    assert_eq!(held[0].reason, format!("locked by {spender}"));
    assert_eq!(held[0].word, "locked");
}

/// A hold restores exactly what it set, and a hold a crashed call left is
/// restored by the next call.
#[tokio::test]
async fn holds_restore_exactly_and_stale_holds_are_restored() {
    let (storage, user, basket) = storage_for(&("02".to_string() + &"ab".repeat(32))).await;
    let a = coin(
        &storage,
        (user, basket),
        &"f1".repeat(32),
        true,
        None,
        5_000,
        1,
    )
    .await;
    let pool = storage.pool();
    hold(pool, "me", &[a]).await.unwrap();
    let change: i64 = sqlx::query_scalar("SELECT change FROM outputs WHERE output_id = ?")
        .bind(a)
        .fetch_one(pool)
        .await
        .unwrap();
    assert_eq!(change, 0);
    assert_eq!(
        restore(pool, Some("other")).await.unwrap(),
        0,
        "not its hold"
    );
    sqlx::query("UPDATE spend_gate_holds SET held_at = 0")
        .execute(pool)
        .await
        .unwrap();
    assert_eq!(restore(pool, None).await.unwrap(), 1, "a stale hold");
}
