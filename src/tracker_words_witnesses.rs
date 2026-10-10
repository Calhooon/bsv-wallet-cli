//! The CLI's host acts against the tracker's words, at the toolbox 0.7.4
//! (bsv-stack-lean #66; the tracker charter sections 1, 4 and 5).
//!
//! Each retire path of the CLI is driven twice: on a transaction a
//! broadcaster took (the tracker's `announced` or later: never abandoned or
//! failed, #24) and on one no broadcaster took (`built`: the host may retire
//! it). The `/abortAction` door is driven on a taken transaction. The spend
//! guard is driven on a coin whose transaction's last word is an
//! immediate-refusal hint, then once a status source holds the transaction.
//!
//! Each test uses only what 0.7.2 also has, so the same file runs on 0.7.2
//! (with `retire_guard` and `spend_guard` absent there, stood in by no-ops):
//! there the taken transactions are failed and the held coin is selected.
//!
//! Real SQLite storage (migrated in memory or in a temporary directory),
//! the toolbox's mock services, local HTTP fixtures; random throwaway keys;
//! nothing reaches the network and nothing is broadcast.

use std::net::SocketAddr;
use std::sync::Arc;

use axum::http::StatusCode;
use axum::routing::get;
use axum::Router;
use bsv_sdk::primitives::{hash160, PrivateKey};
use bsv_sdk::script::{LockingScript, UnlockingScript};
use bsv_sdk::transaction::{Transaction, TransactionInput, TransactionOutput};
use bsv_sdk::wallet::{
    Counterparty, CreateActionArgs, CreateActionOptions, CreateActionOutput, KeyDeriverApi,
    ProtoWallet, Protocol, SecurityLevel, WalletInterface,
};
use bsv_wallet_toolbox::services::mock::{MockResponse, MockWalletServices};
use bsv_wallet_toolbox::{
    GetStatusForTxidsResult, PoisonOutcome, StorageSqlx, TxStatusDetail, Wallet, WalletServices,
    WalletStorageProvider, WalletStorageWriter, BROADCAST_STATUS_ACCEPTED,
    BROADCAST_STATUS_REJECTED, PROVIDER_ARCADE_V2,
};
use chrono::Utc;

use crate::broadcast_reconcile::{run_pass, run_sweep, ReconcileOptions};
use crate::broadcast_verify::{
    BroadcastVerification, BroadcastVerifier, ChainIndexAnswer, NetworkEvidence, PresenceReport,
};
use crate::commands::cleanup_abandoned::{reconcile_with, InputSpend};

const OLD: &str = "2026-01-01T00:00:00+00:00";

/// A migrated in-memory wallet storage for `identity`: (storage, user id,
/// default basket id).
async fn storage_for(identity: &str) -> (StorageSqlx, i64, i64) {
    let storage = StorageSqlx::in_memory().await.unwrap();
    storage.migrate("witness-073", identity).await.unwrap();
    storage.make_available().await.unwrap();
    let (user, _) = storage.find_or_insert_user(identity).await.unwrap();
    let basket = storage
        .find_or_create_default_basket(user.user_id)
        .await
        .unwrap()
        .basket_id;
    (storage, user.user_id, basket)
}

async fn insert_tx(storage: &StorageSqlx, user_id: i64, txid: &str, status: &str) -> i64 {
    sqlx::query(
        "INSERT INTO transactions (user_id, status, reference, is_outgoing, satoshis, version, lock_time, description, txid, raw_tx, created_at, updated_at) \
         VALUES (?, ?, ?, 1, 0, 1, 0, 'witness', ?, X'01000000', ?, ?)",
    )
    .bind(user_id)
    .bind(status)
    .bind(format!("ref-{}", &txid[..8]))
    .bind(txid)
    .bind(OLD)
    .bind(Utc::now())
    .execute(storage.pool())
    .await
    .unwrap()
    .last_insert_rowid()
}

async fn insert_req(storage: &StorageSqlx, txid: &str, status: &str, history: &str) {
    sqlx::query(
        "INSERT INTO proven_tx_reqs (txid, status, attempts, history, notified, notify, raw_tx, created_at, updated_at) \
         VALUES (?, ?, 1, ?, 0, '{}', X'01000000', ?, ?)",
    )
    .bind(txid)
    .bind(status)
    .bind(history)
    .bind(OLD)
    .bind(OLD)
    .execute(storage.pool())
    .await
    .unwrap();
}

