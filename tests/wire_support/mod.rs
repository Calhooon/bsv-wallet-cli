//! The replay machinery the wire tests share, held once: `tests/wire.rs`
//! (the served fixtures and their manifest, #69) and
//! `tests/wire_consumers.rs` (the consumers' calls, #110) each declare it
//! with `mod wire_support;` and keep only what is their own (their fixture,
//! their worlds, their cases, their normalizer's rules).
//!
//! Here: the JSON that keeps its key order, a fixture's request and
//! response, the request as sent, the normalizer's walk and the response as
//! captured, the local stubs, the process's quiet environment, the run's own
//! runtime, the fixture reader and the line diff. An item one test does not
//! use is the other's.

#![allow(dead_code)]

use std::collections::{BTreeMap, HashMap};
use std::path::Path;

use axum::body::Body;
use axum::http::Request;
use axum::Router;
use bsv_wallet_toolbox::{Chain, Services, ServicesOptions};
use serde::de::{DeserializeOwned, MapAccess, SeqAccess, Visitor};
use serde::ser::{SerializeMap, SerializeSeq};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use tower::ServiceExt;

mod mining;

// ---------------------------------------------------------------------------
// JSON that keeps its key order (serde_json's `Value` sorts keys here, and a
// key moved is a byte changed).
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq)]
pub enum J {
    Null,
    Bool(bool),
    Num(serde_json::Number),
    Str(String),
    Arr(Vec<J>),
    Obj(Vec<(String, J)>),
}

impl J {
    pub fn get(&self, key: &str) -> Option<&J> {
        match self {
            J::Obj(fields) => fields.iter().find(|(k, _)| k == key).map(|(_, v)| v),
            _ => None,
        }
    }

    pub fn as_str(&self) -> Option<&str> {
        match self {
            J::Str(s) => Some(s),
            _ => None,
        }
    }

    pub fn compact(&self) -> String {
        serde_json::to_string(self).expect("serialize")
    }

    pub fn parse(bytes: &[u8]) -> Option<J> {
        serde_json::from_slice(bytes).ok()
    }

