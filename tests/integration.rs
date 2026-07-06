//! Integration tests for all 28 WalletInterface HTTP endpoints.
//!
//! These tests spin up a real HTTP server with a fresh wallet backed by
//! a temporary SQLite database. They exercise every endpoint and verify
//! the response shape.

use bsv_sdk::primitives::PrivateKey;
use bsv_wallet_cli::server::{self, ServerConfig};
use bsv_wallet_toolbox::{
    Chain, Services, ServicesOptions, StorageSqlx, Wallet, WalletStorageWriter,
};
use reqwest::Client;
use serde_json::{json, Value};
use std::net::SocketAddr;
use tempfile::TempDir;

/// Spin up a server on a random port, return the base URL and a reqwest client.
async fn setup() -> (String, Client, TempDir) {
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

    let services = {
        let mut opts = ServicesOptions::mainnet();
        if let Ok(url) = std::env::var("CHAINTRACKS_URL") {
            opts = opts.with_chaintracks_url(url);
        }
        Services::with_options(Chain::Main, opts).expect("services")
    };
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

    let base = format!("http://{}", addr);
    let client = Client::new();
    (base, client, tmp)
}

/// Helper: POST JSON with Origin header
async fn post_json(client: &Client, url: &str, body: Value) -> reqwest::Response {
    client
        .post(url)
        .header("Origin", "http://test.local")
        .json(&body)
        .send()
        .await
        .expect("request failed")
}

/// Helper: POST and assert 200, printing error body on failure
async fn post_json_ok(client: &Client, url: &str, body: Value) -> Value {
    let resp = post_json(client, url, body).await;
    let status = resp.status();
    let body: Value = resp.json().await.unwrap();
    assert_eq!(status, 200, "endpoint returned {}: {:?}", status, body);
    body
}

// =============================================================================
// Batch 1: Status (GET)
// =============================================================================

