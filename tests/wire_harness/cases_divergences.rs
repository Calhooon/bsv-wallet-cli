//! #122's divergence rows: a body sent without a JSON content type, which
//! axum refused as 415 in text and the doors refuse as every other request
//! they cannot read (item 3). Recorded last, so no file before it moves.
use super::recorder::{post, Recorder};
use serde_json::json;

pub(crate) async fn record(r: &mut Recorder) {
    r.case(
        "createSignature.noContentType",
        "fresh",
        post("/createSignature", json!({}))
            .raw("{\"data\":[1],\"protocolID\":[2,\"wire test\"],\"keyID\":\"1\",\"counterparty\":\"self\"}")
            .header("content-type", "text/plain"),
    )
    .await;
}
