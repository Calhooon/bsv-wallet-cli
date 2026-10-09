//! The served wallet's two doors that carry a caller's BEEF
//! (`/internalizeAction`'s `tx`, `/createAction`'s `inputBEEF`): a valid BEEF
//! is never refused for its size, and a refusal names the invalid byte.
//!
//! All vectors are synthetic: a random throwaway key, a coinbase-style BUMP
//! validated against a MockChainTracker. No header service and no broadcaster
//! is asked: the proven transaction is never broadcast, and the default
//! services name no header service.

use bsv_sdk::primitives::PrivateKey;
use bsv_sdk::transaction::{Beef, MerklePath, MockChainTracker, Transaction};
use bsv_wallet_cli::server::{self, ServerConfig};
use bsv_wallet_toolbox::{
    Chain, Services, ServicesOptions, StorageSqlx, Wallet, WalletStorageWriter,
};
use reqwest::Client;
use serde_json::{json, Value};
use std::net::SocketAddr;
use std::sync::Arc;
use tempfile::TempDir;

const HEIGHT: u32 = 900_000;
const WALLET_BEARER: &str = "wallet-bearer-secret";
/// The served wallet's body cap through 0.6.0: 50 MiB.
const OLD_CAP: usize = 50 * 1024 * 1024;

/// A server over a fresh wallet whose storage checks roots against a mock
/// tracker that carries `root` at `HEIGHT`.
async fn setup(root: Option<String>, auth_token: Option<&str>) -> (String, TempDir) {
    let tmp = TempDir::new().expect("temp dir");
    let storage = StorageSqlx::open(tmp.path().join("test.db").to_str().unwrap())
        .await
        .expect("open db");
    let key = PrivateKey::random();
    storage
        .migrate("bsv-wallet-test", &key.public_key().to_hex())
        .await
        .expect("migrate db");
    storage.make_available().await.expect("make available");
    bsv_wallet_toolbox::MonitorStorage::set_max_acceptable_proof_height(&storage, u32::MAX)
        .await
        .expect("open the proof gate");
    let mut tracker = MockChainTracker::new(HEIGHT + 1);
    if let Some(root) = root {
        tracker.add_root(HEIGHT, root);
    }
    storage.set_chain_tracker(Arc::new(tracker)).await;

    let services =
        Services::with_options(Chain::Main, ServicesOptions::mainnet()).expect("services");
    let wallet = Wallet::new(Some(key), storage, services)
        .await
        .expect("wallet");
    let config = ServerConfig {
        auth_token: auth_token.map(str::to_string),
        ..Default::default()
    };
    let app = server::make_router(server::make_wallet_state(wallet), config);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind");
    let addr: SocketAddr = listener.local_addr().expect("local addr");
    tokio::spawn(async move {
        axum::serve(listener, app).await.ok();
    });
    (format!("http://{addr}"), tmp)
}

/// A transaction with one input, a P2PKH output, and an unspendable output
/// carrying `data` bytes of 0xff (each written "255," in the JSON body).
fn big_tx(data: usize) -> Vec<u8> {
    let mut tx = vec![1, 0, 0, 0, 1];
    tx.extend_from_slice(&[0x11; 32]);
    tx.extend_from_slice(&0u32.to_le_bytes());
    tx.push(0);
    tx.extend_from_slice(&0xffff_ffffu32.to_le_bytes());
    tx.push(2);
    tx.extend_from_slice(&1000u64.to_le_bytes());
    tx.push(25);
    tx.extend_from_slice(&[0x76, 0xa9, 0x14]);
    tx.extend_from_slice(&[0xdb; 20]);
    tx.extend_from_slice(&[0x88, 0xac]);
    tx.extend_from_slice(&0u64.to_le_bytes());
    let script_len = 3 + 4 + data;
    tx.push(0xfe);
    tx.extend_from_slice(&(script_len as u32).to_le_bytes());
    tx.extend_from_slice(&[0x00, 0x6a, 0x4e]);
    tx.extend_from_slice(&(data as u32).to_le_bytes());
    tx.extend(std::iter::repeat_n(0xffu8, data));
    tx.extend_from_slice(&[0, 0, 0, 0]);
    tx
}

/// The Atomic BEEF of `raw` under a one-transaction block's path, and its txid.
fn atomic_beef(raw: Vec<u8>) -> (Vec<u8>, String) {
    let txid = Transaction::from_binary(&raw).expect("a transaction").id();
    let mut beef = Beef::new();
    let bump = beef.merge_bump(MerklePath::from_coinbase_txid(&txid, HEIGHT));
    beef.merge_raw_tx(raw, Some(bump));
    (beef.to_binary_atomic(&txid).expect("atomic"), txid)
}

