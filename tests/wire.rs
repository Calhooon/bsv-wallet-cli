//! The wire oracle (#69): every served call and every refusal body, as a
//! recorded request and the exact response, under `tests/wire/`.
//!
//! `cargo test --test wire` replays every fixture against the router,
//! in-process (`tower::ServiceExt::oneshot`, no socket for the wallet), and
//! fails on any byte of difference after normalization, printing the diff.
//! `WIRE_RECORD=1 cargo test --test wire` re-records every fixture from the
//! cases below; nothing else re-records. See `docs/WIRE.md`.
//!
//! Each fixture names its world (the wallet and the server config it is
//! served from). A world is built once per run from its name alone, and the
//! fixtures of one world run in their file order, so a fixture may read the
//! state an earlier one of its world left (a certificate acquired, then
//! listed). Nothing reaches the network: the header service is a local
//! stub, the broadcaster a closed local port, and every other request goes
//! through a proxy on a closed local port.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::{Path, PathBuf};

use axum::body::Body;
use axum::http::Request;
use axum::Router;
use bsv_sdk::primitives::PrivateKey;
use bsv_wallet_cli::server::{self, ServerConfig};
use bsv_wallet_toolbox::{
    Chain, Services, ServicesOptions, StorageSqlx, Wallet, WalletStorageWriter,
};
use serde::de::{MapAccess, SeqAccess, Visitor};
use serde::ser::{SerializeMap, SerializeSeq};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use tower::ServiceExt;

// ---------------------------------------------------------------------------
// JSON that keeps its key order (serde_json's `Value` sorts keys here, and a
// key moved is a byte changed).
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq)]
enum J {
    Null,
    Bool(bool),
    Num(serde_json::Number),
    Str(String),
    Arr(Vec<J>),
    Obj(Vec<(String, J)>),
}

impl J {
    fn get(&self, key: &str) -> Option<&J> {
        match self {
            J::Obj(fields) => fields.iter().find(|(k, _)| k == key).map(|(_, v)| v),
            _ => None,
        }
    }

    fn as_str(&self) -> Option<&str> {
        match self {
            J::Str(s) => Some(s),
            _ => None,
        }
    }

    fn compact(&self) -> String {
        serde_json::to_string(self).expect("serialize")
    }

    fn parse(bytes: &[u8]) -> Option<J> {
        serde_json::from_slice(bytes).ok()
    }
}

impl Serialize for J {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        match self {
            J::Null => s.serialize_unit(),
            J::Bool(b) => s.serialize_bool(*b),
            J::Num(n) => n.serialize(s),
            J::Str(v) => s.serialize_str(v),
            J::Arr(items) => {
                let mut seq = s.serialize_seq(Some(items.len()))?;
                for item in items {
                    seq.serialize_element(item)?;
                }
                seq.end()
            }
            J::Obj(fields) => {
                let mut map = s.serialize_map(Some(fields.len()))?;
                for (k, v) in fields {
                    map.serialize_entry(k, v)?;
                }
                map.end()
            }
        }
    }
}

impl<'de> Deserialize<'de> for J {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<J, D::Error> {
        struct V;
        impl<'de> Visitor<'de> for V {
            type Value = J;
            fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
                f.write_str("JSON")
            }
            fn visit_unit<E>(self) -> Result<J, E> {
                Ok(J::Null)
            }
            fn visit_none<E>(self) -> Result<J, E> {
                Ok(J::Null)
            }
            fn visit_bool<E>(self, v: bool) -> Result<J, E> {
                Ok(J::Bool(v))
            }
            fn visit_u64<E>(self, v: u64) -> Result<J, E> {
                Ok(J::Num(v.into()))
            }
            fn visit_i64<E>(self, v: i64) -> Result<J, E> {
                Ok(J::Num(v.into()))
            }
            fn visit_f64<E: serde::de::Error>(self, v: f64) -> Result<J, E> {
                serde_json::Number::from_f64(v)
                    .map(J::Num)
                    .ok_or_else(|| E::custom("non-finite number"))
            }
            fn visit_str<E>(self, v: &str) -> Result<J, E> {
                Ok(J::Str(v.to_string()))
            }
            fn visit_string<E>(self, v: String) -> Result<J, E> {
                Ok(J::Str(v))
            }
            fn visit_seq<A: SeqAccess<'de>>(self, mut a: A) -> Result<J, A::Error> {
                let mut items = Vec::new();
                while let Some(item) = a.next_element()? {
                    items.push(item);
                }
                Ok(J::Arr(items))
            }
            fn visit_map<A: MapAccess<'de>>(self, mut a: A) -> Result<J, A::Error> {
                let mut fields = Vec::new();
                while let Some((k, v)) = a.next_entry::<String, J>()? {
                    fields.push((k, v));
                }
                Ok(J::Obj(fields))
            }
        }
        d.deserialize_any(V)
    }
}

/// `serde_json::json!` into [`J`], its keys in the order written (the
/// macro's map sorts them; a request is sent as recorded either way).
fn j(v: serde_json::Value) -> J {
    serde_json::from_value(v).expect("json")
}

// ---------------------------------------------------------------------------
// The fixture.
// ---------------------------------------------------------------------------

#[derive(Serialize, Deserialize, Clone)]
struct Fixture {
    /// The world the request is served from (see [`World::build`]).
    world: String,
    request: Req,
    response: Resp,
}

#[derive(Serialize, Deserialize, Clone)]
struct Req {
    method: String,
    path: String,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    headers: BTreeMap<String, String>,
    /// A JSON body, sent as its compact bytes with `content-type:
    /// application/json`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    json: Option<J>,
    /// A body sent byte for byte as written (a malformed one).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    raw: Option<String>,
}

#[derive(Serialize, Deserialize, Clone, PartialEq)]
struct Resp {
    status: u16,
    headers: BTreeMap<String, String>,
    /// A JSON body, normalized.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    json: Option<J>,
    /// A body that is not JSON, byte for byte, normalized.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    text: Option<String>,
}

/// The response headers that are part of the contract. Every other header
/// is the transport's.
const KEPT_HEADERS: [&str; 6] = [
    "content-type",
    "retry-after",
    "access-control-allow-origin",
    "access-control-allow-credentials",
    "access-control-expose-headers",
    "vary",
];

