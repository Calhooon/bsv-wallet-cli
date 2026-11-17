//! `POST /internalizeAction` as served under bsv-wallet-toolbox-rs
//! 0.14.0-beta.8.1 (the `internalizeAction` door read whole against the
//! reference): the port of the stable line's 0.18.6
//! (`tests/served_internalize_door.rs` on `main`), in this line's way. The
//! door's refusals a caller can reach are rows 257 to 262 of the wire
//! corpus; this file holds what a fixture of the `internalize` world cannot:
//! the answer with no chain tracker (no world runs without the header stub),
//! a payment merged over a spent output, the two "rows written before"
//! found and taken out, and the restores of a row an earlier release
//! reclassified, each run through to a signed spend whose unlocking script
//! is run against the coin's P2PKH.
//!
//! The shapes are the toolbox's published tests for this release
//! (`tests/internalize_restore_spend_tests.rs`, `tests/common/insertion.rs`
//! and `tests/common/internalize_payment.rs`): the subject is unproven and
//! spends five outputs of a parent proven after Genesis; the spend is
//! `Wallet::create_action` with `noSend` and the `underReceivedRule` label,
//! as the toolbox's tests make it, since a restored row's purpose is
//! `receive` and its transaction unproven. The served `createAction` is not
//! the spend here: its spend gate reads the tracker's word and the header
//! service, and the door under test is `internalizeAction`.
//!
//! This host configures no permissions manager (`git grep -n -i
//! permissions_manager -- src mcp/src` finds none), so the Hosts rows of a
//! manager are not reachable here and have no test.
//!
//! All vectors are synthetic: random throwaway keys, a coinbase-style BUMP
//! checked against a TestChainTracker, a mock broadcaster. Nothing is sent.

#[path = "../src/test_support/chain_tracker.rs"]
mod chain_tracker;
#[path = "../src/test_support/no_tracker.rs"]
#[allow(dead_code)]
mod no_tracker;
use chain_tracker::TestChainTracker;
use no_tracker::NoTracker;

use axum::http::StatusCode;
use bsv_sdk::primitives::bsv::sighash::{parse_transaction, TxSighashCache};
use bsv_sdk::primitives::{PrivateKey, PublicKey};
use bsv_sdk::script::{
    LockingScript, ProtocolEra, ScriptFlags, Spend, TxSpendParams, UnlockingScript,
};
use bsv_sdk::transaction::{Beef, MerklePath, Transaction, TransactionInput, TransactionOutput};
use bsv_sdk::wallet::{
    Counterparty, CreateActionArgs, CreateActionOptions, CreateActionOutput, KeyDeriver, Protocol,
    SecurityLevel, WalletInterface,
};
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

const PREFIX: &str = "aW5zZXJ0LXByZWZpeA==";
const SUFFIX: &str = "aW5zZXJ0LXN1ZmZpeA==";
/// The derivation `fund`, `receive`, `sync`, `split` and `gift-claim` write
/// with the anyone key as the sender (`src/brc29.rs`).
const CLI_PREFIX: &str = "SfKxPIJNgdI=";
const CLI_SUFFIX: &str = "NaGLC6fMH50=";
/// A 25-byte P2PKH paying neither party of the tests.
const OTHER_HASH: &str = "dbc0a7c84983c5bf199b7b2d41b3acf0408ee5aa";
const SPEND: &str = "a spend of the restored coin";
/// The height of the subject's parent's proof: after Genesis, and at least
/// 100 below the tracker's tip (bsv-rs #734).
const PARENT_HEIGHT: u32 = ProtocolEra::MAINNET_GENESIS_HEIGHT + 101;

// ---------------------------------------------------------------------------
// The served wallet
// ---------------------------------------------------------------------------

/// The mock broadcaster, accepting every transaction posted.
fn accepting() -> MockWalletServices {
    MockWalletServices::builder()
        .post_beef_response(MockResponse::Success(vec![success_post_beef_result(
            "arc",
            &[],
        )]))
        .build()
}

/// The served router over a fresh wallet of `root` whose storage checks
/// roots against a tracker carrying `proven` as the root at
/// [`PARENT_HEIGHT`], and posts to a mock that accepts every transaction,
/// read back by its call count.
async fn served(
    root: &PrivateKey,
    proven: Option<&str>,
) -> (WalletState, axum::Router, Arc<MockWalletServices>) {
    let posts = Arc::new(accepting());
    let (wallet, router) = served_with(root, proven, posts.clone()).await;
    (wallet, router, posts)
}

