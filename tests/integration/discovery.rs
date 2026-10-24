//! The discovery endpoints.

use crate::support::{post_json_ok, setup};
use serde_json::json;

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