fn internalize_body(tx: &[u8]) -> Vec<u8> {
    serde_json::to_vec(&json!({
        "tx": tx,
        "outputs": [{
            "outputIndex": 0,
            "protocol": "basket insertion",
            "insertionRemittance": { "basket": "big" }
        }],
        "description": "a large valid BEEF"
    }))
    .unwrap()
}

async fn post(url: &str, body: Vec<u8>, bearer: Option<&str>) -> (u16, Value) {
    let mut req = Client::new()
        .post(url)
        .header("Origin", "http://test.local")
        .header("Content-Type", "application/json");
    if let Some(token) = bearer {
        req = req.header("Authorization", format!("Bearer {token}"));
    }
    let resp = req.body(body).send().await.expect("request");
    let status = resp.status().as_u16();
    let text = resp.text().await.unwrap_or_default();
    (
        status,
        serde_json::from_str(&text).unwrap_or(Value::String(text)),
    )
}

/// A valid Atomic BEEF of 14,000,000 bytes and more, a JSON body over the old
/// 50 MiB cap, is internalized, not refused for its size.
#[tokio::test]
async fn a_valid_beef_over_the_old_body_cap_is_internalized() {
    let (tx, txid) = atomic_beef(big_tx(14_000_000));
    let body = internalize_body(&tx);
    assert!(body.len() > OLD_CAP, "the body is {} bytes", body.len());
    let (base, _tmp) = setup(Some(txid.clone()), None).await;

    let (status, got) = post(&format!("{base}/internalizeAction"), body, None).await;

    assert_eq!(status, 200, "{got}");
    assert_eq!(got["accepted"], true, "{got}");
}

/// The same BEEF with one trailing byte is refused for that byte, at its
/// offset, not for its size.
#[tokio::test]
async fn an_invalid_beef_over_the_old_body_cap_is_refused_at_its_byte() {
    let (mut tx, txid) = atomic_beef(big_tx(14_000_000));
    let at = tx.len();
    tx.push(0);
    let body = internalize_body(&tx);
    assert!(body.len() > OLD_CAP);
    let (base, _tmp) = setup(Some(txid), None).await;

    let (status, got) = post(&format!("{base}/internalizeAction"), body, None).await;

    assert_eq!(status, 400, "{got}");
    assert_eq!(got["code"], "INVALID_BEEF", "{got}");
    let message = got["message"].as_str().unwrap_or_default();
    assert!(
        message.contains(&format!("Invalid BEEF at byte {at}"))
            && message.contains("TrailingBytes"),
        "{message}"
    );
}

/// `/createAction`'s `inputBEEF` takes a body over the old cap: what answers
/// is the empty wallet's funding (a fee of 2 satoshis, nothing to pay it
/// with), not a size.
#[tokio::test]
async fn create_action_takes_an_input_beef_over_the_old_body_cap() {
    let (tx, _) = atomic_beef(big_tx(14_000_000));
    let body = serde_json::to_vec(&json!({
        "description": "an input BEEF over the old cap",
        "inputBEEF": tx,
    }))
    .unwrap();
    assert!(body.len() > OLD_CAP);
    let (base, _tmp) = setup(None, None).await;

    let (status, got) = post(&format!("{base}/createAction"), body, None).await;

    assert_eq!(status, 402, "{got}");
    assert_eq!(got["code"], "INSUFFICIENT_FUNDS", "{got}");
}

/// A caller without the wallet's bearer token is refused before its body is
/// read: an uncapped door is open only to the wallet's own caller.
#[tokio::test]
async fn an_unauthenticated_body_is_refused_before_it_is_read() {
    let (tx, txid) = atomic_beef(big_tx(14_000_000));
    let body = internalize_body(&tx);
    assert!(body.len() > OLD_CAP);
    let (base, _tmp) = setup(Some(txid), Some(WALLET_BEARER)).await;

    let (status, got) = post(&format!("{base}/internalizeAction"), body.clone(), None).await;
    assert_eq!(status, 401, "{got}");

    let (status, got) = post(
        &format!("{base}/internalizeAction"),
        body,
        Some(WALLET_BEARER),
    )
    .await;
    assert_eq!(status, 200, "{got}");
}
