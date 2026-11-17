//! The served `/internalizeAction` over a wallet payment whose subject
//! sweeps more outputs of one parent than bsv-rs's link budget takes
//! (toolbox #819, the toolbox's Hosts lines for 0.14.0-beta.8): the payment
//! check reads the subject unlinked, so a payment output that does not pay
//! the derived key is refused and one that does is stored.
//!
//! Beside `tests/served_beef_doors.rs`, not in the wire corpus: a fixture
//! writes a request's `tx` one byte per line, and this subject is about
//! 120 KB, so its fixture would run past 120,000 lines.
//!
//! All vectors are synthetic: random throwaway keys, a coinbase-style BUMP
//! validated against a TestChainTracker. Nothing leaves the test: the
//! accepted subject is posted to a mock broadcaster alone.

#[path = "../src/test_support/chain_tracker.rs"]
mod chain_tracker;
use chain_tracker::TestChainTracker;

use axum::http::StatusCode;
use bsv_sdk::primitives::{PrivateKey, PublicKey};
use bsv_sdk::script::templates::P2PKH as Template;
use bsv_sdk::script::{LockingScript, SignOutputs, UnlockingScript};
use bsv_sdk::transaction::{Beef, MerklePath, Transaction, TransactionInput, TransactionOutput};
use bsv_sdk::wallet::{Counterparty, KeyDeriver, Protocol, SecurityLevel};
use bsv_wallet_cli::server::{self, ServerConfig};
use bsv_wallet_toolbox::services::mock::{
    success_post_beef_result, MockResponse, MockWalletServices,
};
use bsv_wallet_toolbox::{
    Chain, Services, ServicesOptions, StorageSqlx, Wallet, WalletServices, WalletStorageProvider,
    WalletStorageWriter,
};
use serde_json::{json, Value};
use std::sync::Arc;
use tower::ServiceExt;

const HEIGHT: u32 = 900_000;
/// More outputs of one parent than bsv-rs's link budget takes (bsv-rs
/// measures the default's reach at 702 P2PKH outputs of one parent).
const SWEPT: u32 = 800;
const PREFIX: &str = "c3dlZXA=";
const SUFFIX: &str = "cGF5";
/// A 25-byte P2PKH paying no party of the test.
const OTHER: &str = "76a914dbc0a7c84983c5bf199b7b2d41b3acf0408ee5aa88ac";

type WalletState = Arc<Wallet<StorageSqlx, Services>>;
/// The served wallet, its router and the mock its storage posts to.
type Served = (WalletState, axum::Router, Arc<MockWalletServices>);

fn p2pkh(key: &PublicKey) -> Vec<u8> {
    [&[0x76, 0xa9, 0x14][..], &key.hash160(), &[0x88, 0xac][..]].concat()
}

/// A proven parent of [`SWEPT`] outputs locked to `spender`, and a child
/// that spends every one of them, signed, into one output locked by
/// `script`. The AtomicBEEF, the child's txid and the parent's.
async fn sweep(spender: &PrivateKey, script: &[u8]) -> (Vec<u8>, String, String) {
    let mut parent = Transaction::new();
    let mut funding = TransactionInput::new("11".repeat(32), 0);
    funding.unlocking_script = Some(UnlockingScript::from_hex("00").unwrap());
    parent.add_input(funding).unwrap();
    let lock = LockingScript::from_binary(&p2pkh(&spender.public_key())).unwrap();
    for _ in 0..SWEPT {
        parent
            .add_output(TransactionOutput::new(1_000, lock.clone()))
            .unwrap();
    }
    let parent_txid = parent.id();
    parent.merkle_path = Some(MerklePath::from_coinbase_txid(&parent_txid, HEIGHT));
    let mut child = Transaction::new();
    for vout in 0..SWEPT {
        let mut input = TransactionInput::with_source_transaction(parent.clone(), vout);
        input.set_unlocking_script_template(Template::unlock(spender, SignOutputs::All, false));
        child.add_input(input).unwrap();
    }
    child
        .add_output(TransactionOutput::new(
            u64::from(SWEPT) * 1_000 - 500,
            LockingScript::from_binary(script).unwrap(),
        ))
        .unwrap();
    child.sign().await.unwrap();
    let txid = child.id();
    let mut beef = Beef::new();
    beef.merge_bump(parent.merkle_path.clone().unwrap());
    beef.merge_transaction(parent);
    beef.merge_transaction(child);
    let atomic = beef.to_binary_atomic(&txid).unwrap();
    assert!(
        matches!(
            Transaction::from_beef(&atomic, Some(&txid)),
            Err(bsv_sdk::Error::BeefResourceLimit(_))
        ),
        "the sweep is past the linking door's budget"
    );
    (atomic, txid, parent_txid)
}