async fn insert_output(
    storage: &StorageSqlx,
    user_id: i64,
    basket: i64,
    tx_row: i64,
    txid: &str,
    spendable: bool,
    spent_by: Option<i64>,
) -> i64 {
    let lock = hex::decode("76a914dbc0a7c84983c5bf199b7b2d41b3acf0408ee5aa88ac").unwrap();
    sqlx::query(
        "INSERT INTO outputs (user_id, transaction_id, basket_id, vout, satoshis, locking_script, txid, type, spendable, change, spent_by, provided_by, purpose, output_description, created_at, updated_at) \
         VALUES (?, ?, ?, 0, 5000, ?, ?, 'P2PKH', ?, 1, ?, 'storage', 'change', 'c', ?, ?)",
    )
    .bind(user_id)
    .bind(tx_row)
    .bind(basket)
    .bind(&lock)
    .bind(txid)
    .bind(spendable as i64)
    .bind(spent_by)
    .bind(Utc::now())
    .bind(Utc::now())
    .execute(storage.pool())
    .await
    .unwrap()
    .last_insert_rowid()
}

async fn tx_status(storage: &StorageSqlx, txid: &str) -> String {
    sqlx::query_scalar("SELECT status FROM transactions WHERE txid = ?")
        .bind(txid)
        .fetch_one(storage.pool())
        .await
        .unwrap()
}

async fn output_state(storage: &StorageSqlx, id: i64) -> (i64, Option<i64>) {
    sqlx::query_as("SELECT spendable, spent_by FROM outputs WHERE output_id = ?")
        .bind(id)
        .fetch_one(storage.pool())
        .await
        .unwrap()
}

/// A completed parent G whose coin is locked by our transaction T
/// (`t_status`, its request `req_status`): (storage, G's coin, T's row).
async fn spent_coin_fixture(t: &str, t_status: &str, req_status: &str) -> (StorageSqlx, i64, i64) {
    let (storage, user_id, basket) = storage_for(&("02".to_string() + &"ab".repeat(32))).await;
    let g = insert_tx(&storage, user_id, &"11".repeat(32), "completed").await;
    let t_row = insert_tx(&storage, user_id, t, t_status).await;
    insert_req(&storage, t, req_status, "{}").await;
    let coin = insert_output(
        &storage,
        user_id,
        basket,
        g,
        &"11".repeat(32),
        false,
        Some(t_row),
    )
    .await;
    insert_output(&storage, user_id, basket, t_row, t, true, None).await;
    (storage, coin, t_row)
}

fn seen_by_arcade_absent_from_chain() -> PresenceReport {
    PresenceReport {
        verification: BroadcastVerification::Confirmed,
        evidence: Some(NetworkEvidence::Seen),
        evidence_provider: PROVIDER_ARCADE_V2,
        chain_index: ChainIndexAnswer::Absent,
        broadcaster_fatal: false,
        network_absent: true,
    }
}

fn absent_everywhere() -> PresenceReport {
    PresenceReport {
        verification: BroadcastVerification::Rejected,
        evidence: None,
        evidence_provider: bsv_wallet_toolbox::BROADCAST_PROVIDER_NETWORK,
        chain_index: ChainIndexAnswer::Absent,
        broadcaster_fatal: false,
        network_absent: true,
    }
}

/// A local server answering every lookup path with `code` and `body`.
async fn answering(code: StatusCode, body: &'static str) -> String {
    let handler = move || async move {
        let mut resp = axum::response::Response::new(axum::body::Body::from(body));
        *resp.status_mut() = code;
        resp.headers_mut().insert(
            reqwest::header::CONTENT_TYPE.as_str(),
            "application/json".parse().unwrap(),
        );
        resp
    };
    let app = Router::new()
        .route("/tx/{txid}", get(handler))
        .route("/v1/tx/{txid}", get(handler))
        .route("/tx/hash/{txid}", get(handler));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr: SocketAddr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.ok();
    });
    format!("http://{}", addr)
}

// ---------------------------------------------------------------------------
// The daemon's abandoned-transaction ticker and `cleanup-abandoned`
// ---------------------------------------------------------------------------

