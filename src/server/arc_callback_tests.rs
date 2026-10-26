//! The callback keeps transient store faults typed and payload faults unchanged.

use super::*;
use crate::test_support::{
    arc_rows, busy_read_wallet, p2pkh, raw_tx, response_answer, seed_arc_requests,
    seed_arc_transaction, set_read_lock, test_wallet, txid_of, TestChainTracker,
};
use bsv_sdk::primitives::PrivateKey;
use bsv_sdk::transaction::MerklePath;
use bsv_wallet_toolbox::MonitorStorage;
use sqlx::{ConnectOptions, Connection};
use std::time::{Duration, Instant};
use tower::ServiceExt;

const TXID: &str = "3333333333333333333333333333333333333333333333333333333333333333";

async fn wallet(storage: StorageSqlx) -> WalletState {
    test_wallet(
        storage,
        "callback",
        PrivateKey::from_hex(&"01".repeat(32)).unwrap(),
    )
    .await
}

fn router(wallet: WalletState) -> Router {
    make_router(
        wallet,
        ServerConfig {
            callback_token: Some("callback-test".to_string()),
            ..ServerConfig::default()
        },
    )
}

fn router_with_deadline(wallet: WalletState, deadline: Duration) -> Router {
    Router::new()
        .route(
            "/arc-callback",
            post(move |state, request| arc_callback_with_deadline(state, request, deadline)),
        )
        .layer(axum::Extension(ServerConfig {
            callback_token: Some("callback-test".to_string()),
            ..ServerConfig::default()
        }))
        .with_state(wallet)
}

fn request(payload: &serde_json::Value) -> Request {
    Request::post("/arc-callback")
        .header("x-callbacktoken", "callback-test")
        .body(axum::body::Body::from(payload.to_string()))
        .unwrap()
}

/// A competitor hint reaches one history write before the toolbox's
/// best-effort broadcast-memory write, which would swallow its own busy.
fn competitor_hint() -> serde_json::Value {
    json!({"txid": TXID, "txStatus": "REJECTED", "competingTxs": ["22".repeat(32)]})
}

async fn seed_request(storage: &StorageSqlx) {
    sqlx::query(
        "INSERT INTO proven_tx_reqs (txid, status, attempts, history, notified, notify, raw_tx, created_at, updated_at) \
         VALUES (?, 'unsent', 0, '{}', 0, '{}', X'01000000', CURRENT_TIMESTAMP, CURRENT_TIMESTAMP)",
    )
    .bind(TXID)
    .execute(storage.writer())
    .await
    .unwrap();
}

/// A served write blocked by a second connection's BEGIN EXCLUSIVE waits
/// one busy_timeout, then returns the shared busy body and retry header.
#[tokio::test]
async fn arc_callback_under_a_busy_store_is_503_store_busy() {
    let dir = tempfile::TempDir::new().unwrap();
    let file = dir.path().join("callback.db");
    let wallet = wallet(StorageSqlx::open(file.to_str().unwrap()).await.unwrap()).await;
    let storage = wallet.storage();
    seed_request(storage).await;
    let wait_ms: i64 = sqlx::query_scalar("PRAGMA busy_timeout")
        .fetch_one(storage.writer())
        .await
        .unwrap();
    let wait = Duration::from_millis(wait_ms.try_into().unwrap());
    assert_eq!(wait, Duration::from_secs(5));
    let mut holder = sqlx::sqlite::SqliteConnectOptions::new()
        .filename(&file)
        .connect()
        .await
        .unwrap();
    sqlx::query("BEGIN EXCLUSIVE")
        .execute(&mut holder)
        .await
        .unwrap();
    let started = Instant::now();
    let response = tokio::time::timeout(
        wait + Duration::from_secs(3),
        router(wallet.clone()).oneshot(request(&competitor_hint())),
    )
    .await
    .expect("the callback waits once, not a second busy_timeout")
    .unwrap();
    let elapsed = started.elapsed();
    eprintln!("competitor hint: typed busy after {elapsed:.3?}");
    assert!(elapsed >= wait, "the writer did not wait");
    assert!(
        elapsed < ARC_CALLBACK_DEADLINE,
        "typed busy did not precede the deadline"
    );
    assert_eq!(
        response_answer(response).await,
        (
            StatusCode::SERVICE_UNAVAILABLE,
            Some("1".to_string()),
            json!({"code": "STORE_BUSY", "message":
                "apply_event: Store busy: sqlite code 5"}),
        ),
    );
    holder.close().await.unwrap();
    let history: String = sqlx::query_scalar("SELECT history FROM proven_tx_reqs WHERE txid = ?")
        .bind(TXID)
        .fetch_one(storage.pool())
        .await
        .unwrap();
    assert_eq!(history, "{}", "the blocked write changed no history");
}

