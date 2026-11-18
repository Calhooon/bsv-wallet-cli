//! `POST /internalizeAction` as served, under bsv-wallet-toolbox-rs 0.13.7:
//! the door runs the unlocking script of every input of every transaction
//! of a received AtomicBEEF that the BEEF does not prove (the subject and
//! each unproven ancestor) against the output it spends, and refuses the
//! request, with nothing written, when one does not pass (0.13.7's
//! CHANGELOG, Fixed and Hosts). Each refusal is read through this CLI's
//! router with its HTTP status, its code word and its whole message, and
//! with nothing written, nothing posted and the balance unmoved; a control
//! beside each (the same payment made well) is stored. Then the shapes the
//! toolbox names as passing: a payment through unmined parents, and a
//! transaction one wallet's own `createAction` and `signAction` built,
//! internalized by a second wallet.
//!
//! The fixture that works, from the toolbox's Hosts line: "Lock each parent
//! with a script the placeholder satisfies under the word (this crate's
//! tests use `14 <20 zero bytes> 75 75 51`, which the input `00` spends)
//! and prove it after Genesis." A refusal's input is that placeholder
//! against a P2PKH, or a coin proven below Genesis.
//!
//! All vectors are synthetic: random throwaway keys, coinbase-style BUMPs
//! checked against a TestChainTracker, the toolbox's mock broadcaster.

#[path = "../src/test_support/chain_tracker.rs"]
mod chain_tracker;
#[path = "support/served_internalize.rs"]
#[allow(dead_code)]
mod support;
use chain_tracker::TestChainTracker;
use support::*;

use axum::http::StatusCode;
use bsv_sdk::primitives::PrivateKey;
use bsv_sdk::transaction::{Beef, MerklePath, Transaction};
use bsv_sdk::wallet::{
    Counterparty, CreateActionArgs, CreateActionOptions, CreateActionOutput, KeyDeriver, Protocol,
    SecurityLevel, SignActionArgs, SignActionOptions, WalletInterface,
};
use bsv_wallet_toolbox::services::mock::{
    success_post_beef_result, MockResponse, MockWalletServices,
};
use bsv_wallet_toolbox::{WalletServices, WalletStorageProvider};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::Arc;

/// The interpreter's sentence for a P2PKH coin spent by the push `00`.
const EQUALVERIFY: &str = "OP_EQUALVERIFY requires the top two stack items to be equal.";

/// A height below mainnet's Genesis (620,538), the toolbox's own example.
const BEFORE_GENESIS: u32 = 600_000;

fn assert_answer(status: StatusCode, body: &Value, code: &str, message: &str) {
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert_eq!(body, &json!({ "code": code, "message": message }));
}

/// The toolbox's refusal, as the served door answers it.
fn refused(offset: usize, txid: &str, why: &str) -> String {
    format!("wallet error: Validation error: spend refused at offset {offset}, input 0 of {txid}: {why}")
}

/// The byte offset within `beef` of input 0 of `tx`: the transaction's own
/// bytes, past its 4-byte version and its one-byte input count.
fn input_offset(beef: &[u8], tx: &Transaction) -> usize {
    let raw = tx.to_binary();
    let at = beef
        .windows(raw.len())
        .position(|w| w == raw.as_slice())
        .expect("the transaction's bytes in the BEEF");
    at + 5
}

/// A transaction of one input `00` spending `txid`.0, and one output of
/// `satoshis` locked by `lock`.
fn spending(txid: &str, satoshis: u64, lock: &[u8]) -> Transaction {
    let mut tx = Transaction::new();
    tx.version = 1;
    tx.inputs.push(input(txid, 0));
    tx.outputs.push(output(satoshis, lock));
    tx
}

/// A parent of one output of 10,000 locked by `lock`, seeded by `seed`.
fn parent_locked(seed: &str, lock: &[u8]) -> Transaction {
    spending(&seed.repeat(32), 10_000, lock)
}

/// The AtomicBEEF of `chain`: `chain[0]` proven by a BUMP at `height`, every
/// other entry unproven, the last the subject. Its bytes and the subject's
/// txid.
fn beef_of(chain: &[&Transaction], height: u32) -> (Vec<u8>, String) {
    let mut beef = Beef::new();
    let bump = beef.merge_bump(MerklePath::from_coinbase_txid(&chain[0].id(), height));
    beef.merge_raw_tx(chain[0].to_binary(), Some(bump));
    for tx in &chain[1..] {
        beef.merge_raw_tx(tx.to_binary(), None);
    }
    let txid = chain.last().unwrap().id();
    (beef.to_binary_atomic(&txid).unwrap(), txid)
}

