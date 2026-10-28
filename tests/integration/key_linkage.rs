//! The key linkage endpoints.

use crate::support::{post_json, post_json_ok, setup};
use serde_json::{json, Value};

// =============================================================================
// Batch 10: Key Linkage — revealCounterpartyKeyLinkage,
//           revealSpecificKeyLinkage
// =============================================================================

/// POST /revealCounterpartyKeyLinkage returns the flat beta.2 linkage response.
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
    assert_linkage(&body, test_key);
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
    assert_linkage(&body, test_key);
    assert_eq!(body["protocolID"], json!([2, "tests"]));
    assert_eq!(body["keyID"], "1");
    assert_eq!(body["proofType"], 0);
}

fn assert_linkage(body: &Value, test_key: &str) {
    assert!(body.get("linkage").is_none(), "linkage response is flat");
    for name in ["encryptedLinkage", "encryptedLinkageProof"] {
        let bytes = body[name].as_array().expect("linkage uses byte arrays");
        assert!(!bytes.is_empty());
        assert!(bytes.iter().all(|b| b.as_u64().is_some_and(|n| n <= 255)));
    }
    assert!(body["prover"].is_string());
    assert_eq!(body["verifier"], test_key);
    assert_eq!(body["counterparty"], test_key);
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
