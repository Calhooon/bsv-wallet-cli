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

// =============================================================================
// arcade v0.10.1 production-fixture tests (#259/#260 consumption campaign)
//
// Captured live 2026-07-22 from arcade-v2-us-1 (v0.10.1-alpha.1):
// `GET /tx/{txid}` for the campaign probe tx after block 959,011 mined it.
// The block merkle root is cross-checked against WhatsOnChain's block header —
// so these tests exercise the exact bytes production arcade pushes on MINED
// SSE frames and webhook callbacks, not synthetic vectors.
// =============================================================================

const PROBE_TXID: &str = "104be47e38ae90d7d3ca7804823bd07170cb964bfdc38306df47456ef8939d01";
const PROBE_HEIGHT: u32 = 959_011;
const PROBE_BLOCK_HASH: &str = "00000000000000001044d72145b6986a5778d33094841b986907c8b453546643";
/// Block 959,011's merkle root per WhatsOnChain (independent of arcade).
const PROBE_BLOCK_MERKLE_ROOT: &str =
    "7ec0ebe06c8f4956369ea5e7fc6ee66e642fcce38866a0b85bfd1c41dbbfb131";
/// The BRC-74 BUMP exactly as served/pushed by arcade v0.10.1.
const PROBE_MERKLE_PATH_HEX: &str = "fe23a20e000b023d02019d93f86e4547df0683c3fd4b96cb7071d03b820478cad3d790ae387ee44b103c009bb4bf617a1afdb045f7e1381120856c24e16114c2133d9b37f03ac76528ba86011f006a509c76fc529037078b683b1c19683dd1af8c00d286b8442f8441ea457c0576010e001c73319bf6272d1fe9a4fa62afc8ee112cd14a812956fe0d50bcdaecfee0888301060074df620703883f9f3ba538abbc05a8de30750cdaf6f802bc5cb011a8cb25ccee01020074f9ea21e36f08ef06ffe2b36492bec3f652a4dc1ebaa0b357d954bb1ef8c92401000016ccdba8d1e69a1dfe9d38dd34b13cec2bdc01c472caa476156203a4001d41200101007fba9bf8a9aec9aee46b7871672086a4c0a50b13b518281c553e113e1de505300101009806315c33bb607b5cf2684f872a491cd4cb78a211daf3259c4ed31a7999955101010024a960be0c782aec7773308785c18d09eeeaf291e97cf4d4d3354eb71d0ac47d0101003d7127e87becd268466fd08900113e0430d8fa97e290615f1d0635389e4632650101004c3f71da7a45399a39e2fc0f37d36ace62e5c612ad64c75197ff5ce31de38e97";

/// The production BUMP must compute to the block's TRUE merkle root for the
/// probe txid — proving parser + root computation against real chain data.
#[test]
fn production_fixture_bump_computes_true_block_root() {
    let bytes = hex::decode(PROBE_MERKLE_PATH_HEX).expect("fixture hex");
    let bump = MerklePath::from_binary(&bytes).expect("BUMP parse");
    let root = bump.compute_root(Some(PROBE_TXID)).expect("root");
    assert_eq!(root, PROBE_BLOCK_MERKLE_ROOT, "must match WoC block header");
}