/// RED at 0.7.2: Arcade accepted T (its request `unmined`) and still holds
/// it; the chain index has not seen it for longer than the threshold. 0.7.2
/// abandoned it (`failed`, its coin released). The rule: a broadcaster took
/// it, so it stays, its coin locked.
#[tokio::test]
async fn the_daemon_keeps_an_announced_transaction_absent_past_the_threshold() {
    let t = "a1".repeat(32);
    let (storage, coin, _) = spent_coin_fixture(&t, "unproven", "unmined").await;
    let report = reconcile_with(
        storage.pool(),
        0,
        30,
        true,
        |_txid| async { seen_by_arcade_absent_from_chain() },
        |_src, _vout| async { InputSpend::Unspent },
    )
    .await
    .unwrap();
    assert!(report.dead_txids().is_empty(), "{:?}", report.dead_txids());
    assert_eq!(tx_status(&storage, &t).await, "unproven");
    assert_eq!(
        output_state(&storage, coin).await.0,
        0,
        "the coin stays locked"
    );
}

/// RED at 0.7.2: a broadcaster took T (`unmined`), then neither it nor the
/// chain index could find it. 0.7.2 abandoned it. The rule: kept.
#[tokio::test]
async fn the_daemon_keeps_an_announced_transaction_absent_everywhere() {
    let t = "a2".repeat(32);
    let (storage, coin, _) = spent_coin_fixture(&t, "unproven", "unmined").await;
    let report = reconcile_with(
        storage.pool(),
        0,
        30,
        true,
        |_txid| async { absent_everywhere() },
        |_src, _vout| async { InputSpend::Unspent },
    )
    .await
    .unwrap();
    assert!(report.abandoned.is_empty(), "{:?}", report.abandoned);
    assert_eq!(tx_status(&storage, &t).await, "unproven");
    assert_eq!(output_state(&storage, coin).await.0, 0);
}

/// The other way: no broadcaster ever took T (its post drew no accepting
/// word: `sending`, the request `unsent`), and it is absent everywhere. The
/// host may abandon `built`: failed, its coin back. Green at 0.7.2 too.
#[tokio::test]
async fn the_daemon_abandons_a_transaction_no_broadcaster_took() {
    let t = "a3".repeat(32);
    let (storage, coin, _) = spent_coin_fixture(&t, "sending", "unsent").await;
    let report = reconcile_with(
        storage.pool(),
        0,
        30,
        true,
        |_txid| async { absent_everywhere() },
        |_src, _vout| async { InputSpend::Unspent },
    )
    .await
    .unwrap();
    assert_eq!(report.abandoned, vec![t.clone()]);
    assert_eq!(tx_status(&storage, &t).await, "failed");
    assert_eq!(
        output_state(&storage, coin).await,
        (1, None),
        "the coin is back"
    );
}

// ---------------------------------------------------------------------------
// The served sweep (every 60 s in `serve`; the daemon's ticker)
// ---------------------------------------------------------------------------

/// RED at 0.7.2: Arcade accepted T, then pushed REJECTED. At the toolbox
/// 0.7.4 the push is a hint (no word written), but the memory row turns
/// `rejected`, and 0.7.2's sweep retired every unproven transaction with a
/// `rejected` row. The rule: a broadcaster took it; kept.
#[tokio::test]
async fn the_served_sweep_keeps_an_announced_transaction_a_broadcaster_refused_later() {
    let t = "b1".repeat(32);
    let (storage, coin, _) = spent_coin_fixture(&t, "unproven", "unmined").await;
    storage
        .record_broadcast_status(&t, PROVIDER_ARCADE_V2, BROADCAST_STATUS_ACCEPTED)
        .await
        .unwrap();
    storage
        .record_broadcast_status(&t, PROVIDER_ARCADE_V2, BROADCAST_STATUS_REJECTED)
        .await
        .unwrap();
    let services = MockWalletServices::new();
    let sweep = run_sweep(&storage, &services, true).await.unwrap();
    assert!(
        sweep
            .poison
            .iter()
            .all(|r| r.outcome != PoisonOutcome::Retired),
        "nothing retired"
    );
    assert_eq!(tx_status(&storage, &t).await, "unproven");
    assert_eq!(output_state(&storage, coin).await.0, 0);
}

/// The other way: a broadcaster refused T on its post and none took it.
/// The sweep retires it (the host's act on `built`). Green at 0.7.2 too.
#[tokio::test]
async fn the_served_sweep_retires_a_refused_transaction_no_broadcaster_took() {
    let t = "b2".repeat(32);
    let (storage, _, _) = spent_coin_fixture(&t, "sending", "unsent").await;
    storage
        .record_broadcast_status(&t, PROVIDER_ARCADE_V2, BROADCAST_STATUS_REJECTED)
        .await
        .unwrap();
    let services = MockWalletServices::new();
    let sweep = run_sweep(&storage, &services, true).await.unwrap();
    assert!(sweep
        .poison
        .iter()
        .any(|r| r.outcome == PoisonOutcome::Retired));
    assert_eq!(tx_status(&storage, &t).await, "failed");
}

