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

/// A `WalletServices` whose `is_utxo_with_spender` (toolbox 0.13.0,
/// bsv-wallet-toolbox-rs #44) answers as given. The toolbox's mock has no
/// hook for the bound spender, so the spend probe's tests hand the ladder's
/// answer here and read back what was asked; every other method is the
/// inner mock's, whose `call_count` reads through [`SpenderServices::inner`]
/// for the calls the probe must not make itself.
pub(crate) mod spender_services {
    use std::sync::{Arc, Mutex};

    use async_trait::async_trait;
    use bsv_sdk::transaction::ChainTracker;
    use bsv_wallet_toolbox::services::broadcast_memory::BroadcastMemory;
    use bsv_wallet_toolbox::services::mock::MockWalletServices;
    use bsv_wallet_toolbox::services::{
        BlockHeader, BoundSpender, BroadcastStatus, FiatCurrency, GetBeefResult,
        GetMerklePathResult, GetRawTxResult, GetStatusForTxidsResult, GetUtxoStatusOutputFormat,
        GetUtxoStatusResult, NLockTimeInput, PostBeefResult, ServicesCallHistory, UtxoLeads,
        UtxoVerdict, WalletServices,
    };
    use bsv_wallet_toolbox::Result;

    pub struct SpenderServices {
        inner: MockWalletServices,
        answer: (UtxoVerdict, Option<BoundSpender>),
        /// Each `is_utxo_with_spender` call: the outpoint and the script it
        /// was asked under.
        asked: Mutex<Vec<(String, u32, Vec<u8>)>>,
    }

    impl SpenderServices {
        /// The ladder's answer for every outpoint asked.
        pub fn answering(verdict: UtxoVerdict, spender: Option<BoundSpender>) -> Self {
            Self {
                inner: MockWalletServices::new(),
                answer: (verdict, spender),
                asked: Mutex::new(Vec::new()),
            }
        }

        /// The outpoints asked, each with the script it was asked under.
        pub fn asked(&self) -> Vec<(String, u32, Vec<u8>)> {
            self.asked.lock().expect("the asked list").clone()
        }

        /// The inner mock, for the calls the probe must not make itself.
        pub fn inner(&self) -> &MockWalletServices {
            &self.inner
        }
    }

    #[async_trait]
    impl WalletServices for SpenderServices {
        async fn get_chain_tracker(&self) -> Result<&dyn ChainTracker> {
            self.inner.get_chain_tracker().await
        }

        async fn get_height(&self) -> Result<u32> {
            self.inner.get_height().await
        }

        async fn get_header_for_height(&self, height: u32) -> Result<Vec<u8>> {
            self.inner.get_header_for_height(height).await
        }

        async fn get_chain_tip_header(&self) -> Result<BlockHeader> {
            self.inner.get_chain_tip_header().await
        }

        async fn hash_to_header(&self, hash: &str) -> Result<BlockHeader> {
            self.inner.hash_to_header(hash).await
        }

        async fn get_raw_tx(&self, txid: &str, use_next: bool) -> Result<GetRawTxResult> {
            self.inner.get_raw_tx(txid, use_next).await
        }

        async fn get_merkle_path(&self, txid: &str, use_next: bool) -> Result<GetMerklePathResult> {
            self.inner.get_merkle_path(txid, use_next).await
        }

        async fn post_beef(&self, beef: &[u8], txids: &[String]) -> Result<Vec<PostBeefResult>> {
            self.inner.post_beef(beef, txids).await
        }

        fn set_broadcast_memory(&self, memory: Arc<dyn BroadcastMemory>) {
            self.inner.set_broadcast_memory(memory)
        }

        fn broadcast_memory(&self) -> Option<Arc<dyn BroadcastMemory>> {
            self.inner.broadcast_memory()
        }

        async fn get_utxo_status(
            &self,
            output: &str,
            output_format: Option<GetUtxoStatusOutputFormat>,
            outpoint: Option<&str>,
            use_next: bool,
        ) -> Result<GetUtxoStatusResult> {
            self.inner
                .get_utxo_status(output, output_format, outpoint, use_next)
                .await
        }

        async fn get_status_for_txids(
            &self,
            txids: &[String],
            use_next: bool,
        ) -> Result<GetStatusForTxidsResult> {
            self.inner.get_status_for_txids(txids, use_next).await
        }

        async fn get_bsv_exchange_rate(&self) -> Result<f64> {
            self.inner.get_bsv_exchange_rate().await
        }

        async fn get_fiat_exchange_rate(
            &self,
            currency: FiatCurrency,
            base: Option<FiatCurrency>,
        ) -> Result<f64> {
            self.inner.get_fiat_exchange_rate(currency, base).await
        }

        fn hash_output_script(&self, script: &[u8]) -> String {
            self.inner.hash_output_script(script)
        }

        async fn is_utxo(&self, txid: &str, vout: u32, locking_script: &[u8]) -> UtxoVerdict {
            self.inner.is_utxo(txid, vout, locking_script).await
        }

        async fn is_utxo_with_spender(
            &self,
            txid: &str,
            vout: u32,
            locking_script: &[u8],
        ) -> (UtxoVerdict, Option<BoundSpender>) {
            self.asked.lock().expect("the asked list").push((
                txid.to_string(),
                vout,
                locking_script.to_vec(),
            ));
            self.answer.clone()
        }

        async fn get_utxo_leads(
            &self,
            script_hash: &str,
            output_format: Option<GetUtxoStatusOutputFormat>,
        ) -> Result<UtxoLeads> {
            self.inner.get_utxo_leads(script_hash, output_format).await
        }

        async fn n_lock_time_is_final(&self, n_lock_time: u32) -> Result<bool> {
            self.inner.n_lock_time_is_final(n_lock_time).await
        }

        async fn n_lock_time_is_final_for_tx(&self, input: NLockTimeInput) -> Result<bool> {
            self.inner.n_lock_time_is_final_for_tx(input).await
        }

        async fn get_beef(&self, txid: &str, known_txids: &[String]) -> Result<GetBeefResult> {
            self.inner.get_beef(txid, known_txids).await
        }

        fn get_services_call_history(&self, reset: bool) -> ServicesCallHistory {
            self.inner.get_services_call_history(reset)
        }

        async fn get_broadcaster_statuses(&self, txid: &str) -> Vec<(String, BroadcastStatus)> {
            self.inner.get_broadcaster_statuses(txid).await
        }
    }
}
