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

const ORIGIN: &str = "http://browser.test";

/// A wallet served under `AUTH_TOKEN`.
async fn bearer_served() -> (String, reqwest::Client, tempfile::TempDir) {
    let config = ServerConfig {
        auth_token: Some("test-secret-token".to_string()),
        ..Default::default()
    };
    setup_with_config(None, config).await
}

/// A response's `Access-Control-Allow-Origin`.
fn allowed_origin(resp: &reqwest::Response) -> Option<&str> {
    let value = resp.headers().get("access-control-allow-origin");
    value.map(|v| v.to_str().unwrap())
}

/// A browser under `AUTH_TOKEN` (#138). A CORS preflight carries no credentials
/// (the Fetch standard's CORS-preflight request omits them), so the CORS layer
/// answers it before the bearer is asked, as without a token.
#[tokio::test]
async fn test_auth_token_preflight_passes() {
    let (base, client, _tmp) = bearer_served().await;
    let resp = client
        .request(reqwest::Method::OPTIONS, format!("{base}/createAction"))
        .header("Origin", ORIGIN)
        .header("Access-Control-Request-Method", "POST")
        .header(
            "Access-Control-Request-Headers",
            "content-type, authorization",
        )
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200, "the preflight passes the bearer");
    assert_eq!(allowed_origin(&resp), Some(ORIGIN));
    let h = resp.headers();
    assert_eq!(h["access-control-allow-methods"], "POST");
    assert_eq!(
        h["access-control-allow-headers"],
        "content-type, authorization"
    );
    assert_eq!(h["access-control-allow-credentials"], "true");
    assert_eq!(resp.text().await.unwrap(), "");

    // A bare OPTIONS (no Access-Control-Request-Method) gets the same CORS
    // answer: the CORS layer answers every OPTIONS, and none reaches a route.
    let url = format!("{base}/createAction");
    let resp = client
        .request(reqwest::Method::OPTIONS, url)
        .send()
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        200,
        "a bare OPTIONS is CORS's, as without a token"
    );
    assert_eq!(resp.text().await.unwrap(), "");
}

/// The actual request under `AUTH_TOKEN` (#138): refused without the bearer
/// with the CORS headers, so a browser's script reads the 401; answered with it.
#[tokio::test]
async fn test_auth_token_refusal_reads_in_a_browser() {
    let (base, client, _tmp) = bearer_served().await;
    for (bearer, status) in [(None, 401), (Some("Bearer test-secret-token"), 200)] {
        let mut req = client
            .post(format!("{base}/getPublicKey"))
            .header("Origin", ORIGIN);
        if let Some(b) = bearer {
            req = req.header("Authorization", b);
        }
        let resp = req
            .json(&serde_json::json!({"identityKey": true}))
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status(), status, "bearer {bearer:?}");
        assert_eq!(allowed_origin(&resp), Some(ORIGIN));
        let body: Value = resp.json().await.unwrap();
        match bearer {
            None => assert_eq!(body["code"], "UNAUTHORIZED"),
            Some(_) => assert_eq!(body["publicKey"].as_str().map(str::len), Some(66)),
        }
    }
}
