//! The served wallet's two doors that carry a caller's BEEF
//! (`/internalizeAction`'s `tx`, `/createAction`'s `inputBEEF`): a valid BEEF
//! is never refused for its size, and a refusal names the invalid byte; and
//! the toolbox's payment check (#819) on a subject that sweeps more outputs
//! of one parent than bsv-rs's link budget takes, read unlinked.
//!
//! All vectors are synthetic: a random throwaway key, a coinbase-style BUMP
//! validated against a TestChainTracker. No header service and no broadcaster
//! is asked: the proven transaction is never broadcast, the default
//! services name no header service, and the one unproven subject, the
//! accepted sweep, is posted to a mock alone.

#[path = "../src/test_support/chain_tracker.rs"]
mod chain_tracker;
use chain_tracker::TestChainTracker;

use bsv_sdk::primitives::{PrivateKey, PublicKey};
use bsv_sdk::script::templates::P2PKH as Template;
use bsv_sdk::script::{LockingScript, SignOutputs, UnlockingScript};
use bsv_sdk::transaction::{Beef, MerklePath, Transaction, TransactionInput, TransactionOutput};
use bsv_sdk::wallet::{Counterparty, KeyDeriver, Protocol, SecurityLevel};
use bsv_wallet_cli::server::{self, ServerConfig};
use bsv_wallet_toolbox::services::mock::{
    success_post_beef_result, MockResponse, MockWalletServices,
};
use bsv_wallet_toolbox::{
    Chain, Services, ServicesOptions, StorageSqlx, Wallet, WalletServices, WalletStorageProvider,
    WalletStorageWriter,
};
use reqwest::Client;
use serde_json::{json, Value};
use std::io::ErrorKind;
use std::net::SocketAddr;
use std::sync::Arc;
use tempfile::TempDir;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

const HEIGHT: u32 = 900_000;
const WALLET_BEARER: &str = "wallet-bearer-secret";
/// The served wallet's body cap through 0.6.0: 50 MiB.
const OLD_CAP: usize = 50 * 1024 * 1024;

/// A server over a fresh wallet whose storage checks roots against a mock
/// tracker that carries `root` at `HEIGHT`.
async fn setup(root: Option<String>, auth_token: Option<&str>) -> (String, TempDir) {
    serve(PrivateKey::random(), root, HEIGHT + 1, auth_token, None).await
}

