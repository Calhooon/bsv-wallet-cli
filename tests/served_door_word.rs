//! The door's word and the take (#645, piece P1), served and hermetic: the
//! tracker's word for the creator of a received payment after
//! `POST /internalizeAction`, as this host serves it (`GET /tx/{txid}`) and
//! as the spend gate reads it (`tracker_host::read_creators`).
//!
//! The stack's wallet gate vectors (bsv-stack-lean `590ca1a5`,
//! `conformance/wallet-gate-vectors.json`; `docs/p0/
//! reorg-proof-cut2-2026-11-18.md`, "Cut 2e′") say a new received body with
//! no proof is `built` at the door, and a named broadcaster taking the bytes
//! moves it to `announced`. Each test names what it reads; the cases are a
//! payment the door's post was taken for, one every broadcaster refused, one
//! whose post faulted, one no broadcaster answered, the same body a second time,
//! a body that arrives with its proof, and a store that holds a received
//! payment with no tracker row when the pass runs.
//!
//! All vectors are synthetic: random throwaway keys, a coinbase-style BUMP
//! checked against a `TestChainTracker`, the toolbox's mock broadcaster.
//! Nothing is sent. The served wallet's own `Services` is the concrete type
//! the server holds (built from the mainnet options, so its explorers and
//! broadcasters are real endpoints), and `Wallet::new` hands it to the
//! storage, through which the toolbox's door posts: every wallet here has
//! its storage's services replaced by a mock before any request (#637), and
//! every read below that needs services is handed a mock of its own.

#[path = "../src/test_support/chain_tracker.rs"]
mod chain_tracker;
#[path = "../src/test_support/no_tracker.rs"]
#[allow(dead_code)]
mod no_tracker;
#[path = "support/served_internalize.rs"]
#[allow(dead_code)]
mod support;
use support::*;

use std::sync::Arc;

use axum::http::StatusCode;
use bsv_sdk::primitives::PrivateKey;
use bsv_sdk::transaction::{Beef, MerklePath};
use bsv_wallet_cli::server::{self, ServerConfig};
use bsv_wallet_cli::tracker_host::{self, SystemClock, TickOptions};
use bsv_wallet_toolbox::services::mock::{
    error_post_beef_result, MockErrorKind, MockResponse, MockWalletServices,
};
use bsv_wallet_toolbox::services::BlockHeader;
use bsv_wallet_toolbox::{
    Chain, Services, ServicesOptions, StorageSqlx, Wallet, WalletServices, WalletStorageProvider,
    WalletStorageWriter,
};
use chain_tracker::TestChainTracker;
use serde_json::{json, Value};
use tower::ServiceExt;

/// The header service's tip in every read here.
const TIP: u32 = 900_000;
/// The height of a subject that arrives proven: after its parent's.
const SUBJECT_HEIGHT: u32 = PARENT_HEIGHT + 1;

// ---------------------------------------------------------------------------
// The served wallet and its reads
// ---------------------------------------------------------------------------

