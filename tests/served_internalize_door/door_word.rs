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
//! payment the door's post was taken for, the same with no record of the
//! take, one every broadcaster refused, one whose post faulted, one no
//! broadcaster answered, the same body a second time, a body that arrives
//! with its proof, and a store that holds a received payment with no
//! tracker row when the pass runs.
//!
//! A take is the record a broadcaster's acceptance leaves: the broadcast
//! memory's row for the txid, named by its provider, which the concrete
//! `Services` writes after each accepting answer. The toolbox's plain mock
//! keeps no memory, so the broadcaster here is `Remembering`
//! (`tests/support/remembering.rs`): the mock's answers, each acceptance
//! recorded in the memory the storage attached, as `Services` records it.
//!
//! All vectors are synthetic: random throwaway keys, a coinbase-style BUMP
//! checked against a `TestChainTracker`, the toolbox's mock broadcaster.
//! Nothing is sent. The served wallet's own `Services` is the concrete type
//! the server holds (built from the mainnet options, so its explorers and
//! broadcasters are real endpoints), and `Wallet::new` hands it to the
//! storage, through which the toolbox's door posts: every wallet here has
//! its storage's services replaced by a mock before any request (#637), and
//! every read below that needs services is handed a mock of its own.

use crate::remembering::Remembering;
use crate::support::*;

use std::sync::Arc;

use axum::http::StatusCode;
use bsv_wallet_cli::tracker_host::{self, SystemClock, TickOptions};
use bsv_wallet_toolbox::services::mock::{
    error_post_beef_result, MockErrorKind, MockResponse, MockWalletServices,
};
use bsv_wallet_toolbox::services::BlockHeader;
use bsv_wallet_toolbox::WalletServices;
use serde_json::{json, Value};

/// The header service's tip in every read here.
const TIP: u32 = 900_000;
/// The height of a subject that arrives proven: after its parent's.
const SUBJECT_HEIGHT: u32 = PARENT_HEIGHT + 1;

// ---------------------------------------------------------------------------
// The served wallet and its reads
// ---------------------------------------------------------------------------

/// A mock whose broadcasters answer `post_beef` with `response`, each
/// acceptance recorded in the storage's broadcast memory.
fn posting(
    response: MockResponse<Vec<bsv_wallet_toolbox::services::PostBeefResult>>,
) -> Arc<Remembering> {
    Arc::new(Remembering::new(
        MockWalletServices::builder()
            .post_beef_response(response)
            .build(),
    ))
}

/// The mock broadcaster accepting every post, each acceptance recorded.
fn taking() -> Arc<Remembering> {
    Arc::new(Remembering::new(accepting()))
}

/// A served wallet of `paid` whose broadcaster takes every post, recorded.
async fn served_taking(paid: &Paid) -> (WalletState, axum::Router, Arc<Remembering>) {
    let posts = taking();
    let (wallet, app) = served_with(&paid.root, Some(&paid.parent), posts.clone()).await;
    (wallet, app, posts)
}

/// `GET /tx/{txid}`: the status and the answer.
async fn served_word(app: &axum::Router, txid: &str) -> (StatusCode, Value) {
    get_from(app, &format!("/tx/{txid}")).await
}

