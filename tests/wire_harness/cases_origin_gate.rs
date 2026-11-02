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
    // The legacy Originator header never stands in for a missing Origin
    // (the client falls back to it; the ruling's rule 1 does not).
    r.case(
        "gate.encrypt.originatorWithoutOrigin",
        "fresh",
        post("/encrypt", encrypt())
            .without("origin")
            .header("originator", "wire.test"),
    )
    .await;
    // An Originator header naming another host than the Origin.
    r.case(
        "gate.encrypt.originatorDisagrees",
        "fresh",
        post("/encrypt", encrypt()).header("originator", "evil.example"),
    )
    .await;
    // One naming the same host, as the reference's Node client sends it.
    let agrees = r
        .case(
            "gate.encrypt.originatorAgrees",
            "fresh",
            post("/encrypt", encrypt()).header("originator", "http://wire.test"),
        )
        .await;
    assert!(agrees
        .body
        .as_ref()
        .is_some_and(|b| b.pointer("/ciphertext").is_some()));
    // A body's top-level originator naming another host.
    let mut claimed = encrypt();
    claimed["originator"] = json!("evil.example");
    r.case(
        "gate.encrypt.bodyOriginatorDisagrees",
        "fresh",
        post("/encrypt", claimed),
    )
    .await;
}