// ---------------------------------------------------------------------------
// The normalizer: the one place a value that must differ from run to run is
// replaced by a stable placeholder. Every rule is named in docs/WIRE.md.
// ---------------------------------------------------------------------------

/// Keys whose values differ on every run, anywhere in a body.
const VOLATILE_KEYS: [(&str, &str); 5] = [
    // AES-GCM under a fresh random IV: a new ciphertext each call.
    ("ciphertext", "<ciphertext>"),
    // A key linkage revealed is encrypted for the verifier the same way.
    ("encryptedLinkage", "<ciphertext>"),
    ("encryptedLinkageProof", "<ciphertext>"),
    // A held coin's age, from its row's fixed time to the clock's now.
    ("ageMinutes", "<minutes>"),
    // The clock's now, when a linkage is revealed.
    ("revelationTime", "<timestamp>"),
];

fn normalize(v: J) -> J {
    match v {
        J::Obj(fields) => J::Obj(
            fields
                .into_iter()
                .map(|(k, v)| {
                    let v = match VOLATILE_KEYS.iter().find(|(key, _)| *key == k) {
                        Some((_, placeholder)) if !matches!(v, J::Null) => {
                            J::Str(placeholder.to_string())
                        }
                        _ => normalize(v),
                    };
                    (k, v)
                })
                .collect(),
        ),
        J::Arr(items) => J::Arr(items.into_iter().map(normalize).collect()),
        J::Str(s) => J::Str(normalize_text(&s)),
        other => other,
    }
}

/// Inside any string: the local stubs' ports (a fresh port each run), and
/// a held coin's age in a refusal's listing (`<n> min old`).
fn normalize_text(s: &str) -> String {
    replace_number_before(
        &replace_number_after(s, "127.0.0.1:", "<port>"),
        " min old",
        "<minutes>",
    )
}

/// Every run of digits right after `marker`, replaced by `placeholder`.
fn replace_number_after(s: &str, marker: &str, placeholder: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(at) = rest.find(marker) {
        out.push_str(&rest[..at + marker.len()]);
        rest = &rest[at + marker.len()..];
        let digits = rest.bytes().take_while(u8::is_ascii_digit).count();
        if digits > 0 {
            out.push_str(placeholder);
        }
        rest = &rest[digits..];
    }
    out.push_str(rest);
    out
}

/// Every run of digits right before `marker`, replaced by `placeholder`.
fn replace_number_before(s: &str, marker: &str, placeholder: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(at) = rest.find(marker) {
        let head = &rest[..at];
        let digits = head.bytes().rev().take_while(u8::is_ascii_digit).count();
        out.push_str(&head[..head.len() - digits]);
        if digits > 0 {
            out.push_str(placeholder);
        }
        out.push_str(marker);
        rest = &rest[at + marker.len()..];
    }
    out.push_str(rest);
    out
}

// ---------------------------------------------------------------------------
// The worlds.
// ---------------------------------------------------------------------------

/// The wallet's root key: fixed, so its identity key, every derived key and
/// every (RFC 6979) signature is the same on every run.
const ROOT_KEY: &str = "1f1f1f1f1f1f1f1f1f1f1f1f1f1f1f1f1f1f1f1f1f1f1f1f1f1f1f1f1f1f1f1f";
/// The bearer of the `auth` world, and the callback token of `callback`.
const WIRE_BEARER: &str = "wire-bearer";
const WIRE_CALLBACK: &str = "wire-callback";

/// The header stub's tip, and the height and root of the seeded proofs.
const TIP: u32 = 900_010;
const MINED_AT: u32 = 900_000;
const ROOT: &str = "5555555555555555555555555555555555555555555555555555555555555555";

/// The seeded transactions (hand-written rows, never broadcast).
const MINED: &str = "1111111111111111111111111111111111111111111111111111111111111111";
const PENDING: &str = "2222222222222222222222222222222222222222222222222222222222222222";
const UNKNOWN: &str = "3333333333333333333333333333333333333333333333333333333333333333";
const FROZEN: &str = "4444444444444444444444444444444444444444444444444444444444444444";
const RECEIVED: &str = "6666666666666666666666666666666666666666666666666666666666666666";
const P2PKH: &str = "76a914dbc0a7c84983c5bf199b7b2d41b3acf0408ee5aa88ac";

struct World {
    router: Router,
    /// Held for the world's life: the stubs' tasks, a temp store, the
    /// other connection holding the write lock.
    _keep: Vec<Box<dyn std::any::Any + Send>>,
}

/// A local header service: the tip at [`TIP`], any height's header with
/// [`ROOT`]; or, `down`, a 500 for everything.
async fn header_stub(down: bool) -> String {
    use axum::extract::Query;
    use axum::routing::get;
    fn header(height: u32) -> serde_json::Value {
        serde_json::json!({
            "status": "success",
            "value": {
                "version": 536870912u32,
                "previousHash": "0".repeat(64),
                // The received coin's proof is a one-leaf block: its root
                // is the coin's txid.
                "merkleRoot": RECEIVED,
                "time": 1700000000u32,
                "bits": 402917821u32,
                "nonce": 7u32,
                "height": height,
                "hash": format!("{:064x}", height),
            }
        })
    }
    let app = if down {
        Router::new().fallback(|| async { axum::http::StatusCode::INTERNAL_SERVER_ERROR })
    } else {
        Router::new()
            .route(
                "/findChainTipHeaderHex",
                get(|| async { axum::Json(header(TIP)) }),
            )
            .route(
                "/getPresentHeight",
                get(|| async {
                    axum::Json(serde_json::json!({"status": "success", "value": TIP}))
                }),
            )
            .route(
                "/findHeaderHexForHeight",
                get(|Query(q): Query<HashMap<String, u32>>| async move {
                    axum::Json(header(q.get("height").copied().unwrap_or(0)))
                }),
            )
    };
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.ok();
    });
    format!("http://{addr}")
}

