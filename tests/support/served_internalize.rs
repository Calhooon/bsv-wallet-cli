//! The helpers of the served `POST /internalizeAction` tests, shared by
//! `tests/served_internalize_payment.rs` (0.18.4, the toolbox 0.13.2's
//! payment check) and `tests/served_internalize_insertion.rs` (0.18.5, the
//! toolbox 0.13.3's basket insertion rules): the parties, the proven
//! AtomicBEEF, the served router over a fresh in-memory wallet, the post and
//! the row counts. Each test target declares `mod chain_tracker` at its root
//! (`src/test_support/chain_tracker.rs`), which `served` reads.

use crate::chain_tracker::TestChainTracker;

use axum::http::StatusCode;
use bsv_sdk::primitives::{PrivateKey, PublicKey};
use bsv_sdk::script::{LockingScript, UnlockingScript};
use bsv_sdk::transaction::{Beef, MerklePath, Transaction, TransactionInput, TransactionOutput};
use bsv_sdk::wallet::{Counterparty, KeyDeriver, Protocol, SecurityLevel};
use bsv_wallet_cli::server::{self, ServerConfig};
use bsv_wallet_toolbox::{
    Chain, Services, ServicesOptions, StorageSqlx, Wallet, WalletStorageWriter,
};
use serde_json::{json, Value};
use std::sync::Arc;
use tower::ServiceExt;

/// The served wallet, as `server::make_wallet_state` returns it.
pub(crate) type WalletState = Arc<Wallet<StorageSqlx, Services>>;

pub(crate) const HEIGHT: u32 = 900_000;
pub(crate) const PREFIX: &str = "bG9jay1wcmVmaXg=";
pub(crate) const SUFFIX: &str = "bG9jay1zdWZmaXg=";
/// A 25-byte P2PKH paying neither party of the tests.
pub(crate) const OTHER_HASH: &str = "dbc0a7c84983c5bf199b7b2d41b3acf0408ee5aa";

/// The recipient's root key, a sender, and the key the recipient derives
/// for the sender's remittance.
pub(crate) struct Parties {
    pub(crate) root: PrivateKey,
    pub(crate) sender: PublicKey,
    pub(crate) derived: PublicKey,
}

pub(crate) fn parties() -> Parties {
    let root = PrivateKey::random();
    let sender = PrivateKey::random().public_key();
    let derived = KeyDeriver::new(Some(root.clone()))
        .derive_public_key(
            &Protocol::new(SecurityLevel::Counterparty, "3241645161d8"),
            &format!("{PREFIX} {SUFFIX}"),
            &Counterparty::Other(sender.clone()),
            true,
        )
        .expect("the derived key");
    Parties {
        root,
        sender,
        derived,
    }
}

pub(crate) fn p2pkh(hash160: &[u8]) -> Vec<u8> {
    let mut script = vec![0x76, 0xa9, 0x14];
    script.extend_from_slice(hash160);
    script.extend_from_slice(&[0x88, 0xac]);
    script
}

pub(crate) fn other_p2pkh() -> Vec<u8> {
    p2pkh(&hex::decode(OTHER_HASH).unwrap())
}

pub(crate) fn p2pk(key: &PublicKey) -> Vec<u8> {
    let mut script = vec![0x21];
    script.extend_from_slice(&key.to_compressed());
    script.push(0xac);
    script
}

pub(crate) fn op_return() -> Vec<u8> {
    vec![0x00, 0x6a, 0x04, 0xde, 0xad, 0xbe, 0xef]
}

pub(crate) fn input(txid: &str, vout: u32) -> TransactionInput {
    let mut input = TransactionInput::new(txid.to_string(), vout);
    input.unlocking_script = Some(UnlockingScript::from_hex("00").unwrap());
    input
}

pub(crate) fn output(satoshis: u64, script: &[u8]) -> TransactionOutput {
    TransactionOutput {
        satoshis: Some(satoshis),
        locking_script: LockingScript::from_binary(script).unwrap(),
        change: false,
    }
}