// ---------------------------------------------------------------------------
// The by-hand pass (`reconcile-broadcasts`)
// ---------------------------------------------------------------------------

/// RED at 0.7.2: the broadcaster says SEEN_MULTIPLE_NODES for T, the chain
/// index 404s, T is past the absence threshold. 0.7.2 retired it as a
/// phantom. The rule: the broadcaster holding it is its acceptance; named
/// absent, kept.
#[tokio::test]
async fn the_by_hand_pass_keeps_a_transaction_the_broadcaster_holds_absent_past_the_threshold() {
    let t = "c1".repeat(32);
    let (storage, coin, _) = spent_coin_fixture(&t, "unproven", "unmined").await;
    let broadcaster = answering(
        StatusCode::OK,
        r#"{"txid":"x","txStatus":"SEEN_MULTIPLE_NODES"}"#,
    )
    .await;
    let chain = answering(
        StatusCode::NOT_FOUND,
        r#"{"error":"transaction not found"}"#,
    )
    .await;
    let verifier = BroadcastVerifier::explicit(true, &broadcaster, Some(&chain));
    let services = MockWalletServices::new();
    let opts = ReconcileOptions {
        execute: true,
        max_probes: 20,
        max_age_hours: None,
        absence_minutes: 30,
        max_locked_checks: 20,
        sse: None,
    };
    let report = run_pass(&storage, &services, &verifier, &opts)
        .await
        .unwrap();
    assert_eq!(report.absent.len(), 1, "named absent");
    assert!(report
        .retired
        .iter()
        .all(|r| r.outcome != PoisonOutcome::Retired));
    assert_eq!(tx_status(&storage, &t).await, "unproven");
    assert_eq!(output_state(&storage, coin).await.0, 0);
}

// ---------------------------------------------------------------------------
// The served follow-up after a broadcast
// ---------------------------------------------------------------------------

/// RED at 0.7.2: a broadcaster accepted T (`unmined`); the background probe
/// then found it neither at the broadcaster nor at the chain index, and
/// 0.7.2 retired it. The rule: kept.
#[tokio::test]
async fn the_served_follow_up_keeps_an_accepted_transaction_it_cannot_find() {
    let t = "d1".repeat(32);
    let (storage, coin, _) = spent_coin_fixture(&t, "unproven", "unmined").await;
    let services = MockWalletServices::new();
    crate::server::broadcast_follow_up::apply_presence_report(
        &storage,
        &services,
        &t,
        &[],
        &absent_everywhere(),
    )
    .await;
    assert_eq!(tx_status(&storage, &t).await, "unproven");
    assert_eq!(output_state(&storage, coin).await.0, 0);
}

/// The other way: no broadcaster took T and the probe finds it nowhere:
/// retired, as at 0.7.2.
#[tokio::test]
async fn the_served_follow_up_retires_a_transaction_no_broadcaster_took() {
    let t = "d2".repeat(32);
    let (storage, _, _) = spent_coin_fixture(&t, "sending", "unsent").await;
    let services = MockWalletServices::new();
    crate::server::broadcast_follow_up::apply_presence_report(
        &storage,
        &services,
        &t,
        &[],
        &absent_everywhere(),
    )
    .await;
    assert_eq!(tx_status(&storage, &t).await, "failed");
}

// ---------------------------------------------------------------------------
// The `/abortAction` door
// ---------------------------------------------------------------------------

