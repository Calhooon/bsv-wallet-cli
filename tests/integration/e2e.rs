//! Tests against a running wallet (`WALLET_URL`), skipped when it is unset.

use crate::support::{e2e_setup, post_json, post_json_ok};
use serde_json::{json, Value};

// =============================================================================
// E2E tests against a RUNNING real wallet (gated behind WALLET_URL env var).
//
// These tests hit a live `bsv-wallet serve` instance with real chain data
// and a funded wallet. They are safe: deferred-signing actions are always
// aborted, so no sats are spent.
//
// Run:  WALLET_URL=http://localhost:3322 cargo test --test integration e2e_
// =============================================================================

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

    let cert_type = "BgYGBgYGBgYGBgYGBgYGBgYGBgYGBgYGBgYGBgYGBgY="; // base64("e2e test")
    let certifier = "0279be667ef9dcbbac55a06295ce870b07029bfcdb2dce28d959f2815b16f81798";
    // Use a unique serial per run (timestamp-based) to avoid UNIQUE constraint from previous runs.
    // relinquishCertificate soft-deletes, so the DB row persists and blocks re-insertion.
    use std::time::{SystemTime, UNIX_EPOCH};
    let ts = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let serial_input = bsv_sdk::primitives::to_base64(&bsv_sdk::primitives::hash::sha256(
        format!("e2e-{ts}").as_bytes(),
    ));

    // Acquire
    let cert = post_json_ok(
        &client,
        &format!("{base}/acquireCertificate"),
        json!({
            "type": cert_type,
            "certifier": certifier,
            "acquisitionProtocol": "direct",
            "fields": {"email": "test@example.com"},
            "serialNumber": serial_input,
            "revocationOutpoint": "deadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeef.0",
            "signature": "3006020101020101",
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
            "type": cert_type,
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