/// The served router over a fresh wallet of `root` built as
/// `CHAINTRACKS_URL=off` builds it: no tracker on the storage, and
/// services with none (`Services` with no `chaintracks_url` for the
/// wallet; for the storage, whose services the door reads when it has no
/// tracker and which every post goes through, the same mock behind
/// [`NoTracker`], so the posts are counted).
async fn served_untracked(root: &PrivateKey) -> (WalletState, axum::Router, Arc<NoTracker>) {
    let posts = Arc::new(NoTracker(accepting()));
    let (wallet, router) = served_with(root, None, posts.clone()).await;
    (wallet, router, posts)
}

async fn served_with(
    root: &PrivateKey,
    proven: Option<&str>,
    posts: Arc<dyn WalletServices>,
) -> (WalletState, axum::Router) {
    let storage = StorageSqlx::in_memory().await.expect("in-memory storage");
    storage
        .migrate("served-internalize-door", &root.public_key().to_hex())
        .await
        .expect("migrate");
    storage.make_available().await.expect("make available");
    bsv_wallet_toolbox::MonitorStorage::set_max_acceptable_proof_height(&storage, u32::MAX)
        .await
        .expect("open the proof gate");
    if let Some(txid) = proven {
        let mut tracker = TestChainTracker::new(900_000);
        tracker.add_root(PARENT_HEIGHT, txid.to_string());
        storage.set_chain_tracker(Arc::new(tracker)).await;
    }
    let services =
        Services::with_options(Chain::Main, ServicesOptions::mainnet()).expect("services");
    assert!(services.chaintracks.is_none(), "no header service");
    let wallet = Wallet::new(Some(root.clone()), storage, services)
        .await
        .expect("wallet");
    let state = server::make_wallet_state(wallet);
    WalletStorageProvider::set_services(state.storage(), posts);
    let config = ServerConfig::admitting(["test.local"]).expect("an originator");
    let router = server::make_router(state.clone(), config);
    (state, router)
}

