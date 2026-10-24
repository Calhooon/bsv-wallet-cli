//! The key, signature, HMAC and encryption endpoints, and their edge cases.

use crate::support::{post_json, post_json_ok, setup};
use serde_json::{json, Value};

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