/// The world's rows, written into the store as the tracker and the toolbox
/// would leave them (no transaction here was ever built or broadcast).
async fn seed(storage: &StorageSqlx, identity: &str, frozen: bool) {
    let (user, _) = storage.find_or_insert_user(identity).await.unwrap();
    let basket = storage
        .find_or_create_default_basket(user.user_id)
        .await
        .unwrap()
        .basket_id;
    let pool = storage.writer().clone();
    // The tracker's table, made by a read of it.
    bsv_wallet_cli::tracker_host::stored_word(storage, UNKNOWN)
        .await
        .unwrap();
    let now = chrono::DateTime::parse_from_rfc3339("2026-10-23T00:00:00Z")
        .unwrap()
        .with_timezone(&chrono::Utc);
    for (txid, word, height, reask) in [
        (MINED, "mined", Some(MINED_AT as i64), None),
        (PENDING, "seen", None, Some("age")),
        (RECEIVED, "mined", Some(MINED_AT as i64), None),
    ] {
        sqlx::query("INSERT INTO tracker_states (txid, word, height, reask) VALUES (?, ?, ?, ?)")
            .bind(txid)
            .bind(word)
            .bind(height)
            .bind(reask)
            .execute(&pool)
            .await
            .unwrap();
    }
    // The tracker's mined word's proof (its root only is read), and the
    // received coin's: a one-leaf block at MINED_AT, so its root is its
    // txid and the header stub answers that root.
    let one_leaf = bsv_sdk::transaction::MerklePath::from_coinbase_txid(RECEIVED, MINED_AT);
    for (txid, root, path) in [
        (MINED, ROOT, vec![0u8]),
        (RECEIVED, RECEIVED, one_leaf.to_binary()),
    ] {
        sqlx::query(
            "INSERT INTO proven_txs (txid, height, idx, block_hash, merkle_root, merkle_path, raw_tx, created_at, updated_at) \
             VALUES (?, ?, 0, ?, ?, ?, X'01000000', ?, ?)",
        )
        .bind(txid)
        .bind(MINED_AT as i64)
        .bind(format!("{:064x}", MINED_AT))
        .bind(root)
        .bind(path)
        .bind(now)
        .bind(now)
        .execute(&pool)
        .await
        .unwrap();
    }
    // A received coin, its creator mined at MINED_AT.
    let proven_id: i64 = sqlx::query_scalar("SELECT proven_tx_id FROM proven_txs WHERE txid = ?")
        .bind(RECEIVED)
        .fetch_one(&pool)
        .await
        .unwrap();
    let coins: Vec<(&str, Option<i64>)> = if frozen {
        vec![(FROZEN, None)]
    } else {
        vec![(RECEIVED, Some(proven_id))]
    };
    for (txid, proven) in coins {
        let tx_row = sqlx::query(
            "INSERT INTO transactions (user_id, proven_tx_id, status, reference, is_outgoing, satoshis, version, lock_time, description, txid, raw_tx, created_at, updated_at) \
             VALUES (?, ?, 'completed', ?, 0, 50000, 1, 0, 'a received coin', ?, X'01000000', ?, ?)",
        )
        .bind(user.user_id)
        .bind(proven)
        .bind(format!("ref-{}", &txid[..8]))
        .bind(txid)
        .bind(now)
        .bind(now)
        .execute(&pool)
        .await
        .unwrap()
        .last_insert_rowid();
        sqlx::query(
            "INSERT INTO outputs (user_id, transaction_id, basket_id, vout, satoshis, locking_script, txid, type, spendable, change, provided_by, purpose, output_description, created_at, updated_at) \
             VALUES (?, ?, ?, 0, 50000, ?, ?, 'P2PKH', 1, 1, 'storage', 'change', 'c', ?, ?)",
        )
        .bind(user.user_id)
        .bind(tx_row)
        .bind(basket)
        .bind(hex::decode(P2PKH).unwrap())
        .bind(txid)
        .bind(now)
        .bind(now)
        .execute(&pool)
        .await
        .unwrap();
    }
}

impl World {
    /// The worlds, by name:
    /// - `fresh`: an empty wallet, no bearer, the header stub up.
    /// - `auth`: `fresh` behind the bearer [`WIRE_BEARER`].
    /// - `callback`: `fresh` with the ARC callback token [`WIRE_CALLBACK`].
    /// - `tracked`: the tracker's words for a mined and a pending
    ///   transaction, and one coin of ours the chain feed froze.
    /// - `received`: one received coin whose creator is mined, header stub
    ///   up.
    /// - `headers-down`: `received`, the header service answering 500.
    /// - `broken`: `fresh` with the `outputs` table gone (a store fault).
    /// - `busy`: a file store whose write lock another connection holds.
    async fn build(name: &str) -> World {
        let mut keep: Vec<Box<dyn std::any::Any + Send>> = Vec::new();
        let key = PrivateKey::from_hex(ROOT_KEY).unwrap();
        let identity = key.public_key().to_hex();
        let storage = if name == "busy" {
            let tmp = tempfile::TempDir::new().unwrap();
            let path = tmp.path().join("wire.db");
            let storage = StorageSqlx::open(path.to_str().unwrap()).await.unwrap();
            keep.push(Box::new(tmp));
            storage
        } else {
            StorageSqlx::in_memory().await.unwrap()
        };
        storage.migrate("wire", &identity).await.unwrap();
        storage.make_available().await.unwrap();
        match name {
            "tracked" => seed(&storage, &identity, true).await,
            "received" | "headers-down" => seed(&storage, &identity, false).await,
            _ => {}
        }
        if name == "broken" {
            sqlx::query("DROP TABLE outputs")
                .execute(storage.writer())
                .await
                .unwrap();
        }
        let stub = header_stub(name == "headers-down").await;
        let mut options = ServicesOptions::mainnet()
            .with_chaintracks_url(stub)
            .without_bananablocks();
        options.arc_url = "http://127.0.0.1:9".to_string();
        options.arc_gorillapool_url = None;
        let services = Services::with_options(Chain::Main, options).unwrap();
        if name == "tracked" {
            bsv_wallet_cli::tracker_host::deliver_chain_event(
                &storage,
                &services,
                &bsv_wallet_cli::tracker_host::SystemClock,
                &bsv_wallet_cli::tracker_host::TickOptions::from_env(),
                1,
                bsv_tracker::ChainEvent::Frozen {
                    outpoint: bsv_tracker::Outpoint {
                        txid: FROZEN.to_string(),
                        vout: 0,
                    },
                },
            )
            .await
            .unwrap();
        }
        let wallet = Wallet::new(Some(key), storage, services).await.unwrap();
        // The wallet is opened (its opening writes made), then the lock.
        if name == "busy" {
            use sqlx::{ConnectOptions, Connection};
            let path = storage_path(&keep);
            let mut other = sqlx::sqlite::SqliteConnectOptions::new()
                .filename(path)
                .connect()
                .await
                .unwrap();
            sqlx::query("BEGIN IMMEDIATE")
                .execute(&mut other)
                .await
                .unwrap();
            // Never closed: the lock is held for the world's life.
            let _ = other.ping().await;
            keep.push(Box::new(other));
        }
        let mut config = ServerConfig::default();
        match name {
            "auth" => config.auth_token = Some(WIRE_BEARER.to_string()),
            "callback" => config.callback_token = Some(WIRE_CALLBACK.to_string()),
            _ => {}
        }
        let router = server::make_router(server::make_wallet_state(wallet), config);
        World {
            router,
            _keep: keep,
        }
    }

