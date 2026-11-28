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
//! Arcade, Bitails and `WhatsOnChain` at their mainnet addresses); the spend
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

use crate::remembering::Remembering;
use crate::seal::seal_network;
use crate::support::*;

use std::sync::Arc;

use axum::http::StatusCode;
use bsv_sdk::wallet::WalletInterface;
use bsv_wallet_cli::server::handlers::{create_action_args, internalize_args};
use bsv_wallet_cli::spend_gate::GateOptions;
use bsv_wallet_cli::tracker_host::{self, TickOptions};
use bsv_wallet_toolbox::services::mock::{
    error_post_beef_result, success_post_beef_result, MockResponse, MockWalletServices,
};
use bsv_wallet_toolbox::services::PostBeefResult;
use bsv_wallet_toolbox::{StorageSqlx, Wallet, WalletServices};
use serde_json::{json, Value};

/// The broadcaster's answer to every post.
type Answer = MockResponse<Vec<PostBeefResult>>;

/// The broadcaster takes the post.
fn taking() -> Answer {
    MockResponse::Success(vec![success_post_beef_result("arc", &[])])
}

/// Every broadcaster refuses the post.
fn refusing() -> Answer {
    MockResponse::Success(vec![error_post_beef_result(
        "arc",
        "the transaction was refused",
    )])
}

/// A mock header service at 900,000: the actions are not mined, so no
/// header decides their word.
fn headers() -> MockWalletServices {
    MockWalletServices::builder().height(900_000).build()
}

/// The `createAction` request: 1,000 sats to another key under
/// `allowPending: received`, with `options`.
fn request(options: &Value) -> Value {
    json!({
        "description": "an action of the create door",
        "outputs": [{
            "lockingScript": hex::encode(other_lock()),
            "satoshis": 1000,
            "outputDescription": "a payment",
        }],
        "options": options,
        "allowPending": "received",
    })
}

/// The creator's word of `txid`: as stored (what `GET /tx/{txid}` serves;
/// `(no row)` without one) and as the spend gate reads it over `services`.
async fn stored_and_gate<V: WalletServices>(
    storage: &StorageSqlx,
    services: &V,
    txid: &str,
) -> (String, String) {
    let stored = tracker_host::stored_word(storage, txid)
        .await
        .unwrap()
        .map_or("(no row)".to_string(), |(word, ..)| word);
    let readings = tracker_host::read_creators(
        storage,
        services,
        &TickOptions::default(),
        &[(txid.to_string(), None)],
    )
    .await
    .unwrap();
    (stored, readings[txid].word.to_string())
}

/// The wallet's own actions holding a tracker row.
async fn rows_of_ours(storage: &StorageSqlx) -> i64 {
    sqlx::query_scalar(
        "SELECT COUNT(*) FROM tracker_states t JOIN transactions x ON lower(x.txid) = t.txid \
         WHERE x.is_outgoing = 1",
    )
    .fetch_one(storage.pool())
    .await
    .unwrap_or(0)
}

// ---------------------------------------------------------------------------
// The served doors
// ---------------------------------------------------------------------------

/// A served wallet of `paid`, paid through its door, its storage's
/// broadcaster the mock taking every post, recorded.
async fn paid_wallet(paid: &Paid) -> (WalletState, axum::Router, Arc<Remembering>) {
    seal_network();
    let posts = Arc::new(Remembering::new(
        MockWalletServices::builder()
            .post_beef_response(taking())
            .build(),
    ));
    let (wallet, app) = served_with(&paid.root, Some(&paid.parent), posts.clone()).await;
    admitted(&app, &paid.tx, paid.payment()).await;
    assert_eq!(balance(&wallet).await, 4_500);
    (wallet, app, posts)
}

/// `POST route` with `body`, answered 200: the answer.
async fn served_ok(app: &axum::Router, route: &str, body: Value) -> Value {
    let (status, answer) = post_to(app, route, body).await;
    assert_eq!(status, StatusCode::OK, "{answer}");
    answer
}

/// What the served wallet reads for the action `answer` names: the route's
/// status and word (`(no row)` on its 404) and the gate's word.
async fn served_words(
    wallet: &WalletState,
    app: &axum::Router,
    answer: &Value,
) -> (u16, String, String) {
    let txid = answer["txid"].as_str().expect("a txid");
    let (status, body) = get_from(app, &format!("/tx/{txid}")).await;
    let (_, gate) = stored_and_gate(wallet.storage(), &headers(), txid).await;
    let word = body["word"].as_str().unwrap_or("(no row)").to_string();
    println!("{txid}: served {status} {body}; the gate reads {gate}");
    (status.as_u16(), word, gate)
}

