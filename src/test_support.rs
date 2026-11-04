//! Test support shared by the library and the binary: a local server that
//! stands in an explorer's (or a header service's) place and counts the
//! requests that reach it, and a transaction built by hand. Nothing here
//! reaches the network.

#![allow(dead_code)]

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};

use axum::extract::State;
use axum::http::{header, HeaderMap, HeaderName, HeaderValue, StatusCode, Uri};
use axum::response::{IntoResponse, Response};

mod busy;
pub(crate) mod coins;
pub use busy::no_busy_wait;

mod chain_tracker;
pub use chain_tracker::TestChainTracker;

/// Serve a test router on an ephemeral loopback port, with an abortable task.
pub async fn serve_router(app: axum::Router) -> (String, tokio::task::JoinHandle<()>) {
    let listener = tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
        .await
        .unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let task = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    (url, task)
}

/// A header service's answer for a request to `uri`: the header at its
/// `height=` (900 when it names none), whose merkle root is `roots`' entry
/// for that height (a mined coin's one-leaf block), else `0c..`.
pub fn header_answer(
    roots: &std::collections::HashMap<u32, String>,
    uri: &axum::http::Uri,
) -> axum::Json<serde_json::Value> {
    let height = uri
        .query()
        .and_then(|q| q.strip_prefix("height="))
        .and_then(|h| h.parse().ok())
        .unwrap_or(900);
    axum::Json(serde_json::json!({"status": "success", "value": {
        "version": 536_870_912, "previousHash": "00".repeat(32),
        "merkleRoot": roots.get(&height).cloned().unwrap_or_else(|| "0c".repeat(32)),
        "time": 1_700_000_000, "bits": 402_917_821_u32, "nonce": 7,
        "height": height, "hash": format!("{height:064x}")
    }}))
}

/// A closed local port: the proxy of every request [`seal_network`] keeps
/// off the network.
pub const CLOSED: &str = "http://127.0.0.1:9";

/// Send every non-loopback request of this process to [`CLOSED`], once,
/// before a client is built: the post ladder's Bitails and WhatsOnChain
/// rungs have no base to point at a mock, and through them the handler
/// mutant witnesses posted to mainnet and waited on it, 4 to 65 seconds a
/// test (#477). A request there is a fault ("could not look"), at once;
/// loopback mocks are unproxied. The wire harness seals its worlds the
/// same way (`tests/wire_support`, `quiet_environment`).
pub fn seal_network() {
    static SEALED: std::sync::Once = std::sync::Once::new();
    SEALED.call_once(|| {
        for var in ["HTTPS_PROXY", "HTTP_PROXY", "https_proxy", "http_proxy"] {
            std::env::set_var(var, CLOSED);
        }
        for var in ["NO_PROXY", "no_proxy"] {
            std::env::set_var(var, "127.0.0.1,localhost");
        }
    });
}

/// Route a served test wallet's headers and posts to the same local mock,
/// the network sealed ([`seal_network`]).
pub fn mock_services(url: &str) -> bsv_wallet_toolbox::Services {
    use bsv_wallet_toolbox::{Chain, Services, ServicesOptions};
    seal_network();
    let options = ServicesOptions {
        arc_url: url.to_string(),
        arcade_v2: false, // This local fixture serves classic ARC (#317).
        arc_gorillapool_url: None,
        ..ServicesOptions::mainnet()
            .with_chaintracks_url(url.to_string())
            .without_bananablocks()
    };
    Services::with_options(Chain::Main, options).unwrap()
}

type Routes = Arc<HashMap<String, (u16, String, Vec<(String, String)>)>>;
type Hits = Arc<Mutex<Vec<String>>>;
type Sent = Arc<Mutex<Vec<HashMap<String, String>>>>;

/// One route of [`Fixture::start_with_headers`]: path, status, body and
/// response headers.
pub type HeaderedRoute<'a> = (&'a str, u16, &'a str, &'a [(&'a str, &'a str)]);