    async fn serve(&self, req: &Req) -> Resp {
        self.serve_raw(req).await.0
    }

    /// The normalized response, and the JSON body as served.
    async fn serve_raw(&self, req: &Req) -> (Resp, Option<J>) {
        let mut builder = Request::builder()
            .method(req.method.as_str())
            .uri(req.path.as_str());
        for (k, v) in &req.headers {
            builder = builder.header(k, v);
        }
        let body = match (&req.json, &req.raw) {
            (Some(json), _) => {
                builder = builder.header("content-type", "application/json");
                Body::from(json.compact())
            }
            (None, Some(raw)) => Body::from(raw.clone()),
            (None, None) => Body::empty(),
        };
        let response = self
            .router
            .clone()
            .oneshot(builder.body(body).unwrap())
            .await
            .unwrap();
        let status = response.status().as_u16();
        let headers = KEPT_HEADERS
            .iter()
            .filter_map(|h| {
                let values: Vec<&str> = response
                    .headers()
                    .get_all(*h)
                    .iter()
                    .map(|v| v.to_str().unwrap())
                    .collect();
                (!values.is_empty()).then(|| (h.to_string(), values.join(", ")))
            })
            .collect();
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        match J::parse(&bytes) {
            Some(json) => {
                // The body is compact JSON in its own key order: what the
                // fixture compares is the body itself.
                assert_eq!(
                    json.compact().as_bytes(),
                    &bytes[..],
                    "{} {}: the body is not compact JSON",
                    req.method,
                    req.path
                );
                let resp = Resp {
                    status,
                    headers,
                    json: Some(normalize(json.clone())),
                    text: None,
                };
                (resp, Some(json))
            }
            None => {
                let resp = Resp {
                    status,
                    headers,
                    json: None,
                    text: Some(normalize_text(&String::from_utf8_lossy(&bytes))),
                };
                (resp, None)
            }
        }
    }
}

fn storage_path(keep: &[Box<dyn std::any::Any + Send>]) -> PathBuf {
    keep.iter()
        .find_map(|k| k.downcast_ref::<tempfile::TempDir>())
        .unwrap()
        .path()
        .join("wire.db")
}

/// The process's environment, set once before any world: no request leaves
/// the machine, and no operator setting changes a served answer.
fn quiet_environment() {
    for var in [
        "ROOT_KEY",
        "SPEND_DEPTH",
        "SPEND_OWN_CHANGE",
        "ARC_MODE",
        "ARCADE",
        "ARC_URL",
        "ARC_API_KEY",
        "CHAINTRACKS_URL",
        "BREAK_GLASS_EXPLORER_HEADERS",
        "WOC_API_KEY",
        "BITAILS_API_KEY",
        "BANANABLOCKS_BASE",
        "AUTH_TOKEN",
        "BSV_WALLET_BROADCAST_VERIFY_ATTEMPTS",
        "BSV_WALLET_BROADCAST_VERIFY_DELAY_MS",
    ] {
        std::env::remove_var(var);
    }
    for var in ["HTTPS_PROXY", "HTTP_PROXY", "https_proxy", "http_proxy"] {
        std::env::set_var(var, "http://127.0.0.1:9");
    }
    std::env::set_var("NO_PROXY", "127.0.0.1");
    std::env::set_var("no_proxy", "127.0.0.1");
    std::env::set_var("BSV_WALLET_SKIP_BROADCAST_VERIFY", "1");
}

// ---------------------------------------------------------------------------
// The cases (record only). Replay reads the fixtures, never these.
// ---------------------------------------------------------------------------

const ORIGIN: &str = "http://wire.test";
const G: &str = "0279be667ef9dcbbac55a06295ce870b07029bfcdb2dce28d959f2815b16f81798";
const CERT_TYPE: &str = "dGVzdA==";
/// A ciphertext of `wire` under [`ROOT_KEY`], protocol `[2, "wire test"]`,
/// key `1`, counterparty [`G`].
const DECRYPT_CIPHERTEXT: [u8; 52] = [
    67, 235, 10, 58, 150, 72, 239, 64, 143, 10, 51, 111, 218, 66, 249, 235, 164, 141, 131, 123, 11,
    112, 52, 251, 161, 6, 254, 68, 76, 31, 76, 85, 161, 185, 3, 46, 158, 136, 86, 129, 8, 94, 152,
    124, 223, 53, 88, 226, 122, 43, 16, 152,
];

struct Recorder {
    dir: PathBuf,
    worlds: HashMap<String, World>,
    seq: usize,
    written: Vec<String>,
}

impl Recorder {
    async fn world(&mut self, name: &str) -> &World {
        if !self.worlds.contains_key(name) {
            let world = World::build(name).await;
            self.worlds.insert(name.to_string(), world);
        }
        &self.worlds[name]
    }

    /// Serve `req` in `world` and write it as `<seq>-<name>.json`; the
    /// JSON body as served (a later case may send part of it back).
    async fn case(&mut self, name: &str, world: &str, req: Req) -> Option<J> {
        let (resp, raw) = self.world(world).await.serve_raw(&req).await;
        self.seq += 1;
        let file = format!("{:03}-{name}.json", self.seq);
        let fixture = Fixture {
            world: world.to_string(),
            request: req,
            response: resp,
        };
        let mut text = serde_json::to_string_pretty(&fixture).unwrap();
        text.push('\n');
        std::fs::write(self.dir.join(&file), text).unwrap();
        self.written.push(file);
        raw
    }
}

