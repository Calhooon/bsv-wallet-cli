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

pub(crate) mod coins;

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

/// Route a served test wallet's headers and posts to the same local mock.
pub fn mock_services(url: &str) -> bsv_wallet_toolbox::Services {
    use bsv_wallet_toolbox::{Chain, Services, ServicesOptions};
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
/// it carries them. A `#[tokio::test]` runs on one thread, so a capture taken
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
}

impl Markers {
    pub fn capture() -> (Self, tracing::subscriber::DefaultGuard) {
        use tracing_subscriber::layer::SubscriberExt;
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
        let mut marked = Marked::default();
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
    let storage = wallet.storage();
    storage.pool().set_connect_options(
        storage
            .pool()
            .connect_options()
            .as_ref()
            .clone()
            .busy_timeout(std::time::Duration::ZERO),
    );
    sqlx::query("PRAGMA busy_timeout = 0")
        .execute(storage.pool())
        .await
        .unwrap();
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
