//! #122's divergence rows (item 3): a JSON body is the call under any
//! content type, as the MetaNet Client reads it (express.text for every
//! type, then JSON.parse). Recorded last, so no file before it moves.
use super::recorder::{post, Recorder};
use serde_json::json;

pub(crate) async fn record(r: &mut Recorder) {
    r.case(
        "createSignature.textPlain",
        "fresh",
        post("/createSignature", json!({}))
            .raw("{\"data\":[1],\"protocolID\":[2,\"wire test\"],\"keyID\":\"1\",\"counterparty\":\"self\"}")
            .header("content-type", "text/plain"),
    )
    .await;
}