fn get(path: &str) -> Req {
    Req {
        method: "GET".into(),
        path: path.into(),
        headers: BTreeMap::new(),
        json: None,
        raw: None,
    }
}

fn post(path: &str, body: serde_json::Value) -> Req {
    Req {
        method: "POST".into(),
        path: path.into(),
        headers: BTreeMap::from([("origin".to_string(), ORIGIN.to_string())]),
        json: Some(j(body)),
        raw: None,
    }
}

impl Req {
    fn header(mut self, k: &str, v: &str) -> Req {
        self.headers.insert(k.to_string(), v.to_string());
        self
    }
    fn without(mut self, k: &str) -> Req {
        self.headers.remove(k);
        self
    }
    fn raw(mut self, body: &str) -> Req {
        self.json = None;
        self.raw = Some(body.to_string());
        self.headers
            .insert("content-type".into(), "application/json".into());
        self
    }
}

async fn record_all(r: &mut Recorder) {
    use serde_json::json;
    let pk = |protocol: &str, key_id: &str| json!({"protocolID": [2, protocol], "keyID": key_id, "counterparty": G});

    // --- Status (each served on GET and POST).
    for (path, name) in [
        ("/isAuthenticated", "isAuthenticated"),
        ("/getHeight", "getHeight"),
        ("/getNetwork", "getNetwork"),
        ("/getVersion", "getVersion"),
        ("/waitForAuthentication", "waitForAuthentication"),
    ] {
        r.case(&format!("{name}.get"), "fresh", get(path)).await;
        r.case(&format!("{name}.post"), "fresh", post(path, json!({})))
            .await;
    }
    r.case(
        "getHeaderForHeight.ok",
        "fresh",
        post("/getHeaderForHeight", json!({"height": MINED_AT})),
    )
    .await;

    // --- Keys and crypto.
    r.case(
        "getPublicKey.identity",
        "fresh",
        post("/getPublicKey", json!({"identityKey": true})),
    )
    .await;
    r.case(
        "getPublicKey.derived",
        "fresh",
        post("/getPublicKey", pk("wire test", "1")),
    )
    .await;
    r.case(
        "getPublicKey.forSelf",
        "fresh",
        post(
            "/getPublicKey",
            json!({"protocolID": [1, "wire test"], "keyID": "1", "counterparty": "self", "forSelf": true}),
        ),
    )
    .await;
    let data: Vec<u8> = b"wire".to_vec();
    let mut sign = pk("wire test", "1");
    sign["data"] = json!(data);
    let sig = r
        .case(
            "createSignature.data",
            "fresh",
            post("/createSignature", sign),
        )
        .await;
    let mut sign = pk("wire test", "1");
    sign["hashToDirectlySign"] = json!(vec![7u8; 32]);
    r.case(
        "createSignature.hashToDirectlySign",
        "fresh",
        post("/createSignature", sign),
    )
    .await;
    let mut verify =
        json!({"protocolID": [2, "wire test"], "keyID": "1", "counterparty": G, "forSelf": true});
    verify["data"] = json!(data);
    verify["signature"] = serde_json::to_value(sig.unwrap().get("signature").unwrap()).unwrap();
    r.case(
        "verifySignature.valid",
        "fresh",
        post("/verifySignature", verify.clone()),
    )
    .await;
    verify["data"] = json!(b"other".to_vec());
    r.case(
        "verifySignature.invalid",
        "fresh",
        post("/verifySignature", verify),
    )
    .await;
    let mut enc = pk("wire test", "1");
    enc["plaintext"] = json!(data);
    r.case("encrypt.ok", "fresh", post("/encrypt", enc)).await;
    // A ciphertext of `wire` under the fixed key (one encrypt's answer, kept:
    // a new one carries a new IV, and the fixture would move on every
    // recording).
    let mut dec = pk("wire test", "1");
    dec["ciphertext"] = json!(DECRYPT_CIPHERTEXT.to_vec());
    r.case("decrypt.ok", "fresh", post("/decrypt", dec)).await;
    let mut hmac = pk("wire test", "1");
    hmac["data"] = json!(data);
    let made = r
        .case("createHmac.ok", "fresh", post("/createHmac", hmac.clone()))
        .await;
    hmac["hmac"] = serde_json::to_value(made.unwrap().get("hmac").unwrap()).unwrap();
    r.case(
        "verifyHmac.valid",
        "fresh",
        post("/verifyHmac", hmac.clone()),
    )
    .await;
    hmac["data"] = json!(b"other".to_vec());
    r.case("verifyHmac.invalid", "fresh", post("/verifyHmac", hmac))
        .await;
    let mut short = pk("wire test", "1");
    short["data"] = json!(data);
    short["hmac"] = json!([1, 2, 3]);
    r.case("verifyHmac.short", "fresh", post("/verifyHmac", short))
        .await;
    r.case(
        "revealCounterpartyKeyLinkage.ok",
        "fresh",
        post(
            "/revealCounterpartyKeyLinkage",
            json!({"counterparty": G, "verifier": G}),
        ),
    )
    .await;
    r.case(
        "revealCounterpartyKeyLinkage.badKey",
        "fresh",
        post(
            "/revealCounterpartyKeyLinkage",
            json!({"counterparty": "02zz", "verifier": G}),
        ),
    )
    .await;
    r.case(
        "revealSpecificKeyLinkage.ok",
        "fresh",
        post(
            "/revealSpecificKeyLinkage",
            json!({"counterparty": G, "verifier": G, "protocolID": [2, "wire test"], "keyID": "1"}),
        ),
    )
    .await;

    // --- The caller's malformed requests.
    let mut no_origin = pk("wire test", "1");
    no_origin["plaintext"] = json!(data);
    r.case(
        "encrypt.noOrigin",
        "fresh",
        post("/encrypt", no_origin).without("origin"),
    )
    .await;
    let mut bad_protocol = pk("wire test", "1");
    bad_protocol["protocolID"] = json!("wire test");
    bad_protocol["data"] = json!(data);
    r.case(
        "createSignature.badProtocolID",
        "fresh",
        post("/createSignature", bad_protocol),
    )
    .await;
    r.case(
        "createSignature.noData",
        "fresh",
        post("/createSignature", pk("wire test", "1")),
    )
    .await;
    r.case(
        "createSignature.missingField",
        "fresh",
        post("/createSignature", json!({"data": [1]})),
    )
    .await;
    r.case(
        "createSignature.malformedJson",
        "fresh",
        post("/createSignature", json!({})).raw("{\"data\": [1"),
    )
    .await;
    r.case(
        "getPublicKey.loneSurrogate",
        "fresh",
        post("/getPublicKey", json!({})).raw(
            "{\"protocolID\":[2,\"wire test\"],\"keyID\":\"ab \\ud83d cd\",\"counterparty\":\"self\"}",
        ),
    )
    .await;
    r.case("unknownRoute", "fresh", post("/noSuchCall", json!({})))
        .await;

    // --- Actions and outputs, an empty wallet.
    r.case(
        "listActions.empty",
        "fresh",
        post("/listActions", json!({"labels": []})),
    )
    .await;
    r.case(
        "listOutputs.empty",
        "fresh",
        post("/listOutputs", json!({"basket": "default"})),
    )
    .await;
    r.case(
        "createAction.insufficientFunds",
        "fresh",
        post(
            "/createAction",
            json!({"description": "wire unfunded", "outputs": [
                {"lockingScript": P2PKH, "satoshis": 1000, "outputDescription": "an output"}]}),
        ),
    )
    .await;
    r.case(
        "createAction.badAllowPending",
        "fresh",
        post(
            "/createAction",
            json!({"description": "wire bad word", "allowPending": "everything", "outputs": [
                {"lockingScript": P2PKH, "satoshis": 1000, "outputDescription": "an output"}]}),
        ),
    )
    .await;
    r.case(
        "createAction.scopeNamesForeignCoin",
        "fresh",
        post(
            "/createAction",
            json!({"description": "wire foreign coin", "allowPending": {"received": [format!("{UNKNOWN}:0")]}, "outputs": [
                {"lockingScript": P2PKH, "satoshis": 1000, "outputDescription": "an output"}]}),
        ),
    )
    .await;
    r.case(
        "signAction.unknownReference",
        "fresh",
        post(
            "/signAction",
            json!({"spends": {}, "reference": "bm8tc3VjaC1yZWZlcmVuY2U="}),
        ),
    )
    .await;
    r.case(
        "abortAction.unknownReference",
        "fresh",
        post(
            "/abortAction",
            json!({"reference": "bm8tc3VjaC1yZWZlcmVuY2U="}),
        ),
    )
    .await;
    r.case(
        "relinquishOutput.unknown",
        "fresh",
        post(
            "/relinquishOutput",
            json!({"basket": "default", "output": format!("{UNKNOWN}.0")}),
        ),
    )
    .await;
    r.case(
        "internalizeAction.invalidBeef",
        "fresh",
        post(
            "/internalizeAction",
            json!({"tx": [0xde, 0xad, 0xbe, 0xef], "outputs": [{"outputIndex": 0, "protocol": "wallet payment",
                "paymentRemittance": {"derivationPrefix": "d2lyZQ==", "derivationSuffix": "d2lyZQ==", "senderIdentityKey": G}}],
                "description": "wire invalid beef"}),
        ),
    )
    .await;

    // --- Certificates. BRC-100 names the certificate's type `type`; the
    // server's types take `certificateType` (found by this corpus, #69): the
    // BRC's spelling is recorded as served, then the server's, in order:
    // acquire, list, prove, relinquish.
    for (path, body) in [
        (
            "/acquireCertificate",
            json!({"type": CERT_TYPE, "certifier": G, "acquisitionProtocol": "direct",
                "fields": {"name": "wire"}, "serialNumber": "AQID",
                "revocationOutpoint": format!("{UNKNOWN}.0"), "signature": "deadbeef",
                "keyringRevealer": "certifier", "keyringForSubject": {}}),
        ),
        (
            "/relinquishCertificate",
            json!({"type": CERT_TYPE, "serialNumber": "AQID", "certifier": G}),
        ),
        (
            "/proveCertificate",
            json!({"certificate": {"type": CERT_TYPE, "subject": G, "serialNumber": "AQID", "certifier": G,
                "revocationOutpoint": format!("{UNKNOWN}.0"), "signature": "deadbeef", "fields": {"name": "wire"}},
                "fieldsToReveal": ["name"], "verifier": G}),
        ),
    ] {
        r.case(
            &format!("{}.brc100Type", &path[1..]),
            "fresh",
            post(path, body),
        )
        .await;
    }
    let acquired = r
        .case(
            "acquireCertificate.direct",
            "fresh",
            post(
                "/acquireCertificate",
                json!({"certificateType": CERT_TYPE, "certifier": G, "acquisitionProtocol": "direct",
                    "fields": {"name": "wire"}, "serialNumber": "AQID",
                    "revocationOutpoint": format!("{UNKNOWN}.0"), "signature": "deadbeef",
                    "keyringRevealer": "certifier", "keyringForSubject": {}}),
            ),
        )
        .await;
    r.case(
        "listCertificates.one",
        "fresh",
        post(
            "/listCertificates",
            json!({"certifiers": [G], "types": [CERT_TYPE]}),
        ),
    )
    .await;
    let certificate = acquired
        .map(|c| serde_json::to_value(c).unwrap())
        .unwrap_or_default();
    r.case(
        "proveCertificate.held",
        "fresh",
        post(
            "/proveCertificate",
            json!({"certificate": certificate, "fieldsToReveal": ["name"], "verifier": G}),
        ),
    )
    .await;
    r.case(
        "relinquishCertificate.held",
        "fresh",
        post(
            "/relinquishCertificate",
            json!({"certificateType": CERT_TYPE, "serialNumber": "AQID", "certifier": G}),
        ),
    )
    .await;
    r.case(
        "relinquishCertificate.unknown",
        "fresh",
        post(
            "/relinquishCertificate",
            json!({"certificateType": CERT_TYPE, "serialNumber": "AQID", "certifier": G}),
        ),
    )
    .await;
    r.case(
        "proveCertificate.unknown",
        "fresh",
        post(
            "/proveCertificate",
            json!({"certificate": {"certificateType": CERT_TYPE, "subject": G, "serialNumber": "AQID", "certifier": G,
                "revocationOutpoint": format!("{UNKNOWN}.0"), "signature": "deadbeef", "fields": {"name": "wire"}},
                "fieldsToReveal": ["name"], "verifier": G}),
        ),
    )
    .await;
    r.case(
        "discoverByIdentityKey.ok",
        "fresh",
        post("/discoverByIdentityKey", json!({"identityKey": G})),
    )
    .await;
    r.case(
        "discoverByAttributes.ok",
        "fresh",
        post(
            "/discoverByAttributes",
            json!({"attributes": {"name": "wire"}}),
        ),
    )
    .await;

    // --- The host routes.
    r.case("permission-audit.get", "fresh", get("/permission-audit"))
        .await;
    r.case(
        "permission-audit.reset",
        "fresh",
        post("/permission-audit/reset", json!({})),
    )
    .await;
    r.case(
        "permission-audit.afterReset",
        "fresh",
        get("/permission-audit"),
    )
    .await;
    r.case("chain-feed.none", "fresh", get("/chain-feed")).await;
    r.case("chain-feed.frozen", "tracked", get("/chain-feed"))
        .await;
    r.case("tx.mined", "tracked", get(&format!("/tx/{MINED}")))
        .await;
    r.case("tx.pending", "tracked", get(&format!("/tx/{PENDING}")))
        .await;
    r.case("tx.unknown", "tracked", get(&format!("/tx/{UNKNOWN}")))
        .await;
    r.case("tx.notHex", "tracked", get("/tx/nothex")).await;
    r.case(
        "arc-callback.disabled",
        "fresh",
        post("/arc-callback", json!({"txid": MINED, "txStatus": "MINED"})).without("origin"),
    )
    .await;
    r.case(
        "arc-callback.noToken",
        "callback",
        post("/arc-callback", json!({"txid": MINED, "txStatus": "MINED"})).without("origin"),
    )
    .await;
    r.case(
        "arc-callback.badJson",
        "callback",
        post("/arc-callback", json!({}))
            .without("origin")
            .header("x-callbacktoken", WIRE_CALLBACK)
            .raw("{\"txid\":"),
    )
    .await;
    r.case(
        "arc-callback.unknownTx",
        "callback",
        post(
            "/arc-callback",
            json!({"txid": UNKNOWN, "txStatus": "SEEN_ON_NETWORK"}),
        )
        .without("origin")
        .header("authorization", &format!("Bearer {WIRE_CALLBACK}")),
    )
    .await;

    // --- The bearer.
    r.case(
        "auth.isAuthenticated.noBearer",
        "auth",
        get("/isAuthenticated"),
    )
    .await;
    r.case(
        "auth.isAuthenticated.wrongBearer",
        "auth",
        get("/isAuthenticated").header("authorization", "Bearer not-it"),
    )
    .await;
    r.case(
        "auth.isAuthenticated.bearer",
        "auth",
        get("/isAuthenticated").header("authorization", &format!("Bearer {WIRE_BEARER}")),
    )
    .await;
    r.case("auth.chain-feed.noBearer", "auth", get("/chain-feed"))
        .await;
    r.case(
        "cors.isAuthenticated",
        "fresh",
        get("/isAuthenticated").header("origin", "https://app.example"),
    )
    .await;

    // --- The spend gate's refusals.
    r.case(
        "createAction.inputNotOffered",
        "tracked",
        post(
            "/createAction",
            json!({"description": "wire frozen coin", "allowPending": "received",
                "inputs": [{"outpoint": format!("{FROZEN}.0"), "unlockingScriptLength": 107, "inputDescription": "named"}],
                "outputs": [{"lockingScript": P2PKH, "satoshis": 1000, "outputDescription": "an output"}]}),
        ),
    )
    .await;
    r.case(
        "createAction.headersUnavailable",
        "headers-down",
        post(
            "/createAction",
            json!({"description": "wire headers down", "outputs": [
                {"lockingScript": P2PKH, "satoshis": 1000, "outputDescription": "an output"}]}),
        ),
    )
    .await;
    r.case("getHeight.headersDown", "headers-down", get("/getHeight"))
        .await;
    r.case(
        "createAction.unspendableCoin",
        "received",
        post(
            "/createAction",
            json!({"description": "wire incomplete ancestry", "outputs": [
                {"lockingScript": P2PKH, "satoshis": 1000, "outputDescription": "an output"}]}),
        ),
    )
    .await;
    r.case(
        "createAction.storeFault",
        "broken",
        post(
            "/createAction",
            json!({"description": "wire store fault", "outputs": [
                {"lockingScript": P2PKH, "satoshis": 1000, "outputDescription": "an output"}]}),
        ),
    )
    .await;
    r.case(
        "listOutputs.storeFault",
        "broken",
        post("/listOutputs", json!({"basket": "default"})),
    )
    .await;
    r.case(
        "createAction.storeBusy",
        "busy",
        post(
            "/createAction",
            json!({"description": "wire busy", "outputs": [
                {"lockingScript": P2PKH, "satoshis": 1000, "outputDescription": "an output"}]}),
        ),
    )
    .await;
}