/// RED at 0.7.2: the toolbox honours the abort of a broadcast transaction
/// whenever the wallet holds no chain evidence for it, so a transaction
/// Arcade accepted was failed and its coin released. The rule: the door
/// refuses it (409), nothing written.
#[tokio::test]
async fn the_abort_door_refuses_a_transaction_a_broadcaster_took() {
    let key = PrivateKey::random();
    let identity = key.public_key().to_hex();
    let tmp = tempfile::TempDir::new().unwrap();
    let db = tmp.path().join("abort.db");
    let storage = StorageSqlx::open(db.to_str().unwrap()).await.unwrap();
    storage.migrate("witness-073", &identity).await.unwrap();
    storage.make_available().await.unwrap();
    let (user, _) = storage.find_or_insert_user(&identity).await.unwrap();
    let basket = storage
        .find_or_create_default_basket(user.user_id)
        .await
        .unwrap()
        .basket_id;
    let t = "e1".repeat(32);
    let g = insert_tx(&storage, user.user_id, &"11".repeat(32), "completed").await;
    let t_row = insert_tx(&storage, user.user_id, &t, "unproven").await;
    insert_req(&storage, &t, "unmined", "{}").await;
    storage
        .record_broadcast_status(&t, PROVIDER_ARCADE_V2, BROADCAST_STATUS_ACCEPTED)
        .await
        .unwrap();
    let coin = insert_output(
        &storage,
        user.user_id,
        basket,
        g,
        &"11".repeat(32),
        false,
        Some(t_row),
    )
    .await;
    let pool = storage.pool().clone();

    let services = bsv_wallet_toolbox::Services::with_options(
        bsv_wallet_toolbox::Chain::Main,
        bsv_wallet_toolbox::ServicesOptions::mainnet(),
    )
    .unwrap();
    let wallet = Wallet::new(Some(key), storage, services).await.unwrap();
    let app = crate::server::make_router(
        crate::server::make_wallet_state(wallet),
        crate::server::ServerConfig {
            auth_token: Some("witness-bearer".to_string()),
            ..Default::default()
        },
    );
    // The router in process: no listener, no HTTP client.
    let request = axum::http::Request::builder()
        .method("POST")
        .uri("/abortAction")
        .header("Authorization", "Bearer witness-bearer")
        .header("Origin", "http://witness.local")
        .header("Content-Type", "application/json")
        .body(axum::body::Body::from(
            serde_json::json!({ "reference": t }).to_string(),
        ))
        .unwrap();
    let resp = tower::ServiceExt::oneshot(app, request).await.unwrap();
    let status = resp.status().as_u16();
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let body: serde_json::Value = serde_json::from_slice(&bytes).unwrap_or_default();
    assert_eq!(status, 409, "{body}");
    let (word,): (String,) = sqlx::query_as("SELECT status FROM transactions WHERE txid = ?")
        .bind(&t)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(word, "unproven");
    let (spendable,): (i64,) = sqlx::query_as("SELECT spendable FROM outputs WHERE output_id = ?")
        .bind(coin)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(spendable, 0, "the coin stays locked");
}

// ---------------------------------------------------------------------------
// The spend guard
// ---------------------------------------------------------------------------

const BRC29_PROTOCOL: &str = "3241645161d8";
const PREFIX: &str = "dGVzdC1wcmVmaXg=";
const SUFFIX: &str = "dGVzdC1zdWZmaXg=";

fn pay_args() -> CreateActionArgs {
    CreateActionArgs {
        description: "pay someone".to_string(),
        input_beef: None,
        inputs: None,
        outputs: Some(vec![CreateActionOutput {
            locking_script: hex::decode("76a914dbc0a7c84983c5bf199b7b2d41b3acf0408ee5aa88ac")
                .unwrap(),
            satoshis: 1_000,
            output_description: "payment".to_string(),
            basket: None,
            custom_instructions: None,
            tags: None,
        }]),
        lock_time: None,
        version: None,
        labels: None,
        options: Some(CreateActionOptions {
            sign_and_process: Some(true),
            accept_delayed_broadcast: Some(false),
            ..Default::default()
        }),
    }
}

