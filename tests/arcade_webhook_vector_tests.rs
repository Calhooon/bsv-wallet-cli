//! Arcade's webhook bodies replayed through the daemon's `POST /arc-callback`
//! route against the toolbox's vector (P0-2c, bsv-stack-lean #51).
//!
//! `tests/vectors/arcade_status_verdicts.json` is a byte-identical copy of
//! the toolbox's `tests/vectors/arcade_status_verdicts.json` at
//! bsv-wallet-toolbox-rs@550e553 (0.4.0). Every `push` case and the latch
//! sequence `latch_mined_unmined_mined_again` (bsv-stack-lean
//! `corpus/scenarios/2026-10-08-mined-orphaned-remined-broadcaster-latch.md`,
//! steps 2, 3b and 5) go through the HTTP route, and the verdict each must
//! give is the vector's `toolbox_push` column: the route hands the body to the
//! toolbox's one judgment, so the webhook and the SSE frame agree.
//!
//! All vectors are synthetic: coinbase-style BUMPs checked against an
//! in-test chain tracker. No network, no funded wallet.

use async_trait::async_trait;
use bsv_sdk::primitives::PrivateKey;
use bsv_sdk::transaction::{ChainTracker, ChainTrackerError, MerklePath};
use bsv_wallet_cli::server::{self, ServerConfig};
use bsv_wallet_toolbox::{
    Chain, Services, ServicesOptions, StorageSqlx, Wallet, WalletStorageWriter,
};
use reqwest::Client;
use serde_json::Value;
use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::{Arc, RwLock};
use tempfile::TempDir;

const VECTOR: &str = include_str!("vectors/arcade_status_verdicts.json");
const CB_TOKEN: &str = "0123456789abcdef0123456789abcdef";
const HEIGHT: u32 = 850_000;

/// The headers the wallet trusts, swappable mid-test so a reorg can be
/// replayed: `roots[height] = merkle root`.
#[derive(Default)]
struct Headers {
    tip: RwLock<u32>,
    roots: RwLock<HashMap<u32, String>>,
}

impl Headers {
    fn set(&self, txids_at: &[(&str, u32)], tip: u32) {
        let mut roots = self.roots.write().unwrap();
        roots.clear();
        for (txid, height) in txids_at {
            let root = MerklePath::from_coinbase_txid(txid, *height)
                .compute_root(Some(txid))
                .unwrap();
            roots.insert(*height, root);
        }
        *self.tip.write().unwrap() = tip;
    }
}

struct SharedTracker(Arc<Headers>);

#[async_trait]
impl ChainTracker for SharedTracker {
    async fn is_valid_root_for_height(
        &self,
        root: &str,
        height: u32,
    ) -> Result<bool, ChainTrackerError> {
        Ok(self.0.roots.read().unwrap().get(&height).map(String::as_str) == Some(root))
    }

    async fn current_height(&self) -> Result<u32, ChainTrackerError> {
        Ok(*self.0.tip.read().unwrap())
    }
}

