//! The create door's word (#659, lane L1), served and hermetic: the
//! tracker's word for an action this wallet signs, read straight after the
//! door's answer and before any pass, as this host serves it
//! (`GET /tx/{txid}`) and as the spend gate reads the creator of the
//! action's own change (`tracker_host::read_creators`).
//!
//! The stack's wallet model writes `built` when the wallet signs an action
//! of its own (bsv-stack-lean `lean/Wallet/Default.lean`, `onCreateD`: a
//! signed action takes the tracker's host act `build`), and a
//! broadcaster's take of the bytes moves it to `announced`, as at the
//! internalize door (#645). The keeper's answer K3: a signed action of ours
//! whose creator reads `unknown` is a host that did not record its own act.
//! The roads here: the served `createAction` signed and posted, signed
//! with `noSend`, signed with delayed broadcast, and an unsigned template
//! completed by `signAction`; and the spend gate's door the served route
//! calls (`spend_gate::create_action`), over a wallet whose broadcaster
//! takes the post, or refuses it.
//!
//! Each wallet spends a received payment of 4,500 sats (the payment is
//! unmined, so each action names `allowPending: received`).
//!
//! What the served wallet's concrete `Services` can reach (#637): it is
//! built from the mainnet options with no header service, and the wallet
//! posts its own actions through it (`Wallet::create_action` and
//! `Wallet::sign_action` hand the signed action to the wallet's services:
//! Arcade, Bitails and WhatsOnChain at their mainnet addresses); the spend
//! gate's header reads and the post-broadcast verifier's explorers go
//! through it too. Every non-loopback request of this process goes to a
//! closed local port (`seal_network`), so each is a fault at once and the
//! served action's post is "could not look", never a take: its word is
//! `built`. The storage's services, through which the internalize door
//! posts the payment, are the toolbox's mock before any request.
//!
//! The gate's door is driven over a wallet whose own services are
//! `Remembering` (`tests/support/remembering.rs`): the toolbox's mock, each
//! acceptance recorded in the broadcast memory as the concrete `Services`
//! records it, so a take is the broadcast memory's row, as in production.
//! Nothing is sent.

#[path = "../src/test_support/chain_tracker.rs"]
mod chain_tracker;
#[path = "../src/test_support/no_tracker.rs"]
#[allow(dead_code)]
#[macro_use]
mod no_tracker;
#[path = "support/remembering.rs"]
#[allow(dead_code)]
mod remembering;
#[path = "../src/test_support/seal.rs"]
#[allow(dead_code)]
mod seal;
#[path = "support/served_internalize.rs"]
#[allow(dead_code)]
mod support;

use remembering::Remembering;
use seal::seal_network;
use support::*;

use std::sync::Arc;

use axum::http::StatusCode;
use bsv_sdk::wallet::{
    CreateActionArgs, CreateActionOutput, InternalizeActionArgs, InternalizeOutput,
    WalletInterface, WalletPayment,
};
use bsv_wallet_cli::spend_gate::{AllowPending, GateOptions};
use bsv_wallet_cli::tracker_host::{self, TickOptions};
use bsv_wallet_toolbox::services::mock::{
    error_post_beef_result, MockResponse, MockWalletServices,
};
use bsv_wallet_toolbox::{StorageSqlx, Wallet, WalletStorageWriter};
use serde_json::{json, Value};

/// A served wallet paid 4,500 sats, its broadcaster answering `post_beef`
/// with `response`, each acceptance recorded.
async fn paid_wallet(
    response: MockResponse<Vec<bsv_wallet_toolbox::services::PostBeefResult>>,
) -> (WalletState, axum::Router, Arc<Remembering>) {
    seal_network();
    let paid = Paid::new();
    let posts = Arc::new(Remembering::new(
        MockWalletServices::builder()
            .post_beef_response(response)
            .build(),
    ));
    let (wallet, app) = served_with(&paid.root, Some(&paid.parent), posts.clone()).await;
    admitted(&app, &paid.tx, paid.payment()).await;
    assert_eq!(balance(&wallet).await, 4_500);
    (wallet, app, posts)
}

/// The broadcaster takes every post.
fn taking() -> MockResponse<Vec<bsv_wallet_toolbox::services::PostBeefResult>> {
    MockResponse::Success(accepting_results())
}

fn accepting_results() -> Vec<bsv_wallet_toolbox::services::PostBeefResult> {
    vec![bsv_wallet_toolbox::services::mock::success_post_beef_result("arc", &[])]
}

