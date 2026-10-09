//! Test support shared by the library and the binary: a local server that
//! stands in an explorer's (or a header service's) place and counts the
//! requests that reach it, and a transaction built by hand. Nothing here
//! reaches the network.

#![allow(dead_code)]

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};

use axum::extract::State;
use axum::http::{header, StatusCode, Uri};
use axum::response::{IntoResponse, Response};

type Routes = Arc<HashMap<String, (u16, String)>>;
type Hits = Arc<Mutex<Vec<String>>>;

/// A local HTTP server answering a fixed table of `path -> (status, body)`.
/// A path (with its query) not in the table gets `fallback`. Every request
/// is recorded.
pub struct Fixture {
    /// `http://127.0.0.1:<port>`, no trailing slash.
    pub base: String,
    hits: Hits,
}

impl Fixture {
    /// Serve `routes`; any other path answers `fallback` (a status).
    pub async fn start(routes: &[(&str, u16, &str)], fallback: u16) -> Self {
        let table: Routes = Arc::new(
            routes
                .iter()
                .map(|(p, s, b)| (p.to_string(), (*s, b.to_string())))
                .collect(),
        );
        let hits: Hits = Arc::new(Mutex::new(Vec::new()));
        let app = axum::Router::new()
            .fallback(serve)
            .with_state((table, hits.clone(), fallback));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind");
        let addr: SocketAddr = listener.local_addr().expect("local addr");
        tokio::spawn(async move {
            axum::serve(listener, app).await.ok();
        });
        Self {
            base: format!("http://{addr}"),
            hits,
        }
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

async fn serve(State((table, hits, fallback)): State<(Routes, Hits, u16)>, uri: Uri) -> Response {
    let path = uri
        .path_and_query()
        .map(|p| p.as_str().to_string())
        .unwrap_or_else(|| uri.path().to_string());
    hits.lock().unwrap().push(path.clone());
    let (status, body) = table
        .get(&path)
        .cloned()
        .unwrap_or((fallback, String::new()));
    let content_type = if body.starts_with('{') || body.starts_with('[') {
        "application/json"
    } else {
        "text/plain"
    };
    (
        StatusCode::from_u16(status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR),
        [(header::CONTENT_TYPE, content_type)],
        body,
    )
        .into_response()
}

/// A P2PKH locking script over `hash160`.
pub fn p2pkh(hash160: [u8; 20]) -> Vec<u8> {
    let mut s = vec![0x76, 0xa9, 0x14];
    s.extend_from_slice(&hash160);
    s.extend_from_slice(&[0x88, 0xac]);
    s
}

/// A version 1 transaction spending `inputs` (`(txid as displayed, vout)`,
/// empty unlocking scripts) into `outputs` (`(satoshis, locking script)`).
pub fn raw_tx(inputs: &[(&str, u32)], outputs: &[(u64, Vec<u8>)]) -> Vec<u8> {
    let mut tx = vec![1, 0, 0, 0];
    tx.push(inputs.len() as u8);
    for (txid, vout) in inputs {
        let mut prev = hex::decode(txid).expect("a txid");
        prev.reverse();
        tx.extend_from_slice(&prev);
        tx.extend_from_slice(&vout.to_le_bytes());
        tx.push(0);
        tx.extend_from_slice(&0xffff_ffffu32.to_le_bytes());
    }
    tx.push(outputs.len() as u8);
    for (satoshis, script) in outputs {
        tx.extend_from_slice(&satoshis.to_le_bytes());
        tx.push(script.len() as u8);
        tx.extend_from_slice(script);
    }
    tx.extend_from_slice(&[0, 0, 0, 0]);
    tx
}

/// The txid (as displayed) of raw transaction bytes.
pub fn txid_of(raw: &[u8]) -> String {
    bsv_sdk::transaction::Transaction::from_binary(raw)
        .expect("a transaction")
        .id()
}