/// A server with a callback token and the wallet's headers behind `headers`,
/// one transaction seeded at `(req, tx)` status.
async fn server_with(
    headers: Arc<Headers>,
    txid: &str,
    req: &str,
    tx: &str,
) -> (String, Client, sqlx::SqlitePool, TempDir) {
    let tmp = TempDir::new().expect("temp dir");
    let storage = StorageSqlx::open(tmp.path().join("test.db").to_str().unwrap())
        .await
        .expect("open db");
    let key = PrivateKey::random();
    let identity_key = key.public_key().to_hex();
    storage
        .migrate("bsv-wallet-test", &identity_key)
        .await
        .expect("migrate db");
    storage.make_available().await.expect("make available");
    // The proof LAG gate is closed on a fresh database; these tests pin the
    // webhook's verdicts, so the gate is open (the toolbox pins the deferral).
    bsv_wallet_toolbox::MonitorStorage::set_max_acceptable_proof_height(&storage, u32::MAX)
        .await
        .expect("open the proof gate");
    storage
        .set_chain_tracker(Arc::new(SharedTracker(headers)))
        .await;
    let pool = storage.pool().clone();

    let (user, _) = storage
        .find_or_insert_user(&identity_key)
        .await
        .expect("user");
    let now = chrono::Utc::now();
    sqlx::query(
        "INSERT INTO proven_tx_reqs (txid, status, attempts, history, notified, notify, raw_tx, created_at, updated_at) \
         VALUES (?, ?, 0, '{}', 0, '{}', X'01000000', ?, ?)",
    )
    .bind(txid)
    .bind(req)
    .bind(now)
    .bind(now)
    .execute(&pool)
    .await
    .expect("seed req");
    sqlx::query(
        "INSERT INTO transactions (user_id, txid, status, reference, description, satoshis, \
         version, lock_time, raw_tx, is_outgoing, created_at, updated_at) \
         VALUES (?, ?, ?, 'ref-arcade-vector', 'arcade webhook vector', -500, 1, 0, X'01000000', 1, ?, ?)",
    )
    .bind(user.user_id)
    .bind(txid)
    .bind(tx)
    .bind(now)
    .bind(now)
    .execute(&pool)
    .await
    .expect("seed tx");

    let services =
        Services::with_options(Chain::Main, ServicesOptions::mainnet()).expect("services");
    let wallet = Wallet::new(Some(key), storage, services)
        .await
        .expect("wallet");
    let config = ServerConfig {
        callback_token: Some(CB_TOKEN.to_string()),
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
    (format!("http://{addr}"), Client::new(), pool, tmp)
}

/// POST one body to the webhook route; the HTTP status and the answer.
async fn post(base: &str, client: &Client, body: &str) -> (u16, Value) {
    let resp = client
        .post(format!("{base}/arc-callback"))
        .header("Authorization", format!("Bearer {CB_TOKEN}"))
        .header("Content-Type", "application/json")
        .body(body.to_string())
        .send()
        .await
        .expect("POST /arc-callback");
    let code = resp.status().as_u16();
    (code, resp.json().await.unwrap_or(Value::Null))
}

async fn req_status(pool: &sqlx::SqlitePool, txid: &str) -> String {
    let (s,): (String,) = sqlx::query_as("SELECT status FROM proven_tx_reqs WHERE txid = ?")
        .bind(txid)
        .fetch_one(pool)
        .await
        .expect("req status");
    s
}

async fn proven_height(pool: &sqlx::SqlitePool, txid: &str) -> Option<i64> {
    sqlx::query_as::<_, (i64,)>("SELECT height FROM proven_txs WHERE txid = ?")
        .bind(txid)
        .fetch_optional(pool)
        .await
        .unwrap()
        .map(|(h,)| h)
}

fn coinbase_bump_hex(txid: &str, height: u32) -> String {
    MerklePath::from_coinbase_txid(txid, height).to_hex()
}

fn action_of(answer: &Value) -> &str {
    answer["action"].as_str().unwrap_or_default()
}

/// Every `push` case of the vector through the webhook route: the wallet's
/// records and the route's answer give the vector's `toolbox_push` verdict.
#[tokio::test]
async fn every_arcade_webhook_body_gets_the_toolbox_verdict() {
    let v: Value = serde_json::from_str(VECTOR).expect("the vector parses");
    let cases: Vec<Value> = v["cases"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|c| c["surface"] == "push")
        .cloned()
        .collect();
    assert!(!cases.is_empty(), "the vector carries push cases");
    let mut mismatches = Vec::new();
    for case in &cases {
        let name = case["name"].as_str().unwrap();
        let Some(expect) = case["expect"]["toolbox_push"].as_str() else {
            continue;
        };
        let txid = "ab".repeat(32);
        let headers = Arc::new(Headers::default());
        headers.set(&[(&txid, HEIGHT)], HEIGHT + 1);
        let (base, client, pool, _tmp) = server_with(headers, &txid, "sending", "sending").await;
        let body = serde_json::to_string(&case["body"])
            .unwrap()
            .replace("{TXID}", &txid)
            .replace("{BUMP}", &coinbase_bump_hex(&txid, HEIGHT))
            .replace("{BLOCKHASH}", &"bb".repeat(32))
            .replace("{COMPETITOR}", &"cc".repeat(32))
            .replace("{NOW}", &chrono::Utc::now().to_rfc3339());
        let (code, answer) = post(&base, &client, &body).await;
        if code != 200 {
            mismatches.push(format!("{name}: HTTP {code}, answer {answer}"));
            continue;
        }
        let reask = action_of(&answer) == "Reask";
        let got = match (req_status(&pool, &txid).await.as_str(), reask) {
            ("sending", false) => "none".to_string(),
            ("sending", true) => "reask".to_string(),
            ("unmined", false) => "seen".to_string(),
            ("unmined", true) => "seen+reask".to_string(),
            ("invalid", _) => "invalid".to_string(),
            ("doubleSpend", _) => "double_spend".to_string(),
            ("completed", _) => "proven".to_string(),
            (other, r) => format!("req={other} reask={r}"),
        };
        if got != expect {
            mismatches.push(format!(
                "{name}: expected {expect}, got {got} (action {})",
                action_of(&answer)
            ));
        }
    }
    assert!(
        mismatches.is_empty(),
        "{} of {} webhook bodies (POST /arc-callback):\n  {}",
        mismatches.len(),
        cases.len(),
        mismatches.join("\n  ")
    );
}

/// A word Arcade does not define is refused, and the answer says why.
#[tokio::test]
async fn an_undefined_word_is_refused_with_its_reason() {
    let txid = "ab".repeat(32);
    let headers = Arc::new(Headers::default());
    let (base, client, pool, _tmp) = server_with(headers, &txid, "unmined", "unproven").await;
    let body = format!(r#"{{"txid":"{txid}","txStatus":"NOT_A_STATUS"}}"#);
    let (code, answer) = post(&base, &client, &body).await;
    assert_eq!(code, 200, "{answer}");
    let action = action_of(&answer);
    assert!(
        action.starts_with("Refused") && action.contains("NOT_A_STATUS"),
        "an undefined word is refused with its reason: {action}"
    );
    assert_eq!(req_status(&pool, &txid).await, "unmined");
}

/// A path on a word that is not a mined word is not a proof. ARC's
/// `MINED_IN_STALE_BLOCK` carries the orphaned block's path (bsv-stack-lean
/// `corpus/scenarios/2026-10-08-mined-orphaned-remined-broadcaster-latch.md`,
/// the ARC column); a header service that has not seen the reorg yet still
/// holds that root, so only the word stands between the wallet and the
/// stale anchor.
#[tokio::test]
async fn a_path_on_a_word_that_is_not_mined_is_not_stored() {
    let txid = "ab".repeat(32);
    let headers = Arc::new(Headers::default());
    headers.set(&[(&txid, HEIGHT)], HEIGHT + 1);
    let (base, client, pool, _tmp) = server_with(headers, &txid, "unmined", "unproven").await;
    let body = serde_json::json!({
        "txid": txid,
        "txStatus": "MINED_IN_STALE_BLOCK",
        "blockHash": "a1".repeat(32),
        "blockHeight": HEIGHT,
        "merklePath": coinbase_bump_hex(&txid, HEIGHT),
    })
    .to_string();
    let (code, answer) = post(&base, &client, &body).await;
    assert_eq!(code, 200, "{answer}");
    assert!(
        action_of(&answer).starts_with("Refused"),
        "a stale word is refused, its path unread: {answer}"
    );
    assert_eq!(proven_height(&pool, &txid).await, None, "a stale anchor stored");
    assert_eq!(req_status(&pool, &txid).await, "unmined");
}

/// The scenario of record through the webhook route, the vector's
/// `latch_mined_unmined_mined_again`: MINED with A1's path is stored; A1 is
/// orphaned and `reorg_unmined` changes nothing and asks again; MINED
/// `reorg_reanchor` with B3's path, checked against the headers, replaces the
/// anchor. No latch.
#[tokio::test]
async fn webhook_mined_then_reorg_unmined_then_mined_again_re_anchors_and_never_latches() {
    let v: Value = serde_json::from_str(VECTOR).unwrap();
    let seq = &v["sequences"][0];
    assert_eq!(seq["name"], "latch_mined_unmined_mined_again");
    let steps = seq["steps"].as_array().unwrap().clone();
    assert_eq!(steps.len(), 3);
    let txid = "ab".repeat(32);
    let a1 = HEIGHT;
    let b3 = HEIGHT + 2;
    let fill = |step: &Value| {
        serde_json::to_string(&step["body"])
            .unwrap()
            .replace("{TXID}", &txid)
            .replace("{A1}", &"a1".repeat(32))
            .replace("{B3}", &"b3".repeat(32))
            .replace("{BUMP_A1}", &coinbase_bump_hex(&txid, a1))
            .replace("{BUMP_B3}", &coinbase_bump_hex(&txid, b3))
            .replace("{NOW}", &chrono::Utc::now().to_rfc3339())
    };

    // Step 2: A1 is active; MINED with A1's path is stored.
    let headers = Arc::new(Headers::default());
    headers.set(&[(&txid, a1)], a1 + 1);
    let (base, client, pool, _tmp) =
        server_with(headers.clone(), &txid, "unmined", "unproven").await;
    let (code, answer) = post(&base, &client, &fill(&steps[0])).await;
    assert_eq!(code, 200, "{answer}");
    assert_eq!(action_of(&answer), "ProofIngested", "{answer}");
    assert_eq!(req_status(&pool, &txid).await, "completed");
    assert_eq!(proven_height(&pool, &txid).await, Some(a1 as i64));

    // Step 3b: A1 is orphaned (the headers no longer carry its root); Arcade
    // reverts T with reorg_unmined. The word applies nothing and asks again.
    headers.set(&[], b3 + 1);
    let (code, answer) = post(&base, &client, &fill(&steps[1])).await;
    assert_eq!(code, 200, "{answer}");
    assert_eq!(
        action_of(&answer),
        "Reask",
        "reorg_unmined did not schedule a re-ask: {answer}"
    );
    assert_eq!(req_status(&pool, &txid).await, "completed");
    assert_eq!(proven_height(&pool, &txid).await, Some(a1 as i64));

    // Step 5: B3 carries T; MINED reorg_reanchor with B3's path goes through
    // the same door as a MINED and replaces the anchor.
    headers.set(&[(&txid, b3)], b3 + 1);
    let (code, answer) = post(&base, &client, &fill(&steps[2])).await;
    assert_eq!(code, 200, "{answer}");
    assert_eq!(action_of(&answer), "ProofIngested", "{answer}");
    assert_eq!(req_status(&pool, &txid).await, "completed");
    assert_eq!(
        proven_height(&pool, &txid).await,
        Some(b3 as i64),
        "the re-anchor did not replace the orphaned anchor: the latch"
    );
}