/// The wallet's one coin: the change output of an internalized transaction
/// P whose immediate post the broadcaster refused, as the toolbox 0.7.4
/// leaves it (`internalizeAction` answered `accepted`; P `unproven`; its
/// request `unsent` with the `immediateBroadcastHint` on its history; the
/// output spendable). Returns (storage, root key, P's txid, the coin).
async fn refused_coin() -> (StorageSqlx, PrivateKey, String, i64) {
    let root = PrivateKey::random();
    let identity = root.public_key().to_hex();
    let (storage, user_id, basket) = storage_for(&identity).await;
    let sk = ProtoWallet::new(Some(root.clone()))
        .key_deriver()
        .derive_private_key(
            &Protocol::new(SecurityLevel::Counterparty, BRC29_PROTOCOL),
            &format!("{} {}", PREFIX, SUFFIX),
            &Counterparty::Self_,
        )
        .unwrap();
    let mut lock = vec![0x76, 0xa9, 0x14];
    lock.extend_from_slice(&hash160(&sk.public_key().to_compressed()));
    lock.extend([0x88, 0xac]);
    // G: a completed transaction of a stranger's, whose output P spends.
    let grand = Transaction::with_params(
        1,
        vec![TransactionInput {
            source_transaction: None,
            source_txid: Some("00".repeat(32)),
            source_output_index: 0xffff_ffff,
            unlocking_script: Some(UnlockingScript::from_hex("00").unwrap()),
            unlocking_script_template: None,
            sequence: 0xffff_ffff,
        }],
        vec![TransactionOutput {
            satoshis: Some(100_500),
            locking_script: LockingScript::from_binary(
                &hex::decode("76a914dbc0a7c84983c5bf199b7b2d41b3acf0408ee5aa88ac").unwrap(),
            )
            .unwrap(),
            change: false,
        }],
        0,
    );
    let now = Utc::now();
    sqlx::query(
        "INSERT INTO transactions (user_id, status, reference, is_outgoing, satoshis, version, lock_time, description, txid, raw_tx, created_at, updated_at) \
         VALUES (?, 'completed', 'grand', 0, 0, 1, 0, 'a stranger''s', ?, ?, ?, ?)",
    )
    .bind(user_id)
    .bind(grand.id())
    .bind(grand.to_binary())
    .bind(now)
    .bind(now)
    .execute(storage.pool())
    .await
    .unwrap();
    // P: the payment to us, spending G:0.
    let parent = Transaction::with_params(
        1,
        vec![TransactionInput {
            source_transaction: None,
            source_txid: Some(grand.id()),
            source_output_index: 0,
            unlocking_script: Some(UnlockingScript::from_hex("00").unwrap()),
            unlocking_script_template: None,
            sequence: 0xffff_ffff,
        }],
        vec![TransactionOutput {
            satoshis: Some(100_000),
            locking_script: LockingScript::from_binary(&lock).unwrap(),
            change: false,
        }],
        0,
    );
    let p = parent.id();
    let p_row = sqlx::query(
        "INSERT INTO transactions (user_id, status, reference, is_outgoing, satoshis, version, lock_time, description, txid, raw_tx, created_at, updated_at) \
         VALUES (?, 'unproven', 'internalized', 0, 100000, 1, 0, 'a payment', ?, ?, ?, ?)",
    )
    .bind(user_id)
    .bind(&p)
    .bind(parent.to_binary())
    .bind(now)
    .bind(now)
    .execute(storage.pool())
    .await
    .unwrap()
    .last_insert_rowid();
    sqlx::query(
        "INSERT INTO proven_tx_reqs (txid, status, attempts, history, notified, notify, raw_tx, created_at, updated_at) \
         VALUES (?, 'unsent', 1, ?, 0, '{}', ?, ?, ?)",
    )
    .bind(&p)
    .bind(
        serde_json::json!({"notes": [{
            "when": now.to_rfc3339(),
            "what": "immediateBroadcastHint",
            "outcome": "invalidTx",
            "attempts": 1,
            "words": ["ARC answered 465"],
            "nextReaskMinutes": 2
        }]})
        .to_string(),
    )
    .bind(parent.to_binary())
    .bind(now)
    .bind(now)
    .execute(storage.pool())
    .await
    .unwrap();
    let coin = sqlx::query(
        "INSERT INTO outputs (user_id, transaction_id, basket_id, vout, satoshis, locking_script, \
                              txid, type, spendable, change, derivation_prefix, derivation_suffix, \
                              provided_by, purpose, output_description, created_at, updated_at) \
         VALUES (?, ?, ?, 0, 100000, ?, ?, 'P2PKH', 1, 1, ?, ?, 'storage', 'change', 'a payment', ?, ?)",
    )
    .bind(user_id)
    .bind(p_row)
    .bind(basket)
    .bind(&lock)
    .bind(&p)
    .bind(PREFIX)
    .bind(SUFFIX)
    .bind(now)
    .bind(now)
    .execute(storage.pool())
    .await
    .unwrap()
    .last_insert_rowid();
    (storage, root, p, coin)
}

/// One broadcaster's 465 for a transaction (the toolbox's own witness's
/// shape, `tests/immediate_broadcast_hint_tests.rs` at 0.7.4).
fn refused_465() -> bsv_wallet_toolbox::PostBeefResult {
    bsv_wallet_toolbox::PostBeefResult {
        name: "arc".to_string(),
        status: "error".to_string(),
        txid_results: vec![bsv_wallet_toolbox::PostTxResultForTxid {
            txid: "ab".repeat(32),
            status: "465".to_string(),
            double_spend: false,
            orphan_mempool: false,
            competing_txs: None,
            data: Some("ARC answered 465".to_string()),
            service_error: false,
            block_hash: None,
            block_height: None,
            notes: vec![],
        }],
        error: None,
        notes: vec![],
    }
}

