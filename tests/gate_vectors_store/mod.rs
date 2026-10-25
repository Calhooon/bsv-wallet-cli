//! The store of one wallet-gate vector row (`tests/gate_vectors.rs`, #130):
//! the creator's action and evidence, its dead parent when the row's
//! lineage is dead (#163), the coin with its lock, and the mock header
//! service, each in the store's own records. Nothing here asks the gate.

use bsv_sdk::primitives::hash::{sha256, sha256d};
use bsv_sdk::transaction::MerklePath;
use bsv_tracker::{ChainEnvelope, ChainEvent, EventHeader, Hint, HintStatus};
use bsv_wallet_cli::spend_gate::Origin;
use bsv_wallet_cli::tracker_host::{heard, read_creators, SystemClock, TickOptions};
use bsv_wallet_toolbox::services::mock::MockWalletServices;
use bsv_wallet_toolbox::services::BlockHeader;
use bsv_wallet_toolbox::{StorageSqlx, WalletStorageWriter};
use serde_json::Value;

const LOCK: &str = "76a914dbc0a7c84983c5bf199b7b2d41b3acf0408ee5aa88ac";

pub(crate) fn s<'a>(row: &'a Value, key: &str) -> Option<&'a str> {
    row.get(key).and_then(Value::as_str)
}

pub(crate) fn b(row: &Value, key: &str) -> Option<bool> {
    row.get(key).and_then(Value::as_bool)
}

pub(crate) fn n(row: &Value, key: &str) -> Option<u64> {
    row.get(key).and_then(Value::as_u64)
}

/// The row's key: case, step, coin and the action's word.
pub(crate) fn key_of(row: &Value) -> String {
    format!(
        "{} | step {} | {} | allow_received {}",
        s(row, "case").unwrap_or("?"),
        n(row, "step").unwrap_or(0),
        s(row, "coin").unwrap_or("?"),
        b(row, "allow_received").unwrap_or(false)
    )
}

/// A Lean name (`F`, `T1`, `rB1`) as a 32-byte display-order hash.
pub(crate) fn hash_of(name: &str) -> String {
    hex::encode(sha256(format!("wallet-gate-vectors:{name}").as_bytes()))
}

pub(crate) fn display(raw: &[u8]) -> String {
    let mut reversed = raw.to_vec();
    reversed.reverse();
    hex::encode(reversed)
}

/// A header at `height` committing to `root`, its hash computed.
pub(crate) fn header(height: u32, root: &str) -> BlockHeader {
    let mut h = BlockHeader {
        version: 1,
        previous_hash: "00".repeat(32),
        merkle_root: root.to_string(),
        time: 1_700_000_000,
        bits: 0x1d00_ffff,
        nonce: height,
        hash: String::new(),
        height,
    };
    h.hash = display(&sha256d(&h.to_binary()));
    h
}

pub(crate) fn event_header(h: &BlockHeader) -> EventHeader {
    EventHeader {
        version: h.version,
        previous_hash: h.previous_hash.clone(),
        merkle_root: h.merkle_root.clone(),
        time: h.time,
        bits: h.bits,
        nonce: h.nonce,
        height: h.height,
        hash: h.hash.clone(),
        // A hash-shaped work, as the decoder asks.
        chain_work: format!("{}01", "00".repeat(31)),
    }
}

pub(crate) fn envelope(event: ChainEvent) -> String {
    serde_json::to_string(&ChainEnvelope::new(event)).unwrap()
}

/// A row's number as a height.
pub(crate) fn height(row: &Value, key: &str) -> Option<u32> {
    n(row, key).map(|v| u32::try_from(v).unwrap())
}

/// One row's store and header service, built from its inputs.
pub(crate) struct Built {
    pub(crate) storage: StorageSqlx,
    pub(crate) services: MockWalletServices,
    pub(crate) user: i64,
    pub(crate) basket: i64,
    pub(crate) txid: String,
    pub(crate) vout: u32,
    pub(crate) proof: Option<Vec<u8>>,
    /// The header at the creator's proof height, as the service answers.
    pub(crate) live_at_proof: Option<BlockHeader>,
    pub(crate) output_id: Option<i64>,
    pub(crate) origin: Option<Origin>,
}

