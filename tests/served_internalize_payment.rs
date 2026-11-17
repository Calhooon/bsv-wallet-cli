//! `POST /internalizeAction` as served, for an output that carries a payment
//! remittance: the answers bsv-wallet-toolbox-rs 0.13.2 names in its
//! CHANGELOG's Hosts section, each read through this CLI's router with its
//! HTTP status, its code word and its whole message, and with nothing stored
//! on a refusal.
//!
//! The inputs take the shapes of the toolbox's own published tests:
//! `tests/internalize_payment_subject_tests.rs` (bytes that are not a BEEF,
//! an AtomicBEEF naming a subject it does not carry, the sweep of one parent
//! of `tests/common/mod.rs`) and `tests/internalize_payment_lock_tests.rs`
//! (the scripts that are not the derived key's P2PKH, the basket insertion
//! that carries a payment remittance, the basket insertion that carries
//! none). Where the toolbox's test spends a parent the BEEF proves, the
//! subject here is itself proven (the shape of the #157 tests), so an
//! accepted request posts nothing: no broadcaster and no header service is
//! asked. The one accepted sweep, whose subject is not proven, is posted to
//! a mock broadcaster the test puts in the storage's services.
//!
//! All vectors are synthetic: a random throwaway key, a coinbase-style BUMP
//! checked against a TestChainTracker.

#[path = "../src/test_support/chain_tracker.rs"]
mod chain_tracker;
use chain_tracker::TestChainTracker;

use axum::http::StatusCode;
use bsv_sdk::primitives::{PrivateKey, PublicKey};
use bsv_sdk::script::{LockingScript, UnlockingScript};
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

/// The served wallet, as `server::make_wallet_state` returns it.
type WalletState = Arc<Wallet<StorageSqlx, Services>>;

const HEIGHT: u32 = 900_000;
const PREFIX: &str = "bG9jay1wcmVmaXg=";
const SUFFIX: &str = "bG9jay1zdWZmaXg=";
/// A 25-byte P2PKH paying neither party of the tests.
const OTHER_HASH: &str = "dbc0a7c84983c5bf199b7b2d41b3acf0408ee5aa";
/// Outputs of the one parent the sweep spends, past bsv-rs 0.8.11's default
/// link budget (the toolbox's `SWEPT`).
const SWEPT: u32 = 800;

const NOT_BRC29: &str =
    "wallet error: The paymentRemittance parameter must be locked by script conforming to BRC-29";
const NO_SUBJECT: &str =
    "wallet error: Cannot read the BEEF's subject transaction to check payment outputs: ";

/// The recipient's root key, a sender, and the key the recipient derives
/// for the sender's remittance.
struct Parties {
    root: PrivateKey,
    sender: PublicKey,
    derived: PublicKey,
}

