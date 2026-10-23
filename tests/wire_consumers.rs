//! The consumers' recorded calls (#110): one exchange per call shape a
//! consumer of this binary sends over the wire, under
//! `tests/wire/consumers/<consumer>/`.
//!
//! `cargo test --test wire_consumers` replays every fixture against the
//! router, in-process (`tower::ServiceExt::oneshot`), and fails on any byte
//! of difference after normalization, naming the consumer whose call
//! changed. `WIRE_RECORD=1 cargo test --test wire_consumers` re-records
//! every fixture from the cases below; nothing else re-records. The
//! consumers, what they call and where each shape comes from are in
//! `docs/DEPENDENTS.md`.
//!
//! The fixture format is the wire oracle's (#69, `docs/WIRE.md` on
//! `lane/oracle-fixtures`): `world`, `request`, `response`, with three
//! fields of its own: `consumer` (the dependent, also the directory),
//! `call` (the consumer's name for the call) and `source` (the file in the
//! consumer that sends this shape). Each consumer's fixtures are served
//! from worlds of their own, in file order, so no consumer's state reaches
//! another's. Nothing leaves the machine: the header service is a local
//! stub, the broadcaster a closed local port, every other request goes
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
// The consumers. Every one must have at least one fixture, and every
// fixture must name one of them (`docs/DEPENDENTS.md` has a row for each).
// ---------------------------------------------------------------------------

const CONSUMERS: [&str; 7] = [
    // LOW's fleet: the daemon's HTTP on four hosts, its funder's spends.
    "low-fleet",
    // The stack's witness spend wallet (bsv-stack-lean).
    "stack-witness",
    // The x402 skill's wallet client (lib/metanet.py, scripts/brc31_helpers.py).
    "x402-skill",
    // The MCP companion (mcp/src/main.rs) on behalf of its clients.
    "mcp",
    // The apps' x402 payers (~/bsv/agents, ~/bsv/zanaadu, ~/bsv/teragunv2).
    "app-payers",
    // dolphinmilk: the 0.1.23 crate's server in-process, called over HTTP.
    "dolphinmilk",
    // The e2e harness's client (tests/e2e/lib/wallet-client.js).
    "e2e-harness",
];

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

/// A request body from its JSON text, its keys in the order the consumer
/// writes them (`serde_json::json!` would sort them).
fn body(text: &str) -> J {
    serde_json::from_str(text).unwrap_or_else(|e| panic!("{text}: {e}"))
}

// ---------------------------------------------------------------------------
// The fixture.
// ---------------------------------------------------------------------------

#[derive(Serialize, Deserialize, Clone)]
struct Fixture {
    /// The dependent whose call this is (one of [`CONSUMERS`]).
    consumer: String,
    /// The consumer's own name for the call.
    call: String,
    /// The file in the consumer that sends this shape.
    source: String,
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

/// The response headers that are part of the contract (the wire oracle's
/// list). Every other header is the transport's.
const KEPT_HEADERS: [&str; 6] = [
    "content-type",
    "retry-after",
    "access-control-allow-origin",
    "access-control-allow-credentials",
    "access-control-expose-headers",
    "vary",
];

// ---------------------------------------------------------------------------
// The normalizer: the wire oracle's rules (`docs/WIRE.md`), the values that
// differ from run to run with no change to the code.
// ---------------------------------------------------------------------------

const VOLATILE_KEYS: [(&str, &str); 5] = [
    ("ciphertext", "<ciphertext>"),
    ("encryptedLinkage", "<ciphertext>"),
    ("encryptedLinkageProof", "<ciphertext>"),
    ("ageMinutes", "<minutes>"),
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

fn normalize_text(s: &str) -> String {
    replace_number_before(
        &replace_number_after(s, "127.0.0.1:", "<port>"),
        " min old",
        "<minutes>",
    )
}

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

/// The wire oracle's fixed root key: the identity key, every derived key
/// and every (RFC 6979) signature is the same on every run.
const ROOT_KEY: &str = "1f1f1f1f1f1f1f1f1f1f1f1f1f1f1f1f1f1f1f1f1f1f1f1f1f1f1f1f1f1f1f1f";
/// The bearer of the `auth` world.
const BEARER: &str = "wire-bearer";

/// The header stub's tip.
const TIP: u32 = 900_010;

/// The seeded coins (hand-written rows, never broadcast): a received
/// payment not yet mined, and a second one (the scoped word names one).
const PENDING_A: &str = "7777777777777777777777777777777777777777777777777777777777777777";
const PENDING_B: &str = "8888888888888888888888888888888888888888888888888888888888888888";
/// A txid the wallet has never seen.
const UNKNOWN: &str = "3333333333333333333333333333333333333333333333333333333333333333";
/// The received coins' script: not the key their derivation gives, so a
/// spend the gate lets through stops at signing with a stable answer
/// (signed, its change's fresh derivation would move the txid each run).
const P2PKH: &str = "76a914dbc0a7c84983c5bf199b7b2d41b3acf0408ee5aa88ac";

struct World {
    router: Router,
}

/// A local header service: the tip at [`TIP`], any height's header.
async fn header_stub() -> String {
    use axum::extract::Query;
    use axum::routing::get;
    fn header(height: u32) -> serde_json::Value {
        serde_json::json!({
            "status": "success",
            "value": {
                "version": 536870912u32,
                "previousHash": "0".repeat(64),
                "merkleRoot": "5".repeat(64),
                "time": 1700000000u32,
                "bits": 402917821u32,
                "nonce": 7u32,
                "height": height,
                "hash": format!("{:064x}", height),
            }
        })
    }
    let app = Router::new()
        .route(
            "/findChainTipHeaderHex",
            get(|| async { axum::Json(header(TIP)) }),
        )
        .route(
            "/getPresentHeight",
            get(|| async { axum::Json(serde_json::json!({"status": "success", "value": TIP})) }),
        )
        .route(
            "/findHeaderHexForHeight",
            get(|Query(q): Query<HashMap<String, u32>>| async move {
                axum::Json(header(q.get("height").copied().unwrap_or(0)))
            }),
        );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.ok();
    });
    format!("http://{addr}")
}

