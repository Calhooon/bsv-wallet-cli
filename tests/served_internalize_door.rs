//! `POST /internalizeAction` as served, under bsv-wallet-toolbox-rs 0.13.4
//! (the `internalizeAction` door read whole against the reference): each
//! answer of that release's CHANGELOG `### Hosts` that a caller of this host
//! can reach, read through this CLI's router with its HTTP status, its code
//! word and its whole message, nothing written and nothing posted on a
//! refusal, and a control beside each (the same request made well is
//! stored). Then the restores of a row an earlier release reclassified, each
//! run through to a signed spend whose unlocking script is run against the
//! coin's P2PKH, and the two "rows written before" cases found by the
//! toolbox's queries and taken out by `POST /relinquishOutput`.
//!
//! The shapes are the toolbox's published tests for 0.13.4:
//! `tests/internalize_root_check_tests.rs`, `internalize_no_outputs_tests.rs`,
//! `internalize_output_index_tests.rs`, `internalize_payment_derivation_tests.rs`,
//! `internalize_spent_merge_tests.rs` and `internalize_sender_spend_tests.rs`
//! (the restores, its `spend` and `verify_spend`). Where the toolbox's test
//! reproduces a row an earlier release wrote with an SQL update, so does
//! this file. The spends are made by `Wallet::create_action` (`noSend`), as
//! the toolbox's tests and 0.18.5's served tests make them: the served
//! `createAction`'s spend gate asks a header service these tests do not
//! run, and the door under test is `internalizeAction`.
//!
//! This host configures no permissions manager (`git grep -n -i
//! permissions_manager -- src mcp/src` finds none), so the Hosts rows of a
//! manager (a basket with no grant, `admin` and `p ` baskets and labels)
//! are not reachable here and have no test.
//!
//! All vectors are synthetic: random throwaway keys, coinbase-style BUMPs
//! checked against a TestChainTracker or the toolbox's mock services.

#[path = "../src/test_support/chain_tracker.rs"]
mod chain_tracker;
#[path = "../src/test_support/no_tracker.rs"]
#[allow(dead_code)]
mod no_tracker;
#[path = "support/served_internalize.rs"]
#[allow(dead_code)]
mod support;
use no_tracker::NoTracker;
use support::*;

use axum::http::StatusCode;
use bsv_sdk::primitives::bsv::{parse_transaction, TxInput, TxOutput};
use bsv_sdk::script::{LockingScript, Spend, SpendParams, UnlockingScript};
use bsv_sdk::transaction::{Beef, MerklePath, Transaction};
use bsv_sdk::wallet::{
    Counterparty, CreateActionArgs, CreateActionOptions, CreateActionOutput, KeyDeriver, Protocol,
    SecurityLevel, WalletInterface,
};
use bsv_wallet_toolbox::services::mock::MockWalletServices;
use bsv_wallet_toolbox::{WalletServices, WalletStorageProvider};
use serde_json::{json, Value};
use std::sync::Arc;

const NO_TRACKER: &str = "wallet error: Validation error: No chain tracker is available to verify the BEEF: install one on the storage or configure the services' chain tracker";
const NO_OUTPUTS: &str = "wallet error: The outputs parameter must be at least one output to internalize from the transaction";
const REPEATED_INDEX: &str =
    "wallet error: Validation error: The outputs parameter must be unique outputIndex values";
const UNCOMPRESSED_SENDER: &str = "wallet error: Validation error: Wallet payment outputs require a sender_identity_key that is empty or a compressed public key";
const EMPTY_DERIVATION: &str =
    "wallet error: Wallet payment outputs require non-empty derivation_prefix and derivation_suffix";

fn assert_answer(status: StatusCode, body: &Value, code: &str, message: &str) {
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert_eq!(body, &json!({ "code": code, "message": message }));
}

/// The storage's services replaced by `services`: the door reads their
/// tracker when the storage has none, and every post goes through them.
fn storage_services<S: WalletServices + 'static>(wallet: &WalletState, services: S) -> Arc<S> {
    let services = Arc::new(services);
    WalletStorageProvider::set_services(
        wallet.storage(),
        services.clone() as Arc<dyn WalletServices>,
    );
    services
}

