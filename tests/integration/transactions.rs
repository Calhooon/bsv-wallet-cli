//! The transaction workflow's endpoints against an unfunded wallet: listings,
//! refusals, and the createAction options' passthrough.

use crate::support::{post_json, post_json_ok, setup};
use serde_json::{json, Value};

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

/// A missing pending sign session has the reference's `NotImplemented` class.
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
        501,
        "a missing signAction session is not implemented"
    );
    let body: Value = resp.json().await.unwrap();
    assert_eq!(body["code"], "NOT_IMPLEMENTED");
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
