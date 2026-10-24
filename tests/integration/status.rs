//! The status and header endpoints, against a fresh wallet.

use crate::support::{local_header_service, post_json, setup, setup_with_header_service};
use serde_json::{json, Value};

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

/// The tip height is the header service's tip header's height (Rule 28,
/// the toolbox's T1 and T2): a local header service answers it here, and no
/// explorer is asked.
#[tokio::test]
async fn test_get_height() {
    let header_service = local_header_service(900_001).await;
    let (base, client, _tmp) = setup_with_header_service(Some(header_service)).await;
    let resp = client
        .get(format!("{base}/getHeight"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body: Value = resp.json().await.unwrap();
    assert_eq!(body["height"], 900_001, "the header service's tip");
}

/// With no header service the height is an error, never an explorer's
/// number (the toolbox 0.5.0).
#[tokio::test]
async fn test_get_height_without_a_header_service_is_an_error() {
    let (base, client, _tmp) = setup_with_header_service(None).await;
    let resp = client
        .get(format!("{base}/getHeight"))
        .send()
        .await
        .unwrap();
    assert_ne!(resp.status(), 200, "no header service: no height");
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