/// Posts `body` to `route` of the served router: the status and the JSON
/// answer.
async fn post_to(app: &axum::Router, route: &str, body: Value) -> (StatusCode, Value) {
    let response = app
        .clone()
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

/// `POST /internalizeAction` of `tx` with `outputs`.
async fn internalize(app: &axum::Router, tx: &[u8], outputs: Vec<Value>) -> (StatusCode, Value) {
    post_to(
        app,
        "/internalizeAction",
        json!({ "tx": tx, "outputs": outputs, "description": "an output to check" }),
    )
    .await
}

/// A wallet payment of output `vout` naming `prefix`, `suffix` and `sender`
/// as given (any string, the empty one included).
fn payment_of(vout: u32, prefix: &str, suffix: &str, sender: &str) -> Value {
    json!({
        "outputIndex": vout,
        "protocol": "wallet payment",
        "paymentRemittance": {
            "derivationPrefix": prefix,
            "derivationSuffix": suffix,
            "senderIdentityKey": sender
        }
    })
}

fn insertion(vout: u32, basket: &str) -> Value {
    json!({ "outputIndex": vout, "protocol": "basket insertion",
        "insertionRemittance": { "basket": basket } })
}

fn assert_refused(status: StatusCode, body: &Value, code: &str, message: &str) {
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert_eq!(body, &json!({ "code": code, "message": message }));
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

async fn balance(wallet: &WalletState) -> u64 {
    wallet.balance().await.expect("the balance").total
}

// ---------------------------------------------------------------------------
// The transactions
// ---------------------------------------------------------------------------

fn p2pkh(key: &PublicKey) -> Vec<u8> {
    let mut script = vec![0x76, 0xa9, 0x14];
    script.extend_from_slice(&key.hash160());
    script.extend_from_slice(&[0x88, 0xac]);
    script
}

fn other_lock() -> Vec<u8> {
    hex::decode(format!("76a914{OTHER_HASH}88ac")).unwrap()
}

/// The key `root` derives for a remittance from `sender`.
fn derived(root: &PrivateKey, sender: &PublicKey, prefix: &str, suffix: &str) -> PublicKey {
    KeyDeriver::new(Some(root.clone()))
        .derive_public_key(
            &Protocol::new(SecurityLevel::Counterparty, "3241645161d8"),
            &format!("{prefix} {suffix}"),
            &Counterparty::Other(sender.clone()),
            true,
        )
        .expect("the derived key")
}

fn input(txid: &str, vout: u32) -> TransactionInput {
    let mut input = TransactionInput::new(txid.to_string(), vout);
    input.unlocking_script = Some(UnlockingScript::from_hex("00").unwrap());
    input
}

/// The toolbox's `beef_paying`: one unproven output of 4500 satoshis locked
/// by `lock`, spending five outputs of one parent proven at
/// [`PARENT_HEIGHT`], each locked by a script `00` spends. Its AtomicBEEF,
/// its txid and the parent's.
fn beef_paying(lock: &[u8]) -> (Vec<u8>, String, String) {
    let anyone = LockingScript::from_hex(&format!("14{}757551", "00".repeat(20))).unwrap();
    let mut parent = Transaction::new();
    parent.inputs_mut().push(input(&"11".repeat(32), 0));
    for _ in 0..5 {
        parent.outputs_mut().push(TransactionOutput {
            satoshis: Some(1_000),
            locking_script: anyone.clone(),
            change: false,
        });
    }
    let parent_txid = parent.id();
    let mut child = Transaction::new();
    child.set_version(1);
    for vout in 0..5 {
        child.inputs_mut().push(input(&parent_txid, vout));
    }
    child.outputs_mut().push(TransactionOutput {
        satoshis: Some(4_500),
        locking_script: LockingScript::from_binary(lock).unwrap(),
        change: false,
    });
    let txid = child.id();
    let mut beef = Beef::new();
    let bump = beef.merge_bump(MerklePath::from_coinbase_txid(&parent_txid, PARENT_HEIGHT));
    beef.merge_raw_tx(parent.to_binary(), Some(bump)).unwrap();
    beef.merge_raw_tx(child.to_binary(), None).unwrap();
    (beef.to_binary_atomic(&txid).unwrap(), txid, parent_txid)
}

// ---------------------------------------------------------------------------
// The rows
// ---------------------------------------------------------------------------

/// The stored row of `txid.vout`: basket, type, change, derivation prefix,
/// suffix and sender.
type Row = (
    Option<String>,
    String,
    i64,
    Option<String>,
    Option<String>,
    Option<String>,
);

async fn row(wallet: &WalletState, txid: &str, vout: u32) -> Row {
    sqlx::query_as(
        "SELECT b.name, o.type, o.change, o.derivation_prefix, o.derivation_suffix, \
         o.sender_identity_key FROM outputs o LEFT JOIN output_baskets b \
         ON b.basket_id = o.basket_id WHERE o.txid = ? AND o.vout = ?",
    )
    .bind(txid)
    .bind(vout)
    .fetch_one(wallet.storage().writer())
    .await
    .unwrap()
}

/// The row `txid.vout` as a basket insertion into "tokens" left it before
/// the toolbox's rules (the toolbox test's `reclassify`): `custom`, not
/// change, its custom instructions, derivation and sender NULL.
async fn reclassify(wallet: &WalletState, txid: &str, vout: u32) {
    let user_id: i64 =
        sqlx::query_scalar("SELECT user_id FROM outputs WHERE txid = ? AND vout = ?")
            .bind(txid)
            .bind(vout)
            .fetch_one(wallet.storage().writer())
            .await
            .unwrap();
    let tokens = sqlx::query(
        "INSERT INTO output_baskets (user_id, name, number_of_desired_utxos, \
         minimum_desired_utxo_value) VALUES (?, 'tokens', 6, 10000)",
    )
    .bind(user_id)
    .execute(wallet.storage().writer())
    .await
    .unwrap()
    .last_insert_rowid();
    sqlx::query(
        "UPDATE outputs SET basket_id = ?, type = 'custom', change = 0, \
         custom_instructions = NULL, derivation_prefix = NULL, derivation_suffix = NULL, \
         sender_identity_key = NULL WHERE txid = ? AND vout = ?",
    )
    .bind(tokens)
    .bind(txid)
    .bind(vout)
    .execute(wallet.storage().writer())
    .await
    .unwrap();
}

async fn spent(wallet: &WalletState, txid: &str) -> (bool, Option<i64>) {
    sqlx::query_as("SELECT spendable, spent_by FROM outputs WHERE txid = ? AND vout = 0")
        .bind(txid)
        .fetch_one(wallet.storage().writer())
        .await
        .unwrap()
}

async fn relinquish(app: &axum::Router, txid: &str, vout: u32) {
    let (status, body) = post_to(
        app,
        "/relinquishOutput",
        json!({ "basket": "default", "output": format!("{txid}.{vout}") }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["relinquished"], true, "{body}");
}

// ---------------------------------------------------------------------------
// The spend
// ---------------------------------------------------------------------------

/// A `createAction` of 1000 satoshis to another key with the
/// `underReceivedRule` label, signed and not sent (the toolbox test's
/// `spend_args`): the signed transaction, or the refusal's text.
async fn spend(wallet: &WalletState, labelled: bool) -> Result<Transaction, String> {
    let made = wallet
        .create_action(
            CreateActionArgs {
                description: SPEND.to_string(),
                input_beef: None,
                inputs: None,
                outputs: Some(vec![CreateActionOutput {
                    locking_script: other_lock(),
                    satoshis: 1_000,
                    output_description: "a payment".to_string(),
                    basket: None,
                    custom_instructions: None,
                    tags: None,
                }]),
                lock_time: None,
                version: None,
                labels: labelled.then(|| vec!["underReceivedRule".to_string()]),
                options: Some(CreateActionOptions {
                    no_send: Some(true),
                    sign_and_process: Some(true),
                    ..Default::default()
                }),
            },
            Some("test.local"),
        )
        .await
        .map_err(|e| e.to_string())?;
    let bytes = made.tx.expect("the signed transaction");
    Ok(Transaction::from_atomic_beef(&bytes)
        .or_else(|_| Transaction::from_binary(&bytes))
        .expect("the signed transaction"))
}

/// The input of `tx` spending `txid.vout`, its unlocking script run against
/// the coin's stored P2PKH under the node's post-Chronicle word (the
/// toolbox test's `spends_the_coin`).
async fn spends_the_coin(wallet: &WalletState, tx: &Transaction, txid: &str, vout: u32) {
    let (script, satoshis): (Vec<u8>, i64) =
        sqlx::query_as("SELECT locking_script, satoshis FROM outputs WHERE txid = ? AND vout = ?")
            .bind(txid)
            .bind(vout)
            .fetch_one(wallet.storage().writer())
            .await
            .unwrap();
    assert_eq!(script.len(), 25, "a P2PKH lock");
    assert_eq!(tx.inputs().len(), 1, "the coin alone funds the spend");
    let spent_input = &tx.inputs()[0];
    assert_eq!(
        (
            spent_input.get_source_txid().unwrap(),
            spent_input.source_output_index
        ),
        (txid.to_string(), vout),
        "the spend selects the coin"
    );
    let raw = parse_transaction(&tx.to_binary()).unwrap();
    let cache = Arc::new(TxSighashCache::new(
        raw.version,
        raw.inputs,
        raw.outputs,
        raw.locktime,
    ));
    let mut check = Spend::with_transaction(TxSpendParams {
        transaction: cache,
        input_index: 0,
        source_satoshis: u64::try_from(satoshis).unwrap(),
        locking_script: LockingScript::from_binary(&script).unwrap(),
        unlocking_script: spent_input
            .unlocking_script
            .clone()
            .expect("an unlocking script"),
        memory_limit: None,
        work_limit: None,
    })
    .unwrap();
    check.set_flags(ScriptFlags::standard(ProtocolEra::PostChronicle));
    assert_eq!(check.validate().map_err(|e| e.message), Ok(true));
}

/// A served wallet funded by a received payment of 4500 satoshis, which
/// then makes an action of 1000 satoshis to another key, not sent: its
/// change (3,000-odd satoshis) is the wallet's own, the funding spent. The
/// wallet, the router, the action's AtomicBEEF, its txid and the change's
/// index; the action moved to `unproven`, as its broadcast would leave it.
async fn own_change(
    root: &PrivateKey,
) -> (
    WalletState,
    axum::Router,
    Arc<MockWalletServices>,
    Vec<u8>,
    String,
    u32,
) {
    let sender = PrivateKey::random().public_key();
    let lock = p2pkh(&derived(root, &sender, PREFIX, SUFFIX));
    let (funding, funding_txid, parent) = beef_paying(&lock);
    let (wallet, app, posts) = served(root, Some(&parent)).await;
    let (status, body) = internalize(
        &app,
        &funding,
        vec![payment_of(0, PREFIX, SUFFIX, &sender.to_hex())],
    )
    .await;
    assert_eq!(status, StatusCode::OK, "the funding: {body}");
    assert_eq!(balance(&wallet).await, 4_500);
    let made = spend(&wallet, true).await.expect("the action");
    let txid = made.id();
    sqlx::query("UPDATE transactions SET status = 'unproven' WHERE txid = ?")
        .bind(&txid)
        .execute(wallet.storage().writer())
        .await
        .unwrap();
    let change: i64 = sqlx::query_scalar("SELECT vout FROM outputs WHERE txid = ? AND change = 1")
        .bind(&txid)
        .fetch_one(wallet.storage().writer())
        .await
        .expect("the change output");
    let mut beef = Beef::from_binary(&funding).expect("the funding's BEEF");
    beef.merge_raw_tx(made.to_binary(), None).unwrap();
    let atomic = beef.to_binary_atomic(&txid).unwrap();
    assert_ne!(funding_txid, txid);
    (
        wallet,
        app,
        posts,
        atomic,
        txid,
        u32::try_from(change).unwrap(),
    )
}

// ---------------------------------------------------------------------------
// The answers
// ---------------------------------------------------------------------------

/// No chain tracker on the storage and none on its services, as
/// `CHAINTRACKS_URL=off` builds them: every `internalizeAction` is refused,
/// a wallet payment and a basket insertion alike, 502 `SERVICE_ERROR`,
/// nothing written and nothing posted. The control: the same payment to a wallet with a
/// tracker is stored and its unproven subject posted once.
#[tokio::test]
async fn with_no_chain_tracker_every_internalize_is_refused() {
    let root = PrivateKey::random();
    let sender = PrivateKey::random().public_key();
    let lock = p2pkh(&derived(&root, &sender, PREFIX, SUFFIX));
    let (tx, _, parent) = beef_paying(&lock);
    let (wallet, app, posts) = served_untracked(&root).await;
    let before = stored(&wallet).await;
    for output in [
        payment_of(0, PREFIX, SUFFIX, &sender.to_hex()),
        insertion(0, "tokens"),
    ] {
        let (status, body) = internalize(&app, &tx, vec![output]).await;
        // The toolbox's hold with no header source (#509): a header fault,
        // never a verdict about the payment, served by its class as 502
        // `SERVICE_ERROR` (the stable line's 0.13.4 answers 400
        // `VALIDATION_ERROR` with its own text).
        assert_eq!(status, StatusCode::BAD_GATEWAY, "{body}");
        assert_eq!(
            body,
            json!({
                "code": "SERVICE_ERROR",
                "message": "wallet error: Service error: ChainTracker verification failed: the \
                    merkle proofs of a BEEF the wallet did not sign need headers, and no header \
                    service is configured (no chain tracker, no chaintracks_url)"
            })
        );
    }
    assert_eq!(stored(&wallet).await, before, "nothing written");
    assert_eq!(posts.call_count("post_beef"), 0, "nothing posted");

    let (wallet, app, posts) = served(&root, Some(&parent)).await;
    let (status, body) = internalize(
        &app,
        &tx,
        vec![payment_of(0, PREFIX, SUFFIX, &sender.to_hex())],
    )
    .await;
    assert_eq!(status, StatusCode::OK, "the control: {body}");
    assert_eq!(balance(&wallet).await, 4_500);
    assert_eq!(posts.call_count("post_beef"), 1, "stored and posted");
}

/// A wallet payment merged over an output the store holds as spent leaves
/// it spent (`spendable` and `spent_by` as the row holds them). The
/// control: the unspent row named again stays spendable. The opening
/// internalize posts the unproven subject once; no merge posts again.
#[tokio::test]
async fn a_payment_merged_over_a_spent_output_leaves_it_spent() {
    let root = PrivateKey::random();
    let sender = PrivateKey::random().public_key();
    let lock = p2pkh(&derived(&root, &sender, PREFIX, SUFFIX));
    let (tx, txid, parent) = beef_paying(&lock);
    let (wallet, app, posts) = served(&root, Some(&parent)).await;
    let pay = || vec![payment_of(0, PREFIX, SUFFIX, &sender.to_hex())];
    let (status, body) = internalize(&app, &tx, pay()).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(posts.call_count("post_beef"), 1, "stored and posted");
    let (status, body) = internalize(&app, &tx, pay()).await;
    assert_eq!(status, StatusCode::OK, "the control: {body}");
    assert_eq!(spent(&wallet, &txid).await, (true, None));

    sqlx::query(
        "UPDATE outputs SET spendable = 0, spent_by = transaction_id WHERE txid = ? AND vout = 0",
    )
    .bind(&txid)
    .execute(wallet.storage().writer())
    .await
    .unwrap();
    let held = spent(&wallet, &txid).await;
    assert!(!held.0 && held.1.is_some());
    let (status, body) = internalize(&app, &tx, pay()).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(spent(&wallet, &txid).await, held, "left spent");
    assert_eq!(balance(&wallet).await, 0);
    assert_eq!(posts.call_count("post_beef"), 1, "no merge posts");
}

// ---------------------------------------------------------------------------
// The rows written before
// ---------------------------------------------------------------------------

/// A spent output an earlier merge made spendable (`spendable = 1` with
/// `spent_by` set, reproduced by SQL): the balance counts it, the
/// toolbox's query finds it, and `POST /relinquishOutput` on `default`
/// takes it out of the balance (the toolbox's Hosts: 4500 before, 0 after).
#[tokio::test]
async fn a_spent_row_an_earlier_merge_made_spendable_is_found_and_relinquished() {
    let root = PrivateKey::random();
    let sender = PrivateKey::random().public_key();
    let lock = p2pkh(&derived(&root, &sender, PREFIX, SUFFIX));
    let (tx, txid, parent) = beef_paying(&lock);
    let (wallet, app, _) = served(&root, Some(&parent)).await;
    let (status, body) = internalize(
        &app,
        &tx,
        vec![payment_of(0, PREFIX, SUFFIX, &sender.to_hex())],
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    sqlx::query("UPDATE outputs SET spent_by = transaction_id WHERE txid = ? AND vout = 0")
        .bind(&txid)
        .execute(wallet.storage().writer())
        .await
        .unwrap();
    assert_eq!(balance(&wallet).await, 4_500, "counted");
    let found: Vec<(i64, String, i64)> = sqlx::query_as(
        "SELECT user_id, txid, vout FROM outputs WHERE spendable = 1 AND spent_by IS NOT NULL",
    )
    .fetch_all(wallet.storage().writer())
    .await
    .unwrap();
    assert_eq!(found.len(), 1);
    assert_eq!((found[0].1.as_str(), found[0].2), (txid.as_str(), 0));
    relinquish(&app, &txid, 0).await;
    assert_eq!(balance(&wallet).await, 0, "out of the balance");
}

/// What an internalize of `outputs: []` admitted before left (the
/// toolbox's Hosts: a transaction row with no `outputs` row), found by the
/// query the Hosts names: on this release the request is refused (row 258),
/// so a fresh wallet holds none.
#[tokio::test]
async fn an_empty_outputs_leaves_no_transaction_without_an_output() {
    let root = PrivateKey::random();
    let sender = PrivateKey::random().public_key();
    let lock = p2pkh(&derived(&root, &sender, PREFIX, SUFFIX));
    let (tx, _, parent) = beef_paying(&lock);
    let (wallet, app, posts) = served(&root, Some(&parent)).await;
    let before = stored(&wallet).await;
    let (status, body) = internalize(&app, &tx, vec![]).await;
    assert_refused(
        status,
        &body,
        "VALIDATION_ERROR",
        "Validation error: The outputs parameter must be at least one output to internalize \
         from the transaction",
    );
    assert_eq!(stored(&wallet).await, before, "nothing written");
    assert_eq!(posts.call_count("post_beef"), 0, "nothing posted");
    let orphans: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM transactions t WHERE NOT EXISTS \
         (SELECT 1 FROM outputs o WHERE o.transaction_id = t.transaction_id)",
    )
    .fetch_one(wallet.storage().writer())
    .await
    .unwrap();
    assert_eq!(orphans, 0);
}

// ---------------------------------------------------------------------------
// The restores, through to a signed spend
// ---------------------------------------------------------------------------

/// The wallet's own change an earlier release reclassified, restored
/// through `POST /internalizeAction` as a wallet payment with the prefix
/// and suffix the row held and the wallet's own identity key as the
/// sender: change again in `default`, its sender that key, and a spend
/// that selects it signs. An empty sender, the stable line's 0.13.3
/// restore, is refused on this line, nothing stored; an action without the
/// `underReceivedRule` label does not select the restored coin while its
/// transaction is unproven.
#[tokio::test]
async fn own_change_restored_with_the_wallets_identity_key_signs() {
    let root = PrivateKey::random();
    let (wallet, app, _, bytes, txid, vout) = own_change(&root).await;
    let (_, _, _, prefix, suffix, _) = row(&wallet, &txid, vout).await;
    let (prefix, suffix) = (prefix.unwrap(), suffix.unwrap());
    reclassify(&wallet, &txid, vout).await;
    assert_eq!(balance(&wallet).await, 0);

    let before = stored(&wallet).await;
    let (status, body) =
        internalize(&app, &bytes, vec![payment_of(vout, &prefix, &suffix, "")]).await;
    assert_refused(
        status,
        &body,
        "VALIDATION_ERROR",
        "Validation error: The senderIdentityKey parameter must be hexadecimal string.",
    );
    assert_eq!(stored(&wallet).await, before, "nothing stored");
    assert_eq!(row(&wallet, &txid, vout).await.1, "custom");

    let me = root.public_key().to_hex();
    let (status, body) =
        internalize(&app, &bytes, vec![payment_of(vout, &prefix, &suffix, &me)]).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let restored = row(&wallet, &txid, vout).await;
    assert_eq!(restored.0.as_deref(), Some("default"));
    assert_eq!((restored.1.as_str(), restored.2), ("P2PKH", 1));
    assert_eq!(restored.5, Some(me), "the stored sender");
    assert!(balance(&wallet).await > 0, "counted");

    let unselected = spend(&wallet, false).await.expect_err("not selected");
    assert!(unselected.contains("Insufficient funds"), "{unselected}");
    let tx = spend(&wallet, true).await.expect("the restored coin signs");
    spends_the_coin(&wallet, &tx, &txid, vout).await;
}

/// A received payment reclassified the same way: with its prefix and
/// suffix under the wallet's own key the door derives another key and
/// refuses (`Derivation mismatch`, 400 `VALIDATION_ERROR`); with an empty
/// sender it is refused as a parameter (400 `VALIDATION_ERROR`); with the
/// sender the row held it is change again and a spend signs.
#[tokio::test]
async fn a_reclassified_received_payment_restored_with_its_sender_signs() {
    let root = PrivateKey::random();
    let sender = PrivateKey::random().public_key();
    let lock = p2pkh(&derived(&root, &sender, PREFIX, SUFFIX));
    let (bytes, txid, parent) = beef_paying(&lock);
    let (wallet, app, posts) = served(&root, Some(&parent)).await;
    let (status, body) = internalize(
        &app,
        &bytes,
        vec![payment_of(0, PREFIX, SUFFIX, &sender.to_hex())],
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    reclassify(&wallet, &txid, 0).await;
    assert_eq!(balance(&wallet).await, 0);
    let before = stored(&wallet).await;

    let me = root.public_key().to_hex();
    let (status, body) = internalize(&app, &bytes, vec![payment_of(0, PREFIX, SUFFIX, &me)]).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert_eq!(body["code"], "VALIDATION_ERROR", "{body}");
    let message = body["message"].as_str().unwrap();
    let derived_hash = hex::encode(derived(&root, &root.public_key(), PREFIX, SUFFIX).hash160());
    assert_eq!(
        message,
        format!(
            "Derivation mismatch for output 0: derived {derived_hash} != script {}",
            hex::encode(&lock[3..23])
        )
    );
    let (status, body) = internalize(&app, &bytes, vec![payment_of(0, PREFIX, SUFFIX, "")]).await;
    assert_refused(
        status,
        &body,
        "VALIDATION_ERROR",
        "Validation error: The senderIdentityKey parameter must be hexadecimal string.",
    );
    assert_eq!(stored(&wallet).await, before, "nothing stored");
    assert_eq!(row(&wallet, &txid, 0).await.1, "custom");
    assert_eq!(posts.call_count("post_beef"), 1, "the opening's post alone");

    let (status, body) = internalize(
        &app,
        &bytes,
        vec![payment_of(0, PREFIX, SUFFIX, &sender.to_hex())],
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(row(&wallet, &txid, 0).await.5, Some(sender.to_hex()));
    assert_eq!(balance(&wallet).await, 4_500);
    let tx = spend(&wallet, true).await.expect("the restored coin signs");
    spends_the_coin(&wallet, &tx, &txid, 0).await;
}

/// A received payment written by this CLI's own flows (`fund`, `receive`,
/// `sync`, `split`, `gift-claim` and the tracker host's record of a claim
/// name the anyone key, `KeyDeriver::anyone_key()`, as the sender, with
/// `src/brc29.rs`'s default prefix and suffix), reclassified as the others
/// are: restored with that prefix, that suffix and the anyone key as the
/// sender, it is change again, counted, holds the anyone key as its sender,
/// and a spend that selects it signs. An empty sender is refused.
#[tokio::test]
async fn a_reclassified_payment_this_cli_wrote_restored_with_the_anyone_key_signs() {
    let root = PrivateKey::random();
    let (_, anyone) = KeyDeriver::anyone_key();
    let lock = p2pkh(&derived(&root, &anyone, CLI_PREFIX, CLI_SUFFIX));
    let (bytes, txid, parent) = beef_paying(&lock);
    let (wallet, app, _) = served(&root, Some(&parent)).await;
    let as_written = || vec![payment_of(0, CLI_PREFIX, CLI_SUFFIX, &anyone.to_hex())];
    let (status, body) = internalize(&app, &bytes, as_written()).await;
    assert_eq!(status, StatusCode::OK, "as the CLI writes it: {body}");
    reclassify(&wallet, &txid, 0).await;
    assert_eq!(balance(&wallet).await, 0);

    let (status, body) = internalize(
        &app,
        &bytes,
        vec![payment_of(0, CLI_PREFIX, CLI_SUFFIX, "")],
    )
    .await;
    assert_refused(
        status,
        &body,
        "VALIDATION_ERROR",
        "Validation error: The senderIdentityKey parameter must be hexadecimal string.",
    );
    assert_eq!(row(&wallet, &txid, 0).await.1, "custom");

    let (status, body) = internalize(&app, &bytes, as_written()).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let restored = row(&wallet, &txid, 0).await;
    assert_eq!(restored.0.as_deref(), Some("default"));
    assert_eq!(restored.5, Some(anyone.to_hex()), "the anyone key");
    assert_eq!(balance(&wallet).await, 4_500);
    let tx = spend(&wallet, true).await.expect("the restored coin signs");
    spends_the_coin(&wallet, &tx, &txid, 0).await;
}

/// A change row whose stored sender is an empty string (the stable line's
/// door stores an empty sender as given, its 0.13.3 restore among them, as
/// can a sync from such a store; reproduced by SQL as the toolbox's test
/// does): on this release a spend that selects it signs, with the wallet as
/// its counterparty, and the row is not rewritten.
#[tokio::test]
async fn a_change_row_holding_an_empty_sender_signs() {
    let root = PrivateKey::random();
    let (wallet, _, _, _, txid, vout) = own_change(&root).await;
    sqlx::query("UPDATE outputs SET sender_identity_key = '' WHERE txid = ? AND vout = ?")
        .bind(&txid)
        .bind(vout)
        .execute(wallet.storage().writer())
        .await
        .unwrap();
    let tx = spend(&wallet, true)
        .await
        .expect("the empty sender's row signs");
    spends_the_coin(&wallet, &tx, &txid, vout).await;
    assert_eq!(row(&wallet, &txid, vout).await.5.as_deref(), Some(""));
}
