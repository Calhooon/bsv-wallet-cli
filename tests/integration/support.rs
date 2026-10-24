//! The worlds the tests run against: a fresh wallet served on a local port,
//! a local header service, a running wallet, and the request helpers.

use bsv_sdk::primitives::PrivateKey;
use bsv_wallet_cli::server::{self, ServerConfig};
use bsv_wallet_toolbox::{
    Chain, Services, ServicesOptions, StorageSqlx, Wallet, WalletStorageWriter,
};
use reqwest::Client;
use serde_json::{json, Value};
use std::net::SocketAddr;
use tempfile::TempDir;

/// Spin up a server on a random port, return the base URL and a reqwest client.
pub(crate) async fn setup() -> (String, Client, TempDir) {
    setup_with_header_service(std::env::var("CHAINTRACKS_URL").ok()).await
}

/// A local header service that answers the tip header at `height` and
/// nothing else (the toolbox's `findChainTipHeaderHex` frame).
pub(crate) async fn local_header_service(height: u32) -> String {
    let tip = json!({
        "status": "success",
        "value": {
            "version": 536870912u32,
            "previousHash": "0".repeat(64),
            "merkleRoot": "a".repeat(64),
            "time": 1700000000u32,
            "bits": 402917821u32,
            "nonce": 7u32,
            "height": height,
            "hash": "b".repeat(64),
        }
    });
    let app = axum::Router::new().route(
        "/findChainTipHeaderHex",
        axum::routing::get(move || {
            let tip = tip.clone();
            async move { axum::Json(tip) }
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind");
    let addr: SocketAddr = listener.local_addr().expect("local addr");
    tokio::spawn(async move {
        axum::serve(listener, app).await.ok();
    });
    format!("http://{}", addr)
}

/// `setup` over an explicit header service (or none).
pub(crate) async fn setup_with_header_service(
    chaintracks_url: Option<String>,
) -> (String, Client, TempDir) {
    let tmp = TempDir::new().expect("temp dir");
    let db_path = tmp.path().join("test.db");

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
        if let Some(url) = chaintracks_url {
            opts = opts.with_chaintracks_url(url);
        }
        Services::with_options(Chain::Main, opts).expect("services")
    };
    let wallet = Wallet::new(Some(key), storage, services)
        .await
        .expect("wallet");

    let state = server::make_wallet_state(wallet);
    let app = server::make_router(state, ServerConfig::default());

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind");
    let addr: SocketAddr = listener.local_addr().expect("local addr");
    tokio::spawn(async move {
        axum::serve(listener, app).await.ok();
    });

    let base = format!("http://{}", addr);
    let client = Client::new();
    (base, client, tmp)
}

/// Helper: POST JSON with Origin header
pub(crate) async fn post_json(client: &Client, url: &str, body: Value) -> reqwest::Response {
    client
        .post(url)
        .header("Origin", "http://test.local")
        .json(&body)
        .send()
        .await
        .expect("request failed")
}

/// Helper: POST and assert 200, printing error body on failure
pub(crate) async fn post_json_ok(client: &Client, url: &str, body: Value) -> Value {
    let resp = post_json(client, url, body).await;
    let status = resp.status();
    let body: Value = resp.json().await.unwrap();
    assert_eq!(status, 200, "endpoint returned {}: {:?}", status, body);
    body
}

/// Helper: connect to a running wallet server. Returns None if WALLET_URL not set.
pub(crate) fn e2e_setup() -> Option<(String, Client)> {
    let url = std::env::var("WALLET_URL").ok()?;
    Some((url, Client::new()))
}
