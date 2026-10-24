//! The wire oracle (#69): every served call and every refusal body, as a
//! recorded request and the exact response, under `tests/wire/`.
//!
//! `cargo test --test wire` replays every fixture against the router,
//! in-process (`tower::ServiceExt::oneshot`, no socket for the wallet), and
//! fails on any byte of difference after normalization, printing the diff.
//! `WIRE_RECORD=1 cargo test --test wire` re-records every fixture from the
//! cases below and the manifest `tests/wire/MANIFEST.json` (the recorded
//! set), then fails with the word to replay; nothing else re-records. See
//! `docs/WIRE.md`.
//!
//! The field names the shapes must keep are BRC-100's at the line's pin,
//! bsv-blockchain/BRCs `ed1b015` (2026-10-05, the stack's
//! docs/PROVENANCE.md). Upstream master `5119a9b` (2026-10-19) differs for
//! `listOutputs` (`totalOutputs` may be a string, `spendable` may be false,
//! for BRC-112 and BRC-207 module views) and for `internalizeAction` (an
//! insertion's `customInstructions` bounded at 1000 bytes): upstream, not
//! on the line.
//!
//! Each fixture names its world (the wallet and the server config it is
//! served from). A world is built once per run from its name alone, and the
//! fixtures of one world run in their file order, so a fixture may read the
//! state an earlier one of its world left (a certificate acquired, then
//! listed). Nothing reaches the network: the header service is a local
//! stub, the broadcaster a closed local port (in `funded`, a local stub
//! that takes every post), and every other request goes through a proxy on
//! a closed local port.

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

    /// The value at a JSON pointer (`/a/0/b`), as RFC 6901 reads it.
    fn pointer(&self, pointer: &str) -> Option<&J> {
        pointer
            .strip_prefix('/')
            .map(|p| p.split('/'))
            .into_iter()
            .flatten()
            .try_fold(self, |at, step| match at {
                J::Obj(fields) => fields.iter().find(|(k, _)| k == step).map(|(_, v)| v),
                J::Arr(items) => step.parse::<usize>().ok().and_then(|i| items.get(i)),
                _ => None,
            })
    }

    /// Replace the value at a JSON pointer; the path must exist.
    fn set_pointer(&mut self, pointer: &str, value: J) {
        let Some(path) = pointer.strip_prefix('/') else {
            *self = value;
            return;
        };
        let mut at = self;
        for step in path.split('/') {
            at = match at {
                J::Obj(fields) => fields
                    .iter_mut()
                    .find_map(|(k, v)| (k == step).then_some(v))
                    .unwrap_or_else(|| panic!("no `{step}` on the way to {pointer}")),
                J::Arr(items) => {
                    let i: usize = step
                        .parse()
                        .unwrap_or_else(|_| panic!("`{step}` is not an index, in {pointer}"));
                    items
                        .get_mut(i)
                        .unwrap_or_else(|| panic!("no element {i} on the way to {pointer}"))
                }
                _ => panic!("{pointer} runs through a scalar at `{step}`"),
            };
        }
        *at = value;
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
    /// A body of this many spaces (an oversized one; the file does not
    /// carry the megabyte).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    fill: Option<usize>,
    /// Values of `json` that come from an earlier fixture's served body in
    /// the same run (a reference the wallet drew at random): the JSON
    /// pointer into this request, and where the value is read from. The
    /// recorded `json` carries `<bound>` there.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    bind: BTreeMap<String, Bound>,
}

/// Where a bound request value is read from: a fixture of the same world
/// that ran earlier, and a JSON pointer into the body it was served.
#[derive(Serialize, Deserialize, Clone, PartialEq)]
struct Bound {
    file: String,
    pointer: String,
}

/// The marker written into a recorded request where a bound value goes.
const BOUND: &str = "<bound>";

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

/// The response headers that are part of the contract (the body's type, the
/// retry pause, and every CORS header, the preflight's three included).
/// Every other header is the transport's.
const KEPT_HEADERS: [&str; 9] = [
    "content-type",
    "retry-after",
    "access-control-allow-origin",
    "access-control-allow-credentials",
    "access-control-allow-methods",
    "access-control-allow-headers",
    "access-control-max-age",
    "access-control-expose-headers",
    "vary",
];

// ---------------------------------------------------------------------------
// The normalizer: the one place a value that must differ from run to run is
// replaced by a placeholder. The placeholder keeps the value's shape (its
// JSON type, a byte array's length, a string's length, a number's digit
// count), so a shape change is a byte of difference. Every rule is named in
// docs/WIRE.md.
// ---------------------------------------------------------------------------

/// Keys whose values differ on every run, anywhere in a body.
const VOLATILE_KEYS: [&str; 6] = [
    // AES-GCM under a fresh random IV: a new ciphertext each call.
    "ciphertext",
    // A key linkage revealed is encrypted for the verifier the same way.
    "encryptedLinkage",
    "encryptedLinkageProof",
    // A held coin's age, from its row's time (half an hour before the
    // world was built) to the clock's now.
    "ageMinutes",
    // The clock's now, when a linkage is revealed.
    "revelationTime",
    // The reference of a signable transaction: the toolbox draws it at
    // random.
    "reference",
];

/// The placeholder for a volatile value: its shape, never its bytes.
fn shape(v: &J) -> String {
    match v {
        J::Null => "null".to_string(),
        J::Bool(_) => "<bool>".to_string(),
        J::Num(n) => match (n.as_u64(), n.as_i64()) {
            (Some(u), _) => format!("<uint:{}>", u.to_string().len()),
            (None, Some(i)) => format!("<int:{}>", i.unsigned_abs().to_string().len()),
            _ => "<float>".to_string(),
        },
        J::Str(s) => {
            if chrono::DateTime::parse_from_rfc3339(s).is_ok() {
                "<rfc3339>".to_string()
            } else if s.len() % 2 == 0 && !s.is_empty() && s.bytes().all(|b| b.is_ascii_hexdigit())
            {
                format!("<hex:{}>", s.len())
            } else {
                format!("<string:{}>", s.chars().count())
            }
        }
        J::Arr(items) => {
            let bytes = items
                .iter()
                .all(|i| matches!(i, J::Num(n) if n.as_u64().is_some_and(|b| b <= 255)));
            if bytes {
                format!("<bytes:{}>", items.len())
            } else {
                format!("<array:{}>", items.len())
            }
        }
        J::Obj(fields) => format!("<object:{}>", fields.len()),
    }
}