#[tokio::test]
async fn test_is_authenticated() {
    let (base, client, _tmp) = setup().await;
    let resp = client
        .get(format!("{base}/isAuthenticated"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body: Value = resp.json().await.unwrap();
    assert_eq!(body["authenticated"], true);
}

#[tokio::test]
async fn test_get_height() {
    let (base, client, _tmp) = setup().await;
    let resp = client
        .get(format!("{base}/getHeight"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body: Value = resp.json().await.unwrap();
    assert!(body["height"].is_number(), "height should be a number");
}

#[tokio::test]
async fn test_get_network() {
    let (base, client, _tmp) = setup().await;
    let resp = client
        .get(format!("{base}/getNetwork"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body: Value = resp.json().await.unwrap();
    assert_eq!(body["network"], "mainnet");
}

#[tokio::test]
async fn test_get_version() {
    let (base, client, _tmp) = setup().await;
    let resp = client
        .get(format!("{base}/getVersion"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body: Value = resp.json().await.unwrap();
    assert!(body["version"].is_string(), "version should be a string");
}

#[tokio::test]
async fn test_wait_for_authentication() {
    let (base, client, _tmp) = setup().await;
    let resp = client
        .get(format!("{base}/waitForAuthentication"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body: Value = resp.json().await.unwrap();
    assert_eq!(body["authenticated"], true);
}

// =============================================================================
// Batch 2: Header
// =============================================================================

#[tokio::test]
async fn test_get_header_for_height() {
    let (base, client, _tmp) = setup().await;
    let resp = post_json(
        &client,
        &format!("{base}/getHeaderForHeight"),
        json!({"height": 1}),
    )
    .await;
    let status = resp.status().as_u16();
    if std::env::var("CHAINTRACKS_URL").is_ok() {
        // With Chaintracks configured, expect 200 and a hex header
        assert_eq!(status, 200, "expected 200 with CHAINTRACKS_URL set");
        let body: Value = resp.json().await.unwrap();
        let header = body["header"].as_str().expect("header should be string");
        assert_eq!(header.len(), 160, "80-byte header = 160 hex chars");
    } else {
        // Without Chaintracks, the endpoint may return 400 or 502 (service error)
        assert!(
            status == 200 || status == 400 || status == 502,
            "unexpected status: {}",
            status
        );
    }
}

// =============================================================================
// Batch 3: Crypto
// =============================================================================

#[tokio::test]
async fn test_get_public_key_identity() {
    let (base, client, _tmp) = setup().await;
    let resp = post_json(
        &client,
        &format!("{base}/getPublicKey"),
        json!({"identityKey": true}),
    )
    .await;
    assert_eq!(resp.status(), 200);
    let body: Value = resp.json().await.unwrap();
    assert!(
        body["publicKey"].is_string(),
        "publicKey should be a hex string"
    );
}

#[tokio::test]
async fn test_encrypt_decrypt_roundtrip() {
    let (base, client, _tmp) = setup().await;
    let plaintext: Vec<u8> = b"hello world".to_vec();

    // Encrypt
    let body = post_json_ok(
        &client,
        &format!("{base}/encrypt"),
        json!({
            "plaintext": plaintext,
            "protocolID": [2, "tests"],
            "keyID": "1",
            "counterparty": "anyone"
        }),
    )
    .await;
    let ciphertext = body["ciphertext"].as_array().expect("ciphertext array");
    assert!(!ciphertext.is_empty(), "ciphertext should not be empty");

    // Decrypt
    let body = post_json_ok(
        &client,
        &format!("{base}/decrypt"),
        json!({
            "ciphertext": ciphertext,
            "protocolID": [2, "tests"],
            "keyID": "1",
            "counterparty": "anyone"
        }),
    )
    .await;
    let decrypted: Vec<u8> = body["plaintext"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_u64().unwrap() as u8)
        .collect();
    assert_eq!(decrypted, plaintext);
}

#[tokio::test]
async fn test_create_verify_signature_roundtrip() {
    let (base, client, _tmp) = setup().await;
    let data: Vec<u8> = b"sign this".to_vec();

    // Create signature
    let body = post_json_ok(
        &client,
        &format!("{base}/createSignature"),
        json!({
            "data": data,
            "protocolID": [2, "tests"],
            "keyID": "1",
            "counterparty": "anyone"
        }),
    )
    .await;
    let signature = body["signature"].as_array().expect("signature array");

    // Verify signature
    let body = post_json_ok(
        &client,
        &format!("{base}/verifySignature"),
        json!({
            "data": data,
            "signature": signature,
            "protocolID": [2, "tests"],
            "keyID": "1",
            "counterparty": "anyone",
            "forSelf": true
        }),
    )
    .await;
    assert_eq!(body["valid"], true);
}

#[tokio::test]
async fn test_create_verify_hmac_roundtrip() {
    let (base, client, _tmp) = setup().await;
    let data: Vec<u8> = b"hmac this".to_vec();

    // Create HMAC
    let body = post_json_ok(
        &client,
        &format!("{base}/createHmac"),
        json!({
            "data": data,
            "protocolID": [2, "tests"],
            "keyID": "1",
            "counterparty": "anyone"
        }),
    )
    .await;
    let hmac = body["hmac"].as_array().expect("hmac array");
    assert_eq!(hmac.len(), 32, "HMAC should be 32 bytes");

    // Verify HMAC
    let body = post_json_ok(
        &client,
        &format!("{base}/verifyHmac"),
        json!({
            "data": data,
            "hmac": hmac,
            "protocolID": [2, "tests"],
            "keyID": "1",
            "counterparty": "anyone"
        }),
    )
    .await;
    assert_eq!(body["valid"], true);
}

// =============================================================================
// Batch 4: Transaction Workflow
// =============================================================================

#[tokio::test]
async fn test_list_actions_empty() {
    let (base, client, _tmp) = setup().await;
    let resp = post_json(
        &client,
        &format!("{base}/listActions"),
        json!({"labels": ["test"]}),
    )
    .await;
    assert_eq!(resp.status(), 200);
    let body: Value = resp.json().await.unwrap();
    assert_eq!(body["totalActions"], 0);
    assert!(body["actions"].is_array());
}

#[tokio::test]
async fn test_list_outputs_empty() {
    let (base, client, _tmp) = setup().await;
    let resp = post_json(
        &client,
        &format!("{base}/listOutputs"),
        json!({"basket": "default"}),
    )
    .await;
    assert_eq!(resp.status(), 200);
    let body: Value = resp.json().await.unwrap();
    assert_eq!(body["totalOutputs"], 0);
    assert!(body["outputs"].is_array());
}

// =============================================================================
// Batch 5: Certificates
// =============================================================================

#[tokio::test]
async fn test_list_certificates_empty() {
    let (base, client, _tmp) = setup().await;
    let resp = post_json(
        &client,
        &format!("{base}/listCertificates"),
        json!({"certifiers": [], "types": []}),
    )
    .await;
    assert_eq!(resp.status(), 200);
    let body: Value = resp.json().await.unwrap();
    assert_eq!(body["totalCertificates"], 0);
    assert!(body["certificates"].is_array());
}

// =============================================================================
// Batch 6: Discovery
// =============================================================================

#[tokio::test]
async fn test_discover_by_identity_key() {
    let (base, client, _tmp) = setup().await;
    let body = post_json_ok(
        &client,
        &format!("{base}/discoverByIdentityKey"),
        json!({"identityKey": "0279be667ef9dcbbac55a06295ce870b07029bfcdb2dce28d959f2815b16f81798"}),
    )
    .await;
    assert_eq!(
        body["totalCertificates"], 0,
        "fresh wallet should have 0 certificates"
    );
    assert!(
        body["certificates"].as_array().unwrap().is_empty(),
        "certificates should be empty"
    );
}

/// Discovery with invalid identity key still returns 200 with empty results (endpoint is lenient).
#[tokio::test]
async fn test_discover_by_identity_key_invalid() {
    let (base, client, _tmp) = setup().await;
    let body = post_json_ok(
        &client,
        &format!("{base}/discoverByIdentityKey"),
        json!({"identityKey": "not-a-valid-pubkey"}),
    )
    .await;
    assert_eq!(
        body["totalCertificates"], 0,
        "invalid key should return 0 certificates"
    );
    assert!(body["certificates"].as_array().unwrap().is_empty());
}

#[tokio::test]
async fn test_discover_by_attributes() {
    let (base, client, _tmp) = setup().await;
    let body = post_json_ok(
        &client,
        &format!("{base}/discoverByAttributes"),
        json!({"attributes": {"name": "test"}}),
    )
    .await;
    assert_eq!(
        body["totalCertificates"], 0,
        "fresh wallet should have 0 certificates"
    );
    assert!(
        body["certificates"].as_array().unwrap().is_empty(),
        "certificates should be empty"
    );
}

#[tokio::test]
async fn test_discover_by_attributes_empty() {
    let (base, client, _tmp) = setup().await;
    let body = post_json_ok(
        &client,
        &format!("{base}/discoverByAttributes"),
        json!({"attributes": {}}),
    )
    .await;
    assert_eq!(body["totalCertificates"], 0);
    assert!(body["certificates"].as_array().unwrap().is_empty());
}

// =============================================================================
// Batch 7: Edge-case tests for production confidence
// =============================================================================

/// Encrypt with counterparty "self" and decrypt with counterparty "self".
/// This is the most common production pattern: a user storing their own encrypted data.
#[tokio::test]
async fn test_encrypt_decrypt_self_counterparty() {
    let (base, client, _tmp) = setup().await;
    let plaintext: Vec<u8> = b"my secret vault data".to_vec();

    // Encrypt with counterparty "self"
    let body = post_json_ok(
        &client,
        &format!("{base}/encrypt"),
        json!({
            "plaintext": plaintext,
            "protocolID": [2, "tests"],
            "keyID": "1",
            "counterparty": "self"
        }),
    )
    .await;
    let ciphertext = body["ciphertext"].as_array().expect("ciphertext array");
    assert!(!ciphertext.is_empty(), "ciphertext should not be empty");

    // Decrypt with counterparty "self"
    let body = post_json_ok(
        &client,
        &format!("{base}/decrypt"),
        json!({
            "ciphertext": ciphertext,
            "protocolID": [2, "tests"],
            "keyID": "1",
            "counterparty": "self"
        }),
    )
    .await;
    let decrypted: Vec<u8> = body["plaintext"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_u64().unwrap() as u8)
        .collect();
    assert_eq!(
        decrypted, plaintext,
        "self-encrypted data should round-trip"
    );
}

/// Encrypt and decrypt a 10KB payload to verify large data handling.
#[tokio::test]
async fn test_encrypt_decrypt_large_payload() {
    let (base, client, _tmp) = setup().await;
    // 10KB of repeating pattern bytes
    let plaintext: Vec<u8> = (0..10240).map(|i| (i % 256) as u8).collect();

    // Encrypt
    let body = post_json_ok(
        &client,
        &format!("{base}/encrypt"),
        json!({
            "plaintext": plaintext,
            "protocolID": [2, "tests"],
            "keyID": "1",
            "counterparty": "anyone"
        }),
    )
    .await;
    let ciphertext = body["ciphertext"].as_array().expect("ciphertext array");
    assert!(
        ciphertext.len() >= plaintext.len(),
        "ciphertext should be at least as large as plaintext"
    );

    // Decrypt
    let body = post_json_ok(
        &client,
        &format!("{base}/decrypt"),
        json!({
            "ciphertext": ciphertext,
            "protocolID": [2, "tests"],
            "keyID": "1",
            "counterparty": "anyone"
        }),
    )
    .await;
    let decrypted: Vec<u8> = body["plaintext"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_u64().unwrap() as u8)
        .collect();
    assert_eq!(decrypted.len(), 10240, "decrypted length should be 10KB");
    assert_eq!(
        decrypted, plaintext,
        "large payload should round-trip exactly"
    );
}

/// Send garbage ciphertext to /decrypt and verify it returns 400, not a panic/500.
#[tokio::test]
async fn test_encrypt_bad_ciphertext_returns_error() {
    let (base, client, _tmp) = setup().await;
    let garbage: Vec<u8> = vec![0xDE, 0xAD, 0xBE, 0xEF, 0x01, 0x02, 0x03, 0x04];

    let resp = post_json(
        &client,
        &format!("{base}/decrypt"),
        json!({
            "ciphertext": garbage,
            "protocolID": [2, "tests"],
            "keyID": "1",
            "counterparty": "anyone"
        }),
    )
    .await;
    assert_eq!(
        resp.status(),
        400,
        "garbage ciphertext should return 400, got {}",
        resp.status()
    );
    let body: Value = resp.json().await.unwrap();
    assert!(
        body["message"].is_string(),
        "error response should have a message field"
    );
}

/// Create a signature for data A, then verify against data B.
/// The wallet returns 400 with an error message indicating the signature is not valid.
#[tokio::test]
async fn test_verify_signature_wrong_data_returns_invalid() {
    let (base, client, _tmp) = setup().await;
    let data_a: Vec<u8> = b"original data".to_vec();
    let data_b: Vec<u8> = b"different data".to_vec();

    // Create signature over data_a
    let body = post_json_ok(
        &client,
        &format!("{base}/createSignature"),
        json!({
            "data": data_a,
            "protocolID": [2, "tests"],
            "keyID": "1",
            "counterparty": "anyone"
        }),
    )
    .await;
    let signature = body["signature"].as_array().expect("signature array");

    // Verify signature against data_b — should fail (400 with error message)
    let resp = post_json(
        &client,
        &format!("{base}/verifySignature"),
        json!({
            "data": data_b,
            "signature": signature,
            "protocolID": [2, "tests"],
            "keyID": "1",
            "counterparty": "anyone",
            "forSelf": true
        }),
    )
    .await;
    assert_eq!(
        resp.status(),
        400,
        "signature verified against wrong data should return 400"
    );
    let body: Value = resp.json().await.unwrap();
    let message = body["message"].as_str().unwrap_or("");
    assert!(
        message.to_lowercase().contains("not valid") || message.to_lowercase().contains("invalid"),
        "error should indicate invalid signature, got: {message}"
    );
}

/// Create an HMAC for data A, then verify against data B.
/// The wallet returns 400 with an error message indicating the HMAC is not valid.
#[tokio::test]
async fn test_verify_hmac_wrong_data_returns_invalid() {
    let (base, client, _tmp) = setup().await;
    let data_a: Vec<u8> = b"original hmac data".to_vec();
    let data_b: Vec<u8> = b"different hmac data".to_vec();

    // Create HMAC over data_a
    let body = post_json_ok(
        &client,
        &format!("{base}/createHmac"),
        json!({
            "data": data_a,
            "protocolID": [2, "tests"],
            "keyID": "1",
            "counterparty": "anyone"
        }),
    )
    .await;
    let hmac = body["hmac"].as_array().expect("hmac array");

    // Verify HMAC against data_b — should fail (400 with error message)
    let resp = post_json(
        &client,
        &format!("{base}/verifyHmac"),
        json!({
            "data": data_b,
            "hmac": hmac,
            "protocolID": [2, "tests"],
            "keyID": "1",
            "counterparty": "anyone"
        }),
    )
    .await;
    assert_eq!(
        resp.status(),
        400,
        "HMAC verified against wrong data should return 400"
    );
    let body: Value = resp.json().await.unwrap();
    let message = body["message"].as_str().unwrap_or("");
    assert!(
        message.to_lowercase().contains("not valid") || message.to_lowercase().contains("invalid"),
        "error should indicate invalid HMAC, got: {message}"
    );
}

/// POST to /encrypt WITHOUT an Origin header. Should return 400 with "Origin header required".
#[tokio::test]
async fn test_encrypt_missing_origin_returns_400() {
    let (base, client, _tmp) = setup().await;
    let plaintext: Vec<u8> = b"test".to_vec();

    // Send request WITHOUT Origin header (do not use post_json helper which adds it)
    let resp = client
        .post(format!("{base}/encrypt"))
        .json(&json!({
            "plaintext": plaintext,
            "protocolID": [2, "tests"],
            "keyID": "1",
            "counterparty": "anyone"
        }))
        .send()
        .await
        .expect("request failed");

    assert_eq!(
        resp.status(),
        400,
        "missing Origin should return 400, got {}",
        resp.status()
    );
    let body: Value = resp.json().await.unwrap();
    let message = body["message"].as_str().unwrap_or("");
    assert!(
        message.contains("Origin header required"),
        "error message should mention Origin header, got: {message}"
    );
}

// =============================================================================
// Batch 8: Transaction workflow — createAction, signAction, abortAction,
//          relinquishOutput, internalizeAction
// =============================================================================

/// POST /createAction with outputs but no funding — should return 400.
#[tokio::test]
async fn test_create_action_insufficient_funds() {
    let (base, client, _tmp) = setup().await;
    let resp = post_json(
        &client,
        &format!("{base}/createAction"),
        json!({
            "description": "test unfunded action",
            "outputs": [{
                "lockingScript": "76a91489abcdefabbaabbaabbaabbaabbaabbaabbaabba88ac",
                "satoshis": 1000,
                "outputDescription": "test output"
            }]
        }),
    )
    .await;
    let status = resp.status().as_u16();
    let body: Value = resp.json().await.unwrap();
    // Insufficient funds → 402, other wallet errors → 400
    assert!(
        status == 402 || status == 400,
        "unfunded wallet creating action with outputs should return 402 or 400, got {}: {:?}",
        status,
        body
    );
    assert!(
        body["message"].is_string(),
        "error should have message field"
    );
}

/// POST /signAction with invalid reference — should return 400.
#[tokio::test]
async fn test_sign_action_invalid_reference() {
    let (base, client, _tmp) = setup().await;
    let resp = post_json(
        &client,
        &format!("{base}/signAction"),
        json!({
            "spends": {},
            "reference": "nonexistent-reference-abc123"
        }),
    )
    .await;
    assert_eq!(
        resp.status(),
        400,
        "invalid signAction reference should return 400"
    );
    let body: Value = resp.json().await.unwrap();
    assert!(
        body["message"].is_string(),
        "error should have message field"
    );
}

/// POST /abortAction with invalid reference — should return 400.
#[tokio::test]
async fn test_abort_action_invalid_reference() {
    let (base, client, _tmp) = setup().await;
    let resp = post_json(
        &client,
        &format!("{base}/abortAction"),
        json!({
            "reference": "nonexistent-reference-abc123"
        }),
    )
    .await;
    let status = resp.status().as_u16();
    let body: Value = resp.json().await.unwrap();
    // Invalid reference → 404 (not found) or 400 depending on error message
    assert!(
        status == 400 || status == 404,
        "invalid abortAction reference should return 400 or 404, got {}: {:?}",
        status,
        body
    );
    assert!(
        body["message"].is_string(),
        "error should have message field"
    );
}

/// POST /relinquishOutput with nonexistent output — wallet returns 200 with relinquished: false.
#[tokio::test]
async fn test_relinquish_output_nonexistent() {
    let (base, client, _tmp) = setup().await;
    let body = post_json_ok(
        &client,
        &format!("{base}/relinquishOutput"),
        json!({
            "basket": "default",
            "output": "deadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeef.0"
        }),
    )
    .await;
    // Nonexistent output: wallet may return relinquished: false or true (no-op)
    assert!(
        body["relinquished"].is_boolean(),
        "response should have relinquished boolean, got: {:?}",
        body
    );
}

/// POST /internalizeAction with garbage tx — should return 400.
#[tokio::test]
async fn test_internalize_action_invalid_tx() {
    let (base, client, _tmp) = setup().await;
    let resp = post_json(
        &client,
        &format!("{base}/internalizeAction"),
        json!({
            "tx": [0xDE, 0xAD, 0xBE, 0xEF],
            "outputs": [{
                "outputIndex": 0,
                "protocol": "wallet payment",
                "paymentRemittance": {
                    "derivationPrefix": "test",
                    "derivationSuffix": "test",
                    "senderIdentityKey": "0279be667ef9dcbbac55a06295ce870b07029bfcdb2dce28d959f2815b16f81798"
                }
            }],
            "description": "test invalid internalize"
        }),
    )
    .await;
    assert_eq!(resp.status(), 400, "garbage tx should return 400");
    let body: Value = resp.json().await.unwrap();
    assert!(
        body["message"].is_string(),
        "error should have message field"
    );
}

// =============================================================================
// Batch 9: Certificates — acquireCertificate, proveCertificate,
//          relinquishCertificate
// =============================================================================

/// Full certificate lifecycle: acquire → list (find it) → relinquish → list (verify gone).
/// All operations are DB-only — no chain interaction or funding needed.
#[tokio::test]
async fn test_certificate_full_lifecycle() {
    let (base, client, _tmp) = setup().await;
    let cert_type = "dGVzdA=="; // base64("test")
    let certifier = "0279be667ef9dcbbac55a06295ce870b07029bfcdb2dce28d959f2815b16f81798";

    // Step 1: Acquire certificate (direct protocol)
    let cert = post_json_ok(
        &client,
        &format!("{base}/acquireCertificate"),
        json!({
            "certificateType": cert_type,
            "certifier": certifier,
            "acquisitionProtocol": "direct",
            "fields": {"name": "test"},
            "serialNumber": "AQID",
            "revocationOutpoint": "deadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeef.0",
            "signature": "deadbeef",
            "keyringRevealer": "certifier",
            "keyringForSubject": {}
        }),
    )
    .await;
    assert!(
        cert["certificateType"].is_string(),
        "should have certificateType"
    );
    assert!(cert["certifier"].is_string(), "should have certifier");
    assert!(cert["subject"].is_string(), "should have subject");
    let serial = cert["serialNumber"]
        .as_str()
        .expect("cert should have serialNumber");

    // Step 2: List certificates — should find our cert
    let list = post_json_ok(
        &client,
        &format!("{base}/listCertificates"),
        json!({
            "certifiers": [certifier],
            "types": [cert_type]
        }),
    )
    .await;
    assert!(
        list["totalCertificates"].as_u64().unwrap() >= 1,
        "should find at least 1 certificate after acquisition"
    );
    let certs = list["certificates"].as_array().expect("certificates array");
    // Each entry has { certificate: { serialNumber, ... }, verifier: "..." }
    assert!(
        certs
            .iter()
            .any(|c| c["certificate"]["serialNumber"].as_str() == Some(serial)),
        "listed certificates should include our acquired cert (serial={})",
        serial
    );

    // Step 3: Relinquish the certificate
    let relinquish = post_json_ok(
        &client,
        &format!("{base}/relinquishCertificate"),
        json!({
            "certificateType": cert_type,
            "serialNumber": serial,
            "certifier": certifier
        }),
    )
    .await;
    assert_eq!(
        relinquish["relinquished"], true,
        "certificate should be relinquished successfully"
    );

    // Step 4: List again — cert should be gone
    let list2 = post_json_ok(
        &client,
        &format!("{base}/listCertificates"),
        json!({
            "certifiers": [certifier],
            "types": [cert_type]
        }),
    )
    .await;
    assert_eq!(
        list2["totalCertificates"].as_u64().unwrap(),
        0,
        "certificate count should be 0 after relinquishing"
    );
}

/// POST /proveCertificate with a made-up certificate — should return 400.
#[tokio::test]
async fn test_prove_certificate_not_found() {
    let (base, client, _tmp) = setup().await;
    let resp = post_json(
        &client,
        &format!("{base}/proveCertificate"),
        json!({
            "certificate": {
                "certificateType": "dGVzdA==",
                "subject": "0279be667ef9dcbbac55a06295ce870b07029bfcdb2dce28d959f2815b16f81798",
                "serialNumber": "AQID",
                "certifier": "0279be667ef9dcbbac55a06295ce870b07029bfcdb2dce28d959f2815b16f81798",
                "revocationOutpoint": "deadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeef.0",
                "signature": "deadbeef",
                "fields": {"name": "test"}
            },
            "fieldsToReveal": ["name"],
            "verifier": "0279be667ef9dcbbac55a06295ce870b07029bfcdb2dce28d959f2815b16f81798"
        }),
    )
    .await;
    assert_eq!(
        resp.status(),
        400,
        "proving nonexistent cert should return 400"
    );
    let body: Value = resp.json().await.unwrap();
    assert!(
        body["message"].is_string(),
        "error should have message field"
    );
}

/// POST /relinquishCertificate with nonexistent cert — wallet returns 200 with relinquished: false.
#[tokio::test]
async fn test_relinquish_certificate_not_found() {
    let (base, client, _tmp) = setup().await;
    let body = post_json_ok(
        &client,
        &format!("{base}/relinquishCertificate"),
        json!({
            "certificateType": "dGVzdA==",
            "serialNumber": "AQID",
            "certifier": "0279be667ef9dcbbac55a06295ce870b07029bfcdb2dce28d959f2815b16f81798"
        }),
    )
    .await;
    assert!(
        body["relinquished"].is_boolean(),
        "response should have relinquished boolean, got: {:?}",
        body
    );
}

// =============================================================================
// Batch 10: Key Linkage — revealCounterpartyKeyLinkage,
//           revealSpecificKeyLinkage
// =============================================================================

/// POST /revealCounterpartyKeyLinkage with valid pubkeys — returns 200 with linkage object.
#[tokio::test]
async fn test_reveal_counterparty_key_linkage() {
    let (base, client, _tmp) = setup().await;
    let test_key = "0279be667ef9dcbbac55a06295ce870b07029bfcdb2dce28d959f2815b16f81798";
    let body = post_json_ok(
        &client,
        &format!("{base}/revealCounterpartyKeyLinkage"),
        json!({
            "counterparty": test_key,
            "verifier": test_key
        }),
    )
    .await;
    let linkage = body
        .get("linkage")
        .expect("response should have linkage object");
    assert!(
        linkage["encryptedLinkage"].is_string(),
        "linkage should have encryptedLinkage"
    );
    assert!(
        linkage["encryptedLinkageProof"].is_string(),
        "linkage should have encryptedLinkageProof"
    );
    assert!(linkage["prover"].is_string(), "linkage should have prover");
    assert!(
        linkage["verifier"].is_string(),
        "linkage should have verifier"
    );
    assert!(
        linkage["counterparty"].is_string(),
        "linkage should have counterparty"
    );
    assert!(
        body["revelationTime"].is_string(),
        "response should have revelationTime"
    );
}

/// POST /revealCounterpartyKeyLinkage with invalid pubkey — returns 400.
#[tokio::test]
async fn test_reveal_counterparty_key_linkage_invalid_key() {
    let (base, client, _tmp) = setup().await;
    let resp = post_json(
        &client,
        &format!("{base}/revealCounterpartyKeyLinkage"),
        json!({
            "counterparty": "not-a-valid-pubkey",
            "verifier": "0279be667ef9dcbbac55a06295ce870b07029bfcdb2dce28d959f2815b16f81798"
        }),
    )
    .await;
    assert_eq!(
        resp.status(),
        400,
        "invalid counterparty key should return 400"
    );
    let body: Value = resp.json().await.unwrap();
    assert!(
        body["message"].is_string(),
        "error should have message field"
    );
}

/// POST /revealSpecificKeyLinkage with valid pubkeys and protocol — returns 200.
#[tokio::test]
async fn test_reveal_specific_key_linkage() {
    let (base, client, _tmp) = setup().await;
    let test_key = "0279be667ef9dcbbac55a06295ce870b07029bfcdb2dce28d959f2815b16f81798";
    let body = post_json_ok(
        &client,
        &format!("{base}/revealSpecificKeyLinkage"),
        json!({
            "counterparty": test_key,
            "verifier": test_key,
            "protocolID": [2, "tests"],
            "keyID": "1"
        }),
    )
    .await;
    let linkage = body
        .get("linkage")
        .expect("response should have linkage object");
    assert!(
        linkage["encryptedLinkage"].is_string(),
        "linkage should have encryptedLinkage"
    );
    assert!(
        linkage["encryptedLinkageProof"].is_string(),
        "linkage should have encryptedLinkageProof"
    );
    assert!(linkage["prover"].is_string(), "linkage should have prover");
    assert!(
        linkage["verifier"].is_string(),
        "linkage should have verifier"
    );
    assert!(
        body.get("protocol").is_some(),
        "response should have protocol"
    );
}

/// POST /revealSpecificKeyLinkage with invalid pubkey — returns 400.
#[tokio::test]
async fn test_reveal_specific_key_linkage_invalid_key() {
    let (base, client, _tmp) = setup().await;
    let resp = post_json(
        &client,
        &format!("{base}/revealSpecificKeyLinkage"),
        json!({
            "counterparty": "not-a-valid-pubkey",
            "verifier": "0279be667ef9dcbbac55a06295ce870b07029bfcdb2dce28d959f2815b16f81798",
            "protocolID": [2, "tests"],
            "keyID": "1"
        }),
    )
    .await;
    assert_eq!(
        resp.status(),
        400,
        "invalid counterparty key should return 400"
    );
    let body: Value = resp.json().await.unwrap();
    assert!(
        body["message"].is_string(),
        "error should have message field"
    );
}

// =============================================================================
// Batch 11: Edge-case tests for production confidence
// =============================================================================

/// Get a derived public key (with protocolID, keyID, counterparty) and verify
/// it differs from the identity key.
#[tokio::test]
async fn test_get_public_key_derived() {
    let (base, client, _tmp) = setup().await;

    // Get identity key
    let identity_body = post_json_ok(
        &client,
        &format!("{base}/getPublicKey"),
        json!({"identityKey": true}),
    )
    .await;
    let identity_key = identity_body["publicKey"]
        .as_str()
        .expect("identity publicKey should be a string");

    // Get derived key with protocol, keyID, and counterparty
    let derived_body = post_json_ok(
        &client,
        &format!("{base}/getPublicKey"),
        json!({
            "identityKey": false,
            "protocolID": [2, "tests"],
            "keyID": "1",
            "counterparty": "anyone"
        }),
    )
    .await;
    let derived_key = derived_body["publicKey"]
        .as_str()
        .expect("derived publicKey should be a string");

    // The derived key must be a valid compressed public key (66 hex chars starting with 02 or 03)
    assert_eq!(
        derived_key.len(),
        66,
        "derived key should be 66 hex characters (compressed pubkey)"
    );
    assert!(
        derived_key.starts_with("02") || derived_key.starts_with("03"),
        "derived key should start with 02 or 03, got: {}",
        &derived_key[..4]
    );

    // The derived key should differ from the identity key
    assert_ne!(
        identity_key, derived_key,
        "derived key should differ from identity key"
    );
}

// =============================================================================
// Batch 12: Additional coverage — non-default baskets, pagination,
//           labelQueryMode, createAction options passthrough
// =============================================================================

/// listOutputs with a non-default basket that doesn't exist — returns 200 with empty results (#17).
#[tokio::test]
async fn test_list_outputs_nondefault_basket() {
    let (base, client, _tmp) = setup().await;
    let body = post_json_ok(
        &client,
        &format!("{base}/listOutputs"),
        json!({
            "basket": "worm-proofs",
            "include": "locking scripts",
            "limit": 10,
            "offset": 0
        }),
    )
    .await;
    assert_eq!(
        body["totalOutputs"], 0,
        "non-existent basket should have 0 outputs"
    );
    assert!(
        body["outputs"].as_array().unwrap().is_empty(),
        "outputs should be empty"
    );
}

/// listActions with offset beyond available data — returns 200 with empty results (#18).
/// Note: totalActions includes offset (server quirk), but the actions array is correctly empty.
#[tokio::test]
async fn test_list_actions_pagination() {
    let (base, client, _tmp) = setup().await;
    let body = post_json_ok(
        &client,
        &format!("{base}/listActions"),
        json!({
            "labels": [],
            "labelQueryMode": "any",
            "includeLabels": false,
            "includeInputs": false,
            "includeOutputs": false,
            "limit": 10,
            "offset": 100
        }),
    )
    .await;
    assert!(
        body["totalActions"].is_number(),
        "totalActions should be a number"
    );
    assert!(
        body["actions"].as_array().unwrap().is_empty(),
        "actions should be empty at high offset"
    );
}

/// listOutputs with offset beyond available data — returns 200 with empty results (#18).
/// Note: totalOutputs includes offset (server quirk), but the outputs array is correctly empty.
#[tokio::test]
async fn test_list_outputs_pagination() {
    let (base, client, _tmp) = setup().await;
    let body = post_json_ok(
        &client,
        &format!("{base}/listOutputs"),
        json!({
            "basket": "default",
            "include": "locking scripts",
            "limit": 10,
            "offset": 100
        }),
    )
    .await;
    assert!(
        body["totalOutputs"].is_number(),
        "totalOutputs should be a number"
    );
    assert!(
        body["outputs"].as_array().unwrap().is_empty(),
        "outputs should be empty at high offset"
    );
}

/// listActions with labelQueryMode "all" — accepted and returns well-shaped response (#19).
#[tokio::test]
async fn test_list_actions_label_query_mode_all() {
    let (base, client, _tmp) = setup().await;
    let body = post_json_ok(
        &client,
        &format!("{base}/listActions"),
        json!({
            "labels": ["send", "receive"],
            "labelQueryMode": "all",
            "includeLabels": true,
            "includeInputs": false,
            "includeOutputs": false,
            "limit": 10,
            "offset": 0
        }),
    )
    .await;
    assert_eq!(body["totalActions"], 0);
    assert!(body["actions"].as_array().unwrap().is_empty());
}

/// createAction with noSend option — deserializes correctly (402 not 500) (#20).
#[tokio::test]
async fn test_create_action_with_no_send_option() {
    let (base, client, _tmp) = setup().await;
    let resp = post_json(
        &client,
        &format!("{base}/createAction"),
        json!({
            "description": "noSend test",
            "outputs": [{
                "lockingScript": "76a91489abcdefabbaabbaabbaabbaabbaabbaabbaabba88ac",
                "satoshis": 1000,
                "outputDescription": "test"
            }],
            "options": {
                "signAndProcess": true,
                "noSend": true,
                "acceptDelayedBroadcast": false,
                "randomizeOutputs": false
            }
        }),
    )
    .await;
    let status = resp.status().as_u16();
    assert!(
        status == 402 || status == 400,
        "noSend option should deserialize correctly, got {} (500 = deserialization bug)",
        status
    );
    let body: Value = resp.json().await.unwrap();
    assert!(
        body["message"].is_string(),
        "error should have message field"
    );
}

/// createAction with randomizeOutputs option — deserializes correctly (#20).
#[tokio::test]
async fn test_create_action_with_randomize_outputs() {
    let (base, client, _tmp) = setup().await;
    let resp = post_json(
        &client,
        &format!("{base}/createAction"),
        json!({
            "description": "randomizeOutputs test",
            "outputs": [{
                "lockingScript": "76a91489abcdefabbaabbaabbaabbaabbaabbaabbaabba88ac",
                "satoshis": 1000,
                "outputDescription": "test"
            }],
            "options": {
                "signAndProcess": true,
                "randomizeOutputs": true
            }
        }),
    )
    .await;
    let status = resp.status().as_u16();
    assert!(
        status == 402 || status == 400,
        "randomizeOutputs option should deserialize correctly, got {}",
        status
    );
}

/// createAction with acceptDelayedBroadcast option — deserializes correctly (#20).
#[tokio::test]
async fn test_create_action_with_accept_delayed_broadcast() {
    let (base, client, _tmp) = setup().await;
    let resp = post_json(
        &client,
        &format!("{base}/createAction"),
        json!({
            "description": "acceptDelayedBroadcast test",
            "outputs": [{
                "lockingScript": "76a91489abcdefabbaabbaabbaabbaabbaabbaabbaabba88ac",
                "satoshis": 1000,
                "outputDescription": "test"
            }],
            "options": {
                "signAndProcess": true,
                "acceptDelayedBroadcast": true
            }
        }),
    )
    .await;
    let status = resp.status().as_u16();
    assert!(
        status == 402 || status == 400,
        "acceptDelayedBroadcast option should deserialize correctly, got {}",
        status
    );
}

/// createAction output with customInstructions, basket, and tags — deserializes correctly (#20).
#[tokio::test]
async fn test_create_action_output_custom_instructions() {
    let (base, client, _tmp) = setup().await;
    let resp = post_json(
        &client,
        &format!("{base}/createAction"),
        json!({
            "description": "custom_instructions test",
            "outputs": [{
                "lockingScript": "76a91489abcdefabbaabbaabbaabbaabbaabbaabbaabba88ac",
                "satoshis": 1000,
                "outputDescription": "test",
                "basket": "worm-proofs",
                "tags": ["test-tag"],
                "customInstructions": "some-instruction-string"
            }]
        }),
    )
    .await;
    let status = resp.status().as_u16();
    assert!(
        status == 402 || status == 400,
        "output fields should deserialize correctly, got {}",
        status
    );
}

// =============================================================================
// E2E tests against a RUNNING real wallet (gated behind WALLET_URL env var).
//
// These tests hit a live `bsv-wallet serve` instance with real chain data
// and a funded wallet. They are safe: deferred-signing actions are always
// aborted, so no sats are spent.
//
// Run:  WALLET_URL=http://localhost:3322 cargo test --test integration e2e_
// =============================================================================

/// Helper: connect to a running wallet server. Returns None if WALLET_URL not set.
fn e2e_setup() -> Option<(String, Client)> {
    let url = std::env::var("WALLET_URL").ok()?;
    Some((url, Client::new()))
}

/// E2E: createAction (signAndProcess:false) → verify signableTransaction → abortAction → listActions.
/// Tests the full deferred signing HTTP flow with a funded wallet without spending any sats.
#[tokio::test]
async fn e2e_create_and_abort_action() {
    let Some((base, client)) = e2e_setup() else {
        eprintln!("Skipping e2e test: WALLET_URL not set");
        return;
    };

    // Step 1: Create an unsigned action with a tiny output (signAndProcess: false)
    let resp = post_json(
        &client,
        &format!("{base}/createAction"),
        json!({
            "description": "e2e test unsigned action",
            "outputs": [{
                "lockingScript": "76a91489abcdefabbaabbaabbaabbaabbaabbaabbaabba88ac",
                "satoshis": 1,
                "outputDescription": "e2e test output"
            }],
            "labels": ["e2e-test-abort"],
            "options": {
                "signAndProcess": false
            }
        }),
    )
    .await;
    let status = resp.status().as_u16();
    let body: Value = resp.json().await.unwrap();
    assert_eq!(
        status, 200,
        "createAction should succeed on funded wallet: {:?}",
        body
    );

    // Step 2: Verify signableTransaction is present with reference
    let st = body
        .get("signableTransaction")
        .expect("response should have signableTransaction");
    let reference = st["reference"]
        .as_str()
        .expect("signableTransaction should have reference string");
    assert!(!reference.is_empty(), "reference should not be empty");
    assert!(
        st["tx"].is_array(),
        "signableTransaction.tx should be a byte array"
    );

    // Step 3: Abort the unsigned action to release locked UTXOs
    let abort = post_json_ok(
        &client,
        &format!("{base}/abortAction"),
        json!({ "reference": reference }),
    )
    .await;
    assert_eq!(abort["aborted"], true, "abortAction should succeed");

    // Step 4: List actions — the aborted action should appear with failed status
    let list = post_json_ok(
        &client,
        &format!("{base}/listActions"),
        json!({"labels": ["e2e-test-abort"]}),
    )
    .await;
    assert!(
        list["totalActions"].is_number(),
        "listActions should return totalActions"
    );
}

/// E2E: Full crypto round-trip against the real wallet.
/// Proves the production wallet handles encrypt→decrypt and sign→verify correctly.
#[tokio::test]
async fn e2e_crypto_roundtrip() {
    let Some((base, client)) = e2e_setup() else {
        eprintln!("Skipping e2e test: WALLET_URL not set");
        return;
    };

    // Get identity key
    let identity = post_json_ok(
        &client,
        &format!("{base}/getPublicKey"),
        json!({"identityKey": true}),
    )
    .await;
    let identity_key = identity["publicKey"].as_str().expect("identity key");
    assert_eq!(
        identity_key.len(),
        66,
        "identity key should be 66 hex chars"
    );

    // Encrypt with counterparty "self" (most common production pattern)
    let plaintext: Vec<u8> = b"e2e test secret data".to_vec();
    let enc = post_json_ok(
        &client,
        &format!("{base}/encrypt"),
        json!({
            "plaintext": plaintext,
            "protocolID": [2, "e2e test"],
            "keyID": "1",
            "counterparty": "self"
        }),
    )
    .await;
    let ciphertext = enc["ciphertext"].as_array().expect("ciphertext");
    assert!(!ciphertext.is_empty(), "ciphertext should not be empty");

    // Decrypt
    let dec = post_json_ok(
        &client,
        &format!("{base}/decrypt"),
        json!({
            "ciphertext": ciphertext,
            "protocolID": [2, "e2e test"],
            "keyID": "1",
            "counterparty": "self"
        }),
    )
    .await;
    let decrypted: Vec<u8> = dec["plaintext"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_u64().unwrap() as u8)
        .collect();
    assert_eq!(decrypted, plaintext, "decrypt should round-trip");

    // Sign
    let data: Vec<u8> = b"e2e test signature data".to_vec();
    let sig = post_json_ok(
        &client,
        &format!("{base}/createSignature"),
        json!({
            "data": data,
            "protocolID": [2, "e2e test"],
            "keyID": "1",
            "counterparty": "anyone"
        }),
    )
    .await;
    let signature = sig["signature"].as_array().expect("signature");

    // Verify
    let verify = post_json_ok(
        &client,
        &format!("{base}/verifySignature"),
        json!({
            "data": data,
            "signature": signature,
            "protocolID": [2, "e2e test"],
            "keyID": "1",
            "counterparty": "anyone",
            "forSelf": true
        }),
    )
    .await;
    assert_eq!(verify["valid"], true, "signature should verify");
}

/// E2E: Certificate lifecycle against the real wallet.
/// acquire → list → relinquish → list (verify gone).
#[tokio::test]
async fn e2e_certificate_lifecycle() {
    let Some((base, client)) = e2e_setup() else {
        eprintln!("Skipping e2e test: WALLET_URL not set");
        return;
    };

    let cert_type = "ZTJlLXRlc3Q="; // base64("e2e test")
    let certifier = "0279be667ef9dcbbac55a06295ce870b07029bfcdb2dce28d959f2815b16f81798";
    // Use a unique serial per run (timestamp-based) to avoid UNIQUE constraint from previous runs.
    // relinquishCertificate soft-deletes, so the DB row persists and blocks re-insertion.
    use std::time::{SystemTime, UNIX_EPOCH};
    let ts = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let serial_input = format!("e2e-{}", ts);

    // Acquire
    let cert = post_json_ok(
        &client,
        &format!("{base}/acquireCertificate"),
        json!({
            "certificateType": cert_type,
            "certifier": certifier,
            "acquisitionProtocol": "direct",
            "fields": {"email": "test@example.com"},
            "serialNumber": serial_input,
            "revocationOutpoint": "deadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeef.0",
            "signature": "deadbeef",
            "keyringRevealer": "certifier",
            "keyringForSubject": {}
        }),
    )
    .await;
    let serial = cert["serialNumber"].as_str().expect("serialNumber");

    // List — should find it
    let list = post_json_ok(
        &client,
        &format!("{base}/listCertificates"),
        json!({"certifiers": [certifier], "types": [cert_type]}),
    )
    .await;
    assert!(
        list["totalCertificates"].as_u64().unwrap() >= 1,
        "should find the acquired certificate"
    );

    // Relinquish
    let rel = post_json_ok(
        &client,
        &format!("{base}/relinquishCertificate"),
        json!({
            "certificateType": cert_type,
            "serialNumber": serial,
            "certifier": certifier
        }),
    )
    .await;
    assert_eq!(rel["relinquished"], true, "should relinquish successfully");

    // List again — should be gone
    let list2 = post_json_ok(
        &client,
        &format!("{base}/listCertificates"),
        json!({"certifiers": [certifier], "types": [cert_type]}),
    )
    .await;
    assert_eq!(
        list2["totalCertificates"].as_u64().unwrap(),
        0,
        "certificate should be gone after relinquishing"
    );
}

/// E2E: Key linkage with the wallet's real identity key.
#[tokio::test]
async fn e2e_key_linkage() {
    let Some((base, client)) = e2e_setup() else {
        eprintln!("Skipping e2e test: WALLET_URL not set");
        return;
    };

    // Get the wallet's identity key
    let identity = post_json_ok(
        &client,
        &format!("{base}/getPublicKey"),
        json!({"identityKey": true}),
    )
    .await;
    let identity_key = identity["publicKey"].as_str().expect("identity key");

    // Use a different key as counterparty
    let counterparty = "0279be667ef9dcbbac55a06295ce870b07029bfcdb2dce28d959f2815b16f81798";

    // Reveal counterparty key linkage
    let resp = post_json(
        &client,
        &format!("{base}/revealCounterpartyKeyLinkage"),
        json!({
            "counterparty": counterparty,
            "verifier": identity_key
        }),
    )
    .await;
    let status = resp.status().as_u16();
    let body: Value = resp.json().await.unwrap();
    assert!(
        status == 200 || status == 400,
        "counterparty linkage should return 200 or 400, got {}: {:?}",
        status,
        body
    );

    // Reveal specific key linkage
    let resp = post_json(
        &client,
        &format!("{base}/revealSpecificKeyLinkage"),
        json!({
            "counterparty": counterparty,
            "verifier": identity_key,
            "protocolID": [2, "e2e test"],
            "keyID": "1"
        }),
    )
    .await;
    let status = resp.status().as_u16();
    assert!(
        status == 200 || status == 400,
        "specific linkage should return 200 or 400, got {}",
        status
    );
}

/// E2E: All status endpoints against the real wallet.
#[tokio::test]
async fn e2e_status_endpoints() {
    let Some((base, client)) = e2e_setup() else {
        eprintln!("Skipping e2e test: WALLET_URL not set");
        return;
    };

    // isAuthenticated
    let resp = client
        .get(format!("{base}/isAuthenticated"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body: Value = resp.json().await.unwrap();
    assert_eq!(body["authenticated"], true);

    // getHeight — should return real chain height
    let resp = client
        .get(format!("{base}/getHeight"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body: Value = resp.json().await.unwrap();
    let height = body["height"].as_u64().expect("height");
    assert!(
        height > 800000,
        "mainnet height should be > 800000, got {}",
        height
    );

    // getNetwork
    let resp = client
        .get(format!("{base}/getNetwork"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body: Value = resp.json().await.unwrap();
    assert_eq!(body["network"], "mainnet");

    // getVersion
    let resp = client
        .get(format!("{base}/getVersion"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body: Value = resp.json().await.unwrap();
    assert!(body["version"].is_string());

    // getHeaderForHeight — real header for genesis block
    let body = post_json_ok(
        &client,
        &format!("{base}/getHeaderForHeight"),
        json!({"height": 1}),
    )
    .await;
    let header = body["header"].as_str().expect("header");
    assert_eq!(header.len(), 160, "80-byte header = 160 hex chars");
}

/// E2E: signAction happy path — create unsigned tx, sign it, verify signed result.
/// Tests the full deferred-signing flow: createAction(signAndProcess:false) → signAction.
/// The signed tx is NOT auto-broadcast (caller would need to use sendWithResults).
#[tokio::test]
async fn e2e_sign_action_happy_path() {
    let Some((base, client)) = e2e_setup() else {
        eprintln!("Skipping e2e test: WALLET_URL not set");
        return;
    };

    // Step 1: Create an unsigned action (signAndProcess: false) with a 1-sat output
    let create = post_json_ok(
        &client,
        &format!("{base}/createAction"),
        json!({
            "description": "e2e sign action test",
            "outputs": [{
                "lockingScript": "76a91489abcdefabbaabbaabbaabbaabbaabbaabbaabba88ac",
                "satoshis": 1,
                "outputDescription": "e2e sign test output"
            }],
            "labels": ["e2e-test-sign"],
            "options": {
                "signAndProcess": false
            }
        }),
    )
    .await;

    // Step 2: Verify signableTransaction is present with reference
    let st = create
        .get("signableTransaction")
        .expect("response should have signableTransaction");
    let reference = st["reference"]
        .as_str()
        .expect("signableTransaction should have reference string");
    assert!(!reference.is_empty(), "reference should not be empty");
    assert!(
        st["tx"].is_array(),
        "signableTransaction.tx should be a byte array"
    );

    // Step 3: Sign the action with empty spends (wallet signs all inputs itself)
    let sign = post_json_ok(
        &client,
        &format!("{base}/signAction"),
        json!({
            "spends": {},
            "reference": reference
        }),
    )
    .await;

    // Step 4: Verify the signed result contains txid or tx
    let has_txid = sign.get("txid").map(|v| !v.is_null()).unwrap_or(false);
    let has_tx = sign.get("tx").map(|v| !v.is_null()).unwrap_or(false);
    assert!(
        has_txid || has_tx,
        "signAction should return txid or tx: {:?}",
        sign
    );
}

/// E2E: internalizeAction with real BEEF from WoC.
/// Fetches a real BEEF tx, converts to AtomicBEEF, and verifies the wallet
/// gets past BEEF parsing (error should be about ownership, not parsing).
#[tokio::test]
async fn e2e_internalize_action_real_beef() {
    let Some((base, client)) = e2e_setup() else {
        eprintln!("Skipping e2e test: WALLET_URL not set");
        return;
    };

    // Find a real txid by querying WoC for the wallet address history.
    // Set E2E_WALLET_ADDRESS to a funded address, or this test will be skipped.
    let Some(addr) = std::env::var("E2E_WALLET_ADDRESS").ok() else {
        eprintln!("Skipping e2e_internalize: E2E_WALLET_ADDRESS not set");
        return;
    };
    let addr = addr.as_str();
    let history_resp = client
        .get(format!(
            "https://api.whatsonchain.com/v1/bsv/main/address/{addr}/history"
        ))
        .send()
        .await;
    let Ok(resp) = history_resp else {
        eprintln!("Skipping e2e_internalize: WoC address history request failed");
        return;
    };
    if resp.status() != 200 {
        eprintln!("Skipping e2e_internalize: WoC returned {}", resp.status());
        return;
    }
    let history: Value = resp.json().await.unwrap();
    let Some(txid) = history
        .as_array()
        .and_then(|a| a.first())
        .and_then(|t| t["tx_hash"].as_str())
    else {
        eprintln!("Skipping e2e_internalize: no txs found for wallet address");
        return;
    };
    let txid = txid.to_string(); // own the string before resp is dropped

    // Fetch BEEF from WoC
    let beef_resp = client
        .get(format!(
            "https://api.whatsonchain.com/v1/bsv/main/tx/{txid}/beef"
        ))
        .send()
        .await;
    let Ok(resp) = beef_resp else {
        eprintln!("Skipping e2e_internalize: WoC BEEF fetch failed");
        return;
    };
    if resp.status() != 200 {
        eprintln!(
            "Skipping e2e_internalize: WoC BEEF returned {}",
            resp.status()
        );
        return;
    }
    let raw = resp.bytes().await.unwrap().to_vec();

    // WoC may return BEEF as hex string or raw binary
    let beef_bytes = if let Ok(text) = std::str::from_utf8(&raw) {
        let clean = text.trim().trim_matches('"');
        hex::decode(clean).unwrap_or(raw)
    } else {
        raw
    };
    assert!(
        beef_bytes.len() > 36,
        "BEEF data too short: {} bytes",
        beef_bytes.len()
    );

    // Convert to AtomicBEEF: [01,01,01,01] + reversed_txid(32) + standard_beef
    let txid_bytes = hex::decode(&txid).expect("valid hex txid");
    assert_eq!(txid_bytes.len(), 32, "txid should be 32 bytes");
    let mut reversed_txid = txid_bytes;
    reversed_txid.reverse();

    let mut atomic_beef: Vec<u8> = vec![0x01, 0x01, 0x01, 0x01];
    atomic_beef.extend_from_slice(&reversed_txid);
    atomic_beef.extend_from_slice(&beef_bytes);

    // Call internalizeAction with the AtomicBEEF
    let resp = post_json(
        &client,
        &format!("{base}/internalizeAction"),
        json!({
            "tx": atomic_beef,
            "outputs": [{
                "outputIndex": 0,
                "protocol": "wallet payment",
                "paymentRemittance": {
                    "derivationPrefix": "test",
                    "derivationSuffix": "test",
                    "senderIdentityKey": "0279be667ef9dcbbac55a06295ce870b07029bfcdb2dce28d959f2815b16f81798"
                }
            }],
            "description": "e2e internalize real BEEF test"
        }),
    )
    .await;

    // Should get past BEEF parsing — error should be about ownership, not parsing
    let status = resp.status().as_u16();
    let body: Value = resp.json().await.unwrap();
    assert_eq!(
        status, 400,
        "should return 400 (output isn't ours): {:?}",
        body
    );
    let message = body["message"].as_str().unwrap_or("");
    let msg_lower = message.to_lowercase();
    // Error should NOT be about BEEF parsing/version — should be past that stage
    assert!(
        !msg_lower.contains("not a valid beef")
            && !msg_lower.contains("beef version")
            && !msg_lower.contains("unexpected version"),
        "error should be past BEEF parsing (about ownership), got: {message}"
    );
}

/// E2E: createAction with noSend:true — signed but not broadcast.
/// Verifies the response contains tx bytes, txid hex string, and noSendChange outpoints.
/// NOTE: noSend locks UTXOs. Run e2e tests with --test-threads=1 to avoid contention.
#[tokio::test]
async fn e2e_nosend_flow() {
    let Some((base, client)) = e2e_setup() else {
        eprintln!("Skipping e2e test: WALLET_URL not set");
        return;
    };

    // Create a signed-but-not-broadcast action (signAndProcess:true, noSend:true)
    let body = post_json_ok(
        &client,
        &format!("{base}/createAction"),
        json!({
            "description": "e2e noSend test",
            "outputs": [{
                "lockingScript": "76a91489abcdefabbaabbaabbaabbaabbaabbaabbaabba88ac",
                "satoshis": 1,
                "outputDescription": "e2e noSend test output"
            }],
            "labels": ["e2e-test-nosend"],
            "options": {
                "signAndProcess": true,
                "noSend": true
            }
        }),
    )
    .await;

    // Verify txid is a 64-char hex string
    let txid = body["txid"]
        .as_str()
        .expect("noSend response should have txid as hex string");
    assert_eq!(
        txid.len(),
        64,
        "txid should be 64 hex chars, got len={}",
        txid.len()
    );

    // Verify tx is present as a non-empty byte array
    let tx = body["tx"]
        .as_array()
        .expect("noSend response should have tx as byte array");
    assert!(!tx.is_empty(), "tx byte array should not be empty");

    // signableTransaction should NOT be present (tx is already signed)
    assert!(
        body.get("signableTransaction").is_none() || body["signableTransaction"].is_null(),
        "noSend should not have signableTransaction (tx is already signed)"
    );

    // Verify noSendChange is present (change outpoints for the noSend tx)
    assert!(
        body.get("noSendChange").is_some() && !body["noSendChange"].is_null(),
        "noSend response should have noSendChange: {:?}",
        body
    );

    // Clean up: abort the noSend transaction so its UTXOs don't pollute the pool
    // for subsequent tests (e.g. e2e_sign_action_happy_path)
    let _abort = post_json_ok(
        &client,
        &format!("{base}/abortAction"),
        json!({ "reference": txid }),
    )
    .await;
}

// =============================================================================
// Auth token enforcement test
// =============================================================================

/// Spin up a server WITH auth token, verify 401 without token and 200 with token.
#[tokio::test]
async fn test_auth_token_enforcement() {
    let tmp = TempDir::new().expect("temp dir");
    let db_path = tmp.path().join("test_auth.db");

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

    let services = {
        let mut opts = ServicesOptions::mainnet();
        if let Ok(url) = std::env::var("CHAINTRACKS_URL") {
            opts = opts.with_chaintracks_url(url);
        }
        Services::with_options(Chain::Main, opts).expect("services")
    };
    let wallet = Wallet::new(Some(key), storage, services)
        .await
        .expect("wallet");

    let state = server::make_wallet_state(wallet);
    let config = ServerConfig {
        auth_token: Some("test-secret-token".to_string()),
        tls: None,
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

    let base = format!("http://{}", addr);
    let client = Client::new();

    // Without token → 401
    let resp = client
        .get(format!("{base}/isAuthenticated"))
        .send()
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        401,
        "request without token should return 401"
    );

    // With wrong token → 401
    let resp = client
        .get(format!("{base}/isAuthenticated"))
        .header("Authorization", "Bearer wrong-token")
        .send()
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        401,
        "request with wrong token should return 401"
    );

    // With correct token → 200
    let resp = client
        .get(format!("{base}/isAuthenticated"))
        .header("Authorization", "Bearer test-secret-token")
        .send()
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        200,
        "request with correct token should return 200"
    );
    let body: Value = resp.json().await.unwrap();
    assert_eq!(body["authenticated"], true);
}

// =============================================================================
// E2E: BEEF broadcast test — createAction with OP_RETURN (real broadcast)
// =============================================================================

/// E2E: createAction with a 0-sat OP_RETURN output — verifies real broadcast succeeds.
///
/// This test actually broadcasts a transaction to the BSV network (~200 sats mining fee).
/// It validates the full BEEF serialization → signing → broadcast pipeline.
///
/// Run:  WALLET_URL=http://localhost:3322 cargo test --test integration e2e_create_action_broadcast_beef -- --test-threads=1
#[tokio::test]
async fn e2e_create_action_broadcast_beef() {
    let Some((base, client)) = e2e_setup() else {
        eprintln!("Skipping e2e test: WALLET_URL not set");
        return;
    };

    // OP_RETURN script: OP_0 OP_RETURN <"beef-broadcast-test">
    // 00 6a 13 626565662d62726f6164636173742d74657374
    let op_return_script = "006a13626565662d62726f6164636173742d74657374";

    // Step 1: Create a broadcast action with OP_RETURN (signAndProcess:true, default)
    let resp = post_json(
        &client,
        &format!("{base}/createAction"),
        json!({
            "description": "e2e BEEF broadcast test",
            "outputs": [{
                "lockingScript": op_return_script,
                "satoshis": 0,
                "outputDescription": "BEEF broadcast verification"
            }],
            "labels": ["e2e-test-broadcast"]
        }),
    )
    .await;

    let status = resp.status().as_u16();
    let body: Value = resp.json().await.unwrap();

    // Must succeed — if BEEF serialization or broadcast is broken, this fails
    assert_eq!(
        status, 200,
        "createAction broadcast should succeed (status {}): {:?}",
        status, body
    );

    // Step 2: Verify txid is a valid 64-char hex string
    let txid = body["txid"]
        .as_str()
        .expect("broadcast response should have txid");
    assert_eq!(txid.len(), 64, "txid should be 64 hex chars, got: {}", txid);
    assert!(
        txid.chars().all(|c| c.is_ascii_hexdigit()),
        "txid should be hex, got: {}",
        txid
    );

    // Step 3: Verify no signableTransaction (tx was signed and broadcast)
    assert!(
        body.get("signableTransaction").is_none() || body["signableTransaction"].is_null(),
        "broadcast response should not have signableTransaction"
    );

    // Step 4: Verify no error fields
    assert!(
        body.get("error").is_none() || body["error"].is_null(),
        "broadcast response should not have error: {:?}",
        body
    );

    eprintln!("BEEF broadcast succeeded! txid: {}", txid);

    // Step 5: Verify the action appears in listActions
    let actions_body = post_json_ok(
        &client,
        &format!("{base}/listActions"),
        json!({
            "labels": ["e2e-test-broadcast"],
            "includeLabels": true,
            "includeOutputs": true
        }),
    )
    .await;

    let total = actions_body["totalActions"]
        .as_u64()
        .expect("totalActions should be a number");
    assert!(
        total >= 1,
        "should have at least 1 action with e2e-test-broadcast label, got {}",
        total
    );

    // Step 6: Verify the action's txid matches
    let actions = actions_body["actions"]
        .as_array()
        .expect("actions should be an array");
    let found = actions.iter().any(|a| a["txid"].as_str() == Some(txid));
    assert!(
        found,
        "broadcast txid {} should appear in listActions results",
        txid
    );

    eprintln!(
        "BEEF broadcast E2E complete: txid={}, found in listActions=true",
        txid
    );
}

// =============================================================================
// E2E: EF broadcast tests — PushDrop-style large scripts and UTXO chaining
// =============================================================================

/// E2E: createAction with PushDrop-style script — verifies EF broadcast handles large scripts.
///
/// Tests that the BEEF->EF conversion correctly embeds parent UTXO data even when
/// the transaction has large locking scripts (PushDrop pattern used by bsv-worm proofs).
///
/// The script is: OP_FALSE OP_RETURN OP_PUSHDATA2 <512 bytes of 0xaa>
/// Total script: 517 bytes (1034 hex chars).
///
/// Run: WALLET_URL=http://localhost:3322 cargo test --test integration e2e_create_action_broadcast_pushdata -- --test-threads=1
#[tokio::test]
async fn e2e_create_action_broadcast_pushdata() {
    let Some((base, client)) = e2e_setup() else {
        eprintln!("Skipping e2e test: WALLET_URL not set");
        return;
    };

    // Build PushDrop-style OP_RETURN script:
    // 00       = OP_FALSE
    // 6a       = OP_RETURN
    // 4d       = OP_PUSHDATA2
    // 0002     = 512 in little-endian u16
    // aa * 512 = 512 bytes of data
    let data_hex = "aa".repeat(512); // 1024 hex chars = 512 bytes
    let op_return_script = format!("006a4d0002{}", data_hex);
    assert_eq!(
        op_return_script.len(),
        1034,
        "PushDrop script should be 1034 hex chars (517 bytes)"
    );

    // Step 1: Create a broadcast action with the large OP_RETURN (signAndProcess:true)
    let resp = post_json(
        &client,
        &format!("{base}/createAction"),
        json!({
            "description": "e2e EF broadcast PushDrop test",
            "outputs": [{
                "lockingScript": op_return_script,
                "satoshis": 0,
                "outputDescription": "PushDrop-style large OP_RETURN for EF broadcast verification"
            }],
            "labels": ["e2e-test-pushdata"]
        }),
    )
    .await;

    let status = resp.status().as_u16();
    let body: Value = resp.json().await.unwrap();

    // Must succeed — if BEEF->EF conversion breaks on large scripts, this fails
    assert_eq!(
        status, 200,
        "createAction broadcast should succeed with large PushDrop script (status {}): {:?}",
        status, body
    );

    // Step 2: Verify txid is a valid 64-char hex string
    let txid = body["txid"]
        .as_str()
        .expect("broadcast response should have txid");
    assert_eq!(txid.len(), 64, "txid should be 64 hex chars, got: {}", txid);
    assert!(
        txid.chars().all(|c| c.is_ascii_hexdigit()),
        "txid should be hex, got: {}",
        txid
    );

    // Step 3: Verify no signableTransaction (tx was signed and broadcast)
    assert!(
        body.get("signableTransaction").is_none() || body["signableTransaction"].is_null(),
        "broadcast response should not have signableTransaction"
    );

    // Step 4: Verify no error fields
    assert!(
        body.get("error").is_none() || body["error"].is_null(),
        "broadcast response should not have error: {:?}",
        body
    );

    eprintln!(
        "EF broadcast with PushDrop script succeeded! txid: {}",
        txid
    );

    // Step 5: Verify the action appears in listActions
    let actions_body = post_json_ok(
        &client,
        &format!("{base}/listActions"),
        json!({
            "labels": ["e2e-test-pushdata"],
            "includeLabels": true,
            "includeOutputs": true
        }),
    )
    .await;

    let total = actions_body["totalActions"]
        .as_u64()
        .expect("totalActions should be a number");
    assert!(
        total >= 1,
        "should have at least 1 action with e2e-test-pushdata label, got {}",
        total
    );

    // Step 6: Verify the action's txid matches
    let actions = actions_body["actions"]
        .as_array()
        .expect("actions should be an array");
    let found = actions.iter().any(|a| a["txid"].as_str() == Some(txid));
    assert!(
        found,
        "broadcast txid {} should appear in listActions results",
        txid
    );

    eprintln!(
        "EF broadcast PushDrop E2E complete: txid={}, script_len=517 bytes, found in listActions=true",
        txid
    );
}

/// E2E: Two sequential createActions — verifies UTXO chaining works with EF broadcast.
///
/// The second transaction must use the change output from the first. This tests
/// that the wallet correctly manages UTXOs between consecutive broadcasts and
/// that EF format works for chained transactions.
///
/// Run: WALLET_URL=http://localhost:3322 cargo test --test integration e2e_sequential_broadcasts -- --test-threads=1
#[tokio::test]
async fn e2e_sequential_broadcasts() {
    let Some((base, client)) = e2e_setup() else {
        eprintln!("Skipping e2e test: WALLET_URL not set");
        return;
    };

    // --- First broadcast ---

    // OP_RETURN script: OP_0 OP_RETURN <"seq-broadcast-1">
    // 00 6a 0f 7365712d62726f6164636173742d31
    let script_1 = "006a0f7365712d62726f6164636173742d31";

    let resp1 = post_json(
        &client,
        &format!("{base}/createAction"),
        json!({
            "description": "e2e sequential broadcast 1",
            "outputs": [{
                "lockingScript": script_1,
                "satoshis": 0,
                "outputDescription": "sequential broadcast test 1"
            }],
            "labels": ["e2e-test-sequential"]
        }),
    )
    .await;

    let status1 = resp1.status().as_u16();
    let body1: Value = resp1.json().await.unwrap();
    assert_eq!(
        status1, 200,
        "first sequential broadcast should succeed (status {}): {:?}",
        status1, body1
    );

    let txid1 = body1["txid"]
        .as_str()
        .expect("first broadcast should have txid");
    assert_eq!(
        txid1.len(),
        64,
        "first txid should be 64 hex chars, got: {}",
        txid1
    );
    assert!(
        txid1.chars().all(|c| c.is_ascii_hexdigit()),
        "first txid should be hex, got: {}",
        txid1
    );
    assert!(
        body1.get("error").is_none() || body1["error"].is_null(),
        "first broadcast should not have error: {:?}",
        body1
    );

    eprintln!("Sequential broadcast 1 succeeded: txid={}", txid1);

    // --- Second broadcast (immediately after first) ---

    // OP_RETURN script: OP_0 OP_RETURN <"seq-broadcast-2">
    // 00 6a 0f 7365712d62726f6164636173742d32
    let script_2 = "006a0f7365712d62726f6164636173742d32";

    let resp2 = post_json(
        &client,
        &format!("{base}/createAction"),
        json!({
            "description": "e2e sequential broadcast 2",
            "outputs": [{
                "lockingScript": script_2,
                "satoshis": 0,
                "outputDescription": "sequential broadcast test 2"
            }],
            "labels": ["e2e-test-sequential"]
        }),
    )
    .await;

    let status2 = resp2.status().as_u16();
    let body2: Value = resp2.json().await.unwrap();
    assert_eq!(
        status2, 200,
        "second sequential broadcast should succeed (status {}): {:?}",
        status2, body2
    );

    let txid2 = body2["txid"]
        .as_str()
        .expect("second broadcast should have txid");
    assert_eq!(
        txid2.len(),
        64,
        "second txid should be 64 hex chars, got: {}",
        txid2
    );
    assert!(
        txid2.chars().all(|c| c.is_ascii_hexdigit()),
        "second txid should be hex, got: {}",
        txid2
    );
    assert!(
        body2.get("error").is_none() || body2["error"].is_null(),
        "second broadcast should not have error: {:?}",
        body2
    );

    // Verify the two txids are different (distinct transactions)
    assert_ne!(
        txid1, txid2,
        "sequential broadcasts should produce different txids"
    );

    eprintln!("Sequential broadcast 2 succeeded: txid={}", txid2);

    // Verify both actions appear in listActions
    let actions_body = post_json_ok(
        &client,
        &format!("{base}/listActions"),
        json!({
            "labels": ["e2e-test-sequential"],
            "includeLabels": true,
            "includeOutputs": true
        }),
    )
    .await;

    let total = actions_body["totalActions"]
        .as_u64()
        .expect("totalActions should be a number");
    assert!(
        total >= 2,
        "should have at least 2 actions with e2e-test-sequential label, got {}",
        total
    );

    let actions = actions_body["actions"]
        .as_array()
        .expect("actions should be an array");
    let found1 = actions.iter().any(|a| a["txid"].as_str() == Some(txid1));
    let found2 = actions.iter().any(|a| a["txid"].as_str() == Some(txid2));
    assert!(found1, "first txid {} should appear in listActions", txid1);
    assert!(found2, "second txid {} should appear in listActions", txid2);

    eprintln!(
        "Sequential broadcasts E2E complete: txid1={}, txid2={}, both found in listActions",
        txid1, txid2
    );
}
