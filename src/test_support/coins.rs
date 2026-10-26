//! The shared in-memory store and mined-coin planters for served and gate tests.

use bsv_sdk::transaction::MerklePath;
use bsv_wallet_toolbox::{StorageSqlx, WalletStorageWriter};
use chrono::Utc;

const LOCK: &str = "76a914dbc0a7c84983c5bf199b7b2d41b3acf0408ee5aa88ac";

/// A migrated in-memory storage for `identity`: (storage, user, basket).
pub(crate) async fn storage_for(identity: &str) -> (StorageSqlx, i64, i64) {
    storage_for_named(identity, "spend-gate").await
}

/// The same planter with the calling suite's storage name preserved.
pub(crate) async fn storage_for_named(identity: &str, name: &str) -> (StorageSqlx, i64, i64) {
    let storage = StorageSqlx::in_memory().await.unwrap();
    storage.migrate(name, identity).await.unwrap();
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
pub(crate) async fn coin(
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

/// A mined coin the toolbox can carry in a BEEF: a parseable transaction
/// whose stored proof's root is its txid at `height`. (txid, output id).
pub(crate) async fn mined_coin(
    storage: &StorageSqlx,
    (user, basket): (i64, i64),
    seed: u8,
    satoshis: u64,
    height: u32,
) -> (String, i64) {
    mined_coin_with_lock(storage, (user, basket), seed, satoshis, height, LOCK).await
}

pub(crate) async fn mined_coin_with_lock(
    storage: &StorageSqlx,
    (user, basket): (i64, i64),
    seed: u8,
    satoshis: u64,
    height: u32,
    locking_script: &str,
) -> (String, i64) {
    let lock = bsv_sdk::script::LockingScript::from_hex(locking_script).unwrap();
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
    sqlx::query("UPDATE outputs SET locking_script = ? WHERE output_id = ?")
        .bind(hex::decode(locking_script).unwrap())
        .bind(id)
        .execute(storage.writer())
        .await
        .unwrap();
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
