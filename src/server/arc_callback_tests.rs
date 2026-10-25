//! The callback keeps transient store faults typed and payload faults unchanged.

use super::*;
use crate::test_support::{response_answer, test_wallet};
use bsv_sdk::primitives::PrivateKey;
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
    assert!(started.elapsed() >= wait, "the writer did not wait");
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