/// Two received coins not yet mined (no proof stored), as an
/// `internalizeAction` of a payment leaves them: 30,000 and 20,000 sats.
async fn seed_pending(storage: &StorageSqlx, identity: &str) {
    let (user, _) = storage.find_or_insert_user(identity).await.unwrap();
    let basket = storage
        .find_or_create_default_basket(user.user_id)
        .await
        .unwrap()
        .basket_id;
    let pool = storage.writer().clone();
    let when = chrono::DateTime::parse_from_rfc3339("2026-10-23T00:00:00Z")
        .unwrap()
        .with_timezone(&chrono::Utc);
    for (txid, satoshis) in [(PENDING_A, 30_000i64), (PENDING_B, 20_000)] {
        let tx_row = sqlx::query(
            "INSERT INTO transactions (user_id, status, reference, is_outgoing, satoshis, version, lock_time, description, txid, raw_tx, created_at, updated_at) \
             VALUES (?, 'unproven', ?, 0, ?, 1, 0, 'a received payment', ?, X'01000000', ?, ?)",
        )
        .bind(user.user_id)
        .bind(format!("ref-{}", &txid[..8]))
        .bind(satoshis)
        .bind(txid)
        .bind(when)
        .bind(when)
        .execute(&pool)
        .await
        .unwrap()
        .last_insert_rowid();
        sqlx::query(
            "INSERT INTO outputs (user_id, transaction_id, basket_id, vout, satoshis, locking_script, txid, type, spendable, change, provided_by, purpose, output_description, derivation_prefix, derivation_suffix, sender_identity_key, created_at, updated_at) \
             VALUES (?, ?, ?, 0, ?, ?, ?, 'P2PKH', 1, 1, 'storage', 'change', 'c', 'SfKxPIJNgdI=', 'NaGLC6fMH50=', ?, ?, ?)",
        )
        .bind(user.user_id)
        .bind(tx_row)
        .bind(basket)
        .bind(satoshis)
        .bind(hex::decode(P2PKH).unwrap())
        .bind(txid)
        .bind(G)
        .bind(when)
        .bind(when)
        .execute(&pool)
        .await
        .unwrap();
    }
}

impl World {
    /// The worlds, by name:
    /// - `fresh`: an empty wallet, no bearer, the header stub up.
    /// - `auth`: `fresh` behind the bearer [`BEARER`].
    /// - `pending`: two received coins not yet mined ([`PENDING_A`],
    ///   [`PENDING_B`]), the payments a funder's sweeps leave.
    async fn build(name: &str) -> World {
        let key = PrivateKey::from_hex(ROOT_KEY).unwrap();
        let identity = key.public_key().to_hex();
        let storage = StorageSqlx::in_memory().await.unwrap();
        storage.migrate("wire", &identity).await.unwrap();
        storage.make_available().await.unwrap();
        // The tracker's table, made by a read of it.
        bsv_wallet_cli::tracker_host::stored_word(&storage, UNKNOWN)
            .await
            .unwrap();
        match name {
            "fresh" | "auth" => {}
            "pending" => seed_pending(&storage, &identity).await,
            other => panic!("no world named {other}"),
        }
        let mut options = ServicesOptions::mainnet()
            .with_chaintracks_url(header_stub().await)
            .without_bananablocks();
        options.arc_url = "http://127.0.0.1:9".to_string();
        options.arc_gorillapool_url = None;
        let services = Services::with_options(Chain::Main, options).unwrap();
        let wallet = Wallet::new(Some(key), storage, services).await.unwrap();
        let mut config = ServerConfig::default();
        if name == "auth" {
            config.auth_token = Some(BEARER.to_string());
        }
        World {
            router: server::make_router(server::make_wallet_state(wallet), config),
        }
    }

    async fn serve(&self, req: &Req) -> Resp {
        let mut builder = Request::builder()
            .method(req.method.as_str())
            .uri(req.path.as_str());
        for (k, v) in &req.headers {
            builder = builder.header(k, v);
        }
        let body = match &req.json {
            Some(json) => {
                builder = builder.header("content-type", "application/json");
                Body::from(json.compact())
            }
            None => Body::empty(),
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
                assert_eq!(
                    json.compact().as_bytes(),
                    &bytes[..],
                    "{} {}: the body is not compact JSON",
                    req.method,
                    req.path
                );
                Resp {
                    status,
                    headers,
                    json: Some(normalize(json)),
                    text: None,
                }
            }
            None => Resp {
                status,
                headers,
                json: None,
                text: Some(normalize_text(&String::from_utf8_lossy(&bytes))),
            },
        }
    }
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

