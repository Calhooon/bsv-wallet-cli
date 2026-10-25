//! The cases of the host's own routes, the bearer and the preflights.

use super::recorder::{get, options, post, Recorder};
use super::worlds::{MINED, PENDING, UNKNOWN, WIRE_BEARER, WIRE_CALLBACK};

/// The host routes.
pub(crate) async fn record_host_routes(r: &mut Recorder) {
    use serde_json::json;
    r.case("permission-audit.get", "fresh", get("/permission-audit"))
        .await;
    r.case(
        "permission-audit.reset",
        "fresh",
        post("/permission-audit/reset", json!({})),
    )
    .await;
    r.case(
        "permission-audit.afterReset",
        "fresh",
        get("/permission-audit"),
    )
    .await;
    r.case("chain-feed.none", "fresh", get("/chain-feed")).await;
    r.case("chain-feed.frozen", "tracked", get("/chain-feed"))
        .await;
    r.case("tx.mined", "tracked", get(&format!("/tx/{MINED}")))
        .await;
    r.case("tx.pending", "tracked", get(&format!("/tx/{PENDING}")))
        .await;
    r.case("tx.unknown", "tracked", get(&format!("/tx/{UNKNOWN}")))
        .await;
    r.case("tx.notHex", "tracked", get("/tx/nothex")).await;
}

/// Host routes under the exclusive store lock (#203, #226). Appended to the
/// corpus so every existing fixture keeps its name and bytes.
pub(crate) async fn record_host_store_busy(r: &mut Recorder) {
    r.case("chain-feed.storeBusy", "busy-read", get("/chain-feed"))
        .await;
    r.case("tx.storeBusy", "busy-read", get(&format!("/tx/{MINED}")))
        .await;
    r.case(
        "arc-callback.storeBusy",
        "busy-read",
        post(
            "/arc-callback",
            serde_json::json!({"txid": MINED, "txStatus": "REJECTED", "competingTxs": [UNKNOWN]}),
        )
        .without("origin")
        .header("x-callbacktoken", WIRE_CALLBACK),
    )
    .await;
}

/// The ARC callback: disabled, refused, and taken.
pub(crate) async fn record_arc_callback(r: &mut Recorder) {
    use serde_json::json;
    r.case(
        "arc-callback.disabled",
        "fresh",
        post("/arc-callback", json!({"txid": MINED, "txStatus": "MINED"})).without("origin"),
    )
    .await;
    r.case(
        "arc-callback.noToken",
        "callback",
        post("/arc-callback", json!({"txid": MINED, "txStatus": "MINED"})).without("origin"),
    )
    .await;
    r.case(
        "arc-callback.badJson",
        "callback",
        post("/arc-callback", json!({}))
            .without("origin")
            .header("x-callbacktoken", WIRE_CALLBACK)
            .raw("{\"txid\":"),
    )
    .await;
    r.case(
        "arc-callback.unknownTx",
        "callback",
        post(
            "/arc-callback",
            json!({"txid": UNKNOWN, "txStatus": "SEEN_ON_NETWORK"}),
        )
        .without("origin")
        .header("authorization", &format!("Bearer {WIRE_CALLBACK}")),
    )
    .await;
    r.case(
        "arc-callback.badPayload",
        "callback",
        post("/arc-callback", json!({"hello": "wire"}))
            .without("origin")
            .header("x-callbacktoken", WIRE_CALLBACK),
    )
    .await;
    // One byte over the callback's own cap (a body of spaces; the file
    // carries the count).
    r.case(
        "arc-callback.tooLarge",
        "callback",
        post("/arc-callback", json!({}))
            .without("origin")
            .header("x-callbacktoken", WIRE_CALLBACK)
            .fill(1_000_001),
    )
    .await;
}

/// The bearer.
pub(crate) async fn record_bearer(r: &mut Recorder) {
    r.case(
        "auth.isAuthenticated.noBearer",
        "auth",
        get("/isAuthenticated"),
    )
    .await;
    r.case(
        "auth.isAuthenticated.wrongBearer",
        "auth",
        get("/isAuthenticated").header("authorization", "Bearer not-it"),
    )
    .await;
    r.case(
        "auth.isAuthenticated.bearer",
        "auth",
        get("/isAuthenticated").header("authorization", &format!("Bearer {WIRE_BEARER}")),
    )
    .await;
    r.case("auth.chain-feed.noBearer", "auth", get("/chain-feed"))
        .await;
    r.case(
        "cors.isAuthenticated",
        "fresh",
        get("/isAuthenticated").header("origin", "https://app.example"),
    )
    .await;
}

/// A browser's preflight, one per route class: a status route, a
/// BRC-100 call, each host route; and under the bearer (recorded as
/// served: the bearer is checked before CORS answers, so a preflight,
/// which carries no credentials, is refused there; a known defect,
/// docs/WIRE.md).
pub(crate) async fn record_preflights(r: &mut Recorder) {
    for (name, path, method) in [
        ("options.isAuthenticated", "/isAuthenticated", "GET"),
        ("options.createAction", "/createAction", "POST"),
        ("options.permission-audit", "/permission-audit", "GET"),
        (
            "options.permission-audit.reset",
            "/permission-audit/reset",
            "POST",
        ),
        ("options.chain-feed", "/chain-feed", "GET"),
        ("options.arc-callback", "/arc-callback", "POST"),
    ] {
        r.case(name, "fresh", options(path, method)).await;
    }
    r.case(
        "options.tx",
        "fresh",
        options(&format!("/tx/{MINED}"), "GET"),
    )
    .await;
    r.case(
        "options.auth.createAction",
        "auth",
        options("/createAction", "POST"),
    )
    .await;
}