fn normalize(v: J) -> J {
    match v {
        J::Obj(fields) => J::Obj(
            fields
                .into_iter()
                .map(|(k, v)| {
                    let v = if VOLATILE_KEYS.contains(&k.as_str()) && !matches!(v, J::Null) {
                        J::Str(shape(&v))
                    } else {
                        normalize(v)
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

/// Inside any string: the local stubs' ports (a fresh port each run), a
/// held coin's age in a refusal's listing (`<n> min old`), and the
/// platform's errno in a refused connection's words (`os error <n>`).
fn normalize_text(s: &str) -> String {
    let s = replace_number(s, "127.0.0.1:", "<port>", true);
    let s = replace_number(&s, "os error ", "<errno>", true);
    replace_number(&s, " min old", "<minutes>", false)
}

/// Every run of digits right after `marker` (`after`), or right before it,
/// replaced by `placeholder`.
fn replace_number(s: &str, marker: &str, placeholder: &str, after: bool) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(at) = rest.find(marker) {
        let (head, tail) = (&rest[..at], &rest[at + marker.len()..]);
        if after {
            let digits = tail.bytes().take_while(u8::is_ascii_digit).count();
            out.push_str(head);
            out.push_str(marker);
            out.push_str(if digits > 0 { placeholder } else { "" });
            rest = &tail[digits..];
        } else {
            let digits = head.bytes().rev().take_while(u8::is_ascii_digit).count();
            out.push_str(&head[..head.len() - digits]);
            out.push_str(if digits > 0 { placeholder } else { "" });
            out.push_str(marker);
            rest = tail;
        }
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

/// The names of the worlds (see [`World::build`]).
const WORLDS: [&str; 10] = [
    "fresh",
    "auth",
    "callback",
    "tracked",
    "received",
    "headers-down",
    "broken",
    "broken-host",
    "busy",
    "funded",
];

/// The `funded` world's coins: (seed of the parent's input, height mined,
/// satoshis, derivation suffix). Each parent is a one-leaf block of its own
/// at its height, so the header stub answers its txid as that height's
/// root. The amounts descend: the toolbox takes the largest offered coin
/// before the others, so the cases spend them in this order.
const FUNDED_COINS: [(u8, u32, u64, &str); 4] = [
    (0xa1, 899_901, 50_000, "QQ=="),
    (0xa2, 899_902, 40_000, "Qg=="),
    (0xa3, 899_903, 30_000, "Qw=="),
    (0xa4, 899_904, 20_000, "RA=="),
];
/// The stranger's key: fixed, so each payment's signature (RFC 6979) and
/// txid are the same on every run.
const STRANGER_KEY: &str = "2e2e2e2e2e2e2e2e2e2e2e2e2e2e2e2e2e2e2e2e2e2e2e2e2e2e2e2e2e2e2e2e";
/// The derivation of the stranger's BRC-29 payment to the wallet (the
/// `wallet payment` protocol's remittance).
const PAYMENT_PREFIX: &str = "eDQwMg==";
const PAYMENT_SUFFIX: &str = "cGF5";
/// A puzzle the caller unlocks in `signAction`'s `spends`: `OP_1
/// OP_EQUAL`, unlocked by `OP_1`. Its coin's mined parent's seed, height
/// and satoshis.
const PUZZLE_LOCK: &str = "5187";
const PUZZLE_UNLOCK: &str = "51";
const PUZZLE: (u8, u32, u64) = (0xe4, 899_908, 5_000);
/// The toolbox's fee for the puzzle's one input (one byte of unlocking
/// script) and one P2PKH output: the template asks for the puzzle less
/// this, so no change output is made.
const FEE_PUZZLE_1_OUT: u64 = 9;
/// The toolbox's fee for one P2PKH input and one output, at its rate: a
/// case asks for exactly the coin less this, so no change output is made
/// (a change output's derivation is drawn at random, and the built bytes
/// would move on every run).
const FEE_1_IN_1_OUT: u64 = 20;
/// The derivation prefix of the funded coins; BRC-29's protocol is the
/// toolbox signer's (`3241645161d8`, counterparty self for a coin with no
/// sender key).
const FUNDED_PREFIX: &str = "d2lyZQ==";
const BRC29_PROTOCOL: &str = "3241645161d8";

struct World {
    router: Router,
    /// Held for the world's life: the stubs' tasks, a temp store, the
    /// other connection holding the write lock.
    _keep: Vec<Box<dyn std::any::Any + Send>>,
}

/// A local header service: the tip at [`TIP`], a height's header with the
/// root `roots` gives it ([`RECEIVED`] for any other height); or, `down`, a
/// 500 for everything.
async fn header_stub(down: bool, roots: HashMap<u32, String>) -> String {
    use axum::extract::Query;
    use axum::routing::get;
    fn header(height: u32, root: &str) -> serde_json::Value {
        serde_json::json!({
            "status": "success",
            "value": {
                "version": 536_870_912_u32,
                "previousHash": "0".repeat(64),
                // Every seeded proof is a one-leaf block: its root is the
                // coin's txid.
                "merkleRoot": root,
                "time": 1_700_000_000_u32,
                "bits": 402_917_821_u32,
                "nonce": 7u32,
                "height": height,
                "hash": format!("{:064x}", height),
            }
        })
    }
    let roots = std::sync::Arc::new(roots);
    let app = if down {
        Router::new().fallback(|| async { axum::http::StatusCode::INTERNAL_SERVER_ERROR })
    } else {
        Router::new()
            .route(
                "/findChainTipHeaderHex",
                get(|| async { axum::Json(header(TIP, RECEIVED)) }),
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
                    let height = q.get("height").copied().unwrap_or(0);
                    let root = roots.get(&height).map_or(RECEIVED, String::as_str);
                    axum::Json(header(height, root))
                }),
            )
    };
    serve_local(app).await
}

/// Serve `app` on a fresh local port for the run's life: its base URL.
async fn serve_local(app: Router) -> String {
    use std::future::IntoFuture;
    let socket = tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
        .await
        .expect("a local port");
    let url = format!("http://{}", socket.local_addr().expect("its address"));
    tokio::spawn(axum::serve(socket, app).into_future());
    url
}

/// A local ARC that takes every post (the mock services'
/// `post_beef_success`, over the wire: the server's state holds the real
/// `Services`): `POST /v1/tx` answers `SEEN_ON_NETWORK` for the posted
/// transaction, the BEEF's subject or the raw transaction's own txid. Every
/// other request is a 404.
async fn arc_stub() -> String {
    use axum::routing::post;
    async fn take(body: axum::body::Bytes) -> axum::Json<serde_json::Value> {
        let bytes = serde_json::from_slice::<serde_json::Value>(&body)
            .ok()
            .and_then(|v| v["rawTx"].as_str().and_then(|h| hex::decode(h).ok()))
            .unwrap_or_else(|| body.to_vec());
        let txid = match bsv_sdk::transaction::Beef::from_binary(&bytes) {
            Ok(beef) => beef
                .txs
                .last()
                .map(bsv_sdk::transaction::BeefTx::txid)
                .unwrap_or_default(),
            Err(_) => bsv_sdk::transaction::Transaction::from_binary(&bytes)
                .as_ref()
                .map(bsv_sdk::transaction::Transaction::id)
                .unwrap_or_default(),
        };
        axum::Json(serde_json::json!({"txid": txid, "txStatus": "SEEN_ON_NETWORK"}))
    }
    let app = Router::new().route("/v1/tx", post(take));
    serve_local(app).await
}

/// The time of every seeded row: half an hour before the world is built,
/// so a held coin's age is two digits on any day (the normalizer keeps a
/// number's digit count).
fn row_time() -> chrono::DateTime<chrono::Utc> {
    chrono::Utc::now() - chrono::Duration::minutes(30)
}

/// A one-input, one-output transaction: the input names `[seed; 32]:0`
/// (never a real coin), the output pays `satoshis` to `lock`.
fn parent_tx(seed: u8, satoshis: u64, lock: &[u8]) -> bsv_sdk::transaction::Transaction {
    let mut tx = bsv_sdk::transaction::Transaction::new();
    tx.add_input(bsv_sdk::transaction::TransactionInput::new(
        hex::encode([seed; 32]),
        0,
    ))
    .unwrap();
    tx.add_output(bsv_sdk::transaction::TransactionOutput::new(
        satoshis,
        bsv_sdk::script::LockingScript::from_binary(lock).unwrap(),
    ))
    .unwrap();
    tx
}

/// The P2PKH lock of the wallet's BRC-29 key for `suffix` under
/// [`FUNDED_PREFIX`], as the toolbox's signer derives it.
fn funded_lock(key: &PrivateKey, suffix: &str) -> Vec<u8> {
    use bsv_sdk::wallet::{Counterparty, KeyDeriver, Protocol, SecurityLevel};
    let deriver = KeyDeriver::new(Some(key.clone()));
    let signing = deriver
        .derive_private_key(
            &Protocol::new(SecurityLevel::Counterparty, BRC29_PROTOCOL),
            &format!("{FUNDED_PREFIX} {suffix}"),
            &Counterparty::Self_,
        )
        .unwrap();
    p2pkh(&signing.public_key().hash160())
}

/// The `funded` world's coins ([`FUNDED_COINS`]): each a mined coin the
/// toolbox can carry in a BEEF (the parent's bytes stored, its one-leaf
/// proof at its height) and sign (the BRC-29 derivation on its row). The
/// roots the header stub must answer, by height.
async fn seed_funded(
    storage: &StorageSqlx,
    identity: &str,
    key: &PrivateKey,
) -> HashMap<u32, String> {
    let rows = Rows::open(storage, identity).await;
    // The basket wants no more coins than it has: the toolbox then makes
    // no change output for an action that spends a coin exactly (with the
    // default of six wanted, it adds change outputs, each with a random
    // derivation, and the built bytes move on every run).
    sqlx::query("UPDATE output_baskets SET number_of_desired_utxos = 0 WHERE basket_id = ?")
        .bind(rows.basket)
        .execute(&rows.pool)
        .await
        .unwrap();
    let mut roots = HashMap::new();
    for (seed, height, satoshis, suffix) in FUNDED_COINS {
        let lock = funded_lock(key, suffix);
        let parent = parent_tx(seed, satoshis, &lock);
        let txid = parent.id();
        roots.insert(height, txid.clone());
        let one_leaf = bsv_sdk::transaction::MerklePath::from_coinbase_txid(&txid, height);
        let proven = rows
            .proven(
                &txid,
                height,
                &txid,
                one_leaf.to_binary(),
                parent.to_binary(),
            )
            .await;
        rows.coin(Coin {
            txid: &txid,
            proven: Some(proven),
            satoshis,
            description: "a funded coin",
            raw: parent.to_binary(),
            lock,
            derivation: Some(suffix),
        })
        .await;
    }
    for payment in Payment::ALL {
        let (parent, _) = stranger_payment(payment, key).await;
        roots.insert(payment.parent_at().1, parent.id());
    }
    roots.insert(PUZZLE.1, puzzle_parent().0.id());
    roots
}

/// A seeded world's way into the store: the wallet's user, its default
/// basket, and the store's writer.
struct Rows {
    pool: sqlx::SqlitePool,
    user: i64,
    basket: i64,
}

/// A coin of the wallet's, written as the toolbox would have stored it.
struct Coin<'a> {
    txid: &'a str,
    /// Its creator's proof row, if it is mined.
    proven: Option<i64>,
    satoshis: u64,
    description: &'a str,
    /// Its creator's bytes, as stored.
    raw: Vec<u8>,
    lock: Vec<u8>,
    /// The suffix of its BRC-29 derivation under [`FUNDED_PREFIX`], for a
    /// coin the wallet signs for.
    derivation: Option<&'a str>,
}

impl Rows {
    async fn open(storage: &StorageSqlx, identity: &str) -> Rows {
        let user = storage
            .find_or_insert_user(identity)
            .await
            .unwrap()
            .0
            .user_id;
        let basket = storage.find_or_create_default_basket(user).await;
        Rows {
            pool: storage.writer().clone(),
            user,
            basket: basket.unwrap().basket_id,
        }
    }

    /// A proof row: `txid` mined at `height` in a block whose root is
    /// `root`. Its id.
    async fn proven(
        &self,
        txid: &str,
        height: u32,
        root: &str,
        path: Vec<u8>,
        raw: Vec<u8>,
    ) -> i64 {
        let now = row_time();
        sqlx::query(
            "INSERT INTO proven_txs (txid, height, idx, block_hash, merkle_root, merkle_path, raw_tx, created_at, updated_at) \
             VALUES (?, ?, 0, ?, ?, ?, ?, ?, ?)",
        )
        .bind(txid)
        .bind(i64::from(height))
        .bind(format!("{height:064x}"))
        .bind(root)
        .bind(path)
        .bind(raw)
        .bind(now)
        .bind(now)
        .execute(&self.pool)
        .await
        .unwrap()
        .last_insert_rowid()
    }

    /// A completed incoming transaction and its one output, the coin.
    async fn coin(&self, coin: Coin<'_>) {
        let now = row_time();
        let satoshis = i64::try_from(coin.satoshis).unwrap();
        let tx_row = sqlx::query(
            "INSERT INTO transactions (user_id, proven_tx_id, status, reference, is_outgoing, satoshis, version, lock_time, description, txid, raw_tx, created_at, updated_at) \
             VALUES (?, ?, 'completed', ?, 0, ?, 1, 0, ?, ?, ?, ?, ?)",
        )
        .bind(self.user)
        .bind(coin.proven)
        .bind(format!("ref-{}", &coin.txid[..8]))
        .bind(satoshis)
        .bind(coin.description)
        .bind(coin.txid)
        .bind(coin.raw)
        .bind(now)
        .bind(now)
        .execute(&self.pool)
        .await
        .unwrap()
        .last_insert_rowid();
        sqlx::query(
            "INSERT INTO outputs (user_id, transaction_id, basket_id, vout, satoshis, locking_script, txid, type, spendable, change, provided_by, purpose, output_description, derivation_prefix, derivation_suffix, created_at, updated_at) \
             VALUES (?, ?, ?, 0, ?, ?, ?, 'P2PKH', 1, 1, 'storage', 'change', 'c', ?, ?, ?, ?)",
        )
        .bind(self.user)
        .bind(tx_row)
        .bind(self.basket)
        .bind(satoshis)
        .bind(coin.lock)
        .bind(coin.txid)
        .bind(coin.derivation.map(|_| FUNDED_PREFIX))
        .bind(coin.derivation)
        .bind(now)
        .bind(now)
        .execute(&self.pool)
        .await
        .unwrap();
    }
}

/// The stranger's payments `internalizeAction` is offered in `funded`.
/// Each is a mined parent paying the stranger's P2PKH and a child spending
/// it.
#[derive(Clone, Copy)]
enum Payment {
    /// A valid spend paying 9,000 to [`P2PKH`] (a basket insertion).
    Insertion,
    /// The same, its input's unlocking script `OP_0`: a spend whose script
    /// fails (`OP_EQUALVERIFY`).
    BadUnlock,
    /// A valid spend paying 9,000 to the wallet's BRC-29 key for the
    /// stranger, [`PAYMENT_PREFIX`] and [`PAYMENT_SUFFIX`].
    Wallet,
}

impl Payment {
    const ALL: [Payment; 3] = [Payment::Insertion, Payment::BadUnlock, Payment::Wallet];

    /// The seed of the parent's input and the height it is mined at.
    fn parent_at(self) -> (u8, u32) {
        match self {
            Payment::Insertion => (0xe1, 899_905),
            Payment::BadUnlock => (0xe2, 899_906),
            Payment::Wallet => (0xe3, 899_907),
        }
    }

    /// The lock the payment's one output pays.
    fn pays_to(self, wallet: &PrivateKey) -> Vec<u8> {
        use bsv_sdk::wallet::{Counterparty, KeyDeriver, Protocol, SecurityLevel};
        match self {
            Payment::Insertion | Payment::BadUnlock => hex::decode(P2PKH).unwrap(),
            // As the payer derives it: the stranger's key, the wallet the
            // counterparty.
            Payment::Wallet => p2pkh(
                &KeyDeriver::new(Some(stranger_key()))
                    .derive_public_key(
                        &Protocol::new(SecurityLevel::Counterparty, BRC29_PROTOCOL),
                        &format!("{PAYMENT_PREFIX} {PAYMENT_SUFFIX}"),
                        &Counterparty::Other(wallet.public_key()),
                        false,
                    )
                    .unwrap()
                    .hash160(),
            ),
        }
    }
}

fn stranger_key() -> PrivateKey {
    PrivateKey::from_hex(STRANGER_KEY).unwrap()
}

/// The P2PKH lock of `hash`.
fn p2pkh(hash: &[u8]) -> Vec<u8> {
    [&[0x76, 0xa9, 0x14][..], hash, &[0x88, 0xac][..]].concat()
}

/// A stranger's payment ([`Payment`]): (its mined parent, the child's
/// `AtomicBEEF`). The spend is checked here, before it is offered: a valid
/// one verifies (its script runs true, its parent's proof at its height),
/// the bad one does not.
async fn stranger_payment(
    payment: Payment,
    wallet: &PrivateKey,
) -> (bsv_sdk::transaction::Transaction, Vec<u8>) {
    use bsv_sdk::script::templates::P2PKH as Template;
    use bsv_sdk::script::SignOutputs;
    let stranger = stranger_key();
    let (seed, at) = payment.parent_at();
    let mut parent = parent_tx(seed, 10_000, &p2pkh(&stranger.public_key().hash160()));
    parent.merkle_path = Some(bsv_sdk::transaction::MerklePath::from_coinbase_txid(
        &parent.id(),
        at,
    ));
    let mut child = bsv_sdk::transaction::Transaction::new();
    let mut input =
        bsv_sdk::transaction::TransactionInput::with_source_transaction(parent.clone(), 0);
    match payment {
        Payment::BadUnlock => {
            input.unlocking_script =
                Some(bsv_sdk::script::UnlockingScript::from_hex("00").unwrap());
        }
        Payment::Insertion | Payment::Wallet => input
            .set_unlocking_script_template(Template::unlock(&stranger, SignOutputs::All, false)),
    }
    child.add_input(input).unwrap();
    child
        .add_output(bsv_sdk::transaction::TransactionOutput::new(
            9_000,
            bsv_sdk::script::LockingScript::from_binary(&payment.pays_to(wallet)).unwrap(),
        ))
        .unwrap();
    child.sign().await.unwrap();
    let verified = child
        .verify(
            &bsv_sdk::transaction::AlwaysValidChainTracker::new(at),
            None,
        )
        .await
        .unwrap_or(false);
    assert_eq!(
        verified,
        !matches!(payment, Payment::BadUnlock),
        "the stranger's spend verifies exactly when it is meant to"
    );
    let mut beef = bsv_sdk::transaction::Beef::new();
    beef.merge_bump(parent.merkle_path.clone().unwrap());
    beef.merge_transaction(parent.clone());
    beef.merge_transaction(child.clone());
    let atomic = beef.to_binary_atomic(&child.id()).unwrap();
    (parent, atomic)
}

/// The puzzle's mined parent ([`PUZZLE`]) and the BEEF carrying it with its
/// proof (a template's `inputBEEF`).
fn puzzle_parent() -> (bsv_sdk::transaction::Transaction, Vec<u8>) {
    let (seed, at, satoshis) = PUZZLE;
    let parent = parent_tx(seed, satoshis, &hex::decode(PUZZLE_LOCK).unwrap());
    let mut beef = bsv_sdk::transaction::Beef::new();
    beef.merge_bump(bsv_sdk::transaction::MerklePath::from_coinbase_txid(
        &parent.id(),
        at,
    ));
    beef.merge_transaction(parent.clone());
    (parent, beef.to_binary())
}

/// The world's rows, written into the store as the tracker and the toolbox
/// would leave them (no transaction here was ever built or broadcast).
async fn seed(storage: &StorageSqlx, identity: &str, frozen: bool) {
    let rows = Rows::open(storage, identity).await;
    // The tracker's table, made by a read of it.
    bsv_wallet_cli::tracker_host::stored_word(storage, UNKNOWN)
        .await
        .unwrap();
    let mined = Some(i64::from(MINED_AT));
    for (txid, word, height, reask) in [
        (MINED, "mined", mined, None),
        (PENDING, "seen", None, Some("age")),
        (RECEIVED, "mined", mined, None),
    ] {
        sqlx::query("INSERT INTO tracker_states (txid, word, height, reask) VALUES (?, ?, ?, ?)")
            .bind(txid)
            .bind(word)
            .bind(height)
            .bind(reask)
            .execute(&rows.pool)
            .await
            .unwrap();
    }
    // The tracker's mined word's proof (its root only is read), and the
    // received coin's: a one-leaf block at MINED_AT, so its root is its
    // txid and the header stub answers that root. Neither stores its
    // transaction's bytes.
    let stub = vec![1u8, 0, 0, 0];
    rows.proven(MINED, MINED_AT, ROOT, vec![0u8], stub.clone())
        .await;
    let one_leaf = bsv_sdk::transaction::MerklePath::from_coinbase_txid(RECEIVED, MINED_AT);
    let received = rows
        .proven(
            RECEIVED,
            MINED_AT,
            RECEIVED,
            one_leaf.to_binary(),
            stub.clone(),
        )
        .await;
    // A received coin, its creator mined at MINED_AT; or one of ours the
    // chain feed froze, its creator unproven.
    let (txid, proven) = if frozen {
        (FROZEN, None)
    } else {
        (RECEIVED, Some(received))
    };
    rows.coin(Coin {
        txid,
        proven,
        satoshis: 50_000,
        description: "a received coin",
        raw: stub,
        lock: hex::decode(P2PKH).unwrap(),
        derivation: None,
    })
    .await;
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
    /// - `broken-host`: `fresh` with the tracker's rows unreadable (the
    ///   feed status not JSON, the states table without its columns).
    /// - `busy`: a file store whose write lock another connection holds.
    /// - `funded`: four mined coins the wallet can sign for
    ///   ([`FUNDED_COINS`]), the header stub answering the blocks of the
    ///   stranger's payments ([`Payment`]) and of the puzzle ([`PUZZLE`]),
    ///   and a broadcaster that takes every post ([`arc_stub`]).
    ///
    /// Any other name is a panic naming it.
    async fn build(name: &str) -> World {
        known_world(name);
        let mut keep: Vec<Box<dyn std::any::Any + Send>> = Vec::new();
        let key = PrivateKey::from_hex(ROOT_KEY).unwrap();
        let storage = World::store(name, &mut keep).await;
        let roots = World::fill(name, &storage, &key).await;
        let services = World::services(name, roots).await;
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
            let lock = hold_write_lock(&storage_path(&keep)).await;
            keep.push(Box::new(lock));
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

    /// The world's store, migrated: a file under a temp directory `keep`
    /// holds for `busy` (another connection must open it), else in memory.
    async fn store(name: &str, keep: &mut Vec<Box<dyn std::any::Any + Send>>) -> StorageSqlx {
        let storage = if name == "busy" {
            let tmp = tempfile::TempDir::new().unwrap();
            let path = tmp.path().join("wire.db");
            let storage = StorageSqlx::open(path.to_str().unwrap()).await.unwrap();
            keep.push(Box::new(tmp));
            storage
        } else {
            StorageSqlx::in_memory().await.unwrap()
        };
        let identity = PrivateKey::from_hex(ROOT_KEY)
            .unwrap()
            .public_key()
            .to_hex();
        storage.migrate("wire", &identity).await.unwrap();
        storage.make_available().await.unwrap();
        storage
    }

    /// The world's rows (seeded or spoiled): the roots the header stub must
    /// answer, by height.
    async fn fill(name: &str, storage: &StorageSqlx, key: &PrivateKey) -> HashMap<u32, String> {
        let identity = key.public_key().to_hex();
        match name {
            "tracked" => seed(storage, &identity, true).await,
            "received" | "headers-down" => seed(storage, &identity, false).await,
            "funded" => return seed_funded(storage, &identity, key).await,
            "broken" => {
                sqlx::query("DROP TABLE outputs")
                    .execute(storage.writer())
                    .await
                    .unwrap();
            }
            "broken-host" => {
                // The tracker's tables, made by a read, then spoiled: a feed
                // status that is not JSON, and a states table that keeps the
                // columns the host's migration checks for and none it reads.
                bsv_wallet_cli::tracker_host::stored_word(storage, UNKNOWN)
                    .await
                    .unwrap();
                for sql in [
                    "INSERT INTO tracker_meta (key, value) VALUES ('feed_status', 'not json')",
                    "DROP TABLE tracker_states",
                    "CREATE TABLE tracker_states (txid TEXT PRIMARY KEY, chain TEXT NOT NULL DEFAULT '[]', verdict TEXT, follow_up TEXT)",
                ] {
                    sqlx::query(sql).execute(storage.writer()).await.unwrap();
                }
            }
            _ => {}
        }
        HashMap::new()
    }

    /// The toolbox's services, pointed at the world's stubs.
    async fn services(name: &str, roots: HashMap<u32, String>) -> Services {
        let stub = header_stub(name == "headers-down", roots).await;
        let mut options = ServicesOptions::mainnet()
            .with_chaintracks_url(stub)
            .without_bananablocks();
        options.arc_url = if name == "funded" {
            arc_stub().await
        } else {
            "http://127.0.0.1:9".to_string()
        };
        options.arc_gorillapool_url = None;
        Services::with_options(Chain::Main, options).unwrap()
    }

    /// The normalized response, and the JSON body as served.
    async fn serve_raw(&self, req: &Req) -> (Resp, Option<J>) {
        let response = self.router.clone().oneshot(req.to_http()).await.unwrap();
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
        let mut resp = Resp {
            status,
            headers,
            json: None,
            text: None,
        };
        let Some(json) = J::parse(&bytes) else {
            resp.text = Some(normalize_text(&String::from_utf8_lossy(&bytes)));
            return (resp, None);
        };
        // The body is compact JSON in its own key order: what the fixture
        // compares is the body itself.
        assert_eq!(
            json.compact().as_bytes(),
            &bytes[..],
            "{} {}: the body is not compact JSON",
            req.method,
            req.path
        );
        resp.json = Some(normalize(json.clone()));
        (resp, Some(json))
    }
}

impl Req {
    /// The request as sent: its headers, and its body (`json` with its
    /// content type, `raw`, a `fill` of spaces, or none).
    fn to_http(&self) -> Request<Body> {
        assert!(
            self.bind.is_empty() || !self.json.as_ref().is_some_and(has_bound_marker),
            "{} {}: a bound value was not resolved before serving",
            self.method,
            self.path
        );
        let mut builder = Request::builder()
            .method(self.method.as_str())
            .uri(self.path.as_str());
        for (k, v) in &self.headers {
            builder = builder.header(k, v);
        }
        let body = match (&self.json, &self.raw, self.fill) {
            (Some(json), _, _) => {
                builder = builder.header("content-type", "application/json");
                Body::from(json.compact())
            }
            (None, Some(raw), _) => Body::from(raw.clone()),
            (None, None, Some(fill)) => Body::from(" ".repeat(fill)),
            (None, None, None) => Body::empty(),
        };
        builder.body(body).unwrap()
    }
}

/// A world's name is one of [`WORLDS`]; any other is a panic naming it.
fn known_world(name: &str) {
    assert!(
        WORLDS.contains(&name),
        "unknown world `{name}`: the worlds are {}",
        WORLDS.join(", ")
    );
}

/// Whether a request body still carries the [`BOUND`] marker anywhere.
fn has_bound_marker(v: &J) -> bool {
    match v {
        J::Str(s) => s == BOUND,
        J::Arr(items) => items.iter().any(has_bound_marker),
        J::Obj(fields) => fields.iter().any(|(_, v)| has_bound_marker(v)),
        _ => false,
    }
}

/// The request with each bound value read from the body `served` holds for
/// its file (a panic names a file not served yet or a pointer it lacks).
fn resolve(file: &str, req: &Req, served: &HashMap<String, J>) -> Req {
    let mut req = req.clone();
    for (at, from) in &req.bind {
        let body = served.get(&from.file).unwrap_or_else(|| {
            panic!(
                "{file}: binds {at} to {}, which was not served before it",
                from.file
            )
        });
        let value = body.pointer(&from.pointer).unwrap_or_else(|| {
            panic!(
                "{file}: binds {at} to {}{}, which its body does not carry",
                from.file, from.pointer
            )
        });
        req.json
            .as_mut()
            .unwrap_or_else(|| panic!("{file}: binds {at} on a request with no JSON body"))
            .set_pointer(at, value.clone());
    }
    req
}

/// Another connection to the store at `path`, holding its write lock
/// (never closed: the lock is held for as long as it is kept).
async fn hold_write_lock(path: &Path) -> sqlx::SqliteConnection {
    use sqlx::{ConnectOptions, Connection};
    let mut other = sqlx::sqlite::SqliteConnectOptions::new()
        .filename(path)
        .connect()
        .await
        .unwrap();
    sqlx::query("BEGIN IMMEDIATE")
        .execute(&mut other)
        .await
        .unwrap();
    let _ = other.ping().await;
    other
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
    /// Each file's JSON body as served (a later case may bind to it).
    served: HashMap<String, J>,
}

/// What a recorded case leaves for the cases after it.
struct Recorded {
    file: String,
    body: Option<J>,
}

impl Recorded {
    /// The value at `pointer` of the served body, as JSON.
    fn value(&self, pointer: &str) -> serde_json::Value {
        let body = self
            .body
            .as_ref()
            .unwrap_or_else(|| panic!("{}: no JSON body", self.file));
        let v = body
            .pointer(pointer)
            .unwrap_or_else(|| panic!("{}: nothing at {pointer}", self.file));
        serde_json::to_value(v).unwrap()
    }
}

impl Recorder {
    async fn world(&mut self, name: &str) -> &World {
        if !self.worlds.contains_key(name) {
            let world = World::build(name).await;
            self.worlds.insert(name.to_string(), world);
        }
        &self.worlds[name]
    }

    /// Serve `req` in `world` and write it as `<seq>-<name>.json`. A bound
    /// value is read from the earlier file's served body and sent; the
    /// file carries `<bound>` in its place, so two recordings are
    /// byte-identical.
    async fn case(&mut self, name: &str, world: &str, req: Req) -> Recorded {
        let file = format!("{:03}-{name}.json", self.seq + 1);
        let sent = resolve(&file, &req, &self.served);
        let (resp, raw) = self.world(world).await.serve_raw(&sent).await;
        self.seq += 1;
        let mut recorded = req;
        for at in recorded.bind.keys().cloned().collect::<Vec<_>>() {
            recorded
                .json
                .as_mut()
                .unwrap()
                .set_pointer(&at, J::Str(BOUND.to_string()));
        }
        let fixture = Fixture {
            world: world.to_string(),
            request: recorded,
            response: resp,
        };
        let mut text = serde_json::to_string_pretty(&fixture).unwrap();
        text.push('\n');
        std::fs::write(self.dir.join(&file), text).unwrap();
        self.written.push(file.clone());
        if let Some(body) = &raw {
            self.served.insert(file.clone(), body.clone());
        }
        Recorded { file, body: raw }
    }

    /// The manifest: every fixture written, in order.
    fn write_manifest(&self) {
        let mut text = serde_json::to_string_pretty(&Manifest {
            fixtures: self.written.clone(),
        })
        .unwrap();
        text.push('\n');
        std::fs::write(self.dir.join(MANIFEST), text).unwrap();
    }
}

fn get(path: &str) -> Req {
    Req {
        method: "GET".into(),
        path: path.into(),
        headers: BTreeMap::new(),
        json: None,
        raw: None,
        fill: None,
        bind: BTreeMap::new(),
    }
}

fn post(path: &str, body: serde_json::Value) -> Req {
    Req {
        method: "POST".into(),
        path: path.into(),
        headers: BTreeMap::from([("origin".to_string(), ORIGIN.to_string())]),
        json: Some(j(body)),
        raw: None,
        fill: None,
        bind: BTreeMap::new(),
    }
}

/// A browser's preflight for `method` on `path` from [`ORIGIN`].
fn options(path: &str, method: &str) -> Req {
    Req {
        method: "OPTIONS".into(),
        path: path.into(),
        headers: BTreeMap::from([
            ("origin".to_string(), ORIGIN.to_string()),
            (
                "access-control-request-method".to_string(),
                method.to_string(),
            ),
            (
                "access-control-request-headers".to_string(),
                "content-type, authorization".to_string(),
            ),
        ]),
        json: None,
        raw: None,
        fill: None,
        bind: BTreeMap::new(),
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
    /// A body of `n` spaces.
    fn fill(mut self, n: usize) -> Req {
        self.json = None;
        self.raw = None;
        self.fill = Some(n);
        self.headers
            .insert("content-type".into(), "application/json".into());
        self
    }
    /// The value at `at` in this request comes from `from`'s served body
    /// at `pointer`.
    fn bind(mut self, at: &str, from: &Recorded, pointer: &str) -> Req {
        self.bind.insert(
            at.to_string(),
            Bound {
                file: from.file.clone(),
                pointer: pointer.to_string(),
            },
        );
        self
    }
}

/// Every case, in order: the order is the files' (and a world's state
/// runs through it).
async fn record_all(r: &mut Recorder) {
    record_status_and_keys(r).await;
    record_signatures(r).await;
    record_crypto(r).await;
    record_linkage(r).await;
    record_malformed(r).await;
    record_empty_wallet(r).await;
    record_unknown_references(r).await;
    record_certificate_spelling(r).await;
    record_certificates(r).await;
    record_unknown_certificates_and_discovery(r).await;
    record_host_routes(r).await;
    record_arc_callback(r).await;
    record_bearer(r).await;
    record_preflights(r).await;
    record_gate_refusals(r).await;
    record_store_faults(r).await;
    record_funded_sign(r).await;
    record_funded_abort_and_broadcast(r).await;
    record_funded_relinquish_and_caller_input(r).await;
    record_payments_and_listings(r).await;
}

/// A key derivation request: `protocol` at level 2, `key_id`, counterparty
/// [`G`].
fn pk(protocol: &str, key_id: &str) -> serde_json::Value {
    serde_json::json!({"protocolID": [2, protocol], "keyID": key_id, "counterparty": G})
}

/// A certificate request's body: the certificate's type under `type_key`
/// (BRC-100's `type`, or the server's `certificateType`), then `rest`.
fn typed(type_key: &str, rest: &serde_json::Value) -> serde_json::Value {
    let mut body = serde_json::Map::new();
    body.insert(type_key.to_string(), CERT_TYPE.into());
    body.extend(rest.as_object().cloned().unwrap_or_default());
    body.into()
}

/// An `acquireCertificate` body, the `direct` protocol.
fn acquire_body(type_key: &str) -> serde_json::Value {
    let rest = serde_json::json!({"certifier": G, "acquisitionProtocol": "direct",
        "fields": {"name": "wire"}, "serialNumber": "AQID",
        "revocationOutpoint": format!("{UNKNOWN}.0"), "signature": "deadbeef",
        "keyringRevealer": "certifier", "keyringForSubject": {}});
    typed(type_key, &rest)
}

/// A `relinquishCertificate` body.
fn relinquish_body(type_key: &str) -> serde_json::Value {
    typed(
        type_key,
        &serde_json::json!({"serialNumber": "AQID", "certifier": G}),
    )
}

/// A `proveCertificate` body for a certificate the wallet does not hold.
fn prove_unheld_body(type_key: &str) -> serde_json::Value {
    let rest = serde_json::json!({"subject": G, "serialNumber": "AQID", "certifier": G,
        "revocationOutpoint": format!("{UNKNOWN}.0"), "signature": "deadbeef",
        "fields": {"name": "wire"}});
    serde_json::json!({"certificate": typed(type_key, &rest), "fieldsToReveal": ["name"], "verifier": G})
}

/// A funded coin's satoshis less the fee for spending it alone into one
/// output ([`FEE_1_IN_1_OUT`]).
fn exact(i: usize) -> u64 {
    FUNDED_COINS[i].2 - FEE_1_IN_1_OUT
}

/// Status (each served on GET and POST), and the public keys.
async fn record_status_and_keys(r: &mut Recorder) {
    use serde_json::json;
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
}

/// Signatures: made over data and over a hash, then verified.
async fn record_signatures(r: &mut Recorder) {
    use serde_json::json;
    let data = b"wire".to_vec();
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
    verify["signature"] = sig.value("/signature");
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
}

/// Encryption, decryption and HMACs.
async fn record_crypto(r: &mut Recorder) {
    use serde_json::json;
    let data = b"wire".to_vec();
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
    hmac["hmac"] = made.value("/hmac");
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
}

/// Key linkage revealed, and a counterparty key that is not one.
async fn record_linkage(r: &mut Recorder) {
    use serde_json::json;
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
}

/// The caller's malformed requests.
async fn record_malformed(r: &mut Recorder) {
    use serde_json::json;
    let data = b"wire".to_vec();
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
}

/// Actions and outputs, an empty wallet.
async fn record_empty_wallet(r: &mut Recorder) {
    use serde_json::json;
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
}

/// Actions and outputs the wallet does not know, and a BEEF that is not one.
async fn record_unknown_references(r: &mut Recorder) {
    use serde_json::json;
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
}

/// Certificates. BRC-100 names the certificate's type `type`; the
/// server's types take `certificateType` (found by this corpus, #69): the
/// BRC's spelling is recorded as served, then the server's, in order:
/// acquire, list, prove, relinquish.
async fn record_certificate_spelling(r: &mut Recorder) {
    for (path, body) in [
        ("/acquireCertificate", acquire_body("type")),
        ("/relinquishCertificate", relinquish_body("type")),
        ("/proveCertificate", prove_unheld_body("type")),
    ] {
        r.case(
            &format!("{}.brc100Type", &path[1..]),
            "fresh",
            post(path, body),
        )
        .await;
    }
}

/// A certificate acquired, then listed, proven and relinquished.
async fn record_certificates(r: &mut Recorder) {
    use serde_json::json;
    let acquired = r
        .case(
            "acquireCertificate.direct",
            "fresh",
            post("/acquireCertificate", acquire_body("certificateType")),
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
        .body
        .as_ref()
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
        post("/relinquishCertificate", relinquish_body("certificateType")),
    )
    .await;
}

/// Certificates the wallet does not hold, and discovery.
async fn record_unknown_certificates_and_discovery(r: &mut Recorder) {
    use serde_json::json;
    r.case(
        "relinquishCertificate.unknown",
        "fresh",
        post("/relinquishCertificate", relinquish_body("certificateType")),
    )
    .await;
    r.case(
        "proveCertificate.unknown",
        "fresh",
        post("/proveCertificate", prove_unheld_body("certificateType")),
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
}

/// The host routes.
async fn record_host_routes(r: &mut Recorder) {
    use serde_json::json;
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
}

/// The ARC callback: disabled, refused, and taken.
async fn record_arc_callback(r: &mut Recorder) {
    use serde_json::json;
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
    r.case(
        "arc-callback.badPayload",
        "callback",
        post("/arc-callback", json!({"hello": "wire"}))
            .without("origin")
            .header("x-callbacktoken", WIRE_CALLBACK),
    )
    .await;
    // One byte over the callback's own cap (a body of spaces; the file
    // carries the count).
    r.case(
        "arc-callback.tooLarge",
        "callback",
        post("/arc-callback", json!({}))
            .without("origin")
            .header("x-callbacktoken", WIRE_CALLBACK)
            .fill(1_000_001),
    )
    .await;
}

/// The bearer.
async fn record_bearer(r: &mut Recorder) {
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
}

/// A browser's preflight, one per route class: a status route, a
/// BRC-100 call, each host route; and under the bearer (recorded as
/// served: the bearer is checked before CORS answers, so a preflight,
/// which carries no credentials, is refused there; a known defect,
/// docs/WIRE.md).
async fn record_preflights(r: &mut Recorder) {
    for (name, path, method) in [
        ("options.isAuthenticated", "/isAuthenticated", "GET"),
        ("options.createAction", "/createAction", "POST"),
        ("options.permission-audit", "/permission-audit", "GET"),
        (
            "options.permission-audit.reset",
            "/permission-audit/reset",
            "POST",
        ),
        ("options.chain-feed", "/chain-feed", "GET"),
        ("options.arc-callback", "/arc-callback", "POST"),
    ] {
        r.case(name, "fresh", options(path, method)).await;
    }
    r.case(
        "options.tx",
        "fresh",
        options(&format!("/tx/{MINED}"), "GET"),
    )
    .await;
    r.case(
        "options.auth.createAction",
        "auth",
        options("/createAction", "POST"),
    )
    .await;
}

/// The spend gate's refusals.
async fn record_gate_refusals(r: &mut Recorder) {
    use serde_json::json;
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
    // The received coin's creator is completed (a broadcaster took it): an
    // abort by its reference is refused.
    r.case(
        "abortAction.broadcasterTook",
        "received",
        post(
            "/abortAction",
            json!({"reference": format!("ref-{}", &RECEIVED[..8])}),
        ),
    )
    .await;
}

/// A store fault, a store busy, and the host routes' faults.
async fn record_store_faults(r: &mut Recorder) {
    use serde_json::json;
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
    r.case("chain-feed.hostFault", "broken-host", get("/chain-feed"))
        .await;
    r.case("tx.hostFault", "broken-host", get(&format!("/tx/{MINED}")))
        .await;
}

/// The funded world: a 200 body from each spending or state-changing
/// call. Each action asks for exactly its coin less the fee, so the
/// toolbox builds no change output (whose derivation it draws at
/// random): the built bytes, the txid and the signature are the same
/// on every run. The template's reference is random: the cases that
/// send it back bind it to the served body.
async fn record_funded_sign(r: &mut Recorder) {
    use serde_json::json;
    r.case(
        "createAction.noSend",
        "funded",
        post(
            "/createAction",
            json!({"description": "wire no send", "outputs": [
                {"lockingScript": P2PKH, "satoshis": exact(0), "outputDescription": "an output"}],
                "options": {"noSend": true, "randomizeOutputs": false}}),
        ),
    )
    .await;
    let template = r
        .case(
            "createAction.template",
            "funded",
            post(
                "/createAction",
                json!({"description": "wire template", "outputs": [
                    {"lockingScript": P2PKH, "satoshis": exact(1), "outputDescription": "an output"}],
                    "options": {"signAndProcess": false, "randomizeOutputs": false}}),
            ),
        )
        .await;
    r.case(
        "signAction.ok",
        "funded",
        post(
            "/signAction",
            json!({"spends": {}, "reference": BOUND, "options": {"noSend": true}}),
        )
        .bind("/reference", &template, "/signableTransaction/reference"),
    )
    .await;
}

/// A template aborted, and an action in the default mode (signed and
/// posted at once) on the coin the abort released.
async fn record_funded_abort_and_broadcast(r: &mut Recorder) {
    use serde_json::json;
    let to_abort = r
        .case(
            "createAction.templateToAbort",
            "funded",
            post(
                "/createAction",
                json!({"description": "wire template to abort", "outputs": [
                    {"lockingScript": P2PKH, "satoshis": exact(2), "outputDescription": "an output"}],
                    "options": {"signAndProcess": false, "randomizeOutputs": false}}),
            ),
        )
        .await;
    r.case(
        "abortAction.ok",
        "funded",
        post("/abortAction", json!({"reference": BOUND})).bind(
            "/reference",
            &to_abort,
            "/signableTransaction/reference",
        ),
    )
    .await;
    // The default mode, as x402 asks it: signed and posted at once (the
    // broadcaster takes it), on the coin the abort released.
    r.case(
        "createAction.broadcast",
        "funded",
        post(
            "/createAction",
            json!({"description": "wire broadcast", "outputs": [
                {"lockingScript": P2PKH, "satoshis": exact(2), "outputDescription": "an output"}],
                "options": {"acceptDelayedBroadcast": false, "randomizeOutputs": false}}),
        ),
    )
    .await;
}

/// A coin relinquished, and a template over the caller's own input.
async fn record_funded_relinquish_and_caller_input(r: &mut Recorder) {
    use serde_json::json;
    let key = PrivateKey::from_hex(ROOT_KEY).unwrap();
    let (seed, _, satoshis, suffix) = FUNDED_COINS[3];
    let fourth = parent_tx(seed, satoshis, &funded_lock(&key, suffix)).id();
    r.case(
        "relinquishOutput.ok",
        "funded",
        post(
            "/relinquishOutput",
            json!({"basket": "default", "output": format!("{fourth}.0")}),
        ),
    )
    .await;
    // A template over the caller's own input (the puzzle, carried in
    // `inputBEEF`), its unlocking script supplied in `signAction`'s spends.
    let (puzzle, input_beef) = puzzle_parent();
    let puzzle_template = r
        .case(
            "createAction.callerInput",
            "funded",
            post(
                "/createAction",
                json!({"description": "wire caller input", "inputBEEF": input_beef,
                    "inputs": [{"outpoint": format!("{}.0", puzzle.id()), "unlockingScriptLength": 1,
                        "inputDescription": "the puzzle"}],
                    "outputs": [{"lockingScript": P2PKH, "satoshis": PUZZLE.2 - FEE_PUZZLE_1_OUT,
                        "outputDescription": "an output"}],
                    "options": {"signAndProcess": false, "randomizeOutputs": false}}),
            ),
        )
        .await;
    r.case(
        "signAction.spends",
        "funded",
        post(
            "/signAction",
            json!({"spends": {"0": {"unlockingScript": PUZZLE_UNLOCK}}, "reference": BOUND,
                "options": {"noSend": true}}),
        )
        .bind(
            "/reference",
            &puzzle_template,
            "/signableTransaction/reference",
        ),
    )
    .await;
    // The stranger's payments: a valid spend inserted into a basket, the
    // same with an unlocking script that fails (accepted: a known defect,
    // recorded as served, docs/WIRE.md), and a BRC-29 payment to the
    // wallet (x402's refund).
}

/// The stranger's payments, and what the funded wallet lists after.
async fn record_payments_and_listings(r: &mut Recorder) {
    use serde_json::json;
    let key = PrivateKey::from_hex(ROOT_KEY).unwrap();
    let (_, atomic) = stranger_payment(Payment::Insertion, &key).await;
    r.case(
        "internalizeAction.basketInsertion",
        "funded",
        post(
            "/internalizeAction",
            json!({"tx": atomic, "outputs": [{"outputIndex": 0, "protocol": "basket insertion",
                "insertionRemittance": {"basket": "wire"}}],
                "description": "wire stranger payment"}),
        ),
    )
    .await;
    let (_, atomic) = stranger_payment(Payment::BadUnlock, &key).await;
    r.case(
        "internalizeAction.badUnlock",
        "funded",
        post(
            "/internalizeAction",
            json!({"tx": atomic, "outputs": [{"outputIndex": 0, "protocol": "basket insertion",
                "insertionRemittance": {"basket": "wire"}}],
                "description": "wire bad unlock"}),
        ),
    )
    .await;
    let (_, atomic) = stranger_payment(Payment::Wallet, &key).await;
    r.case(
        "internalizeAction.walletPayment",
        "funded",
        post(
            "/internalizeAction",
            json!({"tx": atomic, "outputs": [{"outputIndex": 0, "protocol": "wallet payment",
                "paymentRemittance": {"derivationPrefix": PAYMENT_PREFIX, "derivationSuffix": PAYMENT_SUFFIX,
                    "senderIdentityKey": stranger_key().public_key().to_hex()}}],
                "description": "wire payment"}),
        ),
    )
    .await;
    r.case(
        "listOutputs.funded",
        "funded",
        post("/listOutputs", json!({"basket": "default"})),
    )
    .await;
    r.case(
        "listActions.funded",
        "funded",
        post("/listActions", json!({"labels": []})),
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

/// `AppError`'s codes (`src/server/handlers.rs`) some world produces, by
/// status; each must be answered at least once. With [`UNPRODUCED`] they
/// are every code the source writes ([`app_codes_in_source`]): a code added
/// there without a fixture fails `every_app_error_code_is_judged`.
const APP_CODES: [(u16, &str); 12] = [
    (400, "VALIDATION_ERROR"),
    (400, "INVALID_BEEF"),
    (400, "ERROR"),
    (402, "INSUFFICIENT_FUNDS"),
    (403, "INPUT_NOT_OFFERED"),
    (404, "NOT_FOUND"),
    (409, "UNSPENDABLE_COIN"),
    (409, "CONFLICT"),
    (500, "INTERNAL_ERROR"),
    (502, "SERVICE_ERROR"),
    (503, "STORE_BUSY"),
    (503, "HEADERS_UNAVAILABLE"),
];

/// The router's own codes (`src/server/mod.rs`), by status; each must be
/// answered at least once.
const ROUTER_CODES: [(u16, &str); 7] = [
    (401, "UNAUTHORIZED"),
    (404, "CALLBACK_DISABLED"),
    (400, "BAD_JSON"),
    (400, "BAD_PAYLOAD"),
    (413, "TOO_LARGE"),
    (500, "ERR_CHAIN_FEED"),
    (500, "ERR_TX_WORD"),
];

/// Every code the corpus judges: a fixture whose code is in neither list is
/// a judgment gap.
fn judged_codes() -> impl Iterator<Item = (u16, &'static str)> {
    APP_CODES.into_iter().chain(ROUTER_CODES)
}

/// `AppError` codes no in-process world produces, each named with why in
/// docs/WIRE.md. A fixture that answers one moves it to [`APP_CODES`].
const UNPRODUCED: [(u16, &str); 3] = [
    (401, "AUTHENTICATION_REQUIRED"),
    (403, "ACCESS_DENIED"),
    (502, "BROADCAST_REJECTED"),
];

/// The `(status, code)` pairs `src/server/handlers.rs` writes: each
/// `(StatusCode::NAME, "CODE")` of the classifier, and each `code:` of a
/// built `AppError` under the `status: StatusCode::NAME` before it (a
/// constant's path read from its module's `const NAME: &str = "..."`).
fn app_codes_in_source() -> BTreeSet<(u16, String)> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let read =
        |p: &str| std::fs::read_to_string(root.join(p)).unwrap_or_else(|e| panic!("{p}: {e}"));
    let source = read("src/server/handlers.rs");
    let status = |name: &str| -> u16 {
        match name {
            "BAD_REQUEST" => 400,
            "UNAUTHORIZED" => 401,
            "PAYMENT_REQUIRED" => 402,
            "FORBIDDEN" => 403,
            "NOT_FOUND" => 404,
            "CONFLICT" => 409,
            "INTERNAL_SERVER_ERROR" => 500,
            "BAD_GATEWAY" => 502,
            "SERVICE_UNAVAILABLE" => 503,
            other => panic!("StatusCode::{other}: not in the scanner's table"),
        }
    };
    let quoted = |s: &str| -> Option<String> {
        let s = s.trim().strip_prefix('"')?;
        Some(s[..s.find('"')?].to_string())
    };
    let mut found = BTreeSet::new();
    let mut last_status = None;
    for line in source.lines().map(str::trim) {
        if let Some(rest) = line.strip_prefix("(StatusCode::") {
            if let Some((name, code)) = rest.split_once(',') {
                if let Some(code) = quoted(code) {
                    found.insert((status(name.trim()), code));
                }
            }
        } else if let Some(rest) = line.strip_prefix("status: StatusCode::") {
            last_status = Some(status(rest.trim_end_matches(',')));
        } else if let Some(rest) = line.strip_prefix("code: ") {
            let rest = rest.trim_end_matches(',');
            let code = quoted(rest).or_else(|| {
                // `crate::module::NAME`: the constant's value in its module.
                let (module, name) = rest.strip_prefix("crate::")?.rsplit_once("::")?;
                let text = read(&format!("src/{}.rs", module.replace("::", "/")));
                let at = text.find(&format!("const {name}: &str = "))?;
                quoted(text[at..].split_once('=')?.1)
            });
            if let Some(code) = code {
                let at = last_status
                    .take()
                    .unwrap_or_else(|| panic!("`code: {rest}` with no `status:` before it"));
                found.insert((at, code));
            }
        }
    }
    found
}

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
    for (status, code) in judged_codes() {
        if !codes.contains(&(status, code.to_string())) {
            missing.push(format!("no fixture answers {status} {code}"));
        }
    }
    for (status, code) in &codes {
        if !judged_codes().any(|c| c == (*status, code.as_str())) {
            let why = if UNPRODUCED.contains(&(*status, code.as_str())) {
                "a world produces it now: move it from UNPRODUCED to APP_CODES"
            } else {
                "neither APP_CODES nor ROUTER_CODES judges it"
            };
            missing.push(format!("a fixture answers {status} {code}: {why}"));
        }
    }
    let worlds: BTreeSet<&str> = fixtures.iter().map(|(_, f)| f.world.as_str()).collect();
    for world in WORLDS {
        if !worlds.contains(world) {
            missing.push(format!("no fixture is served from the `{world}` world"));
        }
    }
    missing
}

// ---------------------------------------------------------------------------
// The manifest: the recorded set, so a fixture lost or added by hand is a
// failure, not a silent pass.
// ---------------------------------------------------------------------------

const MANIFEST: &str = "MANIFEST.json";

#[derive(Serialize, Deserialize)]
struct Manifest {
    fixtures: Vec<String>,
}

/// Whether a file's name is a fixture's: a `.json` file.
fn is_fixture_name(name: &str) -> bool {
    Path::new(name).extension().is_some_and(|x| x == "json")
}

/// The fixtures the manifest lists, in its order, after judging the
/// directory against it: every listed file present, every file on disk
/// listed, and nothing but `.json` fixtures beside the manifest.
fn judge_manifest(dir: &Path) -> Result<Vec<String>, Vec<String>> {
    let mut faults = Vec::new();
    let manifest_path = dir.join(MANIFEST);
    let manifest: Manifest = match std::fs::read_to_string(&manifest_path) {
        Ok(text) => match serde_json::from_str(&text) {
            Ok(m) => m,
            Err(e) => return Err(vec![format!("{}: {e}", manifest_path.display())]),
        },
        Err(e) => return Err(vec![format!("{}: {e}", manifest_path.display())]),
    };
    let listed: BTreeSet<&str> = manifest.fixtures.iter().map(String::as_str).collect();
    let on_disk: BTreeSet<String> = std::fs::read_dir(dir)
        .unwrap_or_else(|e| panic!("{}: {e}", dir.display()))
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .filter(|name| name != MANIFEST)
        .collect();
    for name in &manifest.fixtures {
        if !is_fixture_name(name) {
            faults.push(format!("{name}: listed, not a .json fixture"));
        }
        if !on_disk.contains(name) {
            faults.push(format!("{name}: listed in {MANIFEST}, not on disk"));
        }
    }
    for name in &on_disk {
        if !is_fixture_name(name) {
            faults.push(format!("{name}: on disk, not a .json fixture"));
        } else if !listed.contains(name.as_str()) {
            faults.push(format!("{name}: on disk, not listed in {MANIFEST}"));
        }
    }
    if faults.is_empty() {
        Ok(manifest.fixtures)
    } else {
        Err(faults)
    }
}

// ---------------------------------------------------------------------------
// Record or replay.
// ---------------------------------------------------------------------------

fn wire_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/wire")
}

/// The fixtures the manifest names, in name order; a fixture of an unknown
/// world is a panic naming it.
fn read_fixtures(dir: &Path) -> Result<Vec<(String, Fixture)>, Vec<String>> {
    let mut files = judge_manifest(dir)?;
    files.sort();
    Ok(files
        .into_iter()
        .map(|name| {
            let p = dir.join(&name);
            let text = std::fs::read_to_string(&p).unwrap();
            let fixture: Fixture =
                serde_json::from_str(&text).unwrap_or_else(|e| panic!("{}: {e}", p.display()));
            // A world the run does not know fails before any is built.
            assert!(
                WORLDS.contains(&fixture.world.as_str()),
                "{name}: unknown world `{}`: the worlds are {}",
                fixture.world,
                WORLDS.join(", ")
            );
            (name, fixture)
        })
        .collect())
}

/// A line diff of two pretty renderings (the fixture's, the served).
fn diff(want: &str, got: &str) -> String {
    use std::fmt::Write;
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
            writeln!(out, "  {}", w[i]).unwrap();
            i += 1;
            k += 1;
        } else if k < g.len() && (i == w.len() || lcs[i][k + 1] >= lcs[i + 1][k]) {
            writeln!(out, "+ {}", g[k]).unwrap();
            k += 1;
        } else {
            writeln!(out, "- {}", w[i]).unwrap();
            i += 1;
        }
    }
    out
}

/// The worlds are built on the process's own runtime, serially: the
/// permission audit is the process's, and two runs at once would list
/// each other's calls.
static SERIAL: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// Run `f` on a runtime of its own (the router's futures are deep in a
/// debug build: the run gets its own stacks), serially with any other.
fn on_runtime<M, F, T>(make: M) -> T
where
    M: FnOnce() -> F + Send + 'static,
    F: std::future::Future<Output = T>,
    T: Send + 'static,
{
    const STACK: usize = 64 * 1024 * 1024;
    let _serial = SERIAL
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    quiet_environment();
    std::thread::Builder::new()
        .stack_size(STACK)
        .spawn(move || {
            tokio::runtime::Builder::new_multi_thread()
                .worker_threads(2)
                .thread_stack_size(STACK)
                .enable_all()
                .build()
                .unwrap()
                .block_on(make())
        })
        .unwrap()
        .join()
        // The run's own panic, with its message (a join's error hides it).
        .unwrap_or_else(|payload| std::panic::resume_unwind(payload))
}

/// Whether `WIRE_RECORD` asks for a recording: `1` records, unset
/// replays, and any other value is a panic naming it (a `true` or a `0`
/// would otherwise replay in silence).
fn record_requested() -> bool {
    record_word(std::env::var("WIRE_RECORD").ok().as_deref())
}

fn record_word(value: Option<&str>) -> bool {
    match value {
        None => false,
        Some("1") => true,
        Some(other) => panic!(
            "WIRE_RECORD is `{other}`: `1` records, unset replays, and nothing else is a word"
        ),
    }
}

/// Record every case into `dir` (the old fixtures and manifest removed
/// before). A recording never passes: the message says to replay it.
async fn record(dir: &Path) -> String {
    std::fs::create_dir_all(dir).unwrap();
    for entry in std::fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        let name = path.file_name().unwrap().to_string_lossy().into_owned();
        if path.extension().is_some_and(|x| x == "json") || name == MANIFEST {
            std::fs::remove_file(path).unwrap();
        }
    }
    bsv_wallet_cli::server::audit::reset();
    let mut recorder = Recorder {
        dir: dir.to_path_buf(),
        worlds: HashMap::new(),
        seq: 0,
        written: Vec::new(),
        served: HashMap::new(),
    };
    record_all(&mut recorder).await;
    recorder.write_manifest();
    format!(
        "recorded {} fixtures and {MANIFEST} under {}. A recording is not a replay: \
         run `cargo test --locked --test wire` again, without WIRE_RECORD, to replay what \
         was recorded (and `git diff tests/wire` to see what moved)",
        recorder.written.len(),
        dir.display()
    )
}

/// Replay every fixture the manifest names: the failures, or the counts.
async fn replay(dir: &Path) -> Result<String, String> {
    // The permission audit is the process's: each pass starts it empty.
    bsv_wallet_cli::server::audit::reset();
    let fixtures = read_fixtures(dir).map_err(|faults| {
        format!(
            "the fixtures under {} are not the recorded set:\n{}",
            dir.display(),
            faults.join("\n")
        )
    })?;
    if fixtures.is_empty() {
        return Err(format!("no fixture under {}", dir.display()));
    }
    let mut worlds: HashMap<String, World> = HashMap::new();
    let mut served: HashMap<String, J> = HashMap::new();
    let mut failures = Vec::new();
    for (file, fixture) in &fixtures {
        if !worlds.contains_key(&fixture.world) {
            worlds.insert(fixture.world.clone(), World::build(&fixture.world).await);
        }
        let request = resolve(file, &fixture.request, &served);
        let (got, body) = worlds[&fixture.world].serve_raw(&request).await;
        if let Some(body) = body {
            served.insert(file.clone(), body);
        }
        if got != fixture.response {
            let want = serde_json::to_string_pretty(&fixture.response).unwrap();
            let got = serde_json::to_string_pretty(&got).unwrap();
            failures.push(format!(
                "{file}: {} {} answered differently (- fixture, + served)\n{}",
                fixture.request.method,
                fixture.request.path,
                diff(&want, &got)
            ));
        }
    }
    let missing = judge_coverage(&fixtures);
    let summary = format!(
        "{} fixtures replayed, {} differ, {} coverage gaps",
        fixtures.len(),
        failures.len(),
        missing.len()
    );
    if failures.is_empty() && missing.is_empty() {
        Ok(summary)
    } else {
        Err(format!(
            "{summary}\n{}\n{}\n(a change on purpose: WIRE_RECORD=1 cargo test --test wire, then a \
             replay, in its own commit; see docs/WIRE.md)",
            failures.join("\n"),
            missing.join("\n")
        ))
    }
}

/// The oracle: `WIRE_RECORD=1` records and fails with the word to replay;
/// otherwise every fixture is replayed.
#[test]
fn wire() {
    let dir = wire_dir();
    if record_requested() {
        let message = on_runtime(move || async move { record(&dir).await });
        panic!("{message}");
    }
    match on_runtime(move || async move { replay(&dir).await }) {
        Ok(summary) => println!("{summary}"),
        Err(message) => panic!("\n{message}"),
    }
}

// ---------------------------------------------------------------------------
// The planted cases: the judge is judged.
// ---------------------------------------------------------------------------

/// A recording run is not a pass: it writes the set and the manifest, and
/// ends with the word to replay. The second run replays it clean, and the
/// recording is the committed corpus byte for byte (two recordings in a row
/// are identical, and the cases have not drifted from the fixtures).
#[test]
fn a_recording_is_not_a_replay() {
    let tmp = tempfile::TempDir::new().unwrap();
    let dir = tmp.path().to_path_buf();
    let message = on_runtime(move || async move { record(&dir).await });
    assert!(message.contains("A recording is not a replay"), "{message}");
    let listed = judge_manifest(tmp.path()).unwrap();
    assert!(listed.len() > 79, "{} fixtures recorded", listed.len());
    assert_eq!(listed, {
        let mut sorted = listed.clone();
        sorted.sort();
        sorted
    });
    let dir = tmp.path().to_path_buf();
    let replayed = on_runtime(move || async move { replay(&dir).await });
    assert!(replayed.is_ok(), "the second run: {replayed:?}");
    for name in listed.iter().map(String::as_str).chain([MANIFEST]) {
        let recorded = std::fs::read(tmp.path().join(name)).unwrap();
        let committed = std::fs::read(wire_dir().join(name)).unwrap_or_default();
        assert!(
            recorded == committed,
            "{name}: a fresh recording differs from tests/wire (WIRE_RECORD=1, then replay)"
        );
    }
}

/// `WIRE_RECORD=1` records, unset replays, and any other value is refused
/// by name.
#[test]
fn only_one_word_records() {
    assert!(!record_word(None));
    assert!(record_word(Some("1")));
    for other in ["", "0", "true", "yes"] {
        let refused = std::panic::catch_unwind(|| record_word(Some(other)));
        let payload = refused.expect_err("a word that is not `1` must be refused");
        let message = payload
            .downcast_ref::<String>()
            .cloned()
            .unwrap_or_default();
        assert!(
            message.contains(&format!("WIRE_RECORD is `{other}`")),
            "{message}"
        );
    }
}

/// A fixture missing from disk, a file not in the manifest, and a file
/// that is not a `.json` fixture are each named; the set that matches
/// passes.
#[test]
fn the_manifest_judges_the_set_on_disk() {
    let tmp = tempfile::TempDir::new().unwrap();
    let dir = tmp.path();
    let write = |name: &str, text: &str| std::fs::write(dir.join(name), text).unwrap();
    let manifest = |names: &[&str]| {
        write(
            MANIFEST,
            &serde_json::to_string(&Manifest {
                fixtures: names.iter().map(ToString::to_string).collect(),
            })
            .unwrap(),
        );
    };
    assert!(
        judge_manifest(dir).unwrap_err()[0].contains(MANIFEST),
        "no manifest is a fault"
    );
    write("001-a.json", "{}");
    write("002-b.json", "{}");
    manifest(&["001-a.json", "002-b.json"]);
    assert_eq!(
        judge_manifest(dir).unwrap(),
        vec!["001-a.json", "002-b.json"]
    );

    std::fs::remove_file(dir.join("002-b.json")).unwrap();
    write("003-c.json", "{}");
    write("notes.txt", "a stray file");
    let faults = judge_manifest(dir).unwrap_err();
    assert_eq!(
        faults,
        vec![
            format!("002-b.json: listed in {MANIFEST}, not on disk"),
            format!("003-c.json: on disk, not listed in {MANIFEST}"),
            "notes.txt: on disk, not a .json fixture".to_string(),
        ]
    );

    // The replay refuses the set before any world is built, naming each.
    let at = dir.to_path_buf();
    let refused = on_runtime(move || async move { replay(&at).await }).unwrap_err();
    for name in ["002-b.json", "003-c.json", "notes.txt"] {
        assert!(refused.contains(name), "{refused}");
    }
}

/// A fixture naming a world the run does not know is a panic naming it.
#[test]
#[should_panic(expected = "unknown world `nowhere`")]
fn an_unknown_world_is_named() {
    known_world("nowhere");
}

/// The replay refuses a corpus naming an unknown world before any world
/// is built.
#[test]
fn a_fixture_of_an_unknown_world_fails_the_replay() {
    let tmp = tempfile::TempDir::new().unwrap();
    let fixture = Fixture {
        world: "nowhere".to_string(),
        request: get("/isAuthenticated"),
        response: Resp {
            status: 200,
            headers: BTreeMap::new(),
            json: None,
            text: None,
        },
    };
    std::fs::write(
        tmp.path().join("001-nowhere.json"),
        serde_json::to_string(&fixture).unwrap(),
    )
    .unwrap();
    std::fs::write(
        tmp.path().join(MANIFEST),
        serde_json::to_string(&Manifest {
            fixtures: vec!["001-nowhere.json".to_string()],
        })
        .unwrap(),
    )
    .unwrap();
    let dir = tmp.path().to_path_buf();
    let outcome =
        std::panic::catch_unwind(|| on_runtime(move || async move { replay(&dir).await }));
    let message = match outcome {
        Err(payload) => payload
            .downcast_ref::<String>()
            .cloned()
            .or_else(|| payload.downcast_ref::<&str>().map(ToString::to_string))
            .unwrap_or_default(),
        Ok(result) => panic!("the replay did not panic: {result:?}"),
    };
    assert!(
        message.contains("001-nowhere.json: unknown world `nowhere`"),
        "{message}"
    );
}

/// The normalizer keeps a volatile value's shape: a byte array's length, a
/// string's kind and length, a number's digit count; a key that is not
/// volatile is kept whole.
#[test]
fn the_normalizer_keeps_the_shape() {
    use serde_json::json;
    let body = j(json!({
        "ciphertext": [1, 2, 3, 255],
        "encryptedLinkage": "00ff",
        "revelationTime": "2026-10-23T00:00:00.000Z",
        "ageMinutes": 31,
        "reference": "abc+/=",
        "held": [{"ageMinutes": 1234, "reason": "5 min old"}],
        "txid": "ab".repeat(32),
        "nested": {"ciphertext": null},
    }));
    let want = j(json!({
        "ciphertext": "<bytes:4>",
        "encryptedLinkage": "<hex:4>",
        "revelationTime": "<rfc3339>",
        "ageMinutes": "<uint:2>",
        "reference": "<string:6>",
        "held": [{"ageMinutes": "<uint:4>", "reason": "<minutes> min old"}],
        "txid": "ab".repeat(32),
        "nested": {"ciphertext": null},
    }));
    assert_eq!(normalize(body).compact(), want.compact());
    assert_eq!(
        normalize_text("tcp connect error: Connection refused (os error 61) at 127.0.0.1:4242"),
        "tcp connect error: Connection refused (os error <errno>) at 127.0.0.1:<port>"
    );
}

/// The judged `AppError` codes and the unproduced ones are exactly the
/// codes `src/server/handlers.rs` writes, at their statuses: a code added
/// there with no fixture, or one gone from it, fails here by name.
#[test]
fn every_app_error_code_is_judged() {
    let in_source = app_codes_in_source();
    let listed: BTreeSet<(u16, String)> = APP_CODES
        .iter()
        .chain(UNPRODUCED.iter())
        .map(|(s, c)| (*s, c.to_string()))
        .collect();
    assert_eq!(
        APP_CODES.len() + UNPRODUCED.len(),
        listed.len(),
        "a code is listed twice"
    );
    let unlisted: Vec<_> = in_source.difference(&listed).collect();
    let gone: Vec<_> = listed.difference(&in_source).collect();
    assert!(
        unlisted.is_empty() && gone.is_empty(),
        "src/server/handlers.rs writes {unlisted:?}, which neither APP_CODES (with a fixture) nor \
         UNPRODUCED (with its reason in docs/WIRE.md) lists; and lists {gone:?}, which it no \
         longer writes"
    );
}