/// A local HTTP server answering a fixed table of `path -> (status, body)`.
/// A path (with its query) not in the table gets `fallback`. Every request
/// is recorded.
///
/// Its port is never handed to another fixture in the process (see
/// [`hold_port`]): the toolbox keys a provider's rate gate by its base URL
/// (`BananaBlocks::with_base`, `RateGate::shared`), so a recycled port would
/// carry one test's 429 or spent token bucket into the next.
pub struct Fixture {
    /// `http://127.0.0.1:<port>`, no trailing slash.
    pub base: String,
    hits: Hits,
    sent: Sent,
}

impl Fixture {
    /// Serve `routes`; any other path answers `fallback` (a status).
    pub async fn start(routes: &[(&str, u16, &str)], fallback: u16) -> Self {
        let routes: Vec<HeaderedRoute> = routes
            .iter()
            .map(|(p, s, b)| (*p, *s, *b, &[][..]))
            .collect();
        Self::start_with_headers(&routes, fallback).await
    }

    /// [`Fixture::start`] with response headers per route (a 429's
    /// `Retry-After`).
    pub async fn start_with_headers(routes: &[HeaderedRoute<'_>], fallback: u16) -> Self {
        let table: Routes = Arc::new(
            routes
                .iter()
                .map(|(p, s, b, h)| {
                    let headers = h
                        .iter()
                        .map(|(k, v)| (k.to_string(), v.to_string()))
                        .collect();
                    (p.to_string(), (*s, b.to_string(), headers))
                })
                .collect(),
        );
        let hits: Hits = Arc::new(Mutex::new(Vec::new()));
        let sent: Sent = Arc::new(Mutex::new(Vec::new()));
        let app = axum::Router::new().fallback(serve).with_state((
            table,
            hits.clone(),
            sent.clone(),
            fallback,
        ));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind");
        let listener = hold_port(listener);
        let addr: SocketAddr = listener.local_addr().expect("local addr");
        tokio::spawn(async move {
            axum::serve(listener, app).await.ok();
        });
        Self {
            base: format!("http://{addr}"),
            hits,
            sent,
        }
    }

    /// The request headers of every request so far, in order (names
    /// lowercase).
    pub fn headers(&self) -> Vec<HashMap<String, String>> {
        self.sent.lock().unwrap().clone()
    }

    /// Every path (with its query) requested so far, in order.
    pub fn hits(&self) -> Vec<String> {
        self.hits.lock().unwrap().clone()
    }

    /// How many requests named exactly `path`.
    pub fn count(&self, path: &str) -> usize {
        self.hits().iter().filter(|p| p.as_str() == path).count()
    }

    /// How many requests reached the server at all.
    pub fn total(&self) -> usize {
        self.hits().len()
    }
}

/// The listening sockets of every fixture this process started, held
/// open for the process's life (no one accepts on them once their test's
/// runtime is gone), so the OS never hands a fixture's port to a later one.
static HELD_PORTS: Mutex<Vec<std::net::TcpListener>> = Mutex::new(Vec::new());

/// `listener`, with a duplicate of its socket kept in [`HELD_PORTS`].
fn hold_port(listener: tokio::net::TcpListener) -> tokio::net::TcpListener {
    let std = listener.into_std().expect("std listener");
    let held = std.try_clone().expect("a duplicate of the listener");
    HELD_PORTS
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .push(held);
    tokio::net::TcpListener::from_std(std).expect("tokio listener")
}

async fn serve(
    State((table, hits, sent, fallback)): State<(Routes, Hits, Sent, u16)>,
    request_headers: HeaderMap,
    uri: Uri,
) -> Response {
    sent.lock().unwrap().push(
        request_headers
            .iter()
            .filter_map(|(k, v)| Some((k.as_str().to_string(), v.to_str().ok()?.to_string())))
            .collect(),
    );
    let path = uri
        .path_and_query()
        .map(|p| p.as_str().to_string())
        .unwrap_or_else(|| uri.path().to_string());
    hits.lock().unwrap().push(path.clone());
    let (status, body, extra) =
        table
            .get(&path)
            .cloned()
            .unwrap_or((fallback, String::new(), Vec::new()));
    let content_type = if body.starts_with('{') || body.starts_with('[') {
        "application/json"
    } else {
        "text/plain"
    };
    let mut response = (
        StatusCode::from_u16(status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR),
        [(header::CONTENT_TYPE, content_type)],
        body,
    )
        .into_response();
    for (name, value) in extra {
        if let (Ok(name), Ok(value)) = (
            HeaderName::from_bytes(name.as_bytes()),
            HeaderValue::from_str(&value),
        ) {
            response.headers_mut().insert(name, value);
        }
    }
    response
}

mod tx;
pub use tx::{p2pkh, raw_tx, txid_of};

/// The `marker` field of every event logged while a [`Markers::capture`]
/// guard lives on this thread, with the event's `txid`, `error` and message when
/// it carries them, its level, and every field by name. A `#[tokio::test]` runs on one thread, so a capture taken
/// at its start sees every line the test's awaits log.
#[derive(Clone, Default)]
pub struct Markers(Arc<Mutex<Vec<Marked>>>);

/// One marked line.
#[derive(Debug, Clone, Default)]
pub struct Marked {
    pub marker: String,
    pub txid: Option<String>,
    pub error: Option<String>,
    pub message: Option<String>,
    pub level: Option<tracing::Level>,
    pub fields: HashMap<String, String>,
}

/// Install [`AlwaysInterested`] as this test binary's global default, once
/// (#568). Every capture calls it before its own subscriber registers, and
/// a test that logs before any capture may call it first.
///
/// While at most one dispatcher is registered, tracing-core 0.1.36 asks only
/// the registering thread's default for a callsite's interest
/// (`callsite.rs:545-546`, `has_just_one` gives `Rebuilder::JustOne`, whose
/// `for_each` calls `dispatcher::get_default` at `:564-565`). A capture-less
/// thread that reaches a callsite first, while another thread's capture is
/// the only dispatcher, caches it as never, and the capture's own line at
/// that callsite is then skipped (#553). This one never dies (the global is
/// leaked, `dispatcher.rs:314-318`), so a capture registered after it makes
/// two live dispatchers (`callsite.rs:553-556`) and every rebuild after
/// that asks them all: a callsite's interest is every live dispatcher's,
/// combined (`:497-503`, `Interest::and`: equal stays, unequal is
/// sometimes), and a capture-less thread's default is this one, never none.
/// No callsite is cached as never; the thread-local captures only filter.
///
/// A second install (another binary path that set a global first) is
/// tolerated: the `Err` is the one already in place.
pub fn init() {
    static INSTALLED: std::sync::Once = std::sync::Once::new();
    INSTALLED.call_once(|| {
        let _ = tracing::subscriber::set_global_default(AlwaysInterested);
    });
}

/// A subscriber interested in every callsite at every level that writes
/// nothing: [`init`]'s global default. Not a `tracing_subscriber::fmt`
/// subscriber, whose writer would print every capture-less line of the
/// binary (and whose filter is what may say never); this one records
/// nothing at all.
pub struct AlwaysInterested;

impl tracing::Subscriber for AlwaysInterested {
    fn register_callsite(
        &self,
        _: &'static tracing::Metadata<'static>,
    ) -> tracing::subscriber::Interest {
        tracing::subscriber::Interest::always()
    }