/// The served wallet of `root`, its tracker carrying `roots` (a height and
/// the txid a one-transaction block's root is) under a tip of `HEIGHT + 1`,
/// and the storage's services a mock broadcaster accepting `posted`.
async fn served_over(
    root: &PrivateKey,
    roots: &[(u32, &str)],
    posted: &str,
) -> (WalletState, axum::Router, Arc<MockWalletServices>) {
    let mut tracker = TestChainTracker::new(HEIGHT + 1);
    for (height, txid) in roots {
        tracker.add_root(*height, txid.to_string());
    }
    let (wallet, app) = served_with(root, Some(tracker)).await;
    let services = Arc::new(
        MockWalletServices::builder()
            .post_beef_response(MockResponse::Success(vec![success_post_beef_result(
                "arc",
                &[posted],
            )]))
            .build(),
    );
    WalletStorageProvider::set_services(
        wallet.storage(),
        services.clone() as Arc<dyn WalletServices>,
    );
    (wallet, app, services)
}

async fn balance(wallet: &WalletState) -> u64 {
    wallet.balance().await.expect("the balance").total
}

/// A refused request: its answer, nothing written, the balance unmoved and
/// nothing more posted.
async fn assert_refused(
    wallet: &WalletState,
    app: axum::Router,
    services: &MockWalletServices,
    tx: &[u8],
    remittance: Value,
    message: &str,
) {
    let before = stored(wallet).await;
    let balance_before = balance(wallet).await;
    let posts_before = services.call_count("post_beef");
    let (status, body) = post(app, tx, remittance).await;
    assert_answer(status, &body, "VALIDATION_ERROR", message);
    assert_eq!(stored(wallet).await, before, "nothing written");
    assert_eq!(balance(wallet).await, balance_before, "the balance unmoved");
    assert_eq!(
        services.call_count("post_beef"),
        posts_before,
        "nothing posted"
    );
}

/// (i) The unproven subject's input does not pass: the subject spends a
/// P2PKH parent with the placeholder `00`. 400 `VALIDATION_ERROR`, the
/// interpreter's sentence, `<txid>` the subject's. The control: the same
/// payment over a parent locked by `anyone_lock()` is stored and posted.
/// The refused request was admitted through 0.18.8.
#[tokio::test]
async fn a_payment_whose_unproven_subject_does_not_pass_is_refused() {
    let who = parties();
    let lock = p2pkh(&who.derived.hash160());

    let parent = parent_locked("22", &other_p2pkh());
    let subject = spending(&parent.id(), 1_000, &lock);
    let (tx, txid) = beef_of(&[&parent, &subject], HEIGHT);
    let parent_txid = parent.id();
    let (wallet, app, services) = served_over(&who.root, &[(HEIGHT, &parent_txid)], &txid).await;
    let message = refused(input_offset(&tx, &subject), &txid, EQUALVERIFY);
    assert_refused(&wallet, app, &services, &tx, payment(&who.sender), &message).await;
    assert_eq!(balance(&wallet).await, 0);

    // The control.
    let parent = parent_locked("22", &anyone_lock());
    let subject = spending(&parent.id(), 1_000, &lock);
    let (tx, txid) = beef_of(&[&parent, &subject], HEIGHT);
    let parent_txid = parent.id();
    let (wallet, app, services) = served_over(&who.root, &[(HEIGHT, &parent_txid)], &txid).await;
    let (status, body) = post(app, &tx, payment(&who.sender)).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(balance(&wallet).await, 1_000, "stored");
    assert_eq!(services.call_count("post_beef"), 1, "the control posts");
}

/// (ii) The same in an unproven ANCESTOR: the subject's own input passes,
/// and its unproven parent spends a P2PKH grandparent with `00`. 400
/// `VALIDATION_ERROR`, `<txid>` the parent's. The control: the grandparent
/// locked by `anyone_lock()`, stored and posted. The refused request was
/// admitted through 0.18.8.
#[tokio::test]
async fn a_payment_whose_unproven_ancestor_does_not_pass_is_refused() {
    let who = parties();
    let lock = p2pkh(&who.derived.hash160());

    let grandparent = parent_locked("33", &other_p2pkh());
    let parent = spending(&grandparent.id(), 5_000, &anyone_lock());
    let subject = spending(&parent.id(), 1_000, &lock);
    let (tx, txid) = beef_of(&[&grandparent, &parent, &subject], HEIGHT);
    let proven = grandparent.id();
    let (wallet, app, services) = served_over(&who.root, &[(HEIGHT, &proven)], &txid).await;
    let message = refused(input_offset(&tx, &parent), &parent.id(), EQUALVERIFY);
    assert_refused(&wallet, app, &services, &tx, payment(&who.sender), &message).await;
    assert_eq!(balance(&wallet).await, 0);

    // The control.
    let grandparent = parent_locked("33", &anyone_lock());
    let parent = spending(&grandparent.id(), 5_000, &anyone_lock());
    let subject = spending(&parent.id(), 1_000, &lock);
    let (tx, txid) = beef_of(&[&grandparent, &parent, &subject], HEIGHT);
    let proven = grandparent.id();
    let (wallet, app, services) = served_over(&who.root, &[(HEIGHT, &proven)], &txid).await;
    let (status, body) = post(app, &tx, payment(&who.sender)).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(balance(&wallet).await, 1_000, "stored");
    assert_eq!(services.call_count("post_beef"), 1, "the control posts");
}