/// The secp256k1 generator's key: the counterparty "anyone", and the
/// stand-in for a server's identity key.
const G: &str = "0279be667ef9dcbbac55a06295ce870b07029bfcdb2dce28d959f2815b16f81798";
/// A pay-to-public-key-hash output script (a payee's).
const PAYEE: &str = "76a91489abcdefabbaabbaabbaabbaabbaabbaabbaabba88ac";

struct Recorder {
    dir: PathBuf,
    /// Per consumer, its worlds.
    worlds: HashMap<(String, String), World>,
    /// Per consumer, its sequence.
    seq: HashMap<String, usize>,
    written: usize,
}

struct Call<'a> {
    consumer: &'a str,
    call: &'a str,
    source: &'a str,
    world: &'a str,
    request: Req,
}

impl Recorder {
    async fn case(&mut self, c: Call<'_>) {
        let key = (c.consumer.to_string(), c.world.to_string());
        if !self.worlds.contains_key(&key) {
            self.worlds.insert(key.clone(), World::build(c.world).await);
        }
        let response = self.worlds[&key].serve(&c.request).await;
        let seq = self.seq.entry(c.consumer.to_string()).or_default();
        *seq += 1;
        let dir = self.dir.join(c.consumer);
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join(format!("{:02}-{}.json", seq, c.call));
        let fixture = Fixture {
            consumer: c.consumer.to_string(),
            call: c.call.to_string(),
            source: c.source.to_string(),
            world: c.world.to_string(),
            request: c.request,
            response,
        };
        let mut text = serde_json::to_string_pretty(&fixture).unwrap();
        text.push('\n');
        std::fs::write(file, text).unwrap();
        self.written += 1;
    }
}

fn get(path: &str, origin: &str) -> Req {
    Req {
        method: "GET".into(),
        path: path.into(),
        headers: BTreeMap::from([("origin".to_string(), origin.to_string())]),
        json: None,
    }
}

fn post(path: &str, origin: &str, json: &str) -> Req {
    Req {
        method: "POST".into(),
        path: path.into(),
        headers: BTreeMap::from([("origin".to_string(), origin.to_string())]),
        json: Some(body(json)),
    }
}

impl Req {
    fn bearer(mut self, token: &str) -> Req {
        self.headers
            .insert("authorization".into(), format!("Bearer {token}"));
        self
    }
}

/// The bytes a client signs or sends, as the JSON number array the
/// MetaNet Client's shape carries.
fn bytes_array(bytes: &[u8]) -> String {
    serde_json::to_string(bytes).unwrap()
}

async fn record_all(r: &mut Recorder) {
    record_low_fleet(r).await;
    record_stack_witness(r).await;
    record_x402_skill(r).await;
    record_mcp(r).await;
    record_app_payers(r).await;
    record_dolphinmilk(r).await;
    record_e2e_harness(r).await;
}

