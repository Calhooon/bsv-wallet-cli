//! The served doors' browser gate (#411, the owner's word of 2026-11-01,
//! step (b); #414's switch from L5b): each refusal the gate serves, and
//! the agreeing call it admits. Recorded last, so no file before them moves. The worlds admit
//! the originators `wire.test` and `app.example` (`worlds.rs`), by the
//! reference's hostname scope (the owner's word of 2026-11-02).
use serde_json::json;

use super::owner::Owner;
use super::recorder::{get, options, post, Recorder};
use super::worlds::WIRE_BEARER;

const UNLISTED: &str = "http://unlisted.test";

fn encrypt() -> serde_json::Value {
    json!({"plaintext": b"wire".to_vec(), "protocolID": [2, "wire test"], "keyID": "1",
           "counterparty": super::recorder::G})
}

pub(crate) async fn record(r: &mut Recorder) {
    // An origin the seed does not list (#414 L5b, the switch): asked per
    // power, its protocol's call is held on its pending row and the owner
    // denies it: the client's per-request denial (400 ERR_PERMISSION_DENIED,
    // `Permission denied.`), with the echo, so the page reads it. Served
    // from the `pages` world, whose rows no owner's-door fixture lists.
    let denied = r
        .case_owned(
            "gate.encrypt.heldDenied",
            "pages",
            post("/encrypt", encrypt()).header("origin", UNLISTED),
            Some(Owner {
                after_ms: Some(200),
                answer: Some("deny".into()),
                ..Owner::default()
            }),
        )
        .await;
    assert_eq!(denied.value("/message"), "Permission denied.");
    // Its preflight: answered and echoed, so the page's call reaches its
    // prompt.
    r.case(
        "gate.options.unlisted",
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
    record_fields_and_doors(r).await;
}

/// The other fields a call may name an originator in, and the doors past
/// the BRC-100 calls.
async fn record_fields_and_doors(r: &mut Recorder) {
    // A body's top-level originator naming another host.
    let mut claimed = encrypt();
    claimed["originator"] = json!("evil.example");
    r.case(
        "gate.encrypt.bodyOriginatorDisagrees",
        "fresh",
        post("/encrypt", claimed),
    )
    .await;
    // A status door is gated as a BRC-100 call is (the client asks an
    // Origin of every path).
    r.case(
        "gate.getVersion.noOrigin",
        "fresh",
        get("/getVersion").without("origin"),
    )
    .await;
    // The bearer never stands in for an Origin.
    r.case(
        "gate.isAuthenticated.bearerNoOrigin",
        "auth",
        get("/isAuthenticated")
            .without("origin")
            .header("authorization", &format!("Bearer {WIRE_BEARER}")),
    )
    .await;
    // A body naming its originator twice: a disagreement by itself, read by
    // a walk that fails closed (the review of PR 419; serde's derive refused
    // the duplicate and the check had admitted the call).
    let call = encrypt().to_string();
    let twice = format!(
        r#"{},"originator":"wire.test","originator":"evil.example"}}"#,
        call.strip_suffix('}').unwrap_or(&call)
    );
    r.case(
        "gate.encrypt.bodyOriginatorTwice",
        "fresh",
        post("/encrypt", json!({})).raw(&twice),
    )
    .await;
    record_hostname_scope(r).await;
}

/// The key (the owner's word of 2026-11-02): an `Origin` is judged as its
/// originator, and a listed host admits it under any scheme and on every
/// port, as the reference scopes a grant to the hostname.
async fn record_hostname_scope(r: &mut Recorder) {
    // `https://app.example:8443` is `app.example:8443`, admitted by the
    // entry `app.example`; the Originator header in the reference's form
    // agrees.
    let scoped = r
        .case(
            "gate.encrypt.hostnameScope",
            "fresh",
            post("/encrypt", encrypt())
                .header("origin", "https://app.example:8443")
                .header("originator", "app.example:8443"),
        )
        .await;
    assert!(scoped
        .body
        .as_ref()
        .is_some_and(|b| b.pointer("/ciphertext").is_some()));
    // Its preflight from another port of the host: answered, echoed.
    r.case(
        "gate.options.hostnameScope",
        "fresh",
        options("/encrypt", "POST").header("origin", "http://app.example:5173"),
    )
    .await;
    // A body's originator naming the host without the Origin's port (#448):
    // one originator under the hostname key, compared in the lookup form,
    // so the call is admitted.
    let mut hostname = encrypt();
    hostname["originator"] = json!("app.example");
    let agrees = r
        .case(
            "gate.encrypt.bodyOriginatorHostname",
            "fresh",
            post("/encrypt", hostname).header("origin", "https://app.example:8080"),
        )
        .await;
    assert!(agrees
        .body
        .as_ref()
        .is_some_and(|b| b.pointer("/ciphertext").is_some()));
}