// ---------------------------------------------------------------------------
// The judge: what the corpus must cover.
// ---------------------------------------------------------------------------

/// The 28 BRC-100 calls the router serves.
const ENDPOINTS: [&str; 28] = [
    "/isAuthenticated",
    "/getPublicKey",
    "/createSignature",
    "/createAction",
    "/internalizeAction",
    "/getHeight",
    "/getNetwork",
    "/getVersion",
    "/waitForAuthentication",
    "/getHeaderForHeight",
    "/verifySignature",
    "/encrypt",
    "/decrypt",
    "/createHmac",
    "/verifyHmac",
    "/signAction",
    "/abortAction",
    "/listActions",
    "/listOutputs",
    "/relinquishOutput",
    "/acquireCertificate",
    "/listCertificates",
    "/proveCertificate",
    "/relinquishCertificate",
    "/discoverByIdentityKey",
    "/discoverByAttributes",
    "/revealCounterpartyKeyLinkage",
    "/revealSpecificKeyLinkage",
];

/// The five host routes (not BRC-100).
const HOST_ROUTES: [&str; 5] = [
    "/permission-audit",
    "/permission-audit/reset",
    "/chain-feed",
    "/tx/{txid}",
    "/arc-callback",
];

/// Every refusal status the brief names, with a code each must carry at
/// least once.
const CODES: [(u16, &str); 10] = [
    (400, "VALIDATION_ERROR"),
    (401, "UNAUTHORIZED"),
    (402, "INSUFFICIENT_FUNDS"),
    (403, "INPUT_NOT_OFFERED"),
    (404, "NOT_FOUND"),
    (409, "UNSPENDABLE_COIN"),
    (500, "INTERNAL_ERROR"),
    (502, "SERVICE_ERROR"),
    (503, "STORE_BUSY"),
    (503, "HEADERS_UNAVAILABLE"),
];