/// A migrated in-memory store with the tracker's tables, and its user and
/// default basket.
pub(crate) async fn store() -> (StorageSqlx, i64, i64) {
    let identity = format!("02{}", "ab".repeat(32));
    let storage = StorageSqlx::in_memory().await.unwrap();
    storage.migrate("gate-vectors", &identity).await.unwrap();
    storage.make_available().await.unwrap();
    let user = storage.find_or_insert_user(&identity).await.unwrap().0;
    let basket = storage.find_or_create_default_basket(user.user_id);
    let basket = basket.await.unwrap().basket_id;
    // The tracker's tables, as every read of them makes them.
    let none = MockWalletServices::builder().build();
    read_creators(&storage, &none, &TickOptions::default(), &[])
        .await
        .unwrap();
    (storage, user.user_id, basket)
}

/// The header service of the row: the tip at `headers_tip`, and at the
/// creator's proof height a header carrying the row's root there. The
/// proof is coinbase-style (its root is the creator's txid), so the header
/// carries the txid when the row's two root names agree.
pub(crate) fn headers(row: &Value, txid: &str) -> (MockWalletServices, Option<BlockHeader>) {
    let services = MockWalletServices::builder().build();
    let tip = height(row, "headers_tip").unwrap();
    let proof_root = s(row, "creator_proof_root");
    let live = height(row, "creator_proof_height").map(|h| {
        let root = match s(row, "header_root_at_proof_height") {
            Some(name) if Some(name) == proof_root => txid.to_string(),
            Some(name) => hash_of(name),
            None => hash_of("no header"),
        };
        header(h, &root)
    });
    if let Some(h) = &live {
        services.set_header_for_height(h.clone());
    }
    match &live {
        Some(h) if h.height == tip => services.set_tip_header(h.clone()),
        _ => services.set_tip_header(header(tip, &hash_of(&format!("tip {tip}")))),
    }
    (services, live)
}

pub(crate) async fn build(row: &Value) -> Built {
    let (storage, user, basket) = store().await;
    let (creator, vout) = s(row, "coin").unwrap().rsplit_once('.').unwrap();
    let txid = hash_of(creator);
    let (services, live_at_proof) = headers(row, &txid);
    let mut built = Built {
        storage,
        services,
        user,
        basket,
        txid,
        vout: vout.parse().unwrap(),
        proof: None,
        live_at_proof,
        output_id: None,
        origin: None,
    };
    if b(row, "coin_held") == Some(true) {
        let made = s(row, "creator_origin") == Some("made");
        let mined = s(row, "creator_word") == Some("mined");
        let tx_row = insert_action(
            &built,
            creator,
            &built.txid,
            made,
            if mined { "completed" } else { "unproven" },
        )
        .await;
        evidence(row, &mut built, made).await;
        if b(row, "dead_lineage") == Some(true) {
            dead_parent(&built, creator, tx_row).await;
        }
        built.output_id = Some(insert_coin(row, &built, tx_row).await);
        built.origin = Some(if made { Origin::Made } else { Origin::Received });
    }
    built
}

/// An action of the toolbox's `transactions`: its row id.
pub(crate) async fn insert_action(
    built: &Built,
    name: &str,
    txid: &str,
    ours: bool,
    status: &str,
) -> i64 {
    sqlx::query(
        "INSERT INTO transactions (user_id, status, reference, is_outgoing, satoshis, version, lock_time, description, txid, raw_tx) \
         VALUES (?, ?, ?, ?, 0, 1, 0, 'gate vectors', ?, X'01000000')",
    )
    .bind(built.user)
    .bind(status)
    .bind(format!("ref-{name}"))
    .bind(i64::from(ours))
    .bind(txid)
    .execute(built.storage.writer())
    .await
    .unwrap()
    .last_insert_rowid()
}