    fn enabled(&self, _: &tracing::Metadata<'_>) -> bool {
        true
    }

    fn max_level_hint(&self) -> Option<tracing::level_filters::LevelFilter> {
        None
    }

    fn new_span(&self, _: &tracing::span::Attributes<'_>) -> tracing::span::Id {
        tracing::span::Id::from_u64(1)
    }

    fn record(&self, _: &tracing::span::Id, _: &tracing::span::Record<'_>) {}

    fn record_follows_from(&self, _: &tracing::span::Id, _: &tracing::span::Id) {}

    fn event(&self, _: &tracing::Event<'_>) {}

    fn enter(&self, _: &tracing::span::Id) {}

    fn exit(&self, _: &tracing::span::Id) {}
}

impl Markers {
    pub fn capture() -> (Self, tracing::subscriber::DefaultGuard) {
        use tracing_subscriber::layer::SubscriberExt;
        init();
        let markers = Self::default();
        let subscriber = tracing_subscriber::registry().with(markers.clone());
        (markers, tracing::subscriber::set_default(subscriber))
    }

    pub fn all(&self) -> Vec<Marked> {
        self.0.lock().unwrap().clone()
    }

    /// The lines marked `marker`.
    pub fn named(&self, marker: &str) -> Vec<Marked> {
        self.all()
            .into_iter()
            .filter(|m| m.marker == marker)
            .collect()
    }

    /// The one `daemon_fault` line a test logged, its message and error.
    pub fn fault_line(&self) -> (String, String) {
        let lines = self.named("daemon_fault");
        assert_eq!(lines.len(), 1, "{lines:?}");
        let line = lines[0].clone();
        (
            line.message.unwrap_or_default(),
            line.error.unwrap_or_default(),
        )
    }
}

impl tracing::field::Visit for Marked {
    fn record_str(&mut self, field: &tracing::field::Field, value: &str) {
        self.set(field.name(), value.to_string());
    }

    fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
        self.set(field.name(), format!("{value:?}"));
    }
}

impl Marked {
    fn set(&mut self, name: &str, value: String) {
        self.fields.insert(name.to_string(), value.clone());
        match name {
            "marker" => self.marker = value,
            "txid" => self.txid = Some(value),
            "error" => self.error = Some(value),
            "message" => self.message = Some(value),
            _ => {}
        }
    }
}

impl<S: tracing::Subscriber> tracing_subscriber::Layer<S> for Markers {
    fn on_event(&self, event: &tracing::Event<'_>, _: tracing_subscriber::layer::Context<'_, S>) {
        let mut marked = Marked {
            level: Some(*event.metadata().level()),
            ..Marked::default()
        };
        event.record(&mut marked);
        if !marked.marker.is_empty() {
            self.0.lock().unwrap().push(marked);
        }
    }
}

/// The two halves of a real `SQLITE_BUSY` for the tests: the options of a
/// file at `path` with no busy timeout, and a holder connection that has
/// made `table` there; a test takes the lock on the holder and asks with
/// another connection (#44, #168).
pub fn no_wait_file(path: &std::path::Path) -> sqlx::sqlite::SqliteConnectOptions {
    sqlx::sqlite::SqliteConnectOptions::new()
        .filename(path)
        .create_if_missing(true)
        .busy_timeout(std::time::Duration::ZERO)
}

/// The holder of [`no_wait_file`]'s file, `table` made.
pub async fn busy_holder(
    options: &sqlx::sqlite::SqliteConnectOptions,
    table: &str,
) -> sqlx::SqliteConnection {
    use sqlx::ConnectOptions;
    let mut holder = options.connect().await.unwrap();
    sqlx::query(table).execute(&mut holder).await.unwrap();
    holder
}

/// Read a served JSON error, keeping its status and retry header.
pub async fn response_answer(
    response: axum::response::Response,
) -> (StatusCode, Option<String>, serde_json::Value) {
    let status = response.status();
    let retry = response
        .headers()
        .get(axum::http::header::RETRY_AFTER)
        .map(|v| v.to_str().unwrap().to_string());
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    (status, retry, serde_json::from_slice(&bytes).unwrap())
}

/// A wallet behind another connection's exclusive lock, sharing
/// [`no_wait_file`] and [`busy_holder`] with the abort pre-check test.
/// Dotfile locking keeps every reader out; every reader has no busy wait.
pub async fn busy_read_wallet() -> (
    tempfile::TempDir,
    crate::server::handlers::WalletState,
    sqlx::SqliteConnection,
) {
    let dir = tempfile::TempDir::new().unwrap();
    let file = dir.path().join("busy.db");
    let options = no_wait_file(&file).vfs("unix-dotfile");
    let mut holder = busy_holder(&options, "CREATE TABLE busy_fixture (id INTEGER)").await;
    let storage = bsv_wallet_toolbox::StorageSqlx::new(&format!(
        "sqlite:{}?vfs=unix-dotfile",
        file.display()
    ))
    .await
    .unwrap();
    let key = bsv_sdk::primitives::PrivateKey::from_hex(&"01".repeat(32)).unwrap();
    let wallet = test_wallet(storage, "host-route", key).await;
    no_busy_wait(wallet.storage().pool()).await;
    sqlx::query("BEGIN EXCLUSIVE")
        .execute(&mut holder)
        .await
        .unwrap();
    (dir, wallet, holder)
}

/// Take or release the exclusive lock returned by [`busy_read_wallet`].
pub async fn set_read_lock(holder: &mut sqlx::SqliteConnection, held: bool) {
    let statement = if held { "BEGIN EXCLUSIVE" } else { "ROLLBACK" };
    sqlx::query(statement).execute(holder).await.unwrap();
}

/// Plant pending proof requests whose status a delivered ARC word can change.
pub async fn seed_arc_requests(storage: &bsv_wallet_toolbox::StorageSqlx, txids: &[String]) {
    for txid in txids {
        sqlx::query(
            "INSERT INTO proven_tx_reqs (txid, status, attempts, history, notified, notify, raw_tx, created_at, updated_at) \
             VALUES (?, 'unsent', 0, '{}', 0, '{}', X'01000000', CURRENT_TIMESTAMP, CURRENT_TIMESTAMP)",
        )
        .bind(txid)
        .execute(storage.writer())
        .await
        .unwrap();
    }
}

/// The sending transaction beside a callback's pending proof request.
pub async fn seed_arc_transaction(
    storage: &bsv_wallet_toolbox::StorageSqlx,
    txid: &str,
    raw_tx: &[u8],
) {
    sqlx::query(
        "INSERT INTO transactions (user_id, txid, status, reference, description, satoshis, \
         version, lock_time, raw_tx, is_outgoing, created_at, updated_at) \
         VALUES ((SELECT user_id FROM users LIMIT 1), ?, 'sending', 'callback-retry', \
         'callback retry', -500, 1, 0, ?, 1, CURRENT_TIMESTAMP, CURRENT_TIMESTAMP)",
    )
    .bind(txid)
    .bind(raw_tx)
    .execute(storage.writer())
    .await
    .unwrap();
}

/// Callback state including row identities, proof bytes, links and memory.
/// Timestamps are omitted: repeated delivery may refresh them without changing
/// the status, anchor or rows. Reading on the writer also drains a cancelled
/// SQLite operation before we inspect its effects.
pub async fn arc_rows(storage: &bsv_wallet_toolbox::StorageSqlx, txid: &str) -> serde_json::Value {
    let rows: String = sqlx::query_scalar(
        "SELECT json_object( \
         'request', json_array(r.proven_tx_req_id, r.status, r.proven_tx_id, r.history), \
         'transaction', json_array(t.transaction_id, t.status, t.proven_tx_id), \
         'proof', json_array(p.proven_tx_id, p.height, p.block_hash, p.merkle_root, \
                                hex(p.merkle_path), hex(p.raw_tx)), \
         'proof_count', (SELECT COUNT(*) FROM proven_txs WHERE txid = r.txid), \
         'memory', (SELECT json_group_array(json_array(provider, status)) \
                    FROM broadcast_seen WHERE txid = r.txid)) \
         FROM proven_tx_reqs r LEFT JOIN transactions t ON t.txid = r.txid \
         LEFT JOIN proven_txs p ON p.txid = r.txid WHERE r.txid = ?",
    )
    .bind(txid)
    .fetch_one(storage.writer())
    .await
    .unwrap();
    serde_json::from_str(&rows).unwrap()
}

/// A migrated wallet on the given store for router tests. Constructed
/// before taking a lock so the test exercises only the served read.
pub async fn test_wallet(
    store: bsv_wallet_toolbox::StorageSqlx,
    name: &str,
    key: bsv_sdk::primitives::PrivateKey,
) -> crate::server::handlers::WalletState {
    use bsv_wallet_toolbox::WalletStorageWriter;
    store
        .migrate(name, &key.public_key().to_hex())
        .await
        .unwrap();
    store.make_available().await.unwrap();
    let services = bsv_wallet_toolbox::Services::with_options(
        bsv_wallet_toolbox::Chain::Main,
        bsv_wallet_toolbox::ServicesOptions::mainnet(),
    )
    .unwrap();
    crate::server::make_wallet_state(
        bsv_wallet_toolbox::Wallet::new(Some(key), store, services)
            .await
            .unwrap(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_tracker_keeps_root_and_height_answers_and_supplies_mining_context() {
        use bsv_sdk::transaction::{ChainTracker, MockChainTracker};

        let root = "ab".repeat(32);
        let mut original = MockChainTracker::new(900_001);
        original.add_root(900_000, root.clone());
        let mut rooted = TestChainTracker::new(900_001);
        rooted.add_root(900_000, root.clone());
        for (candidate, height) in [(&root[..], 900_000), ("other", 900_000), (&root, 899_999)] {
            assert_eq!(
                rooted.is_valid_root_for_height(candidate, height).await,
                original.is_valid_root_for_height(candidate, height).await
            );
        }
        let permissive = TestChainTracker::always_valid(900_001);
        assert!(permissive
            .is_valid_root_for_height("other", 0)
            .await
            .unwrap());
        for tracker in [&rooted as &dyn ChainTracker, &permissive] {
            assert_eq!(
                tracker.current_height().await,
                original.current_height().await
            );
            let tip = tracker.mining_tip().await.unwrap().expect("mining context");
            // Genesis is `ChainParams`'s since bsv-rs 0.9.0-beta.4 (#451);
            // the tip's deprecated copy is not read.
            assert_eq!(
                (
                    tip.height,
                    tip.median_time_past,
                    tip.csv_height,
                    tip.next_block_time
                ),
                (900_001, 1_700_000_000, 419_328, None)
            );
        }
    }

    /// #568: a capture-less thread reaches a fresh callsite first, while
    /// this thread's capture is alive, and the capture still sees its own
    /// line at that callsite. Red without [`init`]'s global default when the
    /// capture is the only live dispatcher (run alone): the other thread's
    /// registration asks only its own default, none, and caches never.
    #[test]
    fn a_capture_less_thread_first_at_a_callsite_leaves_a_capture_its_line() {
        fn log_once() {
            tracing::info!(marker = "callsite_568", "the fresh callsite");
        }
        let (markers, _guard) = Markers::capture();
        std::thread::spawn(log_once).join().unwrap();
        log_once();
        assert_eq!(
            markers.named("callsite_568").len(),
            1,
            "{:?}",
            markers.all()
        );
    }

    /// L5 of the #50 review: a fixture's port stays bound after its test's
    /// runtime is gone, so no later fixture (and no gate the toolbox keys by
    /// a fixture's base URL) is handed it. Red without `hold_port`: the
    /// bind succeeds once the runtime has dropped the server.
    #[test]
    fn a_fixtures_port_is_never_recycled() {
        let addr = tokio::runtime::Runtime::new()
            .unwrap()
            .block_on(async { Fixture::start(&[], 404).await.base });
        let addr = addr.trim_start_matches("http://").to_string();
        let again = std::net::TcpListener::bind(&addr);
        assert!(again.is_err(), "{addr} was free for another fixture");
    }
}
