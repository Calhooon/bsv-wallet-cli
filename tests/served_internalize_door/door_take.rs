//! The take's source (#645, piece P1, after the two cold reads): the word
//! `announced` is this wallet's claim that a broadcaster took a
//! transaction's bytes, so it is never written for a transaction no
//! broadcaster was asked to take. Each test below takes a body in through
//! the served `POST /internalizeAction` on a road where the toolbox makes no
//! post of its own, and reads four things: what the door answered, how many
//! posts the mock broadcaster saw, the toolbox's request status
//! (`proven_tx_reqs`), and the word `GET /tx/{txid}` serves.
//!
//! The roads:
//!
//! - a body carrying for its own subject a merkle path the wallet cannot
//!   confirm, in the three shapes the harness builds: a root the headers do
//!   not hold at that height, a height above the header service's tip, and
//!   a confirmed root at a height above the toolbox's proof gate (stored
//!   `unproven` with its request created `unmined`, nothing posted: the
//!   toolbox's door creates the request `unmined` for a proof it could not
//!   store and posts only an `unsent` one);
//! - the merge of the wallet's own `noSend` transaction (the toolbox's
//!   merge advances its request from `nosend` to `unmined`, nothing
//!   posted), with one user in the store and with two, the second user
//!   holding the same transaction as a received payment.
//!
//! Every wallet here is built so that nothing leaves the process. The
//! storage's services are the toolbox's mock broadcaster, set before any
//! request (`served_rooted` in `tests/support/served_internalize.rs` for one
//! user, [`two_users`] below for two, each checked by pointer). The served
//! wallet's own `Services` is the concrete type the server holds, built
//! from the mainnet options; on the paths these tests drive it is never
//! called: `Wallet::internalize_action` hands the body to the storage, the
//! wallet's own posts of `createAction` and `signAction` run only without
//! `noSend` (every action here is `noSend`), and `GET /tx/{txid}` reads the
//! store alone. Should one be made all the same, it reaches nothing: each
//! test seals the process's network before any `Services` is built
//! (`seal_network`, `src/test_support/seal.rs`, #477).

use crate::support::*;

use std::sync::Arc;

use axum::http::StatusCode;
use bsv_sdk::primitives::PrivateKey;
use bsv_sdk::transaction::Beef;
use bsv_wallet_cli::server::{self, ServerConfig};
use bsv_wallet_toolbox::services::mock::MockWalletServices;
use bsv_wallet_toolbox::{
    Chain, MonitorStorage, Services, ServicesOptions, StorageSqlx, Wallet, WalletStorageProvider,
    WalletStorageReader, WalletStorageWriter,
};
use serde_json::{json, Value};

use crate::chain_tracker::TestChainTracker;
use crate::seal::seal_network;

/// The header service's tip of the tracker `served_rooted` builds.
const TIP: u32 = 900_000;
/// The height of a subject that arrives with a proof: after its parent's.
const SUBJECT_HEIGHT: u32 = PARENT_HEIGHT + 1;

// ---------------------------------------------------------------------------
// The readings
// ---------------------------------------------------------------------------

/// The four readings of one road, and the tracker's row.
#[derive(Debug)]
struct Readings {
    /// The door's status and answer.
    door: (u16, Value),
    /// The posts the mock broadcaster saw.
    posts: usize,
    /// The toolbox's request status of the subject.
    request: Option<String>,
    /// `GET /tx/{txid}`: its status and the word, `(no row)` on a 404.
    served: (u16, String),
}

impl Readings {
    /// The rule these tests decide: with no post the word is never
    /// `announced`.
    fn no_take_without_a_post(&self) {
        if self.posts == 0 {
            assert_ne!(
                self.served.1, "announced",
                "no broadcaster was asked, yet the word is announced: {self:?}"
            );
        }
    }
}

