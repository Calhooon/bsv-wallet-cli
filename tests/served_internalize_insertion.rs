//! `POST /internalizeAction` as served, for a basket insertion: the answers
//! bsv-wallet-toolbox-rs 0.13.3 names in its CHANGELOG's Hosts section, each
//! read through this CLI's router with its HTTP status, its code word and its
//! whole message, with nothing written on a refusal; and a control for each
//! rule's accepting side as that CHANGELOG states it.
//!
//! The inputs take the shapes of the toolbox's own published tests for
//! 0.13.3, `tests/internalize_insertion_tests.rs` (the default basket's
//! spellings, the wallet's own change of a `createAction` and a received
//! payment named again by a basket insertion, an output in an application
//! basket named into another, the legacy-recovery sweep, the row an earlier
//! release inserted into `default`) and `tests/common/mod.rs`. Where the
//! toolbox's test reproduces a row an earlier release wrote with an SQL
//! update, so does this file. Every subject here is proven by a BUMP the
//! test's tracker carries, or is a `noSend` action: the storage's services
//! are a mock broadcaster, and no test asks it to post.
//!
//! All vectors are synthetic: a random throwaway key, a coinbase-style BUMP
//! checked against a TestChainTracker.

#[path = "../src/test_support/chain_tracker.rs"]
mod chain_tracker;
#[path = "support/served_internalize.rs"]
#[allow(dead_code)]
mod support;
use support::*;

use axum::http::StatusCode;
use bsv_sdk::transaction::{Beef, MerklePath, Transaction};
use bsv_sdk::wallet::{CreateActionArgs, CreateActionOptions, CreateActionOutput, WalletInterface};
use bsv_wallet_toolbox::services::mock::MockWalletServices;
use bsv_wallet_toolbox::{WalletServices, WalletStorageProvider};
use serde_json::{json, Value};
use std::sync::Arc;

const NON_DEFAULT: &str =
    "wallet error: The insertionRemittance.basket parameter must be a non-default basket";

fn managed_change(vout: u32) -> String {
    format!(
        "wallet error: Validation error: The outputs parameter must be output {vout} is wallet-managed change and cannot be reclassified as a basket insertion"
    )
}

fn another_basket(vout: u32, basket: &str) -> String {
    format!(
        "wallet error: Validation error: The outputs parameter must be output {vout} is already assigned to a different basket and cannot be reclassified as a basket insertion into {basket}"
    )
}

/// Every spelling the toolbox's test names for the default basket.
const DEFAULT_SPELLINGS: [&str; 6] = [
    "default",
    "Default",
    "DEFAULT",
    " default ",
    "\tdefault\n",
    "\u{FEFF}default",
];

fn insertion(vout: u32, basket: &str) -> Value {
    json!({
        "outputIndex": vout,
        "protocol": "basket insertion",
        "insertionRemittance": { "basket": basket }
    })
}

/// One output row whole: its basket, `change`, type, `spendable`, value,
/// derivation prefix, suffix and sender identity key, `provided_by`,
/// `purpose` and custom instructions.
type Row = (
    Option<i64>,
    bool,
    String,
    bool,
    i64,
    Option<String>,
    Option<String>,
    Option<String>,
    String,
    Option<String>,
    Option<String>,
);

async fn row(wallet: &WalletState, txid: &str, vout: u32) -> Row {
    sqlx::query_as(
        "SELECT basket_id, change, type, spendable, satoshis, derivation_prefix, \
         derivation_suffix, sender_identity_key, provided_by, purpose, custom_instructions \
         FROM outputs WHERE txid = ? AND vout = ?",
    )
    .bind(txid)
    .bind(vout as i64)
    .fetch_one(wallet.storage().pool())
    .await
    .unwrap()
}

/// The name of the basket an output sits in; `None` for no basket.
async fn basket_of(wallet: &WalletState, txid: &str, vout: u32) -> Option<String> {
    sqlx::query_scalar(
        "SELECT b.name FROM outputs o LEFT JOIN output_baskets b ON b.basket_id = o.basket_id \
         WHERE o.txid = ? AND o.vout = ?",
    )
    .bind(txid)
    .bind(vout as i64)
    .fetch_one(wallet.storage().pool())
    .await
    .unwrap()
}

/// A mock broadcaster in the storage's services, so nothing a test does
/// can leave it; each test asserts it was never asked.
fn mock_broadcaster(wallet: &WalletState) -> Arc<MockWalletServices> {
    let broadcaster = Arc::new(MockWalletServices::builder().build());
    WalletStorageProvider::set_services(
        wallet.storage(),
        broadcaster.clone() as Arc<dyn WalletServices>,
    );
    broadcaster
}