fn route_of(path: &str) -> String {
    if path.starts_with("/tx/") {
        "/tx/{txid}".to_string()
    } else {
        path.to_string()
    }
}

fn judge_coverage(fixtures: &[(String, Fixture)]) -> Vec<String> {
    let routes: BTreeSet<String> = fixtures
        .iter()
        .map(|(_, f)| route_of(&f.request.path))
        .collect();
    let codes: BTreeSet<(u16, String)> = fixtures
        .iter()
        .filter_map(|(_, f)| {
            let code = f.response.json.as_ref()?.get("code")?.as_str()?;
            Some((f.response.status, code.to_string()))
        })
        .collect();
    let mut missing = Vec::new();
    for route in ENDPOINTS.iter().chain(HOST_ROUTES.iter()) {
        if !routes.contains(*route) {
            missing.push(format!("no fixture serves {route}"));
        }
    }
    for (status, code) in CODES {
        if !codes.contains(&(status, code.to_string())) {
            missing.push(format!("no fixture answers {status} {code}"));
        }
    }
    missing
}

// ---------------------------------------------------------------------------
// Record or replay.
// ---------------------------------------------------------------------------

fn wire_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/wire")
}

fn read_fixtures(dir: &Path) -> Vec<(String, Fixture)> {
    let mut files: Vec<PathBuf> = std::fs::read_dir(dir)
        .unwrap_or_else(|e| panic!("{}: {e}", dir.display()))
        .map(|e| e.unwrap().path())
        .filter(|p| p.extension().is_some_and(|x| x == "json"))
        .collect();
    files.sort();
    files
        .into_iter()
        .map(|p| {
            let text = std::fs::read_to_string(&p).unwrap();
            let fixture: Fixture =
                serde_json::from_str(&text).unwrap_or_else(|e| panic!("{}: {e}", p.display()));
            (
                p.file_name().unwrap().to_string_lossy().into_owned(),
                fixture,
            )
        })
        .collect()
}