/// [`support::served_with`], with every root the tracker carries named.
/// `posts` is never optional: `Wallet::new` hands the storage the wallet's
/// own `Services` (the mainnet options, real endpoints), and the toolbox's
/// door posts through whatever the storage holds, so a served wallet whose
/// storage is not handed a mock posts to a real broadcaster (#637).
async fn served_over(
    root: &PrivateKey,
    roots: &[(u32, &str)],
    posts: Arc<dyn WalletServices>,
) -> (WalletState, axum::Router) {
    let storage = StorageSqlx::in_memory().await.expect("in-memory storage");
    storage
        .migrate("served-door-word", &root.public_key().to_hex())
        .await
        .expect("migrate");
    storage.make_available().await.expect("make available");
    bsv_wallet_toolbox::MonitorStorage::set_max_acceptable_proof_height(&storage, u32::MAX)
        .await
        .expect("open the proof gate");
    let mut tracker = TestChainTracker::new(TIP);
    for (height, root) in roots {
        tracker.add_root(*height, (*root).to_string());
    }
    storage.set_chain_tracker(Arc::new(tracker)).await;
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

/// A mock whose broadcasters answer `post_beef` with `response`.
fn posting(
    response: MockResponse<Vec<bsv_wallet_toolbox::services::PostBeefResult>>,
) -> Arc<MockWalletServices> {
    Arc::new(
        MockWalletServices::builder()
            .post_beef_response(response)
            .build(),
    )
}

/// `GET /tx/{txid}`: the status and the answer.
async fn served_word(app: &axum::Router, txid: &str) -> (StatusCode, Value) {
    let request = axum::http::Request::builder()
        .method("GET")
        .uri(format!("/tx/{txid}"))
        .header("Origin", "http://test.local")
        .body(axum::body::Body::empty())
        .unwrap();
    let response = app.clone().oneshot(request).await.unwrap();
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    (status, serde_json::from_slice(&bytes).unwrap())
}

/// A header at `height` carrying `root`.
fn header(height: u32, root: &str) -> BlockHeader {
    BlockHeader {
        version: 1,
        previous_hash: "00".repeat(32),
        merkle_root: root.to_string(),
        time: 1_700_000_000,
        bits: 0x1d00_ffff,
        nonce: 0,
        hash: "0e".repeat(32),
        height,
    }
}

/// A header service of its own for a read: the tip at [`TIP`], and at
/// `at` the header carrying that root. A mock; it reaches nothing.
fn headers(at: Option<(u32, &str)>) -> MockWalletServices {
    let services = MockWalletServices::builder().build();
    services.set_tip_header(header(TIP, &"0f".repeat(32)));
    if let Some((height, root)) = at {
        services.set_header_for_height(header(height, root));
    }
    services
}

/// The creator's word as the spend gate reads it, with the stored proof the
/// gate hands over (the toolbox's `proven_txs` path, if any).
async fn gate_word(wallet: &WalletState, txid: &str, at: Option<(u32, &str)>) -> &'static str {
    let path: Option<Vec<u8>> =
        sqlx::query_scalar("SELECT merkle_path FROM proven_txs WHERE txid = ?")
            .bind(txid)
            .fetch_optional(wallet.storage().pool())
            .await
            .unwrap();
    let readings = tracker_host::read_creators(
        wallet.storage(),
        &headers(at),
        &TickOptions::default(),
        &[(txid.to_string(), path)],
    )
    .await
    .unwrap();
    readings[txid].word
}

/// The tracker's row of `txid`: whether the host built it, its hints, its
/// word; `None` with no row.
async fn tracked(wallet: &WalletState, txid: &str) -> Option<(i64, Value, String)> {
    tracker_host::stored_word(wallet.storage(), txid)
        .await
        .unwrap();
    let row: Option<(i64, String, String)> =
        sqlx::query_as("SELECT built, hints, word FROM tracker_states WHERE txid = ?")
            .bind(txid)
            .fetch_optional(wallet.storage().pool())
            .await
            .unwrap();
    row.map(|(built, hints, word)| (built, serde_json::from_str(&hints).unwrap(), word))
}

/// The proof request's status of `txid`.
async fn request(wallet: &WalletState, txid: &str) -> Option<String> {
    sqlx::query_scalar("SELECT status FROM proven_tx_reqs WHERE txid = ?")
        .bind(txid)
        .fetch_optional(wallet.storage().pool())
        .await
        .unwrap()
}

/// The broadcast memory's rows of `txid` (`provider`, `status`).
async fn memory(wallet: &WalletState, txid: &str) -> Vec<(String, String)> {
    sqlx::query_as("SELECT provider, status FROM broadcast_seen WHERE txid = ?")
        .bind(txid)
        .fetch_all(wallet.storage().pool())
        .await
        .unwrap_or_default()
}

/// What the door left for `txid`, printed for the record of a run: the
/// served word, the gate's, the tracker's row, the request and the memory.
async fn door_left(wallet: &WalletState, app: &axum::Router, txid: &str) -> (u16, String, String) {
    let (status, body) = served_word(app, txid).await;
    let gate = gate_word(wallet, txid, None).await;
    println!(
        "the door left: GET /tx -> {status} {body}; the gate reads {gate}; row {:?}; request {:?}; \
         memory {:?}",
        tracked(wallet, txid).await,
        request(wallet, txid).await,
        memory(wallet, txid).await,
    );
    let word = body["word"].as_str().unwrap_or("(no row)").to_string();
    (status.as_u16(), word, gate.to_string())
}

// ---------------------------------------------------------------------------
// The door's word and the take
// ---------------------------------------------------------------------------

