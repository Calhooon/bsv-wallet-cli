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

#[cfg(test)]
mod tests {
    use super::*;

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