/// The request status of `txid`.
async fn request(storage: &StorageSqlx, txid: &str) -> Option<String> {
    sqlx::query_scalar("SELECT status FROM proven_tx_reqs WHERE txid = ?")
        .bind(txid)
        .fetch_optional(storage.pool())
        .await
        .unwrap()
}

/// What the door left for `txid` after answering `door`, printed for the
/// record of a run.
async fn readings(
    storage: &StorageSqlx,
    app: &axum::Router,
    door: (StatusCode, Value),
    posts: &MockWalletServices,
    txid: &str,
) -> Readings {
    let (status, body) = get_from(app, &format!("/tx/{txid}")).await;
    let word = body["word"].as_str().unwrap_or("(no row)").to_string();
    let hints: Option<String> =
        sqlx::query_scalar("SELECT hints FROM tracker_states WHERE txid = ?")
            .bind(txid)
            .fetch_optional(storage.pool())
            .await
            .unwrap_or_default();
    let read = Readings {
        door: (door.0.as_u16(), door.1),
        posts: posts.call_count("post_beef"),
        request: request(storage, txid).await,
        served: (status.as_u16(), word),
    };
    println!("{txid}: {read:?}; the row's hints {hints:?}");
    read
}

// ---------------------------------------------------------------------------
// (a) A subject proof the wallet cannot confirm
// ---------------------------------------------------------------------------

/// A payment whose subject carries a proof at `height`, taken in by a
/// wallet whose tracker holds `roots` (`(height, root)`; `subject` names
/// the subject's txid) and whose proof gate is `gate` when given (the
/// harness opens it to `u32::MAX` otherwise): the readings.
async fn proven_subject(height: u32, roots: &[(u32, Option<&str>)], gate: Option<u32>) -> Readings {
    seal_network();
    let paid = Paid::new();
    let tx = paid.carried_proven(height);
    let posts = Arc::new(accepting());
    let roots: Vec<(u32, &str)> = roots
        .iter()
        .map(|(h, root)| (*h, root.unwrap_or(paid.txid.as_str())))
        .collect();
    let (wallet, app) = served_rooted(&paid.root, Some(&roots), posts.clone()).await;
    if let Some(gate) = gate {
        MonitorStorage::set_max_acceptable_proof_height(wallet.storage(), gate)
            .await
            .expect("the proof gate");
    }
    let door = internalize(&app, &tx, paid.payment()).await;
    readings(wallet.storage(), &app, door, &posts, &paid.txid).await
}

/// A root the headers do not hold at the subject's height (the tracker
/// holds another root there): the door refuses the body, nothing is
/// written or posted, and there is no word.
#[tokio::test]
async fn a_subject_proof_whose_root_the_headers_do_not_hold() {
    let other = "ab".repeat(32);
    let read = proven_subject(SUBJECT_HEIGHT, &[(SUBJECT_HEIGHT, Some(&other))], None).await;
    read.no_take_without_a_post();
    assert_eq!(read.door.0, 400, "{read:?}");
    assert_eq!(read.posts, 0, "{read:?}");
    assert_eq!(read.request, None, "{read:?}");
    assert_eq!(read.served, (404, "(no row)".to_string()), "{read:?}");
}

/// A subject proof at a height above the header service's tip, its root
/// the tracker's at that height: the readings.
#[tokio::test]
async fn a_subject_proof_above_the_tip() {
    let read = proven_subject(TIP + 1, &[(TIP + 1, None)], None).await;
    read.no_take_without_a_post();
}

/// A confirmed subject proof above the toolbox's proof gate: the body is
/// stored `unproven` with its request created `unmined`, and the door's
/// post is not made. No broadcaster took the bytes: the word is `built`.
#[tokio::test]
async fn a_subject_proof_the_proof_gate_defers() {
    let read = proven_subject(
        SUBJECT_HEIGHT,
        &[(SUBJECT_HEIGHT, None)],
        Some(PARENT_HEIGHT),
    )
    .await;
    read.no_take_without_a_post();
    assert_eq!(read.door, (200, json!({ "accepted": true })), "{read:?}");
    assert_eq!(read.posts, 0, "nothing posted: {read:?}");
    assert_eq!(read.request.as_deref(), Some("unmined"), "{read:?}");
    assert_eq!(read.served, (200, "built".to_string()), "{read:?}");
}