/// The mock broadcaster takes the door's post: the answer is `accepted`
/// with no `broadcast` word, and the creator reads `announced`, served and
/// at the gate, its row built by the host with one accepting hint.
#[tokio::test]
async fn a_payment_a_broadcaster_took_reads_announced() {
    let paid = Paid::new();
    let (wallet, app, posts) = paid.served().await;
    let (status, body) = internalize(&app, &paid.tx, paid.payment()).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(
        body,
        json!({ "accepted": true }),
        "taken: no broadcast word"
    );
    assert_eq!(posts.call_count("post_beef"), 1, "the door posted once");

    let left = door_left(&wallet, &app, &paid.txid).await;
    assert_eq!(
        left,
        (200, "announced".to_string(), "announced".to_string())
    );
    let (built, hints, word) = tracked(&wallet, &paid.txid).await.expect("a row");
    assert_eq!((built, word.as_str()), (1, "announced"));
    assert_eq!(hints.as_array().map(Vec::len), Some(1), "{hints}");
    assert_eq!(hints[0]["status"], "accepted", "{hints}");
}

/// Every broadcaster refuses the door's post: the answer is `accepted` with
/// the `broadcast` word `built`, and the creator reads `built`, served and
/// at the gate, with no hint.
#[tokio::test]
async fn a_payment_every_broadcaster_refused_reads_built() {
    let paid = Paid::new();
    let posts = posting(MockResponse::Success(vec![error_post_beef_result(
        "arc",
        "the transaction was refused",
    )]));
    let (wallet, app) = served_with(&paid.root, Some(&paid.parent), posts.clone()).await;
    let (status, body) = internalize(&app, &paid.tx, paid.payment()).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["accepted"], true, "{body}");
    assert_eq!(body["broadcast"]["word"], "built", "{body}");
    assert_eq!(posts.call_count("post_beef"), 1, "the door posted once");

    let left = door_left(&wallet, &app, &paid.txid).await;
    assert_eq!(left, (200, "built".to_string(), "built".to_string()));
    let (built, hints, _) = tracked(&wallet, &paid.txid).await.expect("a row");
    assert_eq!((built, hints), (1, json!([])));
}

/// The door's post faults (no broadcaster reached): `built`, as a refusal.
#[tokio::test]
async fn a_payment_whose_post_faulted_reads_built() {
    let paid = Paid::new();
    let posts = posting(MockResponse::Error(
        MockErrorKind::NetworkError,
        "connection refused".to_string(),
    ));
    let (wallet, app) = served_with(&paid.root, Some(&paid.parent), posts.clone()).await;
    let (status, body) = internalize(&app, &paid.tx, paid.payment()).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["broadcast"]["word"], "built", "{body}");

    let left = door_left(&wallet, &app, &paid.txid).await;
    assert_eq!(left, (200, "built".to_string(), "built".to_string()));
}

/// No broadcaster answers (the services name none: `post_beef` returns no
/// result, as a wallet with no broadcaster configured gives): the creator
/// reads `built`.
#[tokio::test]
async fn a_payment_with_no_broadcaster_reads_built() {
    let paid = Paid::new();
    let posts = posting(MockResponse::Success(vec![]));
    let (wallet, app) = served_with(&paid.root, Some(&paid.parent), posts.clone()).await;
    let (status, body) = internalize(&app, &paid.tx, paid.payment()).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["accepted"], true, "{body}");
    assert_eq!(body["broadcast"]["word"], "built", "{body}");

    let left = door_left(&wallet, &app, &paid.txid).await;
    assert_eq!(left, (200, "built".to_string(), "built".to_string()));
    assert_eq!(
        memory(&wallet, &paid.txid).await,
        vec![],
        "no broadcaster named"
    );
}

/// The same body a second time: one row, the same word, the hint heard
/// once, and the door's post not repeated.
#[tokio::test]
async fn a_second_internalize_of_the_same_body_leaves_the_row_as_it_was() {
    let paid = Paid::new();
    let (wallet, app, posts) = paid.stored().await;
    let once = tracked(&wallet, &paid.txid).await;
    admitted(&app, &paid.tx, paid.payment()).await;
    println!(
        "posts after the second internalize: {}",
        posts.call_count("post_beef")
    );

    let left = door_left(&wallet, &app, &paid.txid).await;
    assert_eq!(
        left,
        (200, "announced".to_string(), "announced".to_string())
    );
    let twice = tracked(&wallet, &paid.txid).await;
    assert_eq!(twice, once, "the row is unchanged");
    let (_, hints, _) = twice.expect("a row");
    assert_eq!(
        hints.as_array().map(Vec::len),
        Some(1),
        "heard once: {hints}"
    );
    let rows: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM tracker_states")
        .fetch_one(wallet.storage().pool())
        .await
        .unwrap();
    assert_eq!(rows, 1);
}