fn assert_refusal(status: StatusCode, body: &Value, message: &str) {
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert_eq!(
        body,
        &json!({ "code": "VALIDATION_ERROR", "message": message })
    );
}

/// Rule 1: a basket insertion naming the default basket, in every spelling
/// the toolbox's test names: 400, the reference's sentence, no row written
/// and no basket created. 400 `VALIDATION_ERROR`: the caller's parameter,
/// `classify`'s arm for this text.
#[tokio::test]
async fn a_basket_insertion_naming_the_default_basket_is_refused_in_every_spelling() {
    for spelling in DEFAULT_SPELLINGS {
        let who = parties();
        let (tx, txid) = proven_beef(&other_p2pkh());
        let (wallet, app) = served(&who.root, Some(&txid)).await;
        let broadcaster = mock_broadcaster(&wallet);
        let before = stored(&wallet).await;
        let (status, body) = post(app, &tx, insertion(0, spelling)).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{spelling:?}: {body}");
        assert_eq!(
            body,
            json!({ "code": "VALIDATION_ERROR", "message": NON_DEFAULT }),
            "{spelling:?}"
        );
        assert_eq!(
            stored(&wallet).await,
            before,
            "{spelling:?}: nothing written"
        );
        assert_eq!(broadcaster.call_count("post_beef"), 0, "{spelling:?}");
    }
}

/// Rule 1's control: the rule reads basket insertions alone, so a wallet
/// payment carrying a stray insertion remittance that names `default` is
/// stored as change in the default basket, as 0.18.4 stored it.
#[tokio::test]
async fn a_wallet_payment_carrying_a_stray_insertion_naming_default_is_stored_as_change() {
    let who = parties();
    let (tx, txid) = proven_beef(&p2pkh(&who.derived.hash160()));
    let (wallet, app) = served(&who.root, Some(&txid)).await;
    let mut output = payment(&who.sender);
    output["insertionRemittance"] = json!({ "basket": "default" });
    let (status, body) = post(app, &tx, output).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["accepted"], true, "{body}");
    let (_, change, kind, ..) = row(&wallet, &txid, 0).await;
    assert_eq!((change, kind.as_str()), (true, "P2PKH"));
    assert_eq!(
        basket_of(&wallet, &txid, 0).await.as_deref(),
        Some("default")
    );
}

/// Rule 1's control and the named basket's: a basket insertion into a
/// named application basket is stored there, a `custom` row, not change.
#[tokio::test]
async fn a_basket_insertion_into_a_named_basket_is_stored_there() {
    let who = parties();
    let (tx, txid) = proven_beef(&other_p2pkh());
    let (wallet, app) = served(&who.root, Some(&txid)).await;
    let before = stored(&wallet).await;
    let (status, body) = post(app, &tx, insertion(0, "tokens")).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["accepted"], true, "{body}");
    assert_eq!(stored(&wallet).await.1, before.1 + 1, "one output stored");
    let (_, change, kind, ..) = row(&wallet, &txid, 0).await;
    assert_eq!((change, kind.as_str()), (false, "custom"));
    assert_eq!(
        basket_of(&wallet, &txid, 0).await.as_deref(),
        Some("tokens")
    );
}

/// Rule 2, a received payment: a wallet payment the wallet internalized,
/// named again by a basket insertion of the same transaction: 400
/// `VALIDATION_ERROR`, the reference's sentence; the change row keeps its
/// basket, its `change` flag and its derivation columns byte for byte, and
/// nothing is written.
#[tokio::test]
async fn a_basket_insertion_over_a_received_payment_is_refused_and_the_row_kept() {
    let who = parties();
    let (tx, txid) = proven_beef(&p2pkh(&who.derived.hash160()));
    let (wallet, app) = served(&who.root, Some(&txid)).await;
    let broadcaster = mock_broadcaster(&wallet);
    let (status, body) = post(app.clone(), &tx, payment(&who.sender)).await;
    assert_eq!(status, StatusCode::OK, "the payment: {body}");
    let held = row(&wallet, &txid, 0).await;
    assert!(held.1, "the payment is change");
    let before = stored(&wallet).await;

    let (status, body) = post(app, &tx, insertion(0, "tokens")).await;
    assert_refusal(status, &body, &managed_change(0));
    assert_eq!(row(&wallet, &txid, 0).await, held, "the row byte for byte");
    assert_eq!(
        basket_of(&wallet, &txid, 0).await.as_deref(),
        Some("default")
    );
    assert_eq!(stored(&wallet).await, before, "nothing written");
    assert_eq!(broadcaster.call_count("post_beef"), 0);
}

