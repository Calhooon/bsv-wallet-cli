//! The `funded` worlds' local ARC (#147): it takes every post as the
//! toolbox's classic ARC client sends it and logs each request it takes.

use axum::extract::State;
use axum::routing::post;
use axum::Router;
use bsv_sdk::transaction::{Beef, BeefTx, Transaction};

use crate::wire_support::serve_local;

/// The requests a world's local ARC took: each one's path and the txids
/// its body carried (a BEEF's transactions in order, or a raw
/// transaction's own), read by the cases that count posts (#147).
pub(crate) type ArcPosts = std::sync::Arc<std::sync::Mutex<Vec<(String, Vec<String>)>>>;

/// A local ARC that takes every post (the mock services'
/// `post_beef_success`, over the wire: the server's state holds the real
/// `Services`): `POST /v1/tx` answers `SEEN_ON_NETWORK` for the posted
/// transaction, the BEEF's subject or the raw transaction's own txid. Every
/// other request is a 404. Each request is logged in `posts`.
pub(crate) async fn arc_stub(posts: ArcPosts) -> String {
    let app = Router::new()
        .route("/v1/tx", post(take))
        .fallback(other)
        .with_state(posts);
    serve_local(app).await
}

/// A post's transaction bytes: a JSON body's `rawTx` hex, else the body.
fn bytes_of(body: &[u8]) -> Vec<u8> {
    serde_json::from_slice::<serde_json::Value>(body)
        .ok()
        .and_then(|v| v["rawTx"].as_str().and_then(|h| hex::decode(h).ok()))
        .unwrap_or_else(|| body.to_vec())
}

/// The txid the stub answers: the BEEF's subject or the raw transaction's
/// own txid (empty for anything else, an EF among them).
fn answered(bytes: &[u8]) -> String {
    match Beef::from_binary(bytes) {
        Ok(beef) => beef.txs.last().map(BeefTx::txid).unwrap_or_default(),
        Err(_) => Transaction::from_binary(bytes)
            .as_ref()
            .map(Transaction::id)
            .unwrap_or_default(),
    }
}

/// The txids the log records: a BEEF's transactions in order, else the one
/// transaction, raw or EF.
fn txids(body: &[u8]) -> Vec<String> {
    let bytes = bytes_of(body);
    if let Ok(beef) = Beef::from_binary(&bytes) {
        return beef.txs.iter().map(BeefTx::txid).collect();
    }
    Transaction::from_binary(&bytes)
        .or_else(|_| Transaction::from_ef(&bytes))
        .as_ref()
        .map(Transaction::id)
        .into_iter()
        .collect()
}

/// `POST /v1/tx`: taken, logged.
async fn take(
    State(posts): State<ArcPosts>,
    body: axum::body::Bytes,
) -> axum::Json<serde_json::Value> {
    let txid = answered(&bytes_of(&body));
    posts
        .lock()
        .unwrap()
        .push(("/v1/tx".to_string(), txids(&body)));
    axum::Json(serde_json::json!({"txid": txid, "txStatus": "SEEN_ON_NETWORK"}))
}

/// Any other request: a 404, logged.
async fn other(
    State(posts): State<ArcPosts>,
    uri: axum::http::Uri,
    body: axum::body::Bytes,
) -> axum::http::StatusCode {
    posts
        .lock()
        .unwrap()
        .push((uri.path().to_string(), txids(&body)));
    axum::http::StatusCode::NOT_FOUND
}