    /// The value at a JSON pointer (`/a/0/b`), as RFC 6901 reads it.
    pub fn pointer(&self, pointer: &str) -> Option<&J> {
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
    pub fn set_pointer(&mut self, pointer: &str, value: J) {
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

// ---------------------------------------------------------------------------
// A fixture's request and response.
// ---------------------------------------------------------------------------

/// A message's headers, by lowercase name.
pub type Headers = BTreeMap<String, String>;

/// A recorded request. The consumers' fixtures carry the first four fields
/// only; the others are skipped when empty, so their files are unchanged.
#[derive(Serialize, Deserialize, Clone, Default)]
pub struct Req {
    pub method: String,
    pub path: String,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub headers: Headers,
    /// A JSON body, sent as its compact bytes with `content-type:
    /// application/json`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub json: Option<J>,
    /// A body sent byte for byte as written (a malformed one).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub raw: Option<String>,
    /// A body of this many spaces (an oversized one; the file does not
    /// carry the megabyte).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fill: Option<usize>,
    /// Values of `json` that come from an earlier fixture's served body in
    /// the same run (a reference the wallet drew at random): the JSON
    /// pointer into this request, and where the value is read from. The
    /// recorded `json` carries `<bound>` there.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub bind: BTreeMap<String, Bound>,
}

/// Where a bound request value is read from: a fixture of the same world
/// that ran earlier, and a JSON pointer into the body it was served.
#[derive(Serialize, Deserialize, Clone, PartialEq)]
pub struct Bound {
    pub file: String,
    pub pointer: String,
}

/// The marker written into a recorded request where a bound value goes.
pub const BOUND: &str = "<bound>";

#[derive(Serialize, Deserialize, Clone, PartialEq)]
pub struct Resp {
    pub status: u16,
    pub headers: Headers,
    /// A JSON body, normalized.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub json: Option<J>,
    /// A body that is not JSON, byte for byte, normalized.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
}

impl Req {
    /// The request as sent: its headers, and its body (`json` with its
    /// content type, `raw`, a `fill` of spaces, or none).
    pub fn to_http(&self) -> Request<Body> {
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

/// Whether a request body still carries the [`BOUND`] marker anywhere.
pub fn has_bound_marker(v: &J) -> bool {
    match v {
        J::Str(s) => s == BOUND,
        J::Arr(items) => items.iter().any(has_bound_marker),
        J::Obj(fields) => fields.iter().any(|(_, v)| has_bound_marker(v)),
        _ => false,
    }
}

// ---------------------------------------------------------------------------
// The normalizer's frame: each test names its rules (`docs/WIRE.md`), the
// walk and the capture are one.
// ---------------------------------------------------------------------------

/// One test's normalizer.
pub struct Normalizer {
    /// The response headers that are part of the contract. Every other
    /// header is the transport's.
    pub kept_headers: &'static [&'static str],
    /// The placeholder for a key's value that differs on every run (a null
    /// is kept as served), or none for a key whose value is compared.
    pub volatile: fn(&str, Option<&str>, &J) -> Option<String>,
    /// The rules inside any string.
    pub text: fn(&str) -> String,
}

impl Normalizer {
    pub fn json(&self, v: J) -> J {
        self.walk(v, None)
    }

    fn walk(&self, v: J, parent: Option<&str>) -> J {
        match v {
            J::Obj(fields) => J::Obj(
                fields
                    .into_iter()
                    .map(|(k, v)| {
                        let placeholder = match v {
                            J::Null => None,
                            _ => (self.volatile)(&k, parent, &v),
                        };
                        let v = match placeholder {
                            Some(p) => J::Str(p),
                            None => self.walk(v, Some(&k)),
                        };
                        (k, v)
                    })
                    .collect(),
            ),
            J::Arr(items) => J::Arr(items.into_iter().map(|v| self.walk(v, parent)).collect()),
            J::Str(s) => J::Str((self.text)(&s)),
            other => other,
        }
    }

    /// `req` served by `router`: the normalized response, and the JSON body
    /// as served.
    pub async fn serve(&self, router: &Router, req: &Req) -> (Resp, Option<J>) {
        let response = router.clone().oneshot(req.to_http()).await.unwrap();
        let status = response.status().as_u16();
        let headers = self
            .kept_headers
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
            resp.text = Some((self.text)(&String::from_utf8_lossy(&bytes)));
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
        resp.json = Some(self.json(json.clone()));
        (resp, Some(json))
    }
}

/// This wallet's served version string (`getVersion`, #122 item 6) as its
/// shape, `bsv-wallet-cli-<major>.<minor>.<patch>`: it moves with the
/// package's version, never with a run or a behaviour, so a version commit
/// moves no fixture. `tests/integration/status.rs` pins the exact string.
pub fn package_version(s: &str) -> String {
    s.replace(
        bsv_wallet_cli::server::handlers::WALLET_VERSION,
        "bsv-wallet-cli-<major>.<minor>.<patch>",
    )
}

/// Every run of digits right after `marker` (`after`), or right before it,
/// replaced by `placeholder`.
pub fn replace_number(s: &str, marker: &str, placeholder: &str, after: bool) -> String {
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
// The worlds' common ground: the key, the header stub, the services.
// ---------------------------------------------------------------------------

/// The wallet's root key: fixed, so its identity key, every derived key and
/// every (RFC 6979) signature is the same on every run.
pub const ROOT_KEY: &str = "1f1f1f1f1f1f1f1f1f1f1f1f1f1f1f1f1f1f1f1f1f1f1f1f1f1f1f1f1f1f1f1f";

/// The header stub's tip.
pub const TIP: u32 = 900_010;

/// A closed local port: a broadcaster or proxy that never answers.
pub const CLOSED: &str = "http://127.0.0.1:9";

/// A local header service: the tip at [`TIP`], a height's header with the
/// root `roots` gives it (`other` for any other height, and for the tip);
/// or, `down`, a 500 for everything.
pub async fn header_stub(down: bool, roots: HashMap<u32, String>, other: &'static str) -> String {
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
    let (tip, range) = mining::range(other);
    let app = if down {
        Router::new().fallback(|| async { axum::http::StatusCode::INTERNAL_SERVER_ERROR })
    } else {
        Router::new()
            .route(
                "/getChain",
                get(|| async {
                    axum::Json(serde_json::json!({"status":"success","value":"main"}))
                }),
            )
            .route(
                "/getHeaders",
                get(move || {
                    let range = range.clone();
                    async move { axum::Json(serde_json::json!({"status":"success","value":range})) }
                }),
            )
            .route(
                "/findChainTipHeaderHex",
                get(move || {
                    let tip = tip.clone();
                    async move { axum::Json(tip) }
                }),
            )
            .route(
                "/getPresentHeight",
                get(|| async {
                    axum::Json(serde_json::json!({"status": "success", "value": TIP}))
                }),
            )
            .route(
                "/findHeaderHexForHeight",
                get(move |Query(q): Query<HashMap<String, u32>>| async move {
                    let height = q.get("height").copied().unwrap_or(0);
                    let root = roots.get(&height).map_or(other, String::as_str);
                    axum::Json(header(height, root))
                }),
            )
    };
    serve_local(app).await
}

/// Serve `app` on a fresh local port for the run's life: its base URL.
pub async fn serve_local(app: Router) -> String {
    use std::future::IntoFuture;
    let socket = tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
        .await
        .expect("a local port");
    let url = format!("http://{}", socket.local_addr().expect("its address"));
    tokio::spawn(axum::serve(socket, app).into_future());
    url
}

/// The toolbox's services on mainnet, its headers from `chaintracks`, its
/// broadcasts to `arc` alone, no `BananaBlocks`.
pub fn services(chaintracks: String, arc: String) -> Services {
    let mut options = ServicesOptions::mainnet()
        .with_chaintracks_url(chaintracks)
        .without_bananablocks();
    options.arc_url = arc;
    options.arcade_v2 = false; // The wire fixture serves classic ARC (#317).
    options.arc_gorillapool_url = None;
    Services::with_options(Chain::Main, options).unwrap()
}

// ---------------------------------------------------------------------------
// The run.
// ---------------------------------------------------------------------------

/// The process's environment, set once before any world: no request leaves
/// the machine, and no operator setting changes a served answer (the
/// wallet's own settings are [`quiet_config`]'s, never the environment's).
pub fn quiet_environment() {
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
        std::env::set_var(var, CLOSED);
    }
    std::env::set_var("NO_PROXY", "127.0.0.1");
    std::env::set_var("no_proxy", "127.0.0.1");
}

/// The configuration a world's server is built with: the settings of the
/// [`quiet_environment`], read from it, never from the process's.
pub fn quiet_config() -> std::sync::Arc<bsv_wallet_cli::config::Config> {
    std::sync::Arc::new(
        bsv_wallet_cli::config::Config::from_pairs(&[("BSV_WALLET_SKIP_BROADCAST_VERIFY", "1")])
            .expect("the quiet configuration"),
    )
}

/// The worlds are built on the process's own runtime, serially: the
/// permission audit is the process's, and two runs at once would list
/// each other's calls.
static SERIAL: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// Run `f` on a runtime of its own (the router's futures are deep in a
/// debug build: the run gets its own stacks), serially with any other, in
/// the [`quiet_environment`].
pub fn on_runtime<M, F, T>(make: M) -> T
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

/// A fixture file, parsed (a panic names the file).
pub fn read_fixture<T: DeserializeOwned>(path: &Path) -> T {
    let text = std::fs::read_to_string(path).unwrap();
    serde_json::from_str(&text).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

/// A line diff of two pretty renderings (the fixture's, the served).
pub fn diff(want: &str, got: &str) -> String {
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