async fn deadline_answer(app: Router, payload: &serde_json::Value, deadline: Duration) {
    let started = Instant::now();
    let response = tokio::time::timeout(
        deadline + Duration::from_secs(1),
        app.oneshot(request(payload)),
    )
    .await
    .expect("callback exceeded its deadline plus a one-second margin")
    .unwrap();
    let answer = response_answer(response).await;
    let elapsed = started.elapsed();
    eprintln!(
        "{}: deadline {deadline:.3?}, answered in {elapsed:.3?}",
        payload["txStatus"]
    );
    assert!(elapsed >= deadline, "deadline fired early: {elapsed:?}");
    assert!(elapsed < deadline + Duration::from_secs(1), "{elapsed:?}");
    assert_eq!(
        answer,
        (
            StatusCode::SERVICE_UNAVAILABLE,
            Some("1".to_string()),
            json!({"code": "STORE_BUSY", "message": format!(
                "arc-callback deadline of {deadline:?} elapsed while the store was still busy"
            )}),
        ),
    );
}

async fn busy_status_deadline(status: &str, deadline: Duration, served_default: bool) {
    let (_dir, wallet, mut holder) = busy_read_wallet().await;
    let app = if served_default {
        router(wallet.clone())
    } else {
        router_with_deadline(wallet.clone(), deadline)
    };
    let payload = json!({"txid": TXID, "txStatus": status});
    deadline_answer(app, &payload, deadline).await;
    set_read_lock(&mut holder, false).await;
    let answer = router(wallet).oneshot(request(&payload)).await.unwrap();
    let action = if status == "SEEN_ON_NETWORK" {
        "StatusIgnored"
    } else {
        "Reask"
    };
    assert_eq!(
        response_answer(answer).await,
        (StatusCode::OK, None, json!({"ok": true, "action": action})),
    );
}

#[tokio::test]
async fn arc_callback_deadline_seen() {
    busy_status_deadline("SEEN_ON_NETWORK", Duration::from_secs(1), false).await;
}

#[tokio::test]
async fn arc_callback_deadline_in_block() {
    busy_status_deadline("STUMP_PROCESSING", Duration::from_secs(1), false).await;
}

#[tokio::test]
async fn arc_callback_deadline_mined() {
    busy_status_deadline("MINED", Duration::from_secs(1), false).await;
}

/// This deliberately takes eight seconds: only the served default proves the
/// response still reaches Arcade before its ten-second POST timeout.
#[tokio::test]
async fn arc_callback_deadline_served_default_is_eight_seconds() {
    assert_eq!(ARC_CALLBACK_DEADLINE, Duration::from_secs(8));
    busy_status_deadline("SEEN_ON_NETWORK", ARC_CALLBACK_DEADLINE, true).await;
}

async fn deliver_callback(wallet: WalletState, payload: &serde_json::Value) {
    let response = router(wallet).oneshot(request(payload)).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
}

fn assert_retried_rows(
    rows: &serde_json::Value,
    status: &str,
    bump: &MerklePath,
    raw: &[u8],
    height: u32,
) {
    let mined = status == "MINED";
    assert_eq!(
        rows["request"][1],
        if mined { "completed" } else { "unmined" }
    );
    assert_eq!(
        rows["transaction"][1],
        if mined { "completed" } else { "unproven" }
    );
    assert_eq!(rows["proof_count"], usize::from(mined));
    if mined {
        assert!(rows["proof"][0].is_i64());
        assert_eq!(rows["request"][2], rows["proof"][0]);
        assert_eq!(rows["transaction"][2], rows["proof"][0]);
        assert_eq!(rows["proof"][1], height);
        assert_eq!(rows["proof"][4], hex::encode_upper(bump.to_binary()));
        assert_eq!(rows["proof"][5], hex::encode_upper(raw));
    }
}