/// The served router over a fresh wallet of `root`, whose tracker carries
/// `proven` at `HEIGHT` with its tip 100 above it (bsv-rs #734), and whose
/// storage posts to a mock that takes `txid`.
async fn served(root: &PrivateKey, proven: &str, txid: &str) -> Served {
    let storage = StorageSqlx::in_memory().await.expect("in-memory storage");
    storage
        .migrate("served-internalize-sweep", &root.public_key().to_hex())
        .await
        .expect("migrate");
    storage.make_available().await.expect("make available");
    bsv_wallet_toolbox::MonitorStorage::set_max_acceptable_proof_height(&storage, u32::MAX)
        .await
        .expect("open the proof gate");
    let mut tracker = TestChainTracker::new(HEIGHT + 100);
    tracker.add_root(HEIGHT, proven.to_string());
    storage.set_chain_tracker(Arc::new(tracker)).await;
    let services =
        Services::with_options(Chain::Main, ServicesOptions::mainnet()).expect("services");
    let wallet = Wallet::new(Some(root.clone()), storage, services)
        .await
        .expect("wallet");
    let broadcaster = Arc::new(
        MockWalletServices::builder()
            .post_beef_response(MockResponse::Success(vec![success_post_beef_result(
                "mock",
                &[txid],
            )]))
            .build(),
    );
    WalletStorageProvider::set_services(
        wallet.storage(),
        broadcaster.clone() as Arc<dyn WalletServices>,
    );
    let state = server::make_wallet_state(wallet);
    let config = ServerConfig::admitting(["test.local"]).expect("an originator");
    let router = server::make_router(state.clone(), config);
    (state, router, broadcaster)
}

/// `POST /internalizeAction` of `tx` as a wallet payment from `sender`.
async fn post(app: axum::Router, tx: &[u8], sender: &PublicKey) -> (StatusCode, Value) {
    let body = json!({"tx": tx, "description": "a sweep to check", "outputs": [{
        "outputIndex": 0, "protocol": "wallet payment",
        "paymentRemittance": {"derivationPrefix": PREFIX, "derivationSuffix": SUFFIX,
            "senderIdentityKey": sender.to_hex()}}]});
    let response = app
        .oneshot(
            axum::http::Request::post("/internalizeAction")
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

/// The rows of the tables an internalize writes.
async fn stored(wallet: &WalletState) -> (i64, i64) {
    sqlx::query_as("SELECT (SELECT COUNT(*) FROM transactions), (SELECT COUNT(*) FROM outputs)")
        .fetch_one(wallet.storage().pool())
        .await
        .unwrap()
}

/// The recipient's root key, the sender's key, and the key the recipient
/// derives for the sender's remittance.
fn parties() -> (PrivateKey, PrivateKey, PublicKey) {
    let (root, sender) = (PrivateKey::random(), PrivateKey::random());
    let derived = KeyDeriver::new(Some(root.clone()))
        .derive_public_key(
            &Protocol::new(SecurityLevel::Counterparty, "3241645161d8"),
            &format!("{PREFIX} {SUFFIX}"),
            &Counterparty::Other(sender.public_key()),
            true,
        )
        .expect("the derived key");
    (root, sender, derived)
}

/// A payment output in the sweep that pays the P2PKH of another key: 400
/// `VALIDATION_ERROR` (`WalletRefusal` `InvalidParameter`), the derivation
/// mismatch naming both hashes, nothing stored.
#[tokio::test]
async fn a_payment_to_another_key_in_a_sweep_past_the_link_budget_is_refused() {
    let (root, sender, derived) = parties();
    let (tx, txid, parent) = sweep(&sender, &hex::decode(OTHER).unwrap()).await;
    let (wallet, app, broadcaster) = served(&root, &parent, &txid).await;
    let before = stored(&wallet).await;
    let (status, body) = post(app, &tx, &sender.public_key()).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    let message = format!(
        "Derivation mismatch for output 0: derived {} != script {}",
        hex::encode(derived.hash160()),
        &OTHER[6..46]
    );
    assert_eq!(
        body,
        json!({"code": "VALIDATION_ERROR", "message": message})
    );
    assert_eq!(stored(&wallet).await, before, "nothing is stored");
    assert_eq!(broadcaster.call_count("post_beef"), 0, "nothing is posted");
}

/// The control: the same sweep, its one output locked by the derived key's
/// P2PKH, is admitted and its payment output stored.
#[tokio::test]
async fn a_payment_to_the_derived_key_in_a_sweep_past_the_link_budget_is_stored() {
    let (root, sender, derived) = parties();
    let (tx, txid, parent) = sweep(&sender, &p2pkh(&derived)).await;
    let (wallet, app, broadcaster) = served(&root, &parent, &txid).await;
    let before = stored(&wallet).await;
    let (status, body) = post(app, &tx, &sender.public_key()).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["accepted"], true, "{body}");
    let after = stored(&wallet).await;
    assert_eq!(after.1, before.1 + 1, "the payment output is stored");
    assert_eq!(
        broadcaster.call_count("post_beef"),
        1,
        "the unproven subject is posted to the mock alone"
    );
}