/// LOW's fleet: its app's `WalletClient` over @bsv/sdk 2.1.4's
/// `HTTPWalletJSON` (POST for every call, the args as JSON), served to a
/// seat page whose origin the browser sets; its e2e route adds
/// `allowPending: "received"` to a seat's `/createAction` in transit; its
/// harness's own fetch carries the bearer. The scoped word is #63's text
/// (no LOW code sends it yet). LOW's funder drives the CLI (`--json`),
/// which `docs/DEPENDENTS.md` lists and this test does not reach.
async fn record_low_fleet(r: &mut Recorder) {
    const C: &str = "low-fleet";
    const O: &str = "http://localhost:5173";
    r.case(Call {
        consumer: C,
        call: "isAuthenticated",
        source: "app/src/components/ConnectGate.tsx (WalletClient over HTTPWalletJSON)",
        world: "pending",
        request: post("/isAuthenticated", O, "{}"),
    })
    .await;
    r.case(Call {
        consumer: C,
        call: "getVersion",
        source: "app/src/components/ConnectGate.tsx (WalletClient over HTTPWalletJSON)",
        world: "pending",
        request: post("/getVersion", O, "{}"),
    })
    .await;
    r.case(Call {
        consumer: C,
        call: "getPublicKey.identity",
        source: "app/src/components/ConnectGate.tsx",
        world: "pending",
        request: post("/getPublicKey", O, r#"{"identityKey":true}"#),
    })
    .await;
    r.case(Call {
        consumer: C,
        call: "getPublicKey.brc29Self",
        source: "app/src/lib/stake.ts (the payout key, forSelf)",
        world: "pending",
        request: post(
            "/getPublicKey",
            O,
            r#"{"protocolID":[2,"3241645161d8"],"counterparty":"self","keyID":"SfKxPIJNgdI= NaGLC6fMH50=","forSelf":true}"#,
        ),
    })
    .await;
    r.case(Call {
        consumer: C,
        call: "createSignature.hashToDirectlySign",
        source: "app/src/lib/stake.ts signStakeDigest (STAKE_PROTOCOL)",
        world: "pending",
        request: post(
            "/createSignature",
            O,
            &format!(
                r#"{{"protocolID":[2,"low escrow settle"],"keyID":"game-1","counterparty":"{G}","hashToDirectlySign":{}}}"#,
                bytes_array(&[7u8; 32])
            ),
        ),
    })
    .await;
    r.case(Call {
        consumer: C,
        call: "createHmac.handSecret",
        source: "app/src/lib/handSecrets.ts deriveHandSecrets",
        world: "pending",
        request: post(
            "/createHmac",
            O,
            &format!(
                r#"{{"protocolID":[2,"low hand secret"],"keyID":"game-1:{G}:v1","data":{},"counterparty":"self"}}"#,
                bytes_array(b"low-hand-secret:v1:game-1")
            ),
        ),
    })
    .await;
    r.case(Call {
        consumer: C,
        call: "listActions.hopLedger",
        source: "app/src/lib/stake.ts fetchWalletHopLedger",
        world: "pending",
        request: post(
            "/listActions",
            O,
            r#"{"labels":[],"includeLabels":true,"limit":10000,"offset":0}"#,
        ),
    })
    .await;
    // The app's own spend: no word, so every received unproven coin is held.
    r.case(Call {
        consumer: C,
        call: "createAction.stake",
        source: "app/src/lib/stake.ts (the escrow funding)",
        world: "pending",
        request: post(
            "/createAction",
            O,
            &format!(
                r#"{{"description":"LOW stake escrow funding","outputs":[{{"lockingScript":"{PAYEE}","satoshis":2300,"outputDescription":"LOW stake hop"}}],"options":{{"randomizeOutputs":false,"acceptDelayedBroadcast":false}}}}"#
            ),
        ),
    })
    .await;
    // The same body with the e2e route's word: a received coin is offered
    // and chosen, and the build stops at signing (see [`P2PKH`]): the
    // answer is past the gate, where without the word it is 402.
    r.case(Call {
        consumer: C,
        call: "createAction.allowPending",
        source: "app/e2e/lib/allowPendingRoute.ts withAllowPending",
        world: "pending",
        request: post(
            "/createAction",
            O,
            &format!(
                r#"{{"description":"LOW stake escrow funding","outputs":[{{"lockingScript":"{PAYEE}","satoshis":2300,"outputDescription":"LOW stake hop"}}],"options":{{"randomizeOutputs":false,"acceptDelayedBroadcast":false}},"allowPending":"received"}}"#
            ),
        ),
    })
    .await;
    // #63's scoped word: only the funder's top-up (PENDING_B) is offered.
    r.case(Call {
        consumer: C,
        call: "createAction.scoped",
        source: "bsv-wallet-cli #63 (LOW's fleet, loop 46, pair 32): allowPending {received: [txid:vout]}",
        world: "pending",
        request: post(
            "/createAction",
            O,
            &format!(
                r#"{{"description":"LOW stake escrow funding","outputs":[{{"lockingScript":"{PAYEE}","satoshis":2300,"outputDescription":"LOW stake hop"}}],"options":{{"randomizeOutputs":false,"acceptDelayedBroadcast":false}},"allowPending":{{"received":["{PENDING_B}:0"]}}}}"#
            ),
        ),
    })
    .await;
    r.case(Call {
        consumer: C,
        call: "internalizeAction.potPayout",
        source: "app/src/lib/stake.ts (the pot payout)",
        world: "pending",
        request: post(
            "/internalizeAction",
            O,
            &format!(
                r#"{{"tx":{},"outputs":[{{"outputIndex":0,"protocol":"wallet payment","paymentRemittance":{{"derivationPrefix":"SfKxPIJNgdI=","derivationSuffix":"NaGLC6fMH50=","senderIdentityKey":"{G}"}}}}],"description":"LOW pot payout"}}"#,
                bytes_array(&[1, 1, 1, 1, 0, 0, 0, 0])
            ),
        ),
    })
    .await;
    r.case(Call {
        consumer: C,
        call: "getPublicKey.harnessBearer",
        source: "app/e2e/lib/cli.ts (fetch with authorization when AUTH_TOKEN is set)",
        world: "auth",
        request: post(
            "/getPublicKey",
            "http://localhost",
            &format!(
                r#"{{"protocolID":[2,"3241645161d8"],"keyID":"SfKxPIJNgdI= NaGLC6fMH50=","counterparty":"{G}","forSelf":true}}"#
            ),
        )
        .bearer(BEARER),
    })
    .await;
}

/// The stack's witness spend wallet: its daily job drives the CLI
/// (`address`, `--json split --count 2`, `balance`); its one read over
/// HTTP is the tracker's word for the spend, a plain GET on loopback.
async fn record_stack_witness(r: &mut Recorder) {
    r.case(Call {
        consumer: "stack-witness",
        call: "txWord",
        source: "bsv-stack-lean docs/p0/witness-spend.md (GET /tx/{txid} on loopback)",
        world: "fresh",
        request: Req {
            method: "GET".into(),
            path: format!("/tx/{UNKNOWN}"),
            headers: BTreeMap::new(),
            json: None,
        },
    })
    .await;
}

/// The apps' payers: ~/bsv/agents' `tools/fleet/proof.py` (the x402
/// helper's client, and a `noSend` spend it aborts), Zanaadu's and
/// teragunv2's `WalletClient` over `HTTPWalletJSON` (a browser origin).
async fn record_app_payers(r: &mut Recorder) {
    const C: &str = "app-payers";
    r.case(Call {
        consumer: C,
        call: "agents.createAction.noSend",
        source: "agents tools/fleet/proof.py (/createAction, noSend)",
        world: "fresh",
        request: post(
            "/createAction",
            "http://localhost",
            &format!(
                r#"{{"description":"fleet proof","outputs":[{{"lockingScript":"{PAYEE}","satoshis":1,"outputDescription":"fleet proof"}}],"options":{{"noSend":true,"randomizeOutputs":false,"acceptDelayedBroadcast":false}}}}"#
            ),
        ),
    })
    .await;
    r.case(Call {
        consumer: C,
        call: "agents.abortAction",
        source: "agents tools/fleet/proof.py (abortAction {reference: txid})",
        world: "fresh",
        request: post(
            "/abortAction",
            "http://localhost",
            &format!(r#"{{"reference":"{UNKNOWN}"}}"#),
        ),
    })
    .await;
    r.case(Call {
        consumer: C,
        call: "zanaadu.getPublicKey.identity",
        source: "zanaadu frontend/src/lib/zanaaduSigningProtocols.ts ([2,'zanaadu identity'] '1')",
        world: "fresh",
        request: post(
            "/getPublicKey",
            "http://localhost:5173",
            r#"{"protocolID":[2,"zanaadu identity"],"keyID":"1","counterparty":"self"}"#,
        ),
    })
    .await;
    r.case(Call {
        consumer: C,
        call: "zanaadu.createAction.fund",
        source: "zanaadu frontend/src/lib/features-fund/fund.ts",
        world: "fresh",
        request: post(
            "/createAction",
            "http://localhost:5173",
            &format!(
                r#"{{"description":"Zanaadu fund","outputs":[{{"lockingScript":"{PAYEE}","satoshis":1000,"outputDescription":"Zanaadu fund output"}}],"options":{{"randomizeOutputs":false,"acceptDelayedBroadcast":false}}}}"#
            ),
        ),
    })
    .await;
    r.case(Call {
        consumer: C,
        call: "teragunv2.getPublicKey.payment",
        source: "teragunv2 frontend/src/lib/wallet.ts (PAYMENT_PROTOCOL [2,'wallet payment'])",
        world: "fresh",
        request: post(
            "/getPublicKey",
            "http://localhost:5173",
            &format!(
                r#"{{"protocolID":[2,"wallet payment"],"keyID":"SfKxPIJNgdI= NaGLC6fMH50=","counterparty":"{G}","forSelf":false}}"#
            ),
        ),
    })
    .await;
    r.case(Call {
        consumer: C,
        call: "teragunv2.createAction.pack",
        source: "teragunv2 frontend/src/lib/wallet.ts (options: acceptDelayedBroadcast only)",
        world: "fresh",
        request: post(
            "/createAction",
            "http://localhost:5173",
            &format!(
                r#"{{"outputs":[{{"satoshis":1000,"lockingScript":"{PAYEE}","outputDescription":"API payment"}}],"description":"Teragun 10 TX Pack (1,000 sats)","options":{{"acceptDelayedBroadcast":false}}}}"#
            ),
        ),
    })
    .await;
}

/// dolphinmilk (`src/wallet/http.rs`): the 0.1.23 crate's server
/// in-process, its own reqwest client (`Origin` from its config, the
/// status calls on GET).
async fn record_dolphinmilk(r: &mut Recorder) {
    const C: &str = "dolphinmilk";
    const O: &str = "http://localhost";
    r.case(Call {
        consumer: C,
        call: "getVersion",
        source: "dolphinmilk src/wallet/http.rs call_get",
        world: "pending",
        request: get("/getVersion", O),
    })
    .await;
    r.case(Call {
        consumer: C,
        call: "listOutputs.balance",
        source: "dolphinmilk src/wallet/http.rs (the balance pages)",
        world: "pending",
        request: post(
            "/listOutputs",
            O,
            r#"{"basket":"default","include":"locking scripts","limit":100,"offset":0}"#,
        ),
    })
    .await;
    r.case(Call {
        consumer: C,
        call: "createAction.x402",
        source: "dolphinmilk bsv-x402-server/src/payment.rs -> src/wallet/http.rs create_action",
        world: "pending",
        request: post(
            "/createAction",
            O,
            &format!(
                r#"{{"description":"x402 payment","outputs":[{{"lockingScript":"{PAYEE}","satoshis":100,"outputDescription":"x402 payment"}}],"options":{{"acceptDelayedBroadcast":false,"randomizeOutputs":false}}}}"#
            ),
        ),
    })
    .await;
}

/// The x402 skill (calgooon-x402 1.1.1): every call goes through
/// `lib/metanet.py`'s `_call` (POST, `Origin: http://localhost`, JSON),
/// and `SKILL.md`'s health check is a plain GET.
async fn record_x402_skill(r: &mut Recorder) {
    const C: &str = "x402-skill";
    const O: &str = "http://localhost";
    r.case(Call {
        consumer: C,
        call: "isAuthenticated",
        source: "skills/x402/SKILL.md (curl -H \"Origin: http://localhost\" .../isAuthenticated)",
        world: "fresh",
        request: get("/isAuthenticated", O),
    })
    .await;
    r.case(Call {
        consumer: C,
        call: "getPublicKey.identity",
        source: "skills/x402/lib/metanet.py get_identity_key",
        world: "fresh",
        request: post("/getPublicKey", O, r#"{"identityKey":true}"#),
    })
    .await;
    // BRC-29: the payment key for the server, keyID "<prefix> <suffix>".
    r.case(Call {
        consumer: C,
        call: "getPublicKey.payment",
        source: "skills/x402/lib/payment.py (get_public_key, PAYMENT_PROTOCOL)",
        world: "fresh",
        request: post(
            "/getPublicKey",
            O,
            &format!(
                r#"{{"protocolID":[2,"3241645161d8"],"keyID":"SfKxPIJNgdI= NaGLC6fMH50=","counterparty":"{G}","forSelf":false}}"#
            ),
        ),
    })
    .await;
    // BRC-31: the auth message signature, keyID "<nonce> <server nonce>".
    r.case(Call {
        consumer: C,
        call: "createSignature.auth",
        source: "skills/x402/lib/auth_request.py (create_signature, AUTH_PROTOCOL)",
        world: "fresh",
        request: post(
            "/createSignature",
            O,
            &format!(
                r#"{{"data":{},"protocolID":[2,"auth message signature"],"keyID":"bm9uY2Ux bm9uY2Uy","counterparty":"{G}"}}"#,
                bytes_array(b"x402 auth message")
            ),
        ),
    })
    .await;
    r.case(Call {
        consumer: C,
        call: "createAction.payment",
        source: "skills/x402/lib/payment.py -> lib/metanet.py create_action",
        world: "fresh",
        request: post(
            "/createAction",
            O,
            &format!(
                r#"{{"description":"BRC-29 payment: 1000 sats","outputs":[{{"lockingScript":"{PAYEE}","satoshis":1000,"outputDescription":"BRC-29 payment output"}}],"options":{{"acceptDelayedBroadcast":false,"randomizeOutputs":false}}}}"#
            ),
        ),
    })
    .await;
    r.case(Call {
        consumer: C,
        call: "createAction.executeAction",
        source: "skills/x402/scripts/brc31_helpers.py execute-action",
        world: "fresh",
        request: post(
            "/createAction",
            O,
            &format!(
                r#"{{"description":"Agent action","outputs":[{{"lockingScript":"{PAYEE}","satoshis":1,"outputDescription":"agent output"}}],"options":{{}},"labels":["app:x402"]}}"#
            ),
        ),
    })
    .await;
    // A refund: the server's AtomicBEEF as a number array; these bytes
    // are not one (the shape is the call's, the refusal the parser's).
    r.case(Call {
        consumer: C,
        call: "internalizeAction.refund",
        source: "skills/x402/lib/refund.py -> lib/metanet.py internalize_action",
        world: "fresh",
        request: post(
            "/internalizeAction",
            O,
            &format!(
                r#"{{"tx":{},"outputs":[{{"outputIndex":0,"protocol":"wallet payment","paymentRemittance":{{"derivationPrefix":"SfKxPIJNgdI=","derivationSuffix":"NaGLC6fMH50=","senderIdentityKey":"{G}"}}}}],"description":"Refund: 1000 sats (txid: 0000000000000000...)"}}"#,
                bytes_array(&[1, 1, 1, 1, 0, 0, 0, 0])
            ),
        ),
    })
    .await;
}

/// The MCP companion (`mcp/src/main.rs`, bsv-wallet-mcp 0.3.0): each tool
/// posts its typed parameters, serialized in struct order, with `Origin:
/// http://localhost` (`WALLET_ORIGIN`'s default); the status tools GET.
async fn record_mcp(r: &mut Recorder) {
    const C: &str = "mcp";
    const O: &str = "http://localhost";
    r.case(Call {
        consumer: C,
        call: "is_authenticated",
        source: "mcp/src/main.rs is_authenticated (GET)",
        world: "fresh",
        request: get("/isAuthenticated", O),
    })
    .await;
    r.case(Call {
        consumer: C,
        call: "get_height",
        source: "mcp/src/main.rs get_height (GET)",
        world: "fresh",
        request: get("/getHeight", O),
    })
    .await;
    r.case(Call {
        consumer: C,
        call: "get_public_key.identity",
        source: "mcp/src/main.rs GetPublicKeyParams",
        world: "fresh",
        request: post("/getPublicKey", O, r#"{"identityKey":true}"#),
    })
    .await;
    r.case(Call {
        consumer: C,
        call: "create_signature",
        source: "mcp/src/main.rs CreateSignatureParams",
        world: "fresh",
        request: post(
            "/createSignature",
            O,
            &format!(
                r#"{{"data":{},"protocolID":[2,"mcp test"],"keyID":"1","counterparty":"self"}}"#,
                bytes_array(b"mcp")
            ),
        ),
    })
    .await;
    // `wallet_balance`: the default basket a page at a time.
    r.case(Call {
        consumer: C,
        call: "wallet_balance",
        source: "mcp/src/main.rs wallet_balance (listOutputs pages)",
        world: "pending",
        request: post(
            "/listOutputs",
            O,
            r#"{"basket":"default","include":"locking scripts","limit":100,"offset":0}"#,
        ),
    })
    .await;
    r.case(Call {
        consumer: C,
        call: "list_actions",
        source: "mcp/src/main.rs ListActionsParams (its defaults)",
        world: "fresh",
        request: post(
            "/listActions",
            O,
            r#"{"labels":[],"labelQueryMode":"any","includeLabels":false,"includeInputs":false,"includeOutputs":false,"limit":10,"offset":0}"#,
        ),
    })
    .await;
    // `allowPending` is a string in the MCP's parameters: the plain word.
    r.case(Call {
        consumer: C,
        call: "create_action.allowPending",
        source: "mcp/src/main.rs CreateActionParams",
        world: "pending",
        request: post(
            "/createAction",
            O,
            &format!(
                r#"{{"description":"mcp payment","outputs":[{{"lockingScript":"{PAYEE}","satoshis":60000,"outputDescription":"mcp output"}}],"allowPending":"received"}}"#
            ),
        ),
    })
    .await;
    r.case(Call {
        consumer: C,
        call: "create_action",
        source: "mcp/src/main.rs CreateActionParams",
        world: "pending",
        request: post(
            "/createAction",
            O,
            &format!(
                r#"{{"description":"mcp payment","outputs":[{{"lockingScript":"{PAYEE}","satoshis":1000,"outputDescription":"mcp output"}}],"labels":["mcp"],"options":{{"randomizeOutputs":false}}}}"#
            ),
        ),
    })
    .await;
    r.case(Call {
        consumer: C,
        call: "relinquish_output",
        source: "mcp/src/main.rs RelinquishOutputParams",
        world: "fresh",
        request: post(
            "/relinquishOutput",
            O,
            &format!(r#"{{"basket":"default","output":{{"txid":"{UNKNOWN}","vout":0}}}}"#),
        ),
    })
    .await;
    r.case(Call {
        consumer: C,
        call: "chain_feed_status",
        source: "mcp/src/main.rs chain_feed_status (GET /chain-feed)",
        world: "fresh",
        request: get("/chain-feed", O),
    })
    .await;
    r.case(Call {
        consumer: C,
        call: "tx_word",
        source: "mcp/src/main.rs tx_word (GET /tx/{txid})",
        world: "fresh",
        request: get(&format!("/tx/{UNKNOWN}"), O),
    })
    .await;
}

/// The e2e harness's client (`tests/e2e/lib/wallet-client.js`), `Origin:
/// http://e2e-test`: its own spellings (`protocolId`, `keyId`,
/// `certificateType`) are part of what the server accepts.
async fn record_e2e_harness(r: &mut Recorder) {
    const C: &str = "e2e-harness";
    const O: &str = "http://e2e-test";
    r.case(Call {
        consumer: C,
        call: "getPublicKey.brc29",
        source: "tests/e2e/lib/wallet-client.js getPublicKey (transferTo)",
        world: "fresh",
        request: post(
            "/getPublicKey",
            O,
            &format!(
                r#"{{"protocolId":[2,"3241645161d8"],"keyId":"SfKxPIJNgdI= NaGLC6fMH50=","counterparty":"{G}","forSelf":true}}"#
            ),
        ),
    })
    .await;
    r.case(Call {
        consumer: C,
        call: "createSignature",
        source: "tests/e2e/lib/wallet-client.js createSignature",
        world: "fresh",
        request: post(
            "/createSignature",
            O,
            &format!(
                r#"{{"data":{},"protocolId":[2,"e2e test"],"keyId":"1","counterparty":"self"}}"#,
                bytes_array(b"e2e")
            ),
        ),
    })
    .await;
    r.case(Call {
        consumer: C,
        call: "listOutputs.balance",
        source: "tests/e2e/lib/wallet-client.js balance",
        world: "pending",
        request: post(
            "/listOutputs",
            O,
            r#"{"basket":"default","limit":10000,"includeEnvelope":false}"#,
        ),
    })
    .await;
    // `transferTo` under the word every test wallet's spend names.
    r.case(Call {
        consumer: C,
        call: "createAction.transferTo",
        source: "tests/e2e/lib/wallet-client.js createAction (transferTo)",
        world: "pending",
        request: post(
            "/createAction",
            O,
            &format!(
                r#"{{"outputs":[{{"lockingScript":"{PAYEE}","satoshis":60000,"outputDescription":"e2e transfer","tags":["e2e-test"]}}],"description":"e2e transfer","allowPending":"received","options":{{"signAndProcess":true,"acceptDelayedBroadcast":false,"randomizeOutputs":false,"noSend":false}}}}"#
            ),
        ),
    })
    .await;
    // Scenario 23's scoped word: only PENDING_A is offered.
    r.case(Call {
        consumer: C,
        call: "createAction.scoped",
        source: "tests/e2e/lib/wallet-client.js createAction (opts.allowPending, scenario 23)",
        world: "pending",
        request: post(
            "/createAction",
            O,
            &format!(
                r#"{{"outputs":[{{"lockingScript":"{PAYEE}","satoshis":40000,"outputDescription":"e2e transfer","tags":["e2e-test"]}}],"description":"e2e transfer","allowPending":{{"received":["{PENDING_A}:0"]}},"options":{{"signAndProcess":true,"acceptDelayedBroadcast":false,"randomizeOutputs":false,"noSend":false}}}}"#
            ),
        ),
    })
    .await;
    r.case(Call {
        consumer: C,
        call: "relinquishOutput",
        source: "tests/e2e/lib/wallet-client.js relinquishOutput",
        world: "fresh",
        request: post(
            "/relinquishOutput",
            O,
            &format!(r#"{{"txid":"{UNKNOWN}","vout":0,"basket":"default"}}"#),
        ),
    })
    .await;
    r.case(Call {
        consumer: C,
        call: "isAuthenticated.bearer",
        source: "tests/e2e/lib/wallet-client.js get (a wallet behind AUTH_TOKEN)",
        world: "auth",
        request: get("/isAuthenticated", O).bearer(BEARER),
    })
    .await;
}

// ---------------------------------------------------------------------------
// Record or replay.
// ---------------------------------------------------------------------------

fn consumers_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/wire/consumers")
}

/// Every fixture, by consumer directory then file name.
fn read_fixtures(dir: &Path) -> Vec<(String, Fixture)> {
    let mut files: Vec<PathBuf> = Vec::new();
    for entry in std::fs::read_dir(dir).unwrap_or_else(|e| panic!("{}: {e}", dir.display())) {
        let sub = entry.unwrap().path();
        if !sub.is_dir() {
            continue;
        }
        for file in std::fs::read_dir(&sub).unwrap() {
            let path = file.unwrap().path();
            if path.extension().is_some_and(|x| x == "json") {
                files.push(path);
            }
        }
    }
    files.sort();
    files
        .into_iter()
        .map(|p| {
            let text = std::fs::read_to_string(&p).unwrap();
            let fixture: Fixture =
                serde_json::from_str(&text).unwrap_or_else(|e| panic!("{}: {e}", p.display()));
            let name = p.strip_prefix(dir).unwrap().to_string_lossy().into_owned();
            (name, fixture)
        })
        .collect()
}

/// A line diff of two pretty renderings (the fixture's, the served).
fn diff(want: &str, got: &str) -> String {
    let (w, g): (Vec<&str>, Vec<&str>) = (want.lines().collect(), got.lines().collect());
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

/// What the corpus must keep: a fixture for every consumer, every fixture
/// a known consumer's, in its own directory.
fn judge(fixtures: &[(String, Fixture)]) -> Vec<String> {
    let mut gaps = Vec::new();
    let seen: BTreeSet<&str> = fixtures.iter().map(|(_, f)| f.consumer.as_str()).collect();
    for consumer in CONSUMERS {
        if !seen.contains(consumer) {
            gaps.push(format!("consumer {consumer}: no fixture"));
        }
    }
    for (file, f) in fixtures {
        if !CONSUMERS.contains(&f.consumer.as_str()) {
            gaps.push(format!("{file}: unknown consumer {}", f.consumer));
        }
        if !file.starts_with(&format!("{}/", f.consumer)) {
            gaps.push(format!("{file}: not under {}/", f.consumer));
        }
    }
    gaps
}

/// The router's futures are deep in a debug build: the run gets its own
/// stacks.
#[test]
fn wire_consumers() {
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
    let dir = consumers_dir();
    if std::env::var("WIRE_RECORD").is_ok_and(|v| v == "1") {
        if dir.exists() {
            std::fs::remove_dir_all(&dir).unwrap();
        }
        std::fs::create_dir_all(&dir).unwrap();
        let mut recorder = Recorder {
            dir: dir.clone(),
            worlds: HashMap::new(),
            seq: HashMap::new(),
            written: 0,
        };
        record_all(&mut recorder).await;
        println!("recorded {} consumer fixtures", recorder.written);
    }

    let fixtures = read_fixtures(&dir);
    assert!(!fixtures.is_empty(), "no fixture under {}", dir.display());
    let mut worlds: HashMap<(String, String), World> = HashMap::new();
    let mut failures = Vec::new();
    let mut count: BTreeMap<&str, usize> = BTreeMap::new();
    for (file, f) in &fixtures {
        *count.entry(f.consumer.as_str()).or_default() += 1;
        let key = (f.consumer.clone(), f.world.clone());
        if !worlds.contains_key(&key) {
            worlds.insert(key.clone(), World::build(&f.world).await);
        }
        let got = worlds[&key].serve(&f.request).await;
        if got != f.response {
            let want = serde_json::to_string_pretty(&f.response).unwrap();
            let served = serde_json::to_string_pretty(&got).unwrap();
            failures.push(format!(
                "consumer {} would feel this: {file} ({}, sent by {}): {} {} answered differently (- fixture, + served)\n{}",
                f.consumer,
                f.call,
                f.source,
                f.request.method,
                f.request.path,
                diff(&want, &served)
            ));
        }
    }
    let gaps = judge(&fixtures);
    println!(
        "{} consumer fixtures replayed ({}), {} differ, {} gaps",
        fixtures.len(),
        count
            .iter()
            .map(|(c, n)| format!("{c} {n}"))
            .collect::<Vec<_>>()
            .join(", "),
        failures.len(),
        gaps.len()
    );
    assert!(
        failures.is_empty() && gaps.is_empty(),
        "\n{}\n{}\n(a change on purpose: WIRE_RECORD=1 cargo test --test wire_consumers, in its own commit, and tell the consumer's captain; see docs/DEPENDENTS.md)",
        failures.join("\n"),
        gaps.join("\n")
    );
}