// ---------------------------------------------------------------------------
// (b) The merge of a `noSend` transaction
// ---------------------------------------------------------------------------

/// `wallet`'s `createAction` of 1,000 satoshis to `lock` with the
/// `underReceivedRule` label, signed and not sent, funded by the payment
/// `funding` (its only coin): its `AtomicBEEF` over the funding's BEEF, its
/// txid and its change output's index.
async fn not_sent(wallet: &WalletState, funding: &Paid, lock: &[u8]) -> (Vec<u8>, String, u32) {
    let signed = spend_to(wallet, lock, true).await.expect("the action");
    let txid = signed.id();
    let change: i64 = sqlx::query_scalar("SELECT vout FROM outputs WHERE txid = ? AND change = 1")
        .bind(&txid)
        .fetch_one(wallet.storage().pool())
        .await
        .expect("the change output");
    let mut beef = Beef::from_binary(&funding.tx).expect("the funding's BEEF");
    beef.merge_raw_tx(signed.to_binary(), None).unwrap();
    let tx = beef.to_binary_atomic(&txid).unwrap();
    (tx, txid, u32::try_from(change).unwrap())
}

/// `wallet`'s own change `txid.vout` named back to it as a wallet payment
/// with the derivation its row holds and its own identity key as the
/// sender: the merge the toolbox advances from `nosend`.
async fn own_change_back(
    wallet: &WalletState,
    root: &PrivateKey,
    txid: &str,
    vout: u32,
) -> Vec<Value> {
    let (_, _, _, prefix, suffix, _) = row(wallet, txid, vout).await;
    vec![payment_of(
        vout,
        &prefix.expect("a prefix"),
        &suffix.expect("a suffix"),
        &root.public_key().to_hex(),
    )]
}

/// One user: a funded wallet's `noSend` action merged back through its own
/// door. Nothing is posted for it; the word is `built`.
#[tokio::test]
async fn the_merge_of_a_no_send_action_with_one_user() {
    seal_network();
    let funding = Paid::new();
    let (wallet, app, posts) = funding.stored().await;
    assert_eq!(posts.call_count("post_beef"), 1, "the funding's post");
    let (tx, txid, change) = not_sent(&wallet, &funding, &other_lock()).await;
    println!(
        "before the merge: request {:?}",
        request(wallet.storage(), &txid).await
    );

    let outputs = own_change_back(&wallet, &funding.root, &txid, change).await;
    let door = internalize(&app, &tx, outputs).await;
    let read = readings(wallet.storage(), &app, door, &posts, &txid).await;
    assert_eq!(read.door.0, 200, "{read:?}");
    assert_eq!(read.posts, 1, "the funding's post alone: {read:?}");
    assert_eq!(read.request.as_deref(), Some("unmined"), "{read:?}");
    assert_ne!(read.served.1, "announced", "{read:?}");
}