fn internalize(tx: &[u8], outputs: Vec<Value>) -> Value {
    json!({ "tx": tx, "outputs": outputs, "description": "an output to check" })
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

async fn balance(wallet: &WalletState) -> u64 {
    wallet.balance().await.expect("the balance").total
}

// ---------------------------------------------------------------------------
// The Hosts answers
// ---------------------------------------------------------------------------

/// No chain tracker on the storage and none on its services (`Services`
/// with no `chaintracks_url`, as `CHAINTRACKS_URL=off` builds them): every
/// `internalizeAction` is refused, a wallet payment and a basket insertion
/// alike, 400 `VALIDATION_ERROR`, nothing written; with the storage's
/// services the mock behind a wrapper with no tracker, nothing posted. The
/// control: the same request to a wallet with a tracker is stored.
#[tokio::test]
async fn with_no_chain_tracker_every_internalize_is_refused() {
    let who = parties();
    let (tx, txid) = proven_beef(&p2pkh(&who.derived.hash160()));
    let insertion = json!({
        "outputIndex": 0,
        "protocol": "basket insertion",
        "insertionRemittance": { "basket": "tokens" }
    });

    // As `off` builds it: `Services` with no tracker, none on the storage.
    let (wallet, app) = served_untracked(&who.root).await;
    let before = stored(&wallet).await;
    for output in [payment(&who.sender), insertion.clone()] {
        let (status, body) = post(app.clone(), &tx, output).await;
        assert_answer(status, &body, "VALIDATION_ERROR", NO_TRACKER);
    }
    assert_eq!(stored(&wallet).await, before, "nothing written");

    // The same, the storage's services a mock broadcaster with no tracker.
    let (wallet, app) = served_untracked(&who.root).await;
    let services = storage_services(&wallet, NoTracker::new());
    let before = stored(&wallet).await;
    for output in [payment(&who.sender), insertion] {
        let (status, body) = post(app.clone(), &tx, output).await;
        assert_answer(status, &body, "VALIDATION_ERROR", NO_TRACKER);
    }
    assert_eq!(stored(&wallet).await, before, "nothing written");
    assert_eq!(services.call_count("post_beef"), 0, "nothing posted");

    // The control.
    let (wallet, app) = served(&who.root, Some(&txid)).await;
    let services = storage_services(&wallet, MockWalletServices::new());
    let (status, body) = post(app, &tx, payment(&who.sender)).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(balance(&wallet).await, 1_000, "stored");
    assert_eq!(services.call_count("post_beef"), 0);
}

/// No tracker on the storage, and the services' tracker refutes the BEEF's
/// root (the mock's `roots_valid(false)`): 400 `VALIDATION_ERROR`, the
/// root and its height named, nothing written or posted. The control: the
/// services' tracker confirms it, and the payment is stored.
#[tokio::test]
async fn a_root_the_services_tracker_refutes_is_refused() {
    let who = parties();
    let (tx, txid) = proven_beef(&p2pkh(&who.derived.hash160()));
    let root = MerklePath::from_coinbase_txid(&txid, HEIGHT)
        .compute_root(Some(&txid))
        .expect("the root");

    let (wallet, app) = served_untracked(&who.root).await;
    let services = storage_services(
        &wallet,
        MockWalletServices::builder().roots_valid(false).build(),
    );
    let before = stored(&wallet).await;
    let (status, body) = post(app, &tx, payment(&who.sender)).await;
    assert_answer(
        status,
        &body,
        "VALIDATION_ERROR",
        &format!("wallet error: Validation error: Invalid merkle root {root} at height {HEIGHT}"),
    );
    assert_eq!(stored(&wallet).await, before, "nothing written");
    assert_eq!(services.call_count("post_beef"), 0, "nothing posted");

    let (wallet, app) = served_untracked(&who.root).await;
    let services = storage_services(&wallet, MockWalletServices::new());
    let (status, body) = post(app, &tx, payment(&who.sender)).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(balance(&wallet).await, 1_000, "stored");
    assert_eq!(services.call_count("post_beef"), 0);
}

/// An empty `outputs`: refused before anything is stored or posted. The
/// control: the same transaction with its one output named is stored.
#[tokio::test]
async fn an_empty_outputs_is_refused() {
    let who = parties();
    let (tx, txid) = proven_beef(&p2pkh(&who.derived.hash160()));
    let (wallet, app) = served(&who.root, Some(&txid)).await;
    let services = storage_services(&wallet, MockWalletServices::new());
    let before = stored(&wallet).await;
    let (status, body) = post_to(app.clone(), "/internalizeAction", internalize(&tx, vec![])).await;
    assert_answer(status, &body, "ERROR", NO_OUTPUTS);
    assert_eq!(stored(&wallet).await, before, "nothing written");
    assert_eq!(services.call_count("post_beef"), 0, "nothing posted");

    let (status, body) = post(app, &tx, payment(&who.sender)).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(balance(&wallet).await, 1_000, "stored");
}

/// One `outputIndex` named twice: on a new transaction and on one the
/// wallet holds, 400 `VALIDATION_ERROR`, nothing written (the held row's
/// value as it was) and nothing posted. The controls: the index named once
/// is stored, and named once again on the held transaction is admitted with
/// the row unchanged.
#[tokio::test]
async fn one_output_index_named_twice_is_refused_on_a_new_and_a_held_transaction() {
    let who = parties();
    let (tx, txid) = proven_beef(&p2pkh(&who.derived.hash160()));
    let (wallet, app) = served(&who.root, Some(&txid)).await;
    let services = storage_services(&wallet, MockWalletServices::new());
    let twice = internalize(&tx, vec![payment(&who.sender), payment(&who.sender)]);

    let before = stored(&wallet).await;
    let (status, body) = post_to(app.clone(), "/internalizeAction", twice.clone()).await;
    assert_answer(status, &body, "VALIDATION_ERROR", REPEATED_INDEX);
    assert_eq!(stored(&wallet).await, before, "new: nothing written");

    let (status, body) = post(app.clone(), &tx, payment(&who.sender)).await;
    assert_eq!(status, StatusCode::OK, "the control: {body}");
    assert_eq!(balance(&wallet).await, 1_000);

    let held = stored(&wallet).await;
    let (status, body) = post_to(app.clone(), "/internalizeAction", twice).await;
    assert_answer(status, &body, "VALIDATION_ERROR", REPEATED_INDEX);
    assert_eq!(stored(&wallet).await, held, "held: nothing written");
    assert_eq!(
        balance(&wallet).await,
        1_000,
        "the held row's value as it was"
    );

    let (status, body) = post(app, &tx, payment(&who.sender)).await;
    assert_eq!(status, StatusCode::OK, "the held control: {body}");
    assert_eq!(stored(&wallet).await, held);
    assert_eq!(balance(&wallet).await, 1_000);
    assert_eq!(services.call_count("post_beef"), 0, "nothing posted");
}

/// A wallet payment whose sender identity key is an uncompressed public
/// key (0.13.3 admitted it): 400 `VALIDATION_ERROR`, nothing written or
/// posted. The control: the same key compressed is stored.
#[tokio::test]
async fn a_payment_whose_sender_is_an_uncompressed_key_is_refused() {
    let who = parties();
    let (tx, txid) = proven_beef(&p2pkh(&who.derived.hash160()));
    let (wallet, app) = served(&who.root, Some(&txid)).await;
    let services = storage_services(&wallet, MockWalletServices::new());
    let before = stored(&wallet).await;
    let uncompressed = payment_of(0, PREFIX, SUFFIX, &who.sender.to_hex_uncompressed());
    let (status, body) = post(app.clone(), &tx, uncompressed).await;
    assert_answer(status, &body, "VALIDATION_ERROR", UNCOMPRESSED_SENDER);
    assert_eq!(stored(&wallet).await, before, "nothing written");
    assert_eq!(services.call_count("post_beef"), 0, "nothing posted");

    let (status, body) = post(app, &tx, payment(&who.sender)).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(balance(&wallet).await, 1_000, "stored");
}

/// The storage door's rows, through this host's wallet door: a payment
/// with an empty derivation prefix or suffix meets the wallet door's text
/// (unchanged by 0.13.4), and a sender that is not a point meets the wallet
/// door's parse; nothing written or posted. The storage door's own texts
/// are not reachable here: this host serves the wallet door alone.
#[tokio::test]
async fn the_storage_door_rows_answer_the_wallet_doors_texts_here() {
    let who = parties();
    let (tx, txid) = proven_beef(&p2pkh(&who.derived.hash160()));
    let (wallet, app) = served(&who.root, Some(&txid)).await;
    let services = storage_services(&wallet, MockWalletServices::new());
    let before = stored(&wallet).await;
    let sender = who.sender.to_hex();
    for output in [
        payment_of(0, "", SUFFIX, &sender),
        payment_of(0, PREFIX, "", &sender),
    ] {
        let (status, body) = post(app.clone(), &tx, output).await;
        assert_answer(status, &body, "ERROR", EMPTY_DERIVATION);
    }
    let (status, body) = post(
        app.clone(),
        &tx,
        payment_of(0, PREFIX, SUFFIX, &"05".repeat(33)),
    )
    .await;
    assert_answer(
        status,
        &body,
        "ERROR",
        "wallet error: Invalid sender_identity_key: invalid public key: Point not on curve",
    );
    assert_eq!(stored(&wallet).await, before, "nothing written");
    assert_eq!(services.call_count("post_beef"), 0, "nothing posted");
}

/// A wallet payment merged over an output the store holds as spent leaves
/// it spent (`spendable` and `spent_by` as the row holds them). The
/// control: the unspent row named again stays spendable.
#[tokio::test]
async fn a_payment_merged_over_a_spent_output_leaves_it_spent() {
    let who = parties();
    let (tx, txid) = proven_beef(&p2pkh(&who.derived.hash160()));
    let (wallet, app) = served(&who.root, Some(&txid)).await;
    let services = storage_services(&wallet, MockWalletServices::new());
    let (status, body) = post(app.clone(), &tx, payment(&who.sender)).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let (status, body) = post(app.clone(), &tx, payment(&who.sender)).await;
    assert_eq!(status, StatusCode::OK, "the control: {body}");
    assert_eq!(spent(&wallet, &txid).await, (true, None));

    // Spent by a transaction of the store (its own, for the test).
    sqlx::query(
        "UPDATE outputs SET spendable = 0, spent_by = transaction_id WHERE txid = ? AND vout = 0",
    )
    .bind(&txid)
    .execute(wallet.storage().writer())
    .await
    .unwrap();
    let row = spent(&wallet, &txid).await;
    assert!(!row.0 && row.1.is_some());
    let (status, body) = post(app, &tx, payment(&who.sender)).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(spent(&wallet, &txid).await, row, "left spent");
    assert_eq!(balance(&wallet).await, 0);
    assert_eq!(services.call_count("post_beef"), 0);
}

async fn spent(wallet: &WalletState, txid: &str) -> (bool, Option<i64>) {
    sqlx::query_as("SELECT spendable, spent_by FROM outputs WHERE txid = ? AND vout = 0")
        .bind(txid)
        .fetch_one(wallet.storage().writer())
        .await
        .unwrap()
}

// ---------------------------------------------------------------------------
// The rows written before
// ---------------------------------------------------------------------------

/// A spent output an earlier merge made spendable (`spendable = 1` with
/// `spent_by` set, reproduced by SQL): the balance counts it, the
/// toolbox's query finds it, and `POST /relinquishOutput` on `default`
/// takes it out of the balance.
#[tokio::test]
async fn a_spent_row_an_earlier_merge_made_spendable_is_found_and_relinquished() {
    let who = parties();
    let (tx, txid) = proven_beef(&p2pkh(&who.derived.hash160()));
    let (wallet, app) = served(&who.root, Some(&txid)).await;
    let (status, body) = post(app.clone(), &tx, payment(&who.sender)).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    sqlx::query("UPDATE outputs SET spent_by = transaction_id WHERE txid = ? AND vout = 0")
        .bind(&txid)
        .execute(wallet.storage().writer())
        .await
        .unwrap();
    assert_eq!(balance(&wallet).await, 1_000, "counted");

    let found: Vec<(i64, String, i64)> = sqlx::query_as(
        "SELECT user_id, txid, vout FROM outputs WHERE spendable = 1 AND spent_by IS NOT NULL",
    )
    .fetch_all(wallet.storage().writer())
    .await
    .unwrap();
    assert_eq!(found.len(), 1);
    assert_eq!((found[0].1.as_str(), found[0].2), (txid.as_str(), 0));

    relinquish(app, &txid, 0).await;
    assert_eq!(balance(&wallet).await, 0, "out of the balance");
}

/// A change row with an empty derivation (the storage door wrote one
/// before 0.13.4; this host's wallet door never did: it refuses an empty
/// prefix or suffix, above): reproduced by SQL, the toolbox's query finds
/// it and `POST /relinquishOutput` on `default` takes it out.
#[tokio::test]
async fn a_change_row_with_an_empty_derivation_is_found_and_relinquished() {
    let who = parties();
    let (tx, txid) = proven_beef(&p2pkh(&who.derived.hash160()));
    let (wallet, app) = served(&who.root, Some(&txid)).await;
    let (status, body) = post(app.clone(), &tx, payment(&who.sender)).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    sqlx::query("UPDATE outputs SET derivation_prefix = '' WHERE txid = ? AND vout = 0")
        .bind(&txid)
        .execute(wallet.storage().writer())
        .await
        .unwrap();
    assert_eq!(balance(&wallet).await, 1_000, "counted");
    let found: Vec<(String, i64)> = sqlx::query_as(
        "SELECT txid, vout FROM outputs WHERE change = 1 AND (derivation_prefix = '' OR derivation_suffix = '')",
    )
    .fetch_all(wallet.storage().writer())
    .await
    .unwrap();
    assert_eq!(found, vec![(txid.clone(), 0)]);

    relinquish(app, &txid, 0).await;
    assert_eq!(balance(&wallet).await, 0, "out of the balance");
}

async fn relinquish(app: axum::Router, txid: &str, vout: u32) {
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
// The restores, through to a signed spend
// ---------------------------------------------------------------------------

/// A `createAction` of 300 satoshis to another key, funded by the wallet's
/// change, signed and not sent (the toolbox test's `spend`): the signed
/// transaction, or the refusal's text.
async fn spend(wallet: &WalletState) -> Result<Vec<u8>, String> {
    let created = wallet
        .create_action(
            CreateActionArgs {
                description: "a spend of the wallet's change".to_string(),
                input_beef: None,
                inputs: None,
                outputs: Some(vec![CreateActionOutput {
                    locking_script: other_p2pkh(),
                    satoshis: 300,
                    output_description: "a payment".to_string(),
                    basket: None,
                    custom_instructions: None,
                    tags: None,
                }]),
                lock_time: None,
                version: None,
                labels: None,
                options: Some(CreateActionOptions {
                    sign_and_process: Some(true),
                    no_send: Some(true),
                    randomize_outputs: Some(false),
                    ..Default::default()
                }),
            },
            "test.local",
        )
        .await
        .map_err(|e| e.to_string())?;
    Ok(created.tx.expect("the signed transaction"))
}

/// Runs the input of `raw` spending `txid`.`vout` against `lock` holding
/// `satoshis` in bsv-rs's script interpreter: whether it verifies (the
/// toolbox test's `verify_spend`).
fn verify_spend(raw: &[u8], txid: &str, vout: u32, satoshis: u64, lock: &[u8]) -> bool {
    let parsed = parse_transaction(raw).expect("a transaction");
    let mut outpoint: [u8; 32] = hex::decode(txid).unwrap().try_into().unwrap();
    outpoint.reverse();
    let index = parsed
        .inputs
        .iter()
        .position(|i| i.txid == outpoint && i.output_index == vout)
        .expect("the spend selects the coin");
    let ours = &parsed.inputs[index];
    let other_inputs: Vec<TxInput> = parsed
        .inputs
        .iter()
        .enumerate()
        .filter(|(i, _)| *i != index)
        .map(|(_, i)| i.clone())
        .collect();
    let outputs: Vec<TxOutput> = parsed.outputs.clone();
    let mut spend = Spend::new(SpendParams {
        source_txid: ours.txid,
        source_output_index: vout,
        source_satoshis: satoshis,
        locking_script: LockingScript::from_binary(lock).unwrap(),
        transaction_version: parsed.version,
        other_inputs,
        outputs,
        input_index: index,
        unlocking_script: UnlockingScript::from_binary(&ours.script).unwrap(),
        input_sequence: ours.sequence,
        lock_time: parsed.locktime,
        memory_limit: None,
        work_limit: None,
    });
    matches!(spend.validate(), Ok(true))
}

/// The row's sender identity key, spendable flag and spender.
async fn held(wallet: &WalletState, txid: &str, vout: u32) -> (Option<String>, bool, Option<i64>) {
    sqlx::query_as(
        "SELECT sender_identity_key, spendable, spent_by FROM outputs WHERE txid = ? AND vout = ?",
    )
    .bind(txid)
    .bind(vout)
    .fetch_one(wallet.storage().writer())
    .await
    .unwrap()
}

/// The row an earlier release wrote for a basket insertion into "tokens"
/// (custom, not change, derivation and sender cleared; the toolbox test's
/// `reclassify`).
async fn reclassify(wallet: &WalletState, txid: &str, vout: u32) {
    let pool = wallet.storage().writer();
    sqlx::query(
        "INSERT OR IGNORE INTO output_baskets (user_id, name, created_at, updated_at) \
         SELECT user_id, 'tokens', created_at, updated_at FROM output_baskets WHERE name = 'default'",
    )
    .execute(pool)
    .await
    .unwrap();
    sqlx::query(
        "UPDATE outputs SET basket_id = (SELECT basket_id FROM output_baskets WHERE name = 'tokens'), \
         type = 'custom', change = 0, custom_instructions = NULL, derivation_prefix = NULL, \
         derivation_suffix = NULL, sender_identity_key = NULL WHERE txid = ? AND vout = ?",
    )
    .bind(txid)
    .bind(vout)
    .execute(pool)
    .await
    .unwrap();
}

/// A served wallet whose one spendable coin is the change of an action it
/// made, the change row reclassified as an earlier release wrote it.
struct Reclassified {
    wallet: WalletState,
    app: axum::Router,
    tx: Vec<u8>,
    txid: String,
    vout: u32,
    satoshis: u64,
    lock: Vec<u8>,
    prefix: String,
    suffix: String,
}

async fn reclassified_change(who: &Parties) -> Reclassified {
    let (funding, funding_txid) = proven_beef(&p2pkh(&who.derived.hash160()));
    let (wallet, app) = served(&who.root, Some(&funding_txid)).await;
    storage_services(&wallet, MockWalletServices::new());
    let (status, body) = post(app.clone(), &funding, payment(&who.sender)).await;
    assert_eq!(status, StatusCode::OK, "the funding: {body}");
    let raw = spend(&wallet).await.expect("the action with change");
    let txid = Transaction::from_binary(&raw).unwrap().id();
    let mut beef = Beef::from_binary(&funding).unwrap();
    beef.merge_raw_tx(raw, None);
    let tx = beef.to_binary_atomic(&txid).unwrap();
    let pool = wallet.storage().writer();
    // As if the action had been sent: `unproven`, its change spendable.
    for sql in [
        "UPDATE transactions SET status = 'unproven' WHERE txid = ?",
        "UPDATE outputs SET spendable = 1 WHERE txid = ? AND change = 1",
    ] {
        sqlx::query(sql).bind(&txid).execute(pool).await.unwrap();
    }
    let (vout, satoshis, lock, prefix, suffix, sender): (
        u32,
        i64,
        Vec<u8>,
        String,
        String,
        Option<String>,
    ) = sqlx::query_as(
        "SELECT vout, satoshis, locking_script, derivation_prefix, derivation_suffix, \
         sender_identity_key FROM outputs WHERE txid = ? AND change = 1",
    )
    .bind(&txid)
    .fetch_one(pool)
    .await
    .unwrap();
    assert_eq!(sender, None, "the wallet's own change holds no sender");
    reclassify(&wallet, &txid, vout).await;
    assert_eq!(balance(&wallet).await, 0);
    Reclassified {
        wallet,
        app,
        tx,
        txid,
        vout,
        satoshis: satoshis as u64,
        lock,
        prefix,
        suffix,
    }
}

/// The wallet's own change an earlier release reclassified, restored
/// through `POST /internalizeAction` as a wallet payment with the prefix
/// and suffix the row held and the wallet's own identity key as the
/// sender: change again, counted, and a spend that selects it signs.
#[tokio::test]
async fn own_change_restored_with_the_wallets_identity_key_signs() {
    let who = parties();
    let r = reclassified_change(&who).await;
    let identity = who.root.public_key().to_hex();
    let (status, body) = post(
        r.app.clone(),
        &r.tx,
        payment_of(r.vout, &r.prefix, &r.suffix, &identity),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["accepted"], true, "{body}");
    assert_eq!(balance(&r.wallet).await, r.satoshis);
    let signed = spend(&r.wallet).await.expect("signed");
    assert!(verify_spend(&signed, &r.txid, r.vout, r.satoshis, &r.lock));
}

/// The same with an empty sender: from 0.13.4 the row is stored with no
/// sender and the spend signs.
#[tokio::test]
async fn own_change_restored_with_an_empty_sender_signs() {
    let who = parties();
    let r = reclassified_change(&who).await;
    let (status, body) = post(
        r.app.clone(),
        &r.tx,
        payment_of(r.vout, &r.prefix, &r.suffix, ""),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(balance(&r.wallet).await, r.satoshis);
    assert_eq!(held(&r.wallet, &r.txid, r.vout).await, (None, true, None));
    let signed = spend(&r.wallet).await.expect("signed");
    assert!(verify_spend(&signed, &r.txid, r.vout, r.satoshis, &r.lock));
}

/// A received payment reclassified the same way: with its prefix and
/// suffix and an empty sender the door derives another key and refuses
/// (400 `ERROR`, the `Derivation mismatch` text, the class it has had);
/// with the sender the row held it is change again and a spend signs.
#[tokio::test]
async fn a_reclassified_received_payment_restored_with_its_sender_signs() {
    let who = parties();
    let lock = p2pkh(&who.derived.hash160());
    let (tx, txid) = proven_beef(&lock);
    let (wallet, app) = served(&who.root, Some(&txid)).await;
    let services = storage_services(&wallet, MockWalletServices::new());
    let (status, body) = post(app.clone(), &tx, payment(&who.sender)).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    reclassify(&wallet, &txid, 0).await;
    assert_eq!(balance(&wallet).await, 0);

    let self_derived = KeyDeriver::new(Some(who.root.clone()))
        .derive_public_key(
            &Protocol::new(SecurityLevel::Counterparty, "3241645161d8"),
            &format!("{PREFIX} {SUFFIX}"),
            &Counterparty::Self_,
            true,
        )
        .unwrap();
    let before = stored(&wallet).await;
    let (status, body) = post(app.clone(), &tx, payment_of(0, PREFIX, SUFFIX, "")).await;
    assert_answer(
        status,
        &body,
        "ERROR",
        &format!(
            "wallet error: Derivation mismatch for output 0: derived {} != script {}",
            hex::encode(self_derived.hash160()),
            hex::encode(who.derived.hash160())
        ),
    );
    assert_eq!(stored(&wallet).await, before, "nothing written");
    assert_eq!(balance(&wallet).await, 0);

    let (status, body) = post(app, &tx, payment(&who.sender)).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(balance(&wallet).await, 1_000);
    let signed = spend(&wallet).await.expect("signed");
    assert!(verify_spend(&signed, &txid, 0, 1_000, &lock));
    assert_eq!(services.call_count("post_beef"), 0, "nothing posted");
}

/// A row an earlier release wrote with an empty string as its sender (the
/// 0.13.3 own-change restore with an empty sender wrote one; reproduced by
/// SQL as the toolbox's test does): from 0.13.4 a spend that selects it
/// signs, with no step by the host.
#[tokio::test]
async fn a_row_holding_an_empty_sender_signs() {
    let who = parties();
    let self_derived = KeyDeriver::new(Some(who.root.clone()))
        .derive_public_key(
            &Protocol::new(SecurityLevel::Counterparty, "3241645161d8"),
            &format!("{PREFIX} {SUFFIX}"),
            &Counterparty::Self_,
            true,
        )
        .unwrap();
    let lock = p2pkh(&self_derived.hash160());
    let (tx, txid) = proven_beef(&lock);
    let (wallet, app) = served(&who.root, Some(&txid)).await;
    storage_services(&wallet, MockWalletServices::new());
    let identity = who.root.public_key().to_hex();
    let (status, body) = post(app, &tx, payment_of(0, PREFIX, SUFFIX, &identity)).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    sqlx::query("UPDATE outputs SET sender_identity_key = '' WHERE txid = ? AND vout = 0")
        .bind(&txid)
        .execute(wallet.storage().writer())
        .await
        .unwrap();
    assert_eq!(
        held(&wallet, &txid, 0).await,
        (Some(String::new()), true, None)
    );
    let signed = spend(&wallet).await.expect("signed");
    assert!(verify_spend(&signed, &txid, 0, 1_000, &lock));
}
