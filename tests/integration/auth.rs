//! The bearer token's enforcement.

use bsv_wallet_cli::server::ServerConfig;
use serde_json::Value;

use crate::support::setup_with_config;

// =============================================================================
// Auth token enforcement test
// =============================================================================

/// Spin up a server WITH auth token, verify 401 without token and 200 with token.
#[tokio::test]
async fn test_auth_token_enforcement() {
    let config = ServerConfig {
        auth_token: Some("test-secret-token".to_string()),
        tls: None,
        ..Default::default()
    };
    let (base, client, _tmp) =
        setup_with_config(std::env::var("CHAINTRACKS_URL").ok(), config).await;

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
