//! The key linkage endpoints.

use crate::support::{post_json, post_json_ok, setup};
use serde_json::{json, Value};

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
