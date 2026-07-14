//! Integration tests for the daemon's `POST /arc-callback` webhook route
//! (ARC/Arcade proof delivery straight into wallet storage).
//!
//! All vectors are synthetic: random throwaway keys, coinbase-style BUMPs
//! validated against a MockChainTracker. No network, no funded wallets.

use bsv_sdk::primitives::PrivateKey;
use bsv_sdk::transaction::{MerklePath, MockChainTracker};
use bsv_wallet_cli::server::{self, ServerConfig};
use bsv_wallet_toolbox::{
    Chain, Services, ServicesOptions, StorageSqlx, Wallet, WalletStorageWriter,
};
use reqwest::Client;
use serde_json::{json, Value};
use std::net::SocketAddr;
use std::sync::Arc;
use tempfile::TempDir;

const CB_TOKEN: &str = "0123456789abcdef0123456789abcdef";
const WALLET_BEARER: &str = "wallet-bearer-secret";

/// Spin up a server with a callback token + wallet bearer auth configured.
/// Returns (base_url, client, sqlite pool for direct verification, tempdir).
async fn setup_with_callback(
    chain_tracker: Option<Arc<dyn bsv_sdk::transaction::ChainTracker>>,
    seed: Option<(&str, &str, &str)>, // (txid, req_status, tx_status)
) -> (String, Client, sqlx::SqlitePool, TempDir) {
    let tmp = TempDir::new().expect("temp dir");
    let db_path = tmp.path().join("test.db");

    let storage = StorageSqlx::open(db_path.to_str().unwrap())
        .await
        .expect("open db");

    let key = PrivateKey::random();
    let identity_key = key.public_key().to_hex();
    storage
        .migrate("bsv-wallet-test", &identity_key)
        .await
        .expect("migrate db");
    storage.make_available().await.expect("make available");

    if let Some(tracker) = chain_tracker {
        storage.set_chain_tracker(tracker).await;
    }

    // Keep a pool handle for post-request verification.
    let pool = storage.pool().clone();

    if let Some((txid, req_status, tx_status)) = seed {
        let (user, _) = storage
            .find_or_insert_user(&identity_key)
            .await
            .expect("user");
        let now = chrono::Utc::now();
        sqlx::query(
            r#"
            INSERT INTO proven_tx_reqs (txid, status, attempts, history, notified, notify, raw_tx, created_at, updated_at)
            VALUES (?, ?, 0, '{}', 0, '{}', X'01000000', ?, ?)
            "#,
        )
        .bind(txid)
        .bind(req_status)
        .bind(now)
        .bind(now)
        .execute(&pool)
        .await
        .expect("seed req");
        sqlx::query(
            r#"
            INSERT INTO transactions (user_id, txid, status, reference, description, satoshis,
                                      version, lock_time, raw_tx, is_outgoing, created_at, updated_at)
            VALUES (?, ?, ?, 'ref-arc-cb', 'arc callback test', -500, 1, 0, X'01000000', 1, ?, ?)
            "#,
        )
        .bind(user.user_id)
        .bind(txid)
        .bind(tx_status)
        .bind(now)
        .bind(now)
        .execute(&pool)
        .await
        .expect("seed tx");
    }

    let services =
        Services::with_options(Chain::Main, ServicesOptions::mainnet()).expect("services");
    let wallet = Wallet::new(Some(key), storage, services)
        .await
        .expect("wallet");

    let state = server::make_wallet_state(wallet);
    let config = ServerConfig {
        auth_token: Some(WALLET_BEARER.to_string()),
        callback_token: Some(CB_TOKEN.to_string()),
        ..Default::default()
    };
    let app = server::make_router(state, config);

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind");
    let addr: SocketAddr = listener.local_addr().expect("local addr");
    tokio::spawn(async move {
        axum::serve(listener, app).await.ok();
    });

    (format!("http://{}", addr), Client::new(), pool, tmp)
}

async fn req_status(pool: &sqlx::SqlitePool, txid: &str) -> String {
    let (s,): (String,) = sqlx::query_as("SELECT status FROM proven_tx_reqs WHERE txid = ?")
        .bind(txid)
        .fetch_one(pool)
        .await
        .expect("req status");
    s
}

