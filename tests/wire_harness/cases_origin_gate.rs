//! The served doors' browser gate (#411, the owner's word of 2026-11-01,
//! step (b)): each refusal the gate serves, and the agreeing call it
//! admits. Recorded last, so no file before them moves. The worlds admit
//! `http://wire.test` and `https://app.example` (`worlds.rs`).
use serde_json::json;

use super::recorder::{options, post, Recorder};

const UNLISTED: &str = "http://unlisted.test";

fn encrypt() -> serde_json::Value {
    json!({"plaintext": b"wire".to_vec(), "protocolID": [2, "wire test"], "keyID": "1",
           "counterparty": super::recorder::G})
}

pub(crate) async fn record(r: &mut Recorder) {
    // An origin ALLOWED_ORIGINS does not name: the MetaNet Client's denial
    // (400, ERR_PERMISSION_DENIED), no CORS header a page could read it by;
    // under (b) no pending row is written.
    let denied = r
        .case(
            "gate.encrypt.notListed",
            "fresh",
            post("/encrypt", encrypt()).header("origin", UNLISTED),
        )
        .await;
    assert_eq!(denied.value("/code"), "ERR_PERMISSION_DENIED");
    // Its preflight: refused before CORS answers it.
    r.case(
        "gate.options.notListed",
        "fresh",
        options("/encrypt", "POST").header("origin", UNLISTED),
    )
    .await;
}