/// (iii) A payment spending a coin whose proof is below Genesis is refused
/// as not judged, though the BEEF proves the coin and the tracker confirms
/// its root: `input 0 not judged: coin height 600000 is before Genesis
/// height 620538.` The control: the same shape proven at `HEIGHT`. The
/// refused request was admitted through 0.18.8.
#[tokio::test]
async fn a_payment_spending_a_coin_proven_before_genesis_is_not_judged() {
    let who = parties();
    let lock = p2pkh(&who.derived.hash160());
    let parent = parent_locked("44", &anyone_lock());
    let subject = spending(&parent.id(), 1_000, &lock);
    let parent_txid = parent.id();

    let (tx, txid) = beef_of(&[&parent, &subject], BEFORE_GENESIS);
    let (wallet, app, services) =
        served_over(&who.root, &[(BEFORE_GENESIS, &parent_txid)], &txid).await;
    let message = refused(
        input_offset(&tx, &subject),
        &txid,
        "input 0 not judged: coin height 600000 is before Genesis height 620538.",
    );
    assert_refused(&wallet, app, &services, &tx, payment(&who.sender), &message).await;
    assert_eq!(balance(&wallet).await, 0);

    // The control: the same payment, its parent proven after Genesis.
    let (tx, same) = beef_of(&[&parent, &subject], HEIGHT);
    assert_eq!(same, txid, "the same payment");
    let (wallet, app, services) = served_over(&who.root, &[(HEIGHT, &parent_txid)], &txid).await;
    let (status, body) = post(app, &tx, payment(&who.sender)).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(balance(&wallet).await, 1_000, "stored");
    assert_eq!(services.call_count("post_beef"), 1, "the control posts");
}

/// (iv) "A repeated `internalizeAction` of a transaction the store already
/// holds is checked the same way, ahead of the lookup of the held
/// transaction: a repeat whose BEEF does not pass is refused." The store
/// takes the subject carrying its own BUMP ("A mined transaction carrying
/// its own BUMP is admitted without its scripts run"); its input is the
/// placeholder against a P2PKH. The same subject handed again unproven,
/// beside its proven parent, is refused, nothing written, nothing posted,
/// the balance unmoved. The control: the repeat of the proven BEEF is
/// answered as before and the balance stays. The refused repeat was
/// admitted through 0.18.8.
#[tokio::test]
async fn a_repeated_internalize_whose_beef_does_not_pass_is_refused() {
    let who = parties();
    let lock = p2pkh(&who.derived.hash160());
    let parent = parent_locked("55", &other_p2pkh());
    let subject = spending(&parent.id(), 1_000, &lock);
    let (proven, txid) = beef_of(&[&subject], HEIGHT);
    let below = HEIGHT - 1;
    let (unproven, same) = beef_of(&[&parent, &subject], below);
    assert_eq!(same, txid, "the same subject");
    let parent_txid = parent.id();
    let (wallet, app, services) =
        served_over(&who.root, &[(HEIGHT, &txid), (below, &parent_txid)], &txid).await;

    let (status, body) = post(app.clone(), &proven, payment(&who.sender)).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(balance(&wallet).await, 1_000, "stored");
    assert_eq!(services.call_count("post_beef"), 0, "a proven subject");

    let message = refused(input_offset(&unproven, &subject), &txid, EQUALVERIFY);
    assert_refused(
        &wallet,
        app.clone(),
        &services,
        &unproven,
        payment(&who.sender),
        &message,
    )
    .await;
    assert_eq!(balance(&wallet).await, 1_000);

    // The control.
    let before = stored(&wallet).await;
    let (status, body) = post(app, &proven, payment(&who.sender)).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["accepted"], true, "{body}");
    assert_eq!(stored(&wallet).await, before, "the held transaction");
    assert_eq!(balance(&wallet).await, 1_000);
    assert_eq!(services.call_count("post_beef"), 0, "nothing posted");
}