async fn tx_status(pool: &sqlx::SqlitePool, txid: &str) -> String {
    let (s,): (String,) = sqlx::query_as("SELECT status FROM transactions WHERE txid = ?")
        .bind(txid)
        .fetch_one(pool)
        .await
        .expect("tx status");
    s
}

#[tokio::test]
async fn rejects_missing_and_wrong_token() {
    let (base, client, _pool, _tmp) = setup_with_callback(None, None).await;
    let payload = json!({"txid": "aa".repeat(32), "txStatus": "SEEN_ON_NETWORK"});

    // No token at all → 401.
    let resp = client
        .post(format!("{base}/arc-callback"))
        .json(&payload)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 401);

    // Wrong bearer → 401.
    let resp = client
        .post(format!("{base}/arc-callback"))
        .header("Authorization", "Bearer wrong-token")
        .json(&payload)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 401);

    // The WALLET bearer must NOT open the callback route.
    let resp = client
        .post(format!("{base}/arc-callback"))
        .header("Authorization", format!("Bearer {WALLET_BEARER}"))
        .json(&payload)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 401);
}

#[tokio::test]
async fn exempt_from_wallet_bearer_and_accepts_both_token_headers() {
    let txid = "e".repeat(64);
    let (base, client, pool, _tmp) =
        setup_with_callback(None, Some((&txid, "sending", "sending"))).await;

    // Authorization: Bearer <callback-token> (ARC webhook convention), and
    // NO wallet bearer — proves the route is exempt from wallet auth.
    let resp = client
        .post(format!("{base}/arc-callback"))
        .header("Authorization", format!("Bearer {CB_TOKEN}"))
        .json(&json!({"txid": txid, "txStatus": "SEEN_ON_NETWORK"}))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body: Value = resp.json().await.unwrap();
    assert_eq!(body["ok"], true);

    // Spendability transition applied.
    assert_eq!(req_status(&pool, &txid).await, "unmined");
    assert_eq!(tx_status(&pool, &txid).await, "unproven");

    // X-CallbackToken header also accepted (status now already unmined —
    // idempotent StatusIgnored, still 200).
    let resp = client
        .post(format!("{base}/arc-callback"))
        .header("X-CallbackToken", CB_TOKEN)
        .json(&json!({"txid": txid, "txStatus": "SEEN_ON_NETWORK"}))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
}

#[tokio::test]
async fn rejected_status_marks_double_spend() {
    let txid = "d".repeat(64);
    let (base, client, pool, _tmp) =
        setup_with_callback(None, Some((&txid, "unmined", "unproven"))).await;

    let resp = client
        .post(format!("{base}/arc-callback"))
        .header("Authorization", format!("Bearer {CB_TOKEN}"))
        .json(&json!({"txid": txid, "txStatus": "DOUBLE_SPEND_ATTEMPTED"}))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);

    assert_eq!(req_status(&pool, &txid).await, "doubleSpend");
    assert_eq!(tx_status(&pool, &txid).await, "failed");
}