/// The served `createAction` with `options` reads `built`, served 200 and
/// at the gate; the broadcaster's posts after the payment's.
async fn served_reads_built(options: &Value) -> usize {
    let paid = Paid::new();
    let (wallet, app, posts) = paid_wallet(&paid).await;
    let answer = served_ok(&app, "/createAction", request(options)).await;
    assert_eq!(
        served_words(&wallet, &app, &answer).await,
        (200, "built".to_string(), "built".to_string())
    );
    posts.call_count("post_beef") - 1
}

/// Signed and posted: every broadcaster unreachable, so no take; `built`.
#[tokio::test]
async fn a_posted_action_reads_built() {
    served_reads_built(&json!({})).await;
}

/// Signed with `noSend`: not posted, `built`, and it stays there.
#[tokio::test]
async fn a_no_send_action_reads_built() {
    assert_eq!(served_reads_built(&json!({ "noSend": true })).await, 0);
}

/// Signed with delayed broadcast: `built` until it is sent.
#[tokio::test]
async fn a_delayed_action_reads_built() {
    served_reads_built(&json!({ "acceptDelayedBroadcast": true })).await;
}

/// An unsigned template has no row; `signAction` signs it and posts it
/// (every broadcaster unreachable): `built`.
#[tokio::test]
async fn a_template_signed_by_sign_action_reads_built() {
    let paid = Paid::new();
    let (wallet, app, _) = paid_wallet(&paid).await;
    let template = served_ok(
        &app,
        "/createAction",
        request(&json!({ "signAndProcess": false })),
    )
    .await;
    let reference = template["signableTransaction"]["reference"].clone();
    assert_eq!(
        rows_of_ours(wallet.storage()).await,
        0,
        "no row for the template"
    );
    let signed = served_ok(
        &app,
        "/signAction",
        json!({ "reference": reference, "spends": {} }),
    )
    .await;
    assert_eq!(
        served_words(&wallet, &app, &signed).await,
        (200, "built".to_string(), "built".to_string())
    );
}

// ---------------------------------------------------------------------------
// The gate's door, its broadcaster's word in the broadcast memory
// ---------------------------------------------------------------------------

type Remembered = Wallet<StorageSqlx, Remembering>;

/// A wallet paid 4,500 sats whose own services are [`Remembering`] over a
/// mock answering its posts with `answer` (the payment's post taken): each
/// acceptance is recorded in the store's broadcast memory. The payment is
/// taken in through the wallet's `internalizeAction` and the host's door
/// word, as `fund` takes one.
async fn remembered(answer: Answer) -> Remembered {
    let paid = Paid::new();
    let roots = [(PARENT_HEIGHT, paid.parent.as_str())];
    let storage = rooted_store(&paid.root, Some(&roots)).await;
    let services = Remembering::new(
        MockWalletServices::builder()
            .height(900_000)
            .post_beef_response(MockResponse::Sequence(vec![taking(), answer]))
            .build(),
    );
    let wallet = Wallet::new(Some(paid.root.clone()), storage, services)
        .await
        .expect("wallet");
    let req = serde_json::from_value(json!({
        "tx": paid.tx,
        "outputs": paid.payment(),
        "description": "a payment",
    }))
    .expect("the payment's request");
    let args = internalize_args(req);
    let taken = wallet
        .internalize_action(args, Some("create.door"))
        .await
        .expect("the payment");
    assert!(taken.accepted);
    tracker_host::door_word(wallet.storage(), &paid.txid).await;
    wallet
}

/// The gate's door, as the served route calls it, for [`request`] signed
/// and posted: the action's stored word and the gate's.
async fn through_the_gate(wallet: &Remembered) -> (String, String) {
    let req = serde_json::from_value(request(&json!({}))).expect("the request");
    let (args, allow_pending) = create_action_args(req).expect("the arguments");
    let gate = GateOptions::default().with_allow_pending(allow_pending);
    let made = bsv_wallet_cli::spend_gate::create_action(wallet, args, "create.door", &gate)
        .await
        .expect("the action");
    assert_eq!(
        wallet.services().call_count("post_beef"),
        2,
        "the payment's and the action's"
    );
    let txid = hex::encode(made.txid.expect("a txid"));
    let read = stored_and_gate(wallet.storage(), wallet.services(), &txid).await;
    println!("{txid}: stored and at the gate {read:?}");
    read
}

/// Signed and posted, the broadcaster takes it: `built` at signing, then
/// the take heard, `announced`.
#[tokio::test]
async fn an_action_a_broadcaster_took_reads_announced() {
    let wallet = remembered(taking()).await;
    let announced = "announced".to_string();
    assert_eq!(
        through_the_gate(&wallet).await,
        (announced.clone(), announced)
    );
}

/// Signed and posted, every broadcaster refuses it: `built`, a refusal is
/// no take.
#[tokio::test]
async fn an_action_every_broadcaster_refused_reads_built() {
    let wallet = remembered(refusing()).await;
    let built = "built".to_string();
    assert_eq!(through_the_gate(&wallet).await, (built.clone(), built));
}