fn accepting() -> MockWalletServices {
    MockWalletServices::builder()
        .post_beef_response(MockResponse::Success(vec![
            bsv_wallet_toolbox::services::mock::success_post_beef_result("arc", &[]),
        ]))
        .build()
}

/// RED at 0.7.2: the coin of a transaction no broadcaster took and no status
/// source holds is selected, and the spend is posted behind it. The rule
/// (`Reask.spend`): it is not selected until a status source holds its
/// transaction; with no other coin the spend is refused for funds.
#[tokio::test]
async fn a_coin_whose_transaction_drew_an_immediate_refusal_is_not_selected() {
    let (storage, root, p, coin) = refused_coin().await;
    crate::spend_guard::run(storage.pool()).await.unwrap();
    let wallet = Wallet::new(Some(root), storage, accepting()).await.unwrap();
    let outcome = wallet.create_action(pay_args(), "witness.local").await;
    assert!(
        outcome.is_err(),
        "the refused coin was selected: {:?}",
        outcome.map(|r| r.txid.map(hex::encode))
    );
    let msg = outcome.err().unwrap().to_string();
    assert!(
        msg.to_ascii_lowercase().contains("insufficient funds"),
        "{msg}"
    );
    let (spendable, spent_by): (i64, Option<i64>) =
        sqlx::query_as("SELECT spendable, spent_by FROM outputs WHERE output_id = ?")
            .bind(coin)
            .fetch_one(wallet.storage().pool())
            .await
            .unwrap();
    assert_eq!((spendable, spent_by), (0, None), "held, not spent");
    assert_eq!(
        tx_status(wallet.storage(), &p).await,
        "unproven",
        "no word written"
    );
}

/// The other way: a status source holds P (the toolbox's send-waiting pass
/// reads the status sources and promotes the request to `unmined`); the
/// guard releases the coin and the same spend selects it.
#[tokio::test]
async fn the_same_coin_is_selected_once_a_status_source_holds_its_transaction() {
    let (storage, root, p, coin) = refused_coin().await;
    crate::spend_guard::run(storage.pool()).await.unwrap();
    assert_eq!(output_state(&storage, coin).await.0, 0, "held");

    let holding = Arc::new(
        MockWalletServices::builder()
            .post_beef_response(MockResponse::Success(vec![refused_465()]))
            .get_status_for_txids_response(MockResponse::Success(GetStatusForTxidsResult {
                name: "mock".to_string(),
                status: "success".to_string(),
                error: None,
                results: vec![TxStatusDetail {
                    txid: p.clone(),
                    status: "known".to_string(),
                    depth: None,
                    merkle_path: None,
                    block_height: None,
                    block_hash: None,
                }],
            }))
            .build(),
    );
    WalletStorageProvider::set_services(&storage, holding.clone() as Arc<dyn WalletServices>);
    sqlx::query("UPDATE proven_tx_reqs SET updated_at = ? WHERE txid = ?")
        .bind(Utc::now() - chrono::Duration::days(1))
        .bind(&p)
        .execute(storage.pool())
        .await
        .unwrap();
    bsv_wallet_toolbox::MonitorStorage::send_waiting_transactions(
        &storage,
        std::time::Duration::ZERO,
    )
    .await
    .unwrap();
    let req: String = sqlx::query_scalar("SELECT status FROM proven_tx_reqs WHERE txid = ?")
        .bind(&p)
        .fetch_one(storage.pool())
        .await
        .unwrap();
    assert_eq!(req, "unmined", "a status source holds it");

    crate::spend_guard::run(storage.pool()).await.unwrap();
    let wallet = Wallet::new(Some(root), storage, accepting()).await.unwrap();
    let result = wallet
        .create_action(pay_args(), "witness.local")
        .await
        .expect("the coin is selectable again");
    let spent_by: Option<i64> =
        sqlx::query_scalar("SELECT spent_by FROM outputs WHERE output_id = ?")
            .bind(coin)
            .fetch_one(wallet.storage().pool())
            .await
            .unwrap();
    assert!(
        spent_by.is_some(),
        "spent by {:?}",
        result.txid.map(hex::encode)
    );
}

// ---------------------------------------------------------------------------
// The served doors' word for a refused immediate post
// ---------------------------------------------------------------------------