/// The creator's evidence, as the host keeps it.
pub(crate) async fn evidence(row: &Value, built: &mut Built, made: bool) {
    let word = s(row, "creator_word").unwrap();
    let data = s(row, "creator_word_data").unwrap_or("");
    match word {
        "mined" => mined(row, built, made).await,
        "stale" => stale(row, built, made).await,
        "rejected" => {
            let verdict = serde_json::json!({"kind": "rejected", "reason": data});
            insert_tracked(built, made, None, &[], Some(verdict)).await;
        }
        "conflicted" => {
            let competitors: Vec<String> = data.split_whitespace().map(hash_of).collect();
            let verdict = serde_json::json!({"kind": "conflicted", "competitors": competitors});
            insert_tracked(built, made, None, &[], Some(verdict)).await;
        }
        "built" => insert_tracked(built, true, None, &[], None).await,
        "announced" => hint(built, HintStatus::Accepted).await,
        "seen" => hint(built, HintStatus::Seen).await,
        // No record of the creator but the action: the tracker reads it
        // `unknown`.
        "unknown" => {}
        other => panic!(
            "{}: a creator word this replay does not build: {other}",
            key_of(row)
        ),
    }
}

/// The creator's proof: coinbase-style at the row's proof height.
pub(crate) fn proof_path(row: &Value, built: &Built) -> MerklePath {
    let h = height(row, "creator_proof_height").expect("the creator carries its proof");
    MerklePath::from_coinbase_txid(&built.txid, h)
}

/// A broadcaster's hint, stored as `heard` stores it.
pub(crate) async fn hint(built: &Built, status: HintStatus) {
    let hint = Hint::new("arcade", status, 1_700_000_000);
    let opts = TickOptions::default();
    heard(
        &built.storage,
        &SystemClock,
        &opts,
        &built.txid,
        hint,
        None,
        None,
    )
    .await
    .unwrap();
}

/// A mined creator: the toolbox's proof record, and for the suspect mark
/// the fork stored from the feed.
pub(crate) async fn mined(row: &Value, built: &mut Built, made: bool) {
    let path = proof_path(row, built);
    built.proof = Some(path.to_binary());
    insert_proven(built, &path).await;
    if b(row, "creator_suspect") == Some(true) {
        let h = built.live_at_proof.clone().unwrap();
        let fork = ChainEvent::Fork {
            height: h.height,
            competing_tips: [
                event_header(&h),
                event_header(&header(h.height, &hash_of("fork"))),
            ],
            depth: 1,
        };
        let chain = [envelope(fork)];
        insert_tracked(built, made, Some(&path.to_hex()), &chain, None).await;
    }
}

/// A stale creator, mined in a block that left the active chain: the stored
/// path, and the stored reorg that took the block it was checked in.
pub(crate) async fn stale(row: &Value, built: &Built, made: bool) {
    let path = &proof_path(row, built);
    let tip = height(row, "headers_tip").unwrap();
    let reorg = ChainEvent::Reorg {
        fork_height: path.block_height,
        depth: 1,
        deactivated_headers: vec![event_header(&header(path.block_height, &built.txid))],
        new_tip: event_header(&header(tip, &hash_of(&format!("tip {tip}")))),
    };
    let chain = [envelope(reorg)];
    insert_tracked(built, made, Some(&path.to_hex()), &chain, None).await;
}

/// The toolbox's record of a checked proof (`proven_txs`).
pub(crate) async fn insert_proven(built: &Built, path: &MerklePath) {
    sqlx::query(
        "INSERT INTO proven_txs (txid, height, idx, block_hash, merkle_root, merkle_path, raw_tx) \
         VALUES (?, ?, 0, ?, ?, ?, X'01000000')",
    )
    .bind(&built.txid)
    .bind(i64::from(path.block_height))
    .bind(built.live_at_proof.as_ref().map(|h| h.hash.clone()))
    .bind(&built.txid)
    .bind(path.to_binary())
    .execute(built.storage.writer())
    .await
    .unwrap();
}