#[tokio::test]
async fn mined_webhook_with_merkle_path_ingests_proof() {
    let txid = "a".repeat(64);
    let height = 850_000u32;

    // Synthetic BUMP that validates: coinbase-style single-tx block, so the
    // computed root equals the txid; MockChainTracker knows that root.
    let bump = MerklePath::from_coinbase_txid(&txid, height);
    let bump_hex = hex::encode(bump.to_binary());
    let root = bump.compute_root(Some(&txid)).unwrap();
    let mut tracker = MockChainTracker::new(height + 1);
    tracker.add_root(height, root);

    let (base, client, pool, _tmp) = setup_with_callback(
        Some(Arc::new(tracker)),
        Some((&txid, "unmined", "unproven")),
    )
    .await;

    // Arcade MINED webhook payload: blockHash, blockHeight, merklePath.
    let resp = client
        .post(format!("{base}/arc-callback"))
        .header("Authorization", format!("Bearer {CB_TOKEN}"))
        .json(&json!({
            "txid": txid,
            "txStatus": "MINED",
            "blockHeight": height,
            "blockHash": "b".repeat(64),
            "merklePath": bump_hex,
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body: Value = resp.json().await.unwrap();
    assert_eq!(body["ok"], true);
    assert_eq!(body["action"], "ProofIngested");

    // Proof stored, records completed.
    let (count,): (i64,) = sqlx::query_as("SELECT COUNT(*) FROM proven_txs WHERE txid = ?")
        .bind(&txid)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(count, 1);
    assert_eq!(req_status(&pool, &txid).await, "completed");
    assert_eq!(tx_status(&pool, &txid).await, "completed");
}

#[tokio::test]
async fn mined_webhook_with_bad_proof_is_rejected_not_stored() {
    let txid = "c".repeat(64);
    let height = 850_000u32;

    // Tracker knows a DIFFERENT root — proof must be rejected.
    let bump = MerklePath::from_coinbase_txid(&txid, height);
    let bump_hex = hex::encode(bump.to_binary());
    let mut tracker = MockChainTracker::new(height + 1);
    tracker.add_root(height, "ff".repeat(32));

    let (base, client, pool, _tmp) = setup_with_callback(
        Some(Arc::new(tracker)),
        Some((&txid, "unmined", "unproven")),
    )
    .await;

    let resp = client
        .post(format!("{base}/arc-callback"))
        .header("Authorization", format!("Bearer {CB_TOKEN}"))
        .json(&json!({
            "txid": txid,
            "txStatus": "MINED",
            "blockHeight": height,
            "blockHash": "b".repeat(64),
            "merklePath": bump_hex,
        }))
        .send()
        .await
        .unwrap();
    // Request is well-formed and authenticated → 200, but the proof is
    // rejected and NOT stored.
    assert_eq!(resp.status(), 200);
    let body: Value = resp.json().await.unwrap();
    assert!(body["action"]
        .as_str()
        .unwrap_or_default()
        .starts_with("ProofRejected"));

    let (count,): (i64,) = sqlx::query_as("SELECT COUNT(*) FROM proven_txs WHERE txid = ?")
        .bind(&txid)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(count, 0, "invalid proof must never be stored");
    assert_eq!(req_status(&pool, &txid).await, "unmined");
}

#[tokio::test]
async fn malformed_payload_is_bad_request() {
    let (base, client, _pool, _tmp) = setup_with_callback(None, None).await;

    let resp = client
        .post(format!("{base}/arc-callback"))
        .header("Authorization", format!("Bearer {CB_TOKEN}"))
        .json(&json!({"txStatus": "MINED"})) // no txid
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 400);

    let resp = client
        .post(format!("{base}/arc-callback"))
        .header("Authorization", format!("Bearer {CB_TOKEN}"))
        .json(&json!({"txid": "not-a-txid", "txStatus": "MINED"}))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 400);
}

#[tokio::test]
async fn callback_disabled_without_token_config() {
    // No callback_token in config → route answers 404.
    let tmp = TempDir::new().expect("temp dir");
    let db_path = tmp.path().join("test.db");
    let storage = StorageSqlx::open(db_path.to_str().unwrap())
        .await
        .expect("open db");
    let key = PrivateKey::random();
    let identity_key = key.public_key().to_hex();
    storage
        .migrate("bsv-wallet-test", &identity_key)
        .await
        .expect("migrate db");
    storage.make_available().await.expect("make available");
    let services =
        Services::with_options(Chain::Main, ServicesOptions::mainnet()).expect("services");
    let wallet = Wallet::new(Some(key), storage, services)
        .await
        .expect("wallet");
    let state = server::make_wallet_state(wallet);
    let app = server::make_router(state, ServerConfig::default());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind");
    let addr: SocketAddr = listener.local_addr().expect("local addr");
    tokio::spawn(async move {
        axum::serve(listener, app).await.ok();
    });

    let client = Client::new();
    let resp = client
        .post(format!("http://{addr}/arc-callback"))
        .header("Authorization", format!("Bearer {CB_TOKEN}"))
        .json(&json!({"txid": "aa".repeat(32), "txStatus": "MINED"}))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 404);
}