/// Rule 2, the wallet's own change: a `noSend` `createAction` paying another
/// key, funded by a proven payment received through the served door, holds
/// the wallet's change; a basket
/// insertion naming that change output in the action's AtomicBEEF: 400
/// `VALIDATION_ERROR`, the reference's sentence; the change row byte for
/// byte, nothing written, nothing posted.
#[tokio::test]
async fn a_basket_insertion_over_the_wallets_own_change_is_refused_and_the_row_kept() {
    let who = parties();
    let (funding, funding_txid) = proven_beef(&p2pkh(&who.derived.hash160()));
    let (wallet, app) = served(&who.root, Some(&funding_txid)).await;
    let broadcaster = mock_broadcaster(&wallet);
    let (status, body) = post(app.clone(), &funding, payment(&who.sender)).await;
    assert_eq!(status, StatusCode::OK, "the funding: {body}");

    // The action is made through the wallet, as the toolbox's test makes
    // it: the served createAction's spend gate asks a header service this
    // test does not run, and the door under test is internalizeAction.
    let created = wallet
        .create_action(
            CreateActionArgs {
                description: "a payment with change".to_string(),
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
                    no_send: Some(true),
                    randomize_outputs: Some(false),
                    ..Default::default()
                }),
            },
            "test.local",
        )
        .await
        .expect("createAction");
    let raw = created.tx.expect("the action's transaction");
    let txid = Transaction::from_binary(&raw)
        .expect("the action's transaction")
        .id();
    // The action and its proven parent as one AtomicBEEF, the toolbox
    // test's shape.
    let mut beef = Beef::from_binary(&funding).expect("the funding's BEEF");
    beef.merge_raw_tx(raw, None);
    let tx = beef.to_binary_atomic(&txid).unwrap();

    let vout: i64 = sqlx::query_scalar("SELECT vout FROM outputs WHERE txid = ? AND change = 1")
        .bind(&txid)
        .fetch_one(wallet.storage().pool())
        .await
        .expect("the action holds the wallet's change");
    let vout = vout as u32;
    let held = row(&wallet, &txid, vout).await;
    let before = stored(&wallet).await;

    let (status, body) = post(app, &tx, insertion(vout, "tokens")).await;
    assert_refusal(status, &body, &managed_change(vout));
    let after = row(&wallet, &txid, vout).await;
    assert_eq!(after, held, "the change row byte for byte");
    assert!(after.1, "still change");
    assert!(
        after.5.is_some() && after.6.is_some(),
        "its derivation kept"
    );
    assert_eq!(
        basket_of(&wallet, &txid, vout).await.as_deref(),
        Some("default")
    );
    assert_eq!(stored(&wallet).await, before, "nothing written");
    assert_eq!(broadcaster.call_count("post_beef"), 0, "nothing posted");
}

/// Rule 3: an output in an application basket, named by a basket insertion
/// of the same transaction into another basket: 400 `VALIDATION_ERROR`, the
/// reference's sentence; the row stays, and the other basket is not
/// created. The control: the same basket named again is admitted and
/// leaves it there.
#[tokio::test]
async fn an_output_in_an_application_basket_is_never_moved_into_another() {
    let who = parties();
    let (tx, txid) = proven_beef(&other_p2pkh());
    let (wallet, app) = served(&who.root, Some(&txid)).await;
    let broadcaster = mock_broadcaster(&wallet);
    let (status, body) = post(app.clone(), &tx, insertion(0, "staging")).await;
    assert_eq!(status, StatusCode::OK, "into staging: {body}");
    let held = row(&wallet, &txid, 0).await;
    let before = stored(&wallet).await;

    let (status, body) = post(app.clone(), &tx, insertion(0, "other")).await;
    assert_refusal(status, &body, &another_basket(0, "other"));
    assert_eq!(row(&wallet, &txid, 0).await, held, "the row byte for byte");
    assert_eq!(stored(&wallet).await, before, "no row, no basket");

    let (status, body) = post(app, &tx, insertion(0, "staging")).await;
    assert_eq!(status, StatusCode::OK, "the same basket again: {body}");
    assert_eq!(body["accepted"], true, "{body}");
    assert_eq!(
        basket_of(&wallet, &txid, 0).await.as_deref(),
        Some("staging")
    );
    assert_eq!(stored(&wallet).await, before, "no row, no basket");
    assert_eq!(broadcaster.call_count("post_beef"), 0);
}