async fn retry_is_idempotent(status: &str) {
    let (_dir, wallet, mut holder) = busy_read_wallet().await;
    set_read_lock(&mut holder, false).await;
    let storage = wallet.storage();
    let raw = raw_tx(&[(&"22".repeat(32), 0)], &[(500, p2pkh([3; 20]))]);
    let txid = txid_of(&raw);
    seed_arc_requests(storage, std::slice::from_ref(&txid)).await;
    seed_arc_transaction(storage, &txid, &raw).await;
    let height = 850_000;
    let bump = MerklePath::from_coinbase_txid(&txid, height);
    let mut tracker = TestChainTracker::new(height + 1);
    tracker.add_root(height, bump.compute_root(Some(&txid)).unwrap());
    storage.set_chain_tracker(Arc::new(tracker)).await;
    storage
        .set_max_acceptable_proof_height(height)
        .await
        .unwrap();
    let payload = json!({"txid": txid, "txStatus": status, "blockHeight": height,
            "blockHash": "bb".repeat(32), "merklePath": hex::encode(bump.to_binary())});
    let before = arc_rows(storage, &txid).await;
    set_read_lock(&mut holder, true).await;
    deadline_answer(
        router_with_deadline(wallet.clone(), Duration::from_secs(1)),
        &payload,
        Duration::from_secs(1),
    )
    .await;
    set_read_lock(&mut holder, false).await;
    assert_eq!(arc_rows(storage, &txid).await, before, "cancelled {status}");
    deliver_callback(wallet.clone(), &payload).await;
    let once = arc_rows(storage, &txid).await;
    deliver_callback(wallet.clone(), &payload).await;
    assert_eq!(arc_rows(storage, &txid).await, once, "repeated {status}");
    assert_retried_rows(&once, status, &bump, &raw, height);
}

#[tokio::test]
async fn arc_callback_deadline_retries_are_idempotent() {
    for status in ["SEEN_ON_NETWORK", "STUMP_PROCESSING", "MINED"] {
        retry_is_idempotent(status).await;
    }
}

#[tokio::test]
async fn arc_callback_writer_not_acquired_is_503_store_busy() {
    let wallet = wallet(StorageSqlx::in_memory().await.unwrap()).await;
    seed_request(wallet.storage()).await;
    let held = wallet.storage().writer().acquire().await.unwrap();
    let response = router(wallet.clone())
        .oneshot(request(&competitor_hint()))
        .await
        .unwrap();
    let busy = bsv_wallet_toolbox::Error::StoreBusy {
        reason: bsv_wallet_toolbox::StoreBusyReason::WriterNotAcquired,
    };
    assert_eq!(
        response_answer(response).await,
        (
            StatusCode::SERVICE_UNAVAILABLE,
            Some("1".to_string()),
            json!({"code": "STORE_BUSY", "message": format!("apply_event: {busy}")}),
        ),
    );
    drop(held);
}

#[tokio::test]
async fn arc_callback_malformed_status_and_untyped_busy_words_stay_bad_payload() {
    let wallet = wallet(StorageSqlx::in_memory().await.unwrap()).await;
    let app = router(wallet.clone());
    for (payload, message) in [
        (
            json!({"txid": TXID}),
            "not an Arcade status body: missing field `txStatus`",
        ),
        (
            json!({"txid": TXID, "txStatus": "SEEN_ON_NETWORK",
                "blockHeight": "Store busy: database is locked (code: 517)"}),
            "not an Arcade status body: invalid type: string \"Store busy: database is locked (code: 517)\", expected u32",
        ),
    ] {
        let fault = crate::arc_ingest::ingest_arc_payload(wallet.storage(), &payload)
            .await
            .unwrap_err();
        assert!(!crate::store::is_store_busy(&fault));
        assert_eq!(
            response_answer(app.clone().oneshot(request(&payload)).await.unwrap()).await,
            (
                StatusCode::BAD_REQUEST,
                None,
                json!({"code": "BAD_PAYLOAD", "message": message}),
            ),
        );
    }
}

#[tokio::test]
async fn arc_callback_unknown_txid_stays_status_ignored() {
    let response = router(wallet(StorageSqlx::in_memory().await.unwrap()).await)
        .oneshot(request(
            &json!({"txid": TXID, "txStatus": "SEEN_ON_NETWORK"}),
        ))
        .await
        .unwrap();
    assert_eq!(
        response_answer(response).await,
        (
            StatusCode::OK,
            None,
            json!({"ok": true, "action": "StatusIgnored"}),
        ),
    );
}

/// A real write failure whose words say busy is still a payload fault;
/// context must preserve the old complete message as well as its type.
#[tokio::test]
async fn arc_callback_non_busy_write_keeps_its_message() {
    let wallet = wallet(StorageSqlx::in_memory().await.unwrap()).await;
    seed_request(wallet.storage()).await;
    sqlx::query(
        "CREATE TRIGGER refuse_history BEFORE UPDATE OF history ON proven_tx_reqs \
         BEGIN SELECT RAISE(ABORT, 'Store busy: database is locked (code: 517)'); END",
    )
    .execute(wallet.storage().writer())
    .await
    .unwrap();
    let response = router(wallet)
        .oneshot(request(&competitor_hint()))
        .await
        .unwrap();
    assert_eq!(
        response_answer(response).await,
        (
            StatusCode::BAD_REQUEST,
            None,
            json!({"code": "BAD_PAYLOAD", "message":
                "apply_event: SQLx error: error returned from database: (code: 1811) Store busy: database is locked (code: 517)"}),
        ),
    );
}