/// The coin, a change output of the default basket, with its lock:
/// `locked` and `spent` are `spent_by` a live or a completed action of
/// ours, `lost` the relinquished record (`spendable = 0`).
pub(crate) async fn insert_coin(row: &Value, built: &Built, tx_row: i64) -> i64 {
    let lock = s(row, "coin_lock").unwrap();
    let spender = match lock {
        "locked" | "spent" => {
            let name = s(row, "coin_lock_txid").unwrap();
            let status = if lock == "spent" {
                "completed"
            } else {
                "unproven"
            };
            Some(
                insert_action(
                    built,
                    &format!("spender-{name}"),
                    &hash_of(name),
                    true,
                    status,
                )
                .await,
            )
        }
        _ => None,
    };
    let satoshis = i64::try_from(n(row, "coin_satoshis").unwrap()).unwrap();
    let coin = (built.txid.as_str(), built.vout, satoshis);
    insert_output(built, tx_row, coin, lock == "free", spender).await
}

/// An output `(txid, vout, satoshis)` of the transaction row `tx_row`, a
/// change output of the default basket.
pub(crate) async fn insert_output(
    built: &Built,
    tx_row: i64,
    (txid, vout, satoshis): (&str, u32, i64),
    spendable: bool,
    spent_by: Option<i64>,
) -> i64 {
    sqlx::query(
        "INSERT INTO outputs (user_id, transaction_id, basket_id, vout, satoshis, locking_script, txid, type, spendable, change, provided_by, purpose, output_description, spent_by) \
         VALUES (?, ?, ?, ?, ?, ?, ?, 'P2PKH', ?, 1, 'storage', 'change', 'c', ?)",
    )
    .bind(built.user)
    .bind(tx_row)
    .bind(built.basket)
    .bind(i64::from(vout))
    .bind(satoshis)
    .bind(hex::decode(LOCK).unwrap())
    .bind(txid)
    .bind(i64::from(spendable))
    .bind(spent_by)
    .execute(built.storage.writer())
    .await
    .unwrap()
    .last_insert_rowid()
}

/// A row whose creator's lineage is dead names no ancestor, so the replay
/// builds the scenario's (2026-10-22-wallet-void-parent-descendants-stay-
/// spendable): a parent P of ours, `conflicted` by a mined competitor R
/// (the tracker's stored word and verdict), whose change the creator spent
/// (`spent_by` the creator's row). The mirror's parent is a received
/// coin's creator; the walk reads a parent's word whatever its origin.
pub(crate) async fn dead_parent(built: &Built, creator: &str, tx_row: i64) {
    let name = format!("lineage parent of {creator}");
    let txid = hash_of(&name);
    let parent = insert_action(built, &name, &txid, true, "unproven").await;
    let verdict = serde_json::json!({"kind": "conflicted", "competitors": [hash_of("R")]});
    sqlx::query("INSERT INTO tracker_states (txid, word, verdict) VALUES (?, 'conflicted', ?)")
        .bind(&txid)
        .bind(verdict.to_string())
        .execute(built.storage.writer())
        .await
        .unwrap();
    insert_output(built, parent, (&txid, 0, 1_000), false, Some(tx_row)).await;
}

/// A tracker row in the host's own record (`tracker_states`).
pub(crate) async fn insert_tracked(
    built: &Built,
    was_built: bool,
    bump: Option<&str>,
    chain: &[String],
    verdict: Option<Value>,
) {
    sqlx::query(
        "INSERT INTO tracker_states (txid, built, hints, bump, chain, verdict) VALUES (?, ?, '[]', ?, ?, ?)",
    )
    .bind(&built.txid)
    .bind(i64::from(was_built))
    .bind(bump)
    .bind(serde_json::to_string(chain).unwrap())
    .bind(verdict.map(|v| v.to_string()))
    .execute(built.storage.writer())
    .await
    .unwrap();
}