/// Webhook lane with the REAL enriched payload: ingested, records completed.
#[tokio::test]
async fn production_fixture_webhook_ingests_proof() {
    let mut tracker = MockChainTracker::new(PROBE_HEIGHT + 1);
    tracker.add_root(PROBE_HEIGHT, PROBE_BLOCK_MERKLE_ROOT.to_string());

    let (base, client, pool, _tmp) = setup_with_callback(
        Some(Arc::new(tracker)),
        Some((PROBE_TXID, "unmined", "unproven")),
    )
    .await;

    let resp = client
        .post(format!("{base}/arc-callback"))
        .header("Authorization", format!("Bearer {CB_TOKEN}"))
        .json(&json!({
            "txid": PROBE_TXID,
            "txStatus": "MINED",
            "blockHeight": PROBE_HEIGHT,
            "blockHash": PROBE_BLOCK_HASH,
            "merklePath": PROBE_MERKLE_PATH_HEX,
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body: Value = resp.json().await.unwrap();
    assert_eq!(body["action"], "ProofIngested");
    assert_eq!(req_status(&pool, PROBE_TXID).await, "completed");
    assert_eq!(tx_status(&pool, PROBE_TXID).await, "completed");
}

/// A single flipped byte in the REAL path must be rejected by the SPV gate
/// and never stored — push is a hint, not truth.
#[tokio::test]
async fn production_fixture_tampered_path_rejected() {
    let mut tampered = PROBE_MERKLE_PATH_HEX.to_string();
    // Flip a nibble deep in the path (past the varint header).
    let mid = tampered.len() / 2;
    let orig = tampered.as_bytes()[mid] as char;
    let flipped = if orig == '0' { '1' } else { '0' };
    tampered.replace_range(mid..mid + 1, &flipped.to_string());

    let mut tracker = MockChainTracker::new(PROBE_HEIGHT + 1);
    tracker.add_root(PROBE_HEIGHT, PROBE_BLOCK_MERKLE_ROOT.to_string());

    let (base, client, pool, _tmp) = setup_with_callback(
        Some(Arc::new(tracker)),
        Some((PROBE_TXID, "unmined", "unproven")),
    )
    .await;

    let resp = client
        .post(format!("{base}/arc-callback"))
        .header("Authorization", format!("Bearer {CB_TOKEN}"))
        .json(&json!({
            "txid": PROBE_TXID,
            "txStatus": "MINED",
            "blockHeight": PROBE_HEIGHT,
            "blockHash": PROBE_BLOCK_HASH,
            "merklePath": tampered,
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body: Value = resp.json().await.unwrap();
    assert!(body["action"]
        .as_str()
        .unwrap_or_default()
        .starts_with("ProofRejected"));
    let (count,): (i64,) = sqlx::query_as("SELECT COUNT(*) FROM proven_txs WHERE txid = ?")
        .bind(PROBE_TXID)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(count, 0, "tampered proof must never be stored");
    assert_eq!(req_status(&pool, PROBE_TXID).await, "unmined");
}

/// THE NEW LANE (#259): an enriched MINED SSE frame latches the verified
/// proof inline — no webhook, no fetch-through-services. The fetch trigger
/// must stay UNSET (proving the fallback was not needed), records complete,
/// and the stored proof is the production BUMP.
#[tokio::test]
async fn sse_inline_proof_latches_without_webhook_or_fetch() {
    use bsv_wallet_toolbox::monitor::ArcadeEventsTask;
    use bsv_wallet_toolbox::services::providers::arcade::ArcadeStatusEvent;
    use std::sync::atomic::{AtomicBool, Ordering};

    // Direct storage harness (no HTTP server — this is the SSE task's path).
    let tmp = TempDir::new().expect("temp dir");
    let storage = StorageSqlx::open(tmp.path().join("sse.db").to_str().unwrap())
        .await
        .expect("open db");
    let key = PrivateKey::random();
    let identity_key = key.public_key().to_hex();
    storage
        .migrate("bsv-wallet-test", &identity_key)
        .await
        .expect("migrate");
    storage.make_available().await.expect("available");
    let mut tracker = MockChainTracker::new(PROBE_HEIGHT + 1);
    tracker.add_root(PROBE_HEIGHT, PROBE_BLOCK_MERKLE_ROOT.to_string());
    storage.set_chain_tracker(Arc::new(tracker)).await;
    let pool = storage.pool().clone();
    let (user, _) = storage
        .find_or_insert_user(&identity_key)
        .await
        .expect("user");
    let now = chrono::Utc::now();
    sqlx::query(
        "INSERT INTO proven_tx_reqs (txid, status, attempts, history, notified, notify, raw_tx, created_at, updated_at) \
         VALUES (?, 'unmined', 0, '{}', 0, '{}', X'01000000', ?, ?)",
    )
    .bind(PROBE_TXID)
    .bind(now)
    .bind(now)
    .execute(&pool)
    .await
    .expect("seed req");
    sqlx::query(
        "INSERT INTO transactions (user_id, txid, status, reference, description, satoshis, \
         version, lock_time, raw_tx, is_outgoing, created_at, updated_at) \
         VALUES (?, ?, 'unproven', 'ref-sse', 'sse inline test', -500, 1, 0, X'01000000', 1, ?, ?)",
    )
    .bind(user.user_id)
    .bind(PROBE_TXID)
    .bind(now)
    .bind(now)
    .execute(&pool)
    .await
    .expect("seed tx");

    // The enriched frame exactly as arcade v0.10.1 pushes it.
    let ev = ArcadeStatusEvent {
        txid: PROBE_TXID.to_string(),
        tx_status: "MINED".to_string(),
        timestamp: Some("2026-07-22T19:06:51.907Z".to_string()),
        block_hash: Some(PROBE_BLOCK_HASH.to_string()),
        block_height: Some(PROBE_HEIGHT),
        merkle_path: Some(PROBE_MERKLE_PATH_HEX.to_string()),
        event_id: None,
    };
    let trigger = AtomicBool::new(false);
    let updated = ArcadeEventsTask::<StorageSqlx>::apply_event(&storage, &ev, &trigger)
        .await
        .expect("apply_event");

    assert!(updated, "inline ingest must report an update");
    assert!(
        !trigger.load(Ordering::SeqCst),
        "fetch fallback must NOT fire when the inline proof latches"
    );
    let (count, stored_path): (i64, Vec<u8>) = sqlx::query_as(
        "SELECT COUNT(*), COALESCE(MAX(merkle_path), X'') FROM proven_txs WHERE txid = ?",
    )
    .bind(PROBE_TXID)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(count, 1);
    assert_eq!(
        hex::encode(stored_path),
        PROBE_MERKLE_PATH_HEX,
        "stored proof must be the production BUMP byte-for-byte"
    );
    assert_eq!(req_status(&pool, PROBE_TXID).await, "completed");
    assert_eq!(tx_status(&pool, PROBE_TXID).await, "completed");

    // And the legacy-frame fallback still works: a status-only MINED event
    // for an unknown txid sets the fetch trigger (pre-v0.10.1 behavior).
    let legacy = ArcadeStatusEvent {
        txid: "e".repeat(64),
        tx_status: "MINED".to_string(),
        timestamp: None,
        block_hash: None,
        block_height: None,
        merkle_path: None,
        event_id: None,
    };
    let trigger2 = AtomicBool::new(false);
    ArcadeEventsTask::<StorageSqlx>::apply_event(&storage, &legacy, &trigger2)
        .await
        .expect("legacy apply");
    assert!(
        trigger2.load(Ordering::SeqCst),
        "legacy MINED frame must fall back to the fetch trigger"
    );
}