/// [`setup`] over the wallet of `key`, the tracker's tip at `tip`, and the
/// storage posting to `post_to` when given.
async fn serve(
    key: PrivateKey,
    root: Option<String>,
    tip: u32,
    auth_token: Option<&str>,
    post_to: Option<Arc<dyn WalletServices>>,
) -> (String, TempDir) {
    let tmp = TempDir::new().expect("temp dir");
    let storage = StorageSqlx::open(tmp.path().join("test.db").to_str().unwrap())
        .await
        .expect("open db");
    storage
        .migrate("bsv-wallet-test", &key.public_key().to_hex())
        .await
        .expect("migrate db");
    storage.make_available().await.expect("make available");
    bsv_wallet_toolbox::MonitorStorage::set_max_acceptable_proof_height(&storage, u32::MAX)
        .await
        .expect("open the proof gate");
    let mut tracker = TestChainTracker::new(tip);
    if let Some(root) = root {
        tracker.add_root(HEIGHT, root);
    }
    storage.set_chain_tracker(Arc::new(tracker)).await;

    let services =
        Services::with_options(Chain::Main, ServicesOptions::mainnet()).expect("services");
    let wallet = Wallet::new(Some(key), storage, services)
        .await
        .expect("wallet");
    if let Some(post_to) = post_to {
        WalletStorageProvider::set_services(wallet.storage(), post_to);
    }
    let config = ServerConfig {
        auth_token: auth_token.map(str::to_string),
        ..ServerConfig::admitting(["test.local"]).expect("an originator")
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
    beef.merge_raw_tx(raw, Some(bump)).unwrap();
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

/// `post` for a door that refuses before it reads the body: the server
/// answers and closes while the body is still being written, so the write
/// meets the closed peer (`EPIPE` or `ECONNRESET`), and a client that reads
/// only after its write (`reqwest`'s, by timing) can lose the refusal it was
/// sent (#545). Here the body is written on a spawned task while the response
/// is read; the write must end in the closed peer, never whole.
async fn post_refused_unread(url: &str, body: Vec<u8>) -> (u16, Value) {
    let rest = url.strip_prefix("http://").expect("an http url");
    let (host, path) = rest.split_at(rest.find('/').expect("a path"));
    let stream = TcpStream::connect(host).await.expect("connect");
    let (mut rd, mut wr) = stream.into_split();
    let head = format!(
        "POST {path} HTTP/1.1\r\nHost: {host}\r\nOrigin: http://test.local\r\n\
         Content-Type: application/json\r\nContent-Length: {}\r\n\r\n",
        body.len()
    );
    let writer = tokio::spawn(async move {
        wr.write_all(head.as_bytes()).await?;
        wr.write_all(&body).await
    });

    let mut got = Vec::new();
    let mut buf = [0u8; 4096];
    let (status, at, len) = loop {
        if let Some(at) = got.windows(4).position(|w| w == b"\r\n\r\n") {
            let head = String::from_utf8_lossy(&got[..at]).to_ascii_lowercase();
            let status: u16 = head[9..12].parse().expect("a status");
            let len: usize = head
                .lines()
                .find_map(|l| l.strip_prefix("content-length:"))
                .expect("a content length")
                .trim()
                .parse()
                .expect("a length");
            if got.len() >= at + 4 + len {
                break (status, at + 4, len);
            }
        }
        match rd.read(&mut buf).await {
            Ok(0) => panic!("closed before a whole response: {got:?}"),
            Ok(n) => got.extend_from_slice(&buf[..n]),
            Err(e) => panic!("read: {e}, after {got:?}"),
        }
    };

    let wrote = writer.await.expect("the writer");
    assert!(
        matches!(
            wrote.as_ref().map_err(|e| e.kind()),
            Err(ErrorKind::BrokenPipe | ErrorKind::ConnectionReset)
        ),
        "the body's write ended {wrote:?}, not in the closed peer"
    );
    let text = String::from_utf8_lossy(&got[at..at + len]).into_owned();
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
    assert_eq!(got["code"], "VALIDATION_ERROR", "{got}");
    let message = got["message"].as_str().unwrap_or_default();
    assert!(
        message.contains(&format!("Invalid BEEF at byte {at}"))
            && message.contains("TrailingBytes"),
        "{message}"
    );
}

/// `/createAction`'s `inputBEEF` takes a body over the old cap: what answers
/// is the empty wallet's funding (a fee of 2 satoshis, nothing to pay it
/// with), not a size. The tracker carries the BEEF's root: from toolbox
/// 0.11.0 the caller's roots are read before any allocation (#44).
#[tokio::test]
async fn create_action_takes_an_input_beef_over_the_old_body_cap() {
    let (tx, txid) = atomic_beef(big_tx(14_000_000));
    let body = serde_json::to_vec(&json!({
        "description": "an input BEEF over the old cap",
        "inputBEEF": tx,
    }))
    .unwrap();
    assert!(body.len() > OLD_CAP);
    let (base, _tmp) = setup(Some(txid), None).await;

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

    let (status, got) =
        post_refused_unread(&format!("{base}/internalizeAction"), body.clone()).await;
    assert_eq!(status, 401, "{got}");
    assert_eq!(got["code"], "UNAUTHORIZED", "{got}");

    let (status, got) = post(
        &format!("{base}/internalizeAction"),
        body,
        Some(WALLET_BEARER),
    )
    .await;
    assert_eq!(status, 200, "{got}");
}

/// More outputs of one parent than bsv-rs's link budget takes (bsv-rs
/// measures the default's reach at 702 P2PKH outputs of one parent).
const SWEPT: u32 = 800;
/// The sweep's payment remittance: its derivation prefix and suffix.
const REMITTANCE: (&str, &str) = ("c3dlZXA=", "cGF5");
/// A 25-byte P2PKH paying no party of the test.
const OTHER: &str = "76a914dbc0a7c84983c5bf199b7b2d41b3acf0408ee5aa88ac";

fn p2pkh(key: &PublicKey) -> Vec<u8> {
    [&[0x76, 0xa9, 0x14][..], &key.hash160(), &[0x88, 0xac][..]].concat()
}

/// A proven parent of [`SWEPT`] outputs locked to `spender`, and a child
/// that spends every one of them, signed, into one output locked by
/// `script`: the `AtomicBEEF`, the child's txid and the parent's.
async fn sweep(spender: &PrivateKey, script: &[u8]) -> (Vec<u8>, String, String) {
    let mut parent = Transaction::new();
    let mut funding = TransactionInput::new("11".repeat(32), 0);
    funding.unlocking_script = Some(UnlockingScript::from_hex("00").unwrap());
    parent.add_input(funding).unwrap();
    let lock = LockingScript::from_binary(&p2pkh(&spender.public_key())).unwrap();
    for _ in 0..SWEPT {
        let output = TransactionOutput::new(1_000, lock.clone());
        parent.add_output(output).unwrap();
    }
    let parent_txid = parent.id();
    parent.merkle_path = Some(MerklePath::from_coinbase_txid(&parent_txid, HEIGHT));
    let mut child = Transaction::new();
    for vout in 0..SWEPT {
        let mut input = TransactionInput::with_source_transaction(parent.clone(), vout);
        input.set_unlocking_script_template(Template::unlock(spender, SignOutputs::All, false));
        child.add_input(input).unwrap();
    }
    let swept = u64::from(SWEPT) * 1_000 - 500;
    let output = TransactionOutput::new(swept, LockingScript::from_binary(script).unwrap());
    child.add_output(output).unwrap();
    child.sign().await.unwrap();
    let txid = child.id();
    let mut beef = Beef::new();
    beef.merge_bump(parent.merkle_path.clone().unwrap());
    beef.merge_transaction(parent);
    beef.merge_transaction(child);
    let atomic = beef.to_binary_atomic(&txid).unwrap();
    assert!(
        matches!(
            Transaction::from_beef(&atomic, Some(&txid)),
            Err(bsv_sdk::Error::BeefResourceLimit(_))
        ),
        "the sweep is past the linking door's budget"
    );
    (atomic, txid, parent_txid)
}

/// What the served wallet answered for the sweep paying the derived key
/// (`to_derived`) or another key, as a wallet payment from a random sender:
/// the status, the body, the actions it then lists, the mock's posts, and
/// the derived key.
async fn internalize_sweep(to_derived: bool) -> (u16, Value, Value, usize, PublicKey) {
    let (root, sender) = (PrivateKey::random(), PrivateKey::random());
    let (prefix, suffix) = REMITTANCE;
    let derived = KeyDeriver::new(Some(root.clone()))
        .derive_public_key(
            &Protocol::new(SecurityLevel::Counterparty, "3241645161d8"),
            &format!("{prefix} {suffix}"),
            &Counterparty::Other(sender.public_key()),
            true,
        )
        .expect("the derived key");
    let script = if to_derived {
        p2pkh(&derived)
    } else {
        hex::decode(OTHER).unwrap()
    };
    let (tx, txid, parent) = sweep(&sender, &script).await;
    let mock = Arc::new(
        MockWalletServices::builder()
            .post_beef_response(MockResponse::Success(vec![success_post_beef_result(
                "mock",
                &[txid.as_str()],
            )]))
            .build(),
    );
    // The tracker's tip 100 above the parent's proof (bsv-rs #734).
    let posts_to = mock.clone() as Arc<dyn WalletServices>;
    let (base, _tmp) = serve(root, Some(parent), HEIGHT + 100, None, Some(posts_to)).await;
    let body = serde_json::to_vec(&json!({"tx": tx, "description": "a sweep to check",
        "outputs": [{"outputIndex": 0, "protocol": "wallet payment",
            "paymentRemittance": {"derivationPrefix": prefix, "derivationSuffix": suffix,
                "senderIdentityKey": sender.public_key().to_hex()}}]}))
    .unwrap();
    let (status, got) = post(&format!("{base}/internalizeAction"), body, None).await;
    let list = serde_json::to_vec(&json!({"labels": []})).unwrap();
    let (_, listed) = post(&format!("{base}/listActions"), list, None).await;
    (status, got, listed, mock.call_count("post_beef"), derived)
}

/// A payment output in the sweep that pays the P2PKH of another key: 400
/// `VALIDATION_ERROR` (`WalletRefusal` `InvalidParameter`), the derivation
/// mismatch naming both hashes; no action written, nothing posted.
#[tokio::test]
async fn a_payment_to_another_key_in_a_sweep_past_the_link_budget_is_refused() {
    let (status, got, listed, posts, derived) = internalize_sweep(false).await;
    assert_eq!(status, 400, "{got}");
    let message = format!(
        "Derivation mismatch for output 0: derived {} != script {}",
        hex::encode(derived.hash160()),
        &OTHER[6..46]
    );
    assert_eq!(got, json!({"code": "VALIDATION_ERROR", "message": message}));
    assert_eq!(listed["totalActions"], 0, "nothing is stored: {listed}");
    assert_eq!(posts, 0, "nothing is posted");
}

/// The control: the same sweep, its one output locked by the derived key's
/// P2PKH, is admitted and stored, and posted to the mock alone.
#[tokio::test]
async fn a_payment_to_the_derived_key_in_a_sweep_past_the_link_budget_is_stored() {
    let (status, got, listed, posts, _) = internalize_sweep(true).await;
    assert_eq!(status, 200, "{got}");
    assert_eq!(got["accepted"], true, "{got}");
    assert_eq!(listed["totalActions"], 1, "the payment is stored: {listed}");
    assert_eq!(posts, 1, "the unproven subject is posted to the mock alone");
}
