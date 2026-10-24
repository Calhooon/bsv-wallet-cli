//! The bearer token's enforcement.

use bsv_sdk::primitives::PrivateKey;
use bsv_wallet_cli::server::{self, ServerConfig};
use bsv_wallet_toolbox::{
    Chain, Services, ServicesOptions, StorageSqlx, Wallet, WalletStorageWriter,
};
use reqwest::Client;
use serde_json::Value;
use std::net::SocketAddr;
use tempfile::TempDir;

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