/// A line diff of two pretty renderings (the fixture's, the served).
fn diff(want: &str, got: &str) -> String {
    let (w, g): (Vec<&str>, Vec<&str>) = (want.lines().collect(), got.lines().collect());
    // The longest common subsequence, line by line (the bodies are small).
    let mut lcs = vec![vec![0usize; g.len() + 1]; w.len() + 1];
    for i in (0..w.len()).rev() {
        for k in (0..g.len()).rev() {
            lcs[i][k] = if w[i] == g[k] {
                lcs[i + 1][k + 1] + 1
            } else {
                lcs[i + 1][k].max(lcs[i][k + 1])
            };
        }
    }
    let (mut i, mut k, mut out) = (0, 0, String::new());
    while i < w.len() || k < g.len() {
        if i < w.len() && k < g.len() && w[i] == g[k] {
            out.push_str(&format!("  {}\n", w[i]));
            i += 1;
            k += 1;
        } else if k < g.len() && (i == w.len() || lcs[i][k + 1] >= lcs[i + 1][k]) {
            out.push_str(&format!("+ {}\n", g[k]));
            k += 1;
        } else {
            out.push_str(&format!("- {}\n", w[i]));
            i += 1;
        }
    }
    out
}

/// The router's futures are deep in a debug build: the run gets its own
/// stacks.
#[test]
fn wire() {
    quiet_environment();
    const STACK: usize = 64 * 1024 * 1024;
    std::thread::Builder::new()
        .stack_size(STACK)
        .spawn(|| {
            tokio::runtime::Builder::new_multi_thread()
                .worker_threads(2)
                .thread_stack_size(STACK)
                .enable_all()
                .build()
                .unwrap()
                .block_on(record_or_replay())
        })
        .unwrap()
        .join()
        .unwrap();
}

async fn record_or_replay() {
    let dir = wire_dir();
    if std::env::var("WIRE_RECORD").is_ok_and(|v| v == "1") {
        std::fs::create_dir_all(&dir).unwrap();
        for entry in std::fs::read_dir(&dir).unwrap() {
            let path = entry.unwrap().path();
            if path.extension().is_some_and(|x| x == "json") {
                std::fs::remove_file(path).unwrap();
            }
        }
        bsv_wallet_cli::server::audit::reset();
        let mut recorder = Recorder {
            dir: dir.clone(),
            worlds: HashMap::new(),
            seq: 0,
            written: Vec::new(),
        };
        record_all(&mut recorder).await;
        println!("recorded {} fixtures", recorder.written.len());
    }

    // The permission audit is the process's: each pass starts it empty.
    bsv_wallet_cli::server::audit::reset();
    let fixtures = read_fixtures(&dir);
    assert!(!fixtures.is_empty(), "no fixture under {}", dir.display());
    let mut worlds: HashMap<String, World> = HashMap::new();
    let mut failures = Vec::new();
    for (file, fixture) in &fixtures {
        if !worlds.contains_key(&fixture.world) {
            worlds.insert(fixture.world.clone(), World::build(&fixture.world).await);
        }
        let got = worlds[&fixture.world].serve(&fixture.request).await;
        if got != fixture.response {
            let want = serde_json::to_string_pretty(&fixture.response).unwrap();
            let served = serde_json::to_string_pretty(&got).unwrap();
            failures.push(format!(
                "{file}: {} {} answered differently (- fixture, + served)\n{}",
                fixture.request.method,
                fixture.request.path,
                diff(&want, &served)
            ));
        }
    }
    let missing = judge_coverage(&fixtures);
    println!(
        "{} fixtures replayed, {} differ, {} coverage gaps",
        fixtures.len(),
        failures.len(),
        missing.len()
    );
    assert!(
        failures.is_empty() && missing.is_empty(),
        "\n{}\n{}\n(a change on purpose: WIRE_RECORD=1 cargo test --test wire, in its own commit; see docs/WIRE.md)",
        failures.join("\n"),
        missing.join("\n")
    );
}