/// A header service of its own for a read: the tip at [`TIP`], and at
/// `at` a header carrying that root. A mock; it reaches nothing.
async fn headers(at: Option<(u32, &str)>) -> MockWalletServices {
    let services = MockWalletServices::builder().height(TIP).build();
    if let Some((height, root)) = at {
        let tip = services.get_chain_tip_header().await.unwrap();
        services.set_header_for_height(BlockHeader {
            height,
            merkle_root: root.to_string(),
            ..tip
        });
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
        &headers(at).await,
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

/// `POST /internalizeAction` of `tx` as `paid`'s payment, answered 200: the
/// answer.
async fn taken_in(app: &axum::Router, tx: &[u8], paid: &Paid) -> Value {
    let (status, body) = internalize(app, tx, paid.payment()).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    body
}

/// [`door_left`] is 200 with `word`, served and at the gate.
async fn reads(wallet: &WalletState, app: &axum::Router, txid: &str, word: &str) {
    let left = door_left(wallet, app, txid).await;
    assert_eq!(left, (200, word.to_string(), word.to_string()));
}

// ---------------------------------------------------------------------------
// The door's word and the take
// ---------------------------------------------------------------------------

/// The mock broadcaster takes the door's post: the answer is `accepted`
/// with no `broadcast` word, the broadcast memory holds the broadcaster's
/// acceptance, and the creator reads `announced`, served and at the gate,
/// its row built by the host with one accepting hint naming the provider.
#[tokio::test]
async fn a_payment_a_broadcaster_took_reads_announced() {
    let paid = Paid::new();
    let (wallet, app, posts) = served_taking(&paid).await;
    let body = taken_in(&app, &paid.tx, &paid).await;
    assert_eq!(
        body,
        json!({ "accepted": true }),
        "taken: no broadcast word"
    );
    assert_eq!(posts.call_count("post_beef"), 1, "the door posted once");

    reads(&wallet, &app, &paid.txid, "announced").await;
    let (built, hints, word) = tracked(&wallet, &paid.txid).await.expect("a row");
    assert_eq!((built, word.as_str()), (1, "announced"));
    assert_eq!(hints.as_array().map(Vec::len), Some(1), "{hints}");
    assert_eq!(hints[0]["status"], "accepted", "{hints}");
    assert_eq!(hints[0]["source"], "arc|accepted", "the provider: {hints}");
    assert_eq!(
        memory(&wallet, &paid.txid).await,
        vec![("arc".to_string(), "accepted".to_string())]
    );
}

/// The door's post taken with no record of the take (the toolbox's plain
/// mock keeps no broadcast memory): the toolbox's request reads `unmined`,
/// and that status alone is not a take (it reads the same with no post at
/// all, `door_take.rs`), so the creator stays `built`.
#[tokio::test]
async fn a_post_taken_with_no_record_of_the_take_reads_built() {
    let paid = Paid::new();
    let (wallet, app, posts) = paid.served().await;
    taken_in(&app, &paid.tx, &paid).await;
    assert_eq!(posts.call_count("post_beef"), 1, "the door posted once");
    assert_eq!(
        request(&wallet, &paid.txid).await.as_deref(),
        Some("unmined")
    );
    assert_eq!(memory(&wallet, &paid.txid).await, vec![]);
    reads(&wallet, &app, &paid.txid, "built").await;
}

/// The mock's answer for a post every broadcaster refused.
fn refusing() -> Arc<Remembering> {
    posting(MockResponse::Success(vec![error_post_beef_result(
        "arc",
        "the transaction was refused",
    )]))
}

/// A payment taken in over `posts`, whose broadcasters did not take it: the
/// answer is `accepted` with the `broadcast` word `built`, the door posted
/// once, and the creator reads `built`, served and at the gate, its row
/// built by the host with no hint.
async fn untaken_reads_built(posts: Arc<Remembering>) {
    let paid = Paid::new();
    let (wallet, app) = served_with(&paid.root, Some(&paid.parent), posts.clone()).await;
    let body = taken_in(&app, &paid.tx, &paid).await;
    assert_eq!(body["accepted"], true, "{body}");
    assert_eq!(body["broadcast"]["word"], "built", "{body}");
    assert_eq!(posts.call_count("post_beef"), 1, "the door posted once");

    reads(&wallet, &app, &paid.txid, "built").await;
    assert_eq!(
        tracked(&wallet, &paid.txid).await,
        Some((1, json!([]), "built".to_string()))
    );
    assert_eq!(memory(&wallet, &paid.txid).await, vec![]);
}

/// Every broadcaster refuses the door's post: `built`.
#[tokio::test]
async fn a_payment_every_broadcaster_refused_reads_built() {
    untaken_reads_built(refusing()).await;
}

/// The door's post faults (no broadcaster reached): `built`, as a refusal.
#[tokio::test]
async fn a_payment_whose_post_faulted_reads_built() {
    untaken_reads_built(posting(MockResponse::Error(
        MockErrorKind::NetworkError,
        "connection refused".to_string(),
    )))
    .await;
}

/// No broadcaster answers (the services name none: `post_beef` returns no
/// result, as a wallet with no broadcaster configured gives): `built`.
#[tokio::test]
async fn a_payment_with_no_broadcaster_reads_built() {
    untaken_reads_built(posting(MockResponse::Success(vec![]))).await;
}

/// The same body a second time: one row, the same word, the hint heard
/// once, and the door's post not repeated.
#[tokio::test]
async fn a_second_internalize_of_the_same_body_leaves_the_row_as_it_was() {
    let paid = Paid::new();
    let (wallet, app, posts) = served_taking(&paid).await;
    admitted(&app, &paid.tx, paid.payment()).await;
    let once = tracked(&wallet, &paid.txid).await;
    admitted(&app, &paid.tx, paid.payment()).await;
    println!(
        "posts after the second internalize: {}",
        posts.call_count("post_beef")
    );

    reads(&wallet, &app, &paid.txid, "announced").await;
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
    // The payment's own transaction, carried with a proof of its own.
    let paid = Paid::new();
    let tx = paid.carried_proven(SUBJECT_HEIGHT);
    let txid = paid.txid.clone();

    let posts = Arc::new(accepting());
    let roots = [(SUBJECT_HEIGHT, txid.as_str())];
    let (wallet, app) = served_rooted(&paid.root, Some(&roots), posts.clone()).await;
    let body = taken_in(&app, &tx, &paid).await;
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
/// row. The payment a broadcaster took (its acceptance in the broadcast
/// memory) reads `announced`; the one every broadcaster refused reads
/// `built`. A second pass changes neither.
#[tokio::test]
async fn a_pass_gives_a_received_payment_with_no_row_its_word() {
    for (taken, word) in [(true, "announced"), (false, "built")] {
        let paid = Paid::new();
        let posts = if taken { taking() } else { refusing() };
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

        let services = headers(None).await;
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
            reads(&wallet, &app, &paid.txid, word).await;
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

// ---------------------------------------------------------------------------
// A courier's sighting
// ---------------------------------------------------------------------------

/// A payment every broadcaster refused is `built`; a courier that then
/// hands this host its bytes with no proof (`receive`'s couriers) is heard
/// as the crate's mempool presence, and the creator reads `announced`. The
/// same courier a second time adds nothing.
#[tokio::test]
async fn a_couriers_sighting_moves_a_built_payment_to_announced() {
    let paid = Paid::new();
    let (wallet, app) = served_with(&paid.root, Some(&paid.parent), refusing()).await;
    admitted(&app, &paid.tx, paid.payment()).await;
    reads(&wallet, &app, &paid.txid, "built").await;

    for _ in 0..2 {
        let word = tracker_host::door(
            wallet.storage(),
            &SystemClock,
            &TickOptions::default(),
            &paid.txid,
            Some("WhatsOnChain"),
        )
        .await
        .unwrap();
        assert_eq!(word.map(|w| w.0).as_deref(), Some("announced"));
    }
    reads(&wallet, &app, &paid.txid, "announced").await;
    let (_, hints, _) = tracked(&wallet, &paid.txid).await.expect("a row");
    assert_eq!(
        hints,
        json!([{ "source": "WhatsOnChain|mempool_present", "status": "mempool_present",
                 "courier": "WhatsOnChain", "observed": hints[0]["observed"] }])
    );
}