/// The AtomicBEEF of a transaction with one output locked by `script`,
/// proven by a one-transaction block's path at `HEIGHT`, and its txid.
pub(crate) fn proven_beef(script: &[u8]) -> (Vec<u8>, String) {
    let mut tx = Transaction::new();
    tx.version = 1;
    tx.inputs.push(input(&"11".repeat(32), 0));
    tx.outputs.push(output(1_000, script));
    let txid = tx.id();
    let mut beef = Beef::new();
    let bump = beef.merge_bump(MerklePath::from_coinbase_txid(&txid, HEIGHT));
    beef.merge_raw_tx(tx.to_binary(), Some(bump));
    (beef.to_binary_atomic(&txid).unwrap(), txid)
}

/// The served router over a fresh wallet of `root`, whose storage checks
/// roots against a tracker that carries `proven` at `HEIGHT`.
pub(crate) async fn served(root: &PrivateKey, proven: Option<&str>) -> (WalletState, axum::Router) {
    let mut tracker = TestChainTracker::new(HEIGHT + 1);
    if let Some(txid) = proven {
        tracker.add_root(HEIGHT, txid.to_string());
    }
    served_with(root, Some(tracker)).await
}

/// The served router over a fresh wallet of `root` built as
/// `CHAINTRACKS_URL=off` builds it: no tracker on its storage, and
/// `Services` with no `chaintracks_url`. From bsv-wallet-toolbox-rs 0.13.4
/// the door reads the tracker of the storage's services when the storage
/// has none; a test sets those services.
pub(crate) async fn served_untracked(root: &PrivateKey) -> (WalletState, axum::Router) {
    served_with(root, None).await
}

async fn served_with(
    root: &PrivateKey,
    tracker: Option<TestChainTracker>,
) -> (WalletState, axum::Router) {
    let storage = StorageSqlx::in_memory().await.expect("in-memory storage");
    storage
        .migrate("served-internalize-payment", &root.public_key().to_hex())
        .await
        .expect("migrate");
    storage.make_available().await.expect("make available");
    bsv_wallet_toolbox::MonitorStorage::set_max_acceptable_proof_height(&storage, u32::MAX)
        .await
        .expect("open the proof gate");
    if let Some(tracker) = tracker {
        storage.set_chain_tracker(Arc::new(tracker)).await;
    }
    let services =
        Services::with_options(Chain::Main, ServicesOptions::mainnet()).expect("services");
    let wallet = Wallet::new(Some(root.clone()), storage, services)
        .await
        .expect("wallet");
    let state = server::make_wallet_state(wallet);
    let router = server::make_router(state.clone(), ServerConfig::default());
    (state, router)
}

pub(crate) fn payment(sender: &PublicKey) -> Value {
    json!({
        "outputIndex": 0,
        "protocol": "wallet payment",
        "paymentRemittance": {
            "derivationPrefix": PREFIX,
            "derivationSuffix": SUFFIX,
            "senderIdentityKey": sender.to_hex()
        }
    })
}

pub(crate) async fn post(app: axum::Router, tx: &[u8], output: Value) -> (StatusCode, Value) {
    let body = json!({ "tx": tx, "outputs": [output], "description": "an output to check" });
    post_to(app, "/internalizeAction", body).await
}

/// Posts `body` to `route` of the served router: the status and the JSON
/// answer.
pub(crate) async fn post_to(app: axum::Router, route: &str, body: Value) -> (StatusCode, Value) {
    let response = app
        .oneshot(
            axum::http::Request::post(route)
                .header("Origin", "http://test.local")
                .header("Content-Type", "application/json")
                .body(axum::body::Body::from(serde_json::to_vec(&body).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    (status, serde_json::from_slice(&bytes).unwrap())
}

/// The rows of every table an internalize writes.
pub(crate) async fn stored(wallet: &WalletState) -> (i64, i64, i64, i64, i64) {
    sqlx::query_as(
        "SELECT (SELECT COUNT(*) FROM transactions), (SELECT COUNT(*) FROM outputs), \
         (SELECT COUNT(*) FROM proven_txs), (SELECT COUNT(*) FROM proven_tx_reqs), \
         (SELECT COUNT(*) FROM output_baskets)",
    )
    .fetch_one(wallet.storage().pool())
    .await
    .unwrap()
}