/// The wallet's one coin: the change output of a completed parent.
async fn funded() -> (StorageSqlx, PrivateKey, String) {
    let root = PrivateKey::random();
    let identity = root.public_key().to_hex();
    let (storage, user_id, basket) = storage_for(&identity).await;
    let sk = ProtoWallet::new(Some(root.clone()))
        .key_deriver()
        .derive_private_key(
            &Protocol::new(SecurityLevel::Counterparty, BRC29_PROTOCOL),
            &format!("{} {}", PREFIX, SUFFIX),
            &Counterparty::Self_,
        )
        .unwrap();
    let mut lock = vec![0x76, 0xa9, 0x14];
    lock.extend_from_slice(&hash160(&sk.public_key().to_compressed()));
    lock.extend([0x88, 0xac]);
    let parent = Transaction::with_params(
        1,
        vec![TransactionInput {
            source_transaction: None,
            source_txid: Some("00".repeat(32)),
            source_output_index: 0xffff_ffff,
            unlocking_script: Some(UnlockingScript::from_hex("00").unwrap()),
            unlocking_script_template: None,
            sequence: 0xffff_ffff,
        }],
        vec![TransactionOutput {
            satoshis: Some(100_000),
            locking_script: LockingScript::from_binary(&lock).unwrap(),
            change: false,
        }],
        0,
    );
    let p = parent.id();
    let now = Utc::now();
    let p_row = sqlx::query(
        "INSERT INTO transactions (user_id, status, reference, is_outgoing, satoshis, version, lock_time, description, txid, raw_tx, created_at, updated_at) \
         VALUES (?, 'completed', 'parent', 0, 100000, 1, 0, 'parent', ?, ?, ?, ?)",
    )
    .bind(user_id)
    .bind(&p)
    .bind(parent.to_binary())
    .bind(now)
    .bind(now)
    .execute(storage.pool())
    .await
    .unwrap()
    .last_insert_rowid();
    sqlx::query(
        "INSERT INTO outputs (user_id, transaction_id, basket_id, vout, satoshis, locking_script, \
                              txid, type, spendable, change, derivation_prefix, derivation_suffix, \
                              provided_by, purpose, output_description, created_at, updated_at) \
         VALUES (?, ?, ?, 0, 100000, ?, ?, 'P2PKH', 1, 1, ?, ?, 'storage', 'change', 'funding', ?, ?)",
    )
    .bind(user_id)
    .bind(p_row)
    .bind(basket)
    .bind(&lock)
    .bind(&p)
    .bind(PREFIX)
    .bind(SUFFIX)
    .bind(now)
    .bind(now)
    .execute(storage.pool())
    .await
    .unwrap();
    (storage, root, p)
}

/// RED at 0.7.2 (the toolbox 0.7.3): a 465 on the immediate post failed the
/// transaction, released its inputs and returned "Transaction broadcast
/// failed", which the served door answered 502 `BROADCAST_REJECTED`, "the
/// tx is already failed and its inputs released". At the toolbox 0.7.4 the
/// post's refusal is a hint: the txid with `sending`, and the door's word
/// is `built` with the toolbox's hint, the inputs `locked`; nothing is
/// released.
#[tokio::test]
async fn a_refused_create_action_is_answered_with_the_word_built_and_its_hint() {
    let (storage, root, parent) = funded().await;
    let wallet = Wallet::new(
        Some(root),
        storage,
        MockWalletServices::builder()
            .post_beef_response(MockResponse::Success(vec![refused_465()]))
            .build(),
    )
    .await
    .unwrap();
    let result = wallet
        .create_action(pay_args(), "witness.local")
        .await
        .expect("a broadcaster's refusal is not an error");
    let txid = result.txid.expect("the txid");
    assert!(crate::server::broadcast_follow_up::sending(
        &result.send_with_results,
        &txid
    ));
    let word = crate::server::post_word::post_word(wallet.storage(), &hex::encode(txid))
        .await
        .expect("the door's word");
    assert_eq!(
        (word.word, word.request.as_str(), word.inputs),
        ("built", "unsent", "locked")
    );
    assert_eq!(word.hint.as_ref().unwrap()["outcome"], "invalidTx");
    let json = serde_json::to_value(&word).unwrap();
    assert_eq!(json["word"], "built");
    assert!(json["hint"]["nextReaskMinutes"].is_number());
    // Nothing released: the funding coin stays locked by the transaction.
    let (spendable, spent_by): (i64, Option<i64>) =
        sqlx::query_as("SELECT spendable, spent_by FROM outputs WHERE txid = ? AND vout = 0")
            .bind(&parent)
            .fetch_one(wallet.storage().pool())
            .await
            .unwrap();
    assert_eq!(spendable, 0);
    assert!(spent_by.is_some());
}