fn parties() -> Parties {
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

fn p2pkh(hash160: &[u8]) -> Vec<u8> {
    let mut script = vec![0x76, 0xa9, 0x14];
    script.extend_from_slice(hash160);
    script.extend_from_slice(&[0x88, 0xac]);
    script
}

fn other_p2pkh() -> Vec<u8> {
    p2pkh(&hex::decode(OTHER_HASH).unwrap())
}

fn p2pk(key: &PublicKey) -> Vec<u8> {
    let mut script = vec![0x21];
    script.extend_from_slice(&key.to_compressed());
    script.push(0xac);
    script
}

fn op_return() -> Vec<u8> {
    vec![0x00, 0x6a, 0x04, 0xde, 0xad, 0xbe, 0xef]
}

fn input(txid: &str, vout: u32) -> TransactionInput {
    let mut input = TransactionInput::new(txid.to_string(), vout);
    input.unlocking_script = Some(UnlockingScript::from_hex("00").unwrap());
    input
}

fn output(satoshis: u64, script: &[u8]) -> TransactionOutput {
    TransactionOutput {
        satoshis: Some(satoshis),
        locking_script: LockingScript::from_binary(script).unwrap(),
        change: false,
    }
}

/// The AtomicBEEF of a transaction with one output locked by `script`,
/// proven by a one-transaction block's path at `HEIGHT`, and its txid.
fn proven_beef(script: &[u8]) -> (Vec<u8>, String) {
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

/// The toolbox's `sweep_beef`: a proven parent of `SWEPT` outputs and a
/// child sweeping every one of them into one output locked by `script`.
/// Returns the AtomicBEEF, the child's txid and the proven parent's.
fn sweep_beef(script: &[u8]) -> (Vec<u8>, String, String) {
    let mut parent = Transaction::new();
    parent.inputs.push(input(&"11".repeat(32), 0));
    for _ in 0..SWEPT {
        parent.outputs.push(output(1_000, &other_p2pkh()));
    }
    let parent_txid = parent.id();
    let mut child = Transaction::new();
    child.version = 1;
    for vout in 0..SWEPT {
        child.inputs.push(input(&parent_txid, vout));
    }
    child
        .outputs
        .push(output(u64::from(SWEPT) * 1_000 - 500, script));
    let txid = child.id();
    let mut beef = Beef::new();
    let bump = beef.merge_bump(MerklePath::from_coinbase_txid(&parent_txid, HEIGHT));
    beef.merge_raw_tx(parent.to_binary(), Some(bump));
    beef.merge_raw_tx(child.to_binary(), None);
    (beef.to_binary_atomic(&txid).unwrap(), txid, parent_txid)
}

/// The served router over a fresh wallet of `root`, whose storage checks
/// roots against a tracker that carries `proven` at `HEIGHT`.
async fn served(root: &PrivateKey, proven: Option<&str>) -> (WalletState, axum::Router) {
    let storage = StorageSqlx::in_memory().await.expect("in-memory storage");
    storage
        .migrate("served-internalize-payment", &root.public_key().to_hex())
        .await
        .expect("migrate");
    storage.make_available().await.expect("make available");
    bsv_wallet_toolbox::MonitorStorage::set_max_acceptable_proof_height(&storage, u32::MAX)
        .await
        .expect("open the proof gate");
    let mut tracker = TestChainTracker::new(HEIGHT + 1);
    if let Some(txid) = proven {
        tracker.add_root(HEIGHT, txid.to_string());
    }
    storage.set_chain_tracker(Arc::new(tracker)).await;
    let services =
        Services::with_options(Chain::Main, ServicesOptions::mainnet()).expect("services");
    let wallet = Wallet::new(Some(root.clone()), storage, services)
        .await
        .expect("wallet");
    let state = server::make_wallet_state(wallet);
    let router = server::make_router(state.clone(), ServerConfig::default());
    (state, router)
}

fn payment(sender: &PublicKey) -> Value {
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

/// A basket insertion that also carries a payment remittance.
fn insertion_with_payment(sender: &PublicKey) -> Value {
    let mut output = payment(sender);
    output["protocol"] = json!("basket insertion");
    output["insertionRemittance"] = json!({ "basket": "tokens" });
    output
}

async fn post(app: axum::Router, tx: &[u8], output: Value) -> (StatusCode, Value) {
    let body = json!({ "tx": tx, "outputs": [output], "description": "an output to check" });
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

/// The rows of every table an internalize writes.
async fn stored(wallet: &WalletState) -> (i64, i64, i64, i64, i64) {
    sqlx::query_as(
        "SELECT (SELECT COUNT(*) FROM transactions), (SELECT COUNT(*) FROM outputs), \
         (SELECT COUNT(*) FROM proven_txs), (SELECT COUNT(*) FROM proven_tx_reqs), \
         (SELECT COUNT(*) FROM output_baskets)",
    )
    .fetch_one(wallet.storage().pool())
    .await
    .unwrap()
}

/// Posts `output` of `tx` to a fresh served wallet of `who` whose tracker
/// carries the root `txid` at `HEIGHT`, and asserts the refusal whole: 400, the code word
/// `code`, `message`, and no row written.
async fn assert_refused(
    who: &Parties,
    tx: &[u8],
    txid: &str,
    output: Value,
    code: &str,
    message: &str,
) {
    let (wallet, app) = served(&who.root, Some(txid)).await;
    let before = stored(&wallet).await;
    let (status, body) = post(app, tx, output).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert_eq!(body, json!({ "code": code, "message": message }));
    assert_eq!(
        stored(&wallet).await,
        before,
        "{message}: nothing is stored"
    );
}

fn mismatch(who: &Parties) -> String {
    format!(
        "wallet error: Derivation mismatch for output 0: derived {} != script {OTHER_HASH}",
        hex::encode(who.derived.hash160())
    )
}

/// The control: a payment output locked by the derived key's P2PKH is
/// accepted and stored, so the wallet under these tests stores what it is
/// not refusing.
#[tokio::test]
async fn a_payment_locked_by_the_derived_p2pkh_is_accepted_and_stored() {
    let who = parties();
    let (tx, txid) = proven_beef(&p2pkh(&who.derived.hash160()));
    let (wallet, app) = served(&who.root, Some(&txid)).await;
    let before = stored(&wallet).await;
    let (status, body) = post(app, &tx, payment(&who.sender)).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["accepted"], true, "{body}");
    let after = stored(&wallet).await;
    assert_eq!(after.1, before.1 + 1, "the payment output is stored");
}

/// Bytes that are not a BEEF, with a payment output: 400 `INVALID_BEEF`
/// (the code word 0.18.3 served for these bytes), the subject cannot be
/// read, nothing stored.
#[tokio::test]
async fn a_payment_in_bytes_that_are_not_a_beef_is_refused_before_storage() {
    let who = parties();
    assert_refused(
        &who,
        &[0xde, 0xad, 0xbe, 0xef],
        &"00".repeat(32),
        payment(&who.sender),
        "INVALID_BEEF",
        &format!("{NO_SUBJECT}BEEF error: Invalid BEEF version: 0xEFBEADDE"),
    )
    .await;
}

/// An AtomicBEEF naming a subject it does not carry, with a payment output:
/// 400 `VALIDATION_ERROR` (the code word 0.18.3 served for this BEEF), the
/// subject cannot be read, nothing stored.
#[tokio::test]
async fn a_payment_in_a_beef_naming_a_subject_it_does_not_carry_is_refused_before_storage() {
    let who = parties();
    let (mut tx, txid) = proven_beef(&p2pkh(&who.derived.hash160()));
    tx[4..36].copy_from_slice(&[0x5a; 32]);
    assert_refused(
        &who,
        &tx,
        &txid,
        payment(&who.sender),
        "VALIDATION_ERROR",
        &format!(
            "{NO_SUBJECT}transaction error: Transaction {} not found in BEEF",
            "5a".repeat(32)
        ),
    )
    .await;
}

/// A payment output that pays the P2PKH of another key: 400 `ERROR`, the
/// derivation mismatch naming both hashes, nothing stored. Read in a proven
/// subject and in a subject that sweeps more outputs of one parent than
/// bsv-rs's link budget takes.
#[tokio::test]
async fn a_payment_to_the_p2pkh_of_another_key_is_refused_whatever_the_ancestry() {
    let who = parties();
    let (tx, txid) = proven_beef(&other_p2pkh());
    assert_refused(
        &who,
        &tx,
        &txid,
        payment(&who.sender),
        "ERROR",
        &mismatch(&who),
    )
    .await;

    // The tracker carries the root of the sweep's proven parent.
    let (tx, txid, parent_txid) = sweep_beef(&other_p2pkh());
    assert!(
        matches!(
            Transaction::from_beef(&tx, Some(&txid)),
            Err(bsv_sdk::Error::BeefResourceLimit(_))
        ),
        "the sweep is past the linking door's budget"
    );
    assert_refused(
        &who,
        &tx,
        &parent_txid,
        payment(&who.sender),
        "ERROR",
        &mismatch(&who),
    )
    .await;
}

/// The control beside the sweep's refusal: the same sweep, past bsv-rs's
/// link budget, whose one output is locked by the derived key's P2PKH, is
/// accepted and its payment output stored (the toolbox's Hosts row "a
/// payment output that does pay the derived key is stored as before"). So
/// storage takes this BEEF, and the mismatch above is refused for its
/// script alone. The sweep's subject is not proven, so the toolbox posts it
/// once accepted: the storage's services are a mock broadcaster here, and
/// nothing leaves the test.
#[tokio::test]
async fn a_payment_to_the_derived_p2pkh_in_a_sweep_past_the_link_budget_is_stored() {
    let who = parties();
    // The tracker carries the root of the sweep's proven parent.
    let (tx, txid, parent_txid) = sweep_beef(&p2pkh(&who.derived.hash160()));
    assert!(
        matches!(
            Transaction::from_beef(&tx, Some(&txid)),
            Err(bsv_sdk::Error::BeefResourceLimit(_))
        ),
        "the sweep is past the linking door's budget"
    );
    let (wallet, app) = served(&who.root, Some(&parent_txid)).await;
    let broadcaster = Arc::new(
        MockWalletServices::builder()
            .post_beef_response(MockResponse::Success(vec![success_post_beef_result(
                "mock",
                &[txid.as_str()],
            )]))
            .build(),
    );
    WalletStorageProvider::set_services(
        wallet.storage(),
        broadcaster.clone() as Arc<dyn WalletServices>,
    );
    let before = stored(&wallet).await;
    let (status, body) = post(app, &tx, payment(&who.sender)).await;
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

/// A payment output whose script is not the derived key's P2PKH, in each
/// shape the toolbox's test names: 400 `VALIDATION_ERROR`, the reference's sentence,
/// nothing stored.
#[tokio::test]
async fn a_payment_not_locked_by_the_derived_p2pkh_is_refused_in_every_shape() {
    let who = parties();
    let mut one_byte_more = p2pkh(&who.derived.hash160());
    one_byte_more.push(0x51);
    let shapes: [(&str, Vec<u8>); 4] = [
        ("p2pk of the derived key", p2pk(&who.derived)),
        ("op_return", op_return()),
        ("p2pkh of the derived key and one byte more", one_byte_more),
        ("empty", Vec::new()),
    ];
    for (name, script) in shapes {
        let (tx, txid) = proven_beef(&script);
        let (wallet, app) = served(&who.root, Some(&txid)).await;
        let before = stored(&wallet).await;
        let (status, body) = post(app, &tx, payment(&who.sender)).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{name}: {body}");
        assert_eq!(
            body,
            json!({ "code": "VALIDATION_ERROR", "message": NOT_BRC29 }),
            "{name}"
        );
        assert_eq!(stored(&wallet).await, before, "{name}: nothing is stored");
    }
}

/// A basket insertion that also carries a payment remittance is held to the
/// same rule and answers the same two texts; nothing stored.
#[tokio::test]
async fn a_basket_insertion_carrying_a_payment_remittance_is_held_to_the_rule() {
    let who = parties();
    let (tx, txid) = proven_beef(&op_return());
    assert_refused(
        &who,
        &tx,
        &txid,
        insertion_with_payment(&who.sender),
        "VALIDATION_ERROR",
        NOT_BRC29,
    )
    .await;

    let (tx, txid) = proven_beef(&other_p2pkh());
    assert_refused(
        &who,
        &tx,
        &txid,
        insertion_with_payment(&who.sender),
        "ERROR",
        &mismatch(&who),
    )
    .await;
}

/// A basket insertion with no payment remittance is not held to the rule:
/// a script that is not a P2PKH is accepted and stored.
#[tokio::test]
async fn a_basket_insertion_with_no_payment_remittance_is_stored() {
    let who = parties();
    let (tx, txid) = proven_beef(&p2pk(&who.derived));
    let (wallet, app) = served(&who.root, Some(&txid)).await;
    let before = stored(&wallet).await;
    let insertion = json!({
        "outputIndex": 0,
        "protocol": "basket insertion",
        "insertionRemittance": { "basket": "tokens" }
    });
    let (status, body) = post(app, &tx, insertion).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["accepted"], true, "{body}");
    let after = stored(&wallet).await;
    assert_eq!(after.1, before.1 + 1, "the inserted output is stored");
}