/// A body that arrives with its proof: the door posts nothing, the gate
/// reads `mined` from the stored proof against the header at its height,
/// and the door writes no row for it (the pass adopts a proven creator with
/// its checked proof), so the route answers 404 until then.
#[tokio::test]
async fn a_body_that_arrives_with_its_proof_reads_mined() {
    let root = PrivateKey::random();
    let sender = PrivateKey::random().public_key();
    let lock = p2pkh(&derived(&root, &sender, PREFIX, SUFFIX));
    let (_, child) = paying(&lock, 5);
    let txid = child.id();
    let mut beef = Beef::new();
    let bump = beef.merge_bump(MerklePath::from_coinbase_txid(&txid, SUBJECT_HEIGHT));
    beef.merge_raw_tx(child.to_binary(), Some(bump)).unwrap();
    let tx = beef.to_binary_atomic(&txid).unwrap();

    let posts = Arc::new(accepting());
    let (wallet, app) = served_over(&root, &[(SUBJECT_HEIGHT, &txid)], posts.clone()).await;
    let (status, body) = internalize(
        &app,
        &tx,
        vec![payment_of(0, PREFIX, SUFFIX, &sender.to_hex())],
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body, json!({ "accepted": true }));
    assert_eq!(
        posts.call_count("post_beef"),
        0,
        "a proven body is not posted"
    );

    let gate = gate_word(&wallet, &txid, Some((SUBJECT_HEIGHT, &txid))).await;
    let (status, served) = served_word(&app, &txid).await;
    println!(
        "a proven body: the gate reads {gate}; GET /tx -> {status} {served}; row {:?}",
        tracked(&wallet, &txid).await
    );
    assert_eq!(gate, "mined");
    assert_eq!(status, StatusCode::NOT_FOUND, "{served}");
}

// ---------------------------------------------------------------------------
// A store that already holds a received payment with no row
// ---------------------------------------------------------------------------

/// A store holding a received, unproven payment and no tracker row for it
/// (as every store written before this door's word): one pass gives it its
/// row. The payment a broadcaster took (its request `unmined`, no broadcast
/// memory row: the mock keeps none) reads `announced`; the one every
/// broadcaster refused reads `built`. A second pass changes neither.
#[tokio::test]
async fn a_pass_gives_a_received_payment_with_no_row_its_word() {
    for (taken, word) in [(true, "announced"), (false, "built")] {
        let paid = Paid::new();
        let posts = if taken {
            Arc::new(accepting())
        } else {
            posting(MockResponse::Success(vec![error_post_beef_result(
                "arc",
                "the transaction was refused",
            )]))
        };
        let (wallet, app) = served_with(&paid.root, Some(&paid.parent), posts).await;
        admitted(&app, &paid.tx, paid.payment()).await;
        // The store as an earlier release left it: no row.
        tracker_host::stored_word(wallet.storage(), &paid.txid)
            .await
            .unwrap();
        sqlx::query("DELETE FROM tracker_states")
            .execute(wallet.storage().writer())
            .await
            .unwrap();
        assert_eq!(tracked(&wallet, &paid.txid).await, None);
        println!(
            "seeded without a row: taken {taken}, request {:?}, memory {:?}",
            request(&wallet, &paid.txid).await,
            memory(&wallet, &paid.txid).await
        );

        let services = headers(None);
        for pass in 1..=2 {
            let report = tracker_host::tick(
                wallet.storage(),
                &services,
                &SystemClock,
                &TickOptions::default(),
            )
            .await
            .unwrap();
            println!("pass {pass}: {}", report.summary());
            let left = door_left(&wallet, &app, &paid.txid).await;
            assert_eq!(
                left,
                (200, word.to_string(), word.to_string()),
                "pass {pass}, taken {taken}"
            );
        }
        let (built, hints, _) = tracked(&wallet, &paid.txid).await.expect("a row");
        assert_eq!(built, 1);
        assert_eq!(
            hints.as_array().map(Vec::len),
            Some(usize::from(taken)),
            "{hints}"
        );
    }
}