/// Two served wallets of `roots` over one store: their storage handles are
/// clones of one `StorageSqlx` (one pool, one services slot), so the mock
/// set after both wallets are built is every storage's services; the
/// tracker carries `proven` at [`PARENT_HEIGHT`].
async fn two_users(
    roots: [&PrivateKey; 2],
    proven: &str,
    posts: Arc<MockWalletServices>,
) -> [(WalletState, axum::Router); 2] {
    let storage = StorageSqlx::in_memory().await.expect("in-memory storage");
    storage
        .migrate("door-take-two-users", &roots[0].public_key().to_hex())
        .await
        .expect("migrate");
    storage.make_available().await.expect("make available");
    MonitorStorage::set_max_acceptable_proof_height(&storage, u32::MAX)
        .await
        .expect("open the proof gate");
    let mut tracker = TestChainTracker::new(TIP);
    tracker.add_root(PARENT_HEIGHT, proven.to_string());
    storage.set_chain_tracker(Arc::new(tracker)).await;
    let served = |root: &PrivateKey, storage: StorageSqlx| {
        let root = root.clone();
        async move {
            let services =
                Services::with_options(Chain::Main, ServicesOptions::mainnet()).expect("services");
            let wallet = Wallet::new(Some(root), storage, services)
                .await
                .expect("wallet");
            server::make_wallet_state(wallet)
        }
    };
    let a = served(roots[0], storage.clone()).await;
    let b = served(roots[1], storage.clone()).await;
    // After both `Wallet::new` calls, each of which hands the storage its
    // own `Services`: the one services slot both handles share.
    WalletStorageProvider::set_services(&storage, posts.clone());
    for state in [&a, &b] {
        let held = WalletStorageReader::get_services(state.storage()).expect("services");
        assert!(
            std::ptr::eq(
                Arc::as_ptr(&held).cast::<()>(),
                Arc::as_ptr(&posts).cast::<()>()
            ),
            "every storage handle posts through the mock"
        );
    }
    let config = || ServerConfig::admitting(["test.local"]).expect("an originator");
    let app_a = server::make_router(a.clone(), config());
    let app_b = server::make_router(b.clone(), config());
    [(a, app_a), (b, app_b)]
}

/// Two users in one store: the sender's `noSend` action pays the second
/// user, who takes it in (its request already exists, `nosend`: no post);
/// then the sender merges its action back (the request advanced to
/// `unmined`: no post). No broadcaster was asked at either door, so
/// neither reads `announced`.
#[tokio::test]
async fn the_merge_of_a_no_send_action_with_two_users() {
    seal_network();
    let sender_root = PrivateKey::random();
    let receiver_root = PrivateKey::random();
    let funding = Paid::to(&sender_root);
    let posts = Arc::new(accepting());
    let [(sender, sender_app), (receiver, receiver_app)] = two_users(
        [&sender_root, &receiver_root],
        &funding.parent,
        posts.clone(),
    )
    .await;
    admitted(&sender_app, &funding.tx, funding.payment()).await;
    assert_eq!(posts.call_count("post_beef"), 1, "the funding's post");

    let sender_key = sender_root.public_key();
    let lock = p2pkh(&derived(&receiver_root, &sender_key, PREFIX, SUFFIX));
    let (tx, txid, change) = not_sent(&sender, &funding, &lock).await;
    println!(
        "the action: request {:?}",
        request(sender.storage(), &txid).await
    );

    let paid = vec![payment_of(0, PREFIX, SUFFIX, &sender_key.to_hex())];
    let door = internalize(&receiver_app, &tx, paid).await;
    let read = readings(receiver.storage(), &receiver_app, door, &posts, &txid).await;
    assert_eq!(read.door.0, 200, "{read:?}");
    read.no_take_without_a_post();
    assert_eq!(read.posts, 1, "the funding's post alone: {read:?}");

    let outputs = own_change_back(&sender, &sender_root, &txid, change).await;
    let door = internalize(&sender_app, &tx, outputs).await;
    let read = readings(sender.storage(), &sender_app, door, &posts, &txid).await;
    assert_eq!(read.door.0, 200, "{read:?}");
    assert_eq!(read.request.as_deref(), Some("unmined"), "{read:?}");
    assert_eq!(read.posts, 1, "the funding's post alone: {read:?}");
    read.no_take_without_a_post();
    let (_, word) = get_from(&receiver_app, &format!("/tx/{txid}")).await;
    assert_ne!(word["word"], "announced", "{word}");
    // One row for the transaction, both users': the word is about it.
    let rows: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM tracker_states WHERE txid = ?")
        .bind(&txid)
        .fetch_one(sender.storage().pool())
        .await
        .unwrap();
    assert_eq!(rows, 1);
}