/// `POST /createAction` of 1,000 sats to another key under
/// `allowPending: received`, with `options`: the answer, 200.
async fn create(app: &axum::Router, options: Value) -> Value {
    let (status, body) = post_to(
        app,
        "/createAction",
        json!({
            "description": "an action of the create door",
            "outputs": [{
                "lockingScript": hex::encode(other_lock()),
                "satoshis": 1000,
                "outputDescription": "a payment",
            }],
            "options": options,
            "allowPending": "received",
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    body
}

/// The creator's word of `txid`: served (`GET /tx/{txid}`: its status and
/// its word, `(no row)` on a 404) and as the gate reads it, the gate's
/// header service a mock of its own (the action is not mined, so no header
/// decides it).
async fn words(wallet: &WalletState, app: &axum::Router, txid: &str) -> (u16, String, String) {
    let (status, body) = get_from(app, &format!("/tx/{txid}")).await;
    let headers = MockWalletServices::builder().height(900_000).build();
    let readings = tracker_host::read_creators(
        wallet.storage(),
        &headers,
        &TickOptions::default(),
        &[(txid.to_string(), None)],
    )
    .await
    .unwrap();
    let status_word: Option<String> =
        sqlx::query_scalar("SELECT status FROM transactions WHERE txid = ?")
            .bind(txid)
            .fetch_optional(wallet.storage().pool())
            .await
            .unwrap();
    let read = (
        status.as_u16(),
        body["word"].as_str().unwrap_or("(no row)").to_string(),
        readings[txid].word.to_string(),
    );
    println!("{txid}: status {status_word:?}; served and at the gate {read:?}; {body}");
    read
}

/// `word`, served 200 and at the gate.
fn reads(word: &str) -> (u16, String, String) {
    (200, word.to_string(), word.to_string())
}

/// The txid an answer names.
fn txid_of(body: &Value) -> String {
    body["txid"].as_str().expect("a txid").to_string()
}

/// Signed and posted: every broadcaster unreachable, so no take; `built`.
#[tokio::test]
async fn a_posted_action_reads_built() {
    let (wallet, app, _) = paid_wallet(taking()).await;
    let body = create(&app, json!({})).await;
    let txid = txid_of(&body);
    assert_eq!(words(&wallet, &app, &txid).await, reads("built"));
}

/// Signed with `noSend`: not posted, `built`, and it stays there.
#[tokio::test]
async fn a_no_send_action_reads_built() {
    let (wallet, app, posts) = paid_wallet(taking()).await;
    let body = create(&app, json!({ "noSend": true })).await;
    assert_eq!(posts.call_count("post_beef"), 1, "the payment's alone");
    let txid = txid_of(&body);
    assert_eq!(words(&wallet, &app, &txid).await, reads("built"));
}

/// Signed with delayed broadcast: `built` until it is sent.
#[tokio::test]
async fn a_delayed_action_reads_built() {
    let (wallet, app, _) = paid_wallet(taking()).await;
    let body = create(&app, json!({ "acceptDelayedBroadcast": true })).await;
    let txid = txid_of(&body);
    assert_eq!(words(&wallet, &app, &txid).await, reads("built"));
}

/// The wallet's own actions holding a tracker row.
async fn rows_of_ours(wallet: &WalletState) -> i64 {
    sqlx::query_scalar(
        "SELECT COUNT(*) FROM tracker_states t JOIN transactions x ON lower(x.txid) = t.txid \
         WHERE x.is_outgoing = 1",
    )
    .fetch_one(wallet.storage().pool())
    .await
    .unwrap_or(0)
}

/// An unsigned template has no row; `signAction` signs it and posts it
/// (every broadcaster unreachable): `built`.
#[tokio::test]
async fn a_template_signed_by_sign_action_reads_built() {
    let (wallet, app, _) = paid_wallet(taking()).await;
    let template = create(&app, json!({ "signAndProcess": false })).await;
    let reference = template["signableTransaction"]["reference"]
        .as_str()
        .expect("a reference")
        .to_string();
    assert_eq!(rows_of_ours(&wallet).await, 0, "no row for the template");
    let (status, signed) = post_to(
        &app,
        "/signAction",
        json!({ "reference": reference, "spends": {} }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{signed}");
    let txid = txid_of(&signed);
    assert_eq!(words(&wallet, &app, &txid).await, reads("built"));
}

// ---------------------------------------------------------------------------
// The gate's door, its broadcaster's word in the broadcast memory
// ---------------------------------------------------------------------------

type Remembered = Wallet<StorageSqlx, Remembering>;

/// A wallet paid 4,500 sats whose own services are [`Remembering`] over a
/// mock answering `post_beef` with `response`: its posts are the mock's and
/// each acceptance is recorded in the store's broadcast memory. The
/// payment is taken in through the wallet's `internalizeAction` and the
/// host's door word, as `fund` takes one.
async fn remembered(
    response: MockResponse<Vec<bsv_wallet_toolbox::services::PostBeefResult>>,
) -> Remembered {
    let paid = Paid::new();
    let storage = StorageSqlx::in_memory().await.expect("in-memory storage");
    storage
        .migrate("served-create-door", &paid.root.public_key().to_hex())
        .await
        .expect("migrate");
    storage.make_available().await.expect("make available");
    bsv_wallet_toolbox::MonitorStorage::set_max_acceptable_proof_height(&storage, u32::MAX)
        .await
        .expect("open the proof gate");
    let mut tracker = chain_tracker::TestChainTracker::new(900_000);
    tracker.add_root(PARENT_HEIGHT, paid.parent.clone());
    storage.set_chain_tracker(Arc::new(tracker)).await;
    let services = Remembering::new(
        MockWalletServices::builder()
            .height(900_000)
            .post_beef_response(response)
            .build(),
    );
    let wallet = Wallet::new(Some(paid.root.clone()), storage, services)
        .await
        .expect("wallet");
    let taken = wallet
        .internalize_action(
            InternalizeActionArgs {
                tx: paid.tx.clone(),
                outputs: vec![InternalizeOutput {
                    output_index: 0,
                    protocol: "wallet payment".to_string(),
                    payment_remittance: Some(WalletPayment {
                        derivation_prefix: PREFIX.to_string(),
                        derivation_suffix: SUFFIX.to_string(),
                        sender_identity_key: paid.sender.to_hex(),
                    }),
                    insertion_remittance: None,
                }],
                description: "a payment".to_string(),
                labels: None,
                seek_permission: None,
            },
            Some("create.door"),
        )
        .await
        .expect("the payment");
    assert!(taken.accepted);
    tracker_host::door_word(wallet.storage(), &paid.txid).await;
    wallet
}

/// The gate's door: 1,000 sats to another key under `allowPending:
/// received`, signed and posted. Its txid.
async fn through_the_gate(wallet: &Remembered) -> String {
    let args = CreateActionArgs {
        description: "an action of the create door".to_string(),
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
        labels: None,
        options: None,
    };
    let gate = GateOptions::default().with_allow_pending(AllowPending::Received);
    let made = bsv_wallet_cli::spend_gate::create_action(wallet, args, "create.door", &gate)
        .await
        .expect("the action");
    hex::encode(made.txid.expect("a txid"))
}

/// The creator's word of `txid` on `wallet`: stored (what `GET /tx/{txid}`
/// serves; `(no row)` without one) and as the gate reads it.
async fn remembered_words(wallet: &Remembered, txid: &str) -> (String, String) {
    let served = tracker_host::stored_word(wallet.storage(), txid)
        .await
        .unwrap()
        .map_or("(no row)".to_string(), |(word, ..)| word);
    let readings = tracker_host::read_creators(
        wallet.storage(),
        wallet.services(),
        &TickOptions::default(),
        &[(txid.to_string(), None)],
    )
    .await
    .unwrap();
    let read = (served, readings[txid].word.to_string());
    println!("{txid}: stored and at the gate {read:?}");
    read
}

/// Signed and posted, the broadcaster takes it: `built` at signing, then
/// the take heard, `announced`.
#[tokio::test]
async fn an_action_a_broadcaster_took_reads_announced() {
    let wallet = remembered(taking()).await;
    let txid = through_the_gate(&wallet).await;
    assert_eq!(
        wallet.services().call_count("post_beef"),
        2,
        "the payment's and the action's"
    );
    assert_eq!(
        remembered_words(&wallet, &txid).await,
        ("announced".to_string(), "announced".to_string())
    );
}

/// Signed and posted, every broadcaster refuses it: `built`, a refusal is
/// no take. The payment's own post was taken, so it is spendable under the
/// word; the action's is refused.
#[tokio::test]
async fn an_action_every_broadcaster_refused_reads_built() {
    let wallet = remembered(MockResponse::Sequence(vec![
        MockResponse::Success(accepting_results()),
        MockResponse::Success(vec![error_post_beef_result(
            "arc",
            "the transaction was refused",
        )]),
    ]))
    .await;
    let txid = through_the_gate(&wallet).await;
    assert_eq!(
        remembered_words(&wallet, &txid).await,
        ("built".to_string(), "built".to_string())
    );
}