/// (v) A payment through unmined parents that pass: two unproven ancestors
/// of the subject, the oldest spending a parent proven after Genesis, every
/// input `00` against `anyone_lock()` (the toolbox's "What passes", "a
/// chain of two unproven ancestors among them"). Stored and posted once.
#[tokio::test]
async fn a_payment_through_two_unmined_parents_that_pass_is_stored() {
    let who = parties();
    let proven = parent_locked("66", &anyone_lock());
    let older = spending(&proven.id(), 8_000, &anyone_lock());
    let newer = spending(&older.id(), 6_000, &anyone_lock());
    let subject = spending(&newer.id(), 1_000, &p2pkh(&who.derived.hash160()));
    let (tx, txid) = beef_of(&[&proven, &older, &newer, &subject], HEIGHT);
    let proven_txid = proven.id();
    let (wallet, app, services) = served_over(&who.root, &[(HEIGHT, &proven_txid)], &txid).await;
    let (status, body) = post(app, &tx, payment(&who.sender)).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["accepted"], true, "{body}");
    assert_eq!(balance(&wallet).await, 1_000, "stored");
    assert_eq!(services.call_count("post_beef"), 1, "posted once");
}

/// (v) A transaction one wallet's own `createAction` (`signAndProcess:
/// false`) and `signAction` built and signed, paying a second wallet's
/// derived P2PKH, internalized by that second wallet through its served
/// door: its scripts are run (the payer's P2PKH signature against its proven
/// coin) and pass. Stored and posted once.
#[tokio::test]
async fn a_transaction_a_wallets_own_create_and_sign_built_is_stored_by_another() {
    // The payer holds one proven coin, taken in through its own door.
    let payer = parties();
    let (coin, coin_txid) = proven_beef(&p2pkh(&payer.derived.hash160()));
    let (payer_wallet, payer_app) = served(&payer.root, Some(&coin_txid)).await;
    let (status, body) = post(payer_app, &coin, payment(&payer.sender)).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(balance(&payer_wallet).await, 1_000);

    // The payee's key for the payer's remittance.
    let payee = PrivateKey::random();
    let payer_identity = payer.root.public_key();
    let payee_key = KeyDeriver::new(Some(payee.clone()))
        .derive_public_key(
            &Protocol::new(SecurityLevel::Counterparty, "3241645161d8"),
            &format!("{PREFIX} {SUFFIX}"),
            &Counterparty::Other(payer_identity.clone()),
            true,
        )
        .unwrap();

    let created = payer_wallet
        .create_action(
            CreateActionArgs {
                description: "a payment to another wallet".to_string(),
                input_beef: None,
                inputs: None,
                outputs: Some(vec![CreateActionOutput {
                    locking_script: p2pkh(&payee_key.hash160()),
                    satoshis: 300,
                    output_description: "the payment".to_string(),
                    basket: None,
                    custom_instructions: None,
                    tags: None,
                }]),
                lock_time: None,
                version: None,
                labels: None,
                options: Some(CreateActionOptions {
                    sign_and_process: Some(false),
                    no_send: Some(true),
                    randomize_outputs: Some(false),
                    ..Default::default()
                }),
            },
            "test.local",
        )
        .await
        .expect("the template");
    let signable = created
        .signable_transaction
        .expect("a signable transaction");
    let signed = payer_wallet
        .sign_action(
            SignActionArgs {
                spends: HashMap::new(),
                reference: String::from_utf8(signable.reference).unwrap(),
                options: Some(SignActionOptions {
                    no_send: Some(true),
                    ..Default::default()
                }),
            },
            "test.local",
        )
        .await
        .expect("signed");
    let tx = signed.tx.expect("the signed AtomicBEEF");
    let txid = hex::encode(signed.txid.expect("the txid"));

    let (wallet, app, services) = served_over(&payee, &[(HEIGHT, &coin_txid)], &txid).await;
    let remittance = json!({
        "outputIndex": 0,
        "protocol": "wallet payment",
        "paymentRemittance": {
            "derivationPrefix": PREFIX,
            "derivationSuffix": SUFFIX,
            "senderIdentityKey": payer_identity.to_hex()
        }
    });
    let (status, body) = post(app, &tx, remittance).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["accepted"], true, "{body}");
    assert_eq!(balance(&wallet).await, 300, "stored");
    assert_eq!(services.call_count("post_beef"), 1, "posted once");
}