/// Rule 3's control, the legacy-recovery state: an output in no basket, or
/// in the default basket and not change (the state an earlier release could
/// leave, reproduced by SQL as the toolbox's test does), is swept into a
/// named basket by a basket insertion of the same transaction: 200, a
/// `custom` row in that basket.
#[tokio::test]
async fn an_output_in_no_basket_or_in_default_and_not_change_is_swept_into_a_named_basket() {
    for state in ["no basket", "default"] {
        let who = parties();
        let (tx, txid) = proven_beef(&other_p2pkh());
        let (wallet, app) = served(&who.root, Some(&txid)).await;
        let broadcaster = mock_broadcaster(&wallet);
        let (status, body) = post(app.clone(), &tx, insertion(0, "staging")).await;
        assert_eq!(status, StatusCode::OK, "{state}: {body}");
        sqlx::query(
            "UPDATE outputs SET basket_id = CASE WHEN ? = 'default' THEN \
             (SELECT basket_id FROM output_baskets WHERE name = 'default') ELSE NULL END \
             WHERE txid = ? AND vout = 0",
        )
        .bind(state)
        .bind(&txid)
        .execute(wallet.storage().writer())
        .await
        .unwrap();
        let expected = (state == "default").then(|| "default".to_string());
        assert_eq!(basket_of(&wallet, &txid, 0).await, expected, "{state}");

        let (status, body) = post(app, &tx, insertion(0, "recovered")).await;
        assert_eq!(status, StatusCode::OK, "{state}: {body}");
        assert_eq!(body["accepted"], true, "{state}: {body}");
        let (_, change, kind, ..) = row(&wallet, &txid, 0).await;
        assert_eq!((change, kind.as_str()), (false, "custom"), "{state}");
        assert_eq!(
            basket_of(&wallet, &txid, 0).await.as_deref(),
            Some("recovered"),
            "{state}"
        );
        assert_eq!(broadcaster.call_count("post_beef"), 0, "{state}");
    }
}

/// A row an earlier release wrote for a basket insertion naming `default`
/// (reproduced by SQL as the toolbox's test does): `POST /listOutputs` on
/// `default` lists its outpoint beside a received payment, the two entries
/// carrying the same members; `POST /relinquishOutput` on `default` takes
/// it out, and the listing no longer holds it.
#[tokio::test]
async fn an_output_inserted_into_default_before_is_listed_there_and_leaves_by_a_relinquish() {
    let who = parties();
    // One proven transaction: output 0 pays the derived key (a received
    // payment), output 1 is inserted into "tokens" and then moved to
    // `default` as an earlier release left it.
    let mut subject = Transaction::new();
    subject.version = 1;
    subject.inputs.push(input(&"11".repeat(32), 0));
    subject
        .outputs
        .push(output(1_000, &p2pkh(&who.derived.hash160())));
    subject.outputs.push(output(1_000, &other_p2pkh()));
    let txid = subject.id();
    let mut beef = Beef::new();
    let bump = beef.merge_bump(MerklePath::from_coinbase_txid(&txid, HEIGHT));
    beef.merge_raw_tx(subject.to_binary(), Some(bump));
    let tx = beef.to_binary_atomic(&txid).unwrap();
    let (wallet, app) = served(&who.root, Some(&txid)).await;
    let mut received = payment(&who.sender);
    received["outputIndex"] = json!(0);
    let (status, body) = post_to(
        app.clone(),
        "/internalizeAction",
        json!({
            "tx": tx,
            "outputs": [received, insertion(1, "tokens")],
            "description": "a payment and an insertion"
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    sqlx::query(
        "UPDATE outputs SET basket_id = \
         (SELECT basket_id FROM output_baskets WHERE name = 'default') WHERE txid = ? AND vout = 1",
    )
    .bind(&txid)
    .execute(wallet.storage().writer())
    .await
    .unwrap();

    let list = json!({ "basket": "default", "include": "locking scripts" });
    let (status, listed) = post_to(app.clone(), "/listOutputs", list.clone()).await;
    assert_eq!(status, StatusCode::OK, "{listed}");
    let outputs = listed["outputs"].as_array().expect("outputs");
    let legacy = format!("{txid}.1");
    let entry = |outpoint: &str| {
        outputs
            .iter()
            .find(|o| o["outpoint"] == outpoint)
            .unwrap_or_else(|| panic!("{outpoint} listed in default: {listed}"))
            .clone()
    };
    let (inserted, received) = (entry(&legacy), entry(&format!("{txid}.0")));
    let members = |v: &Value| {
        let mut keys: Vec<String> = v.as_object().unwrap().keys().cloned().collect();
        keys.sort();
        keys
    };
    assert_eq!(members(&inserted), members(&received), "the same members");
    assert_eq!(inserted["spendable"], true);
    assert_eq!(
        inserted["lockingScript"],
        hex::encode(other_p2pkh()),
        "a P2PKH like the wallet's own"
    );

    let (status, body) = post_to(
        app.clone(),
        "/relinquishOutput",
        json!({ "basket": "default", "output": legacy }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["relinquished"], true, "{body}");
    let (_, listed) = post_to(app, "/listOutputs", list).await;
    let still: Vec<&Value> = listed["outputs"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|o| o["outpoint"] == legacy.as_str())
        .collect();
    assert!(still.is_empty(), "no longer listed in default: {listed}");
}
