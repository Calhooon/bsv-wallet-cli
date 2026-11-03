//! The owner's door (#414, the design's L3): the four host routes over the
//! per-origin grant rows, each admitted from the owner's side (no
//! `Origin`, a loopback `Host`) and each of the admission's refusals.
//! Recorded last, so no file before them moves. The worlds' gate is
//! unchanged: the doors still admit by `ALLOWED_ORIGINS`.
use serde_json::json;

use super::recorder::{get, options, post, Recorder, ORIGIN};
use crate::wire_support::Req;

/// The owner's side: no `Origin`, the loopback `Host` the daemon is
/// asked by.
fn owner(req: Req) -> Req {
    req.without("origin").header("host", "127.0.0.1:3322")
}

pub(crate) async fn record(r: &mut Recorder) {
    // The listing of a wallet with no rows.
    r.case("owner.origins.empty", "fresh", owner(get("/origins")))
        .await;
    // A direct grant of one power (a basket, its canonical key), its row
    // in section (4)'s keys; then listed.
    let grant = json!({
        "originator": "owner.example:8443",
        "kind": "basket",
        "key": "[\"todo items\"]"
    });
    let granted = r
        .case("owner.grant", "fresh", owner(post("/origins/grant", grant)))
        .await;
    assert_eq!(granted.value("/grants/0/originator"), "owner.example");
    assert_eq!(granted.value("/grants/0/key"), "[\"todo items\"]");
    r.case("owner.origins.granted", "fresh", owner(get("/origins")))
        .await;
    // The revoke with no kind: every live row of the host (its one basket
    // row) stamped, none deleted.
    let revoked = r
        .case(
            "owner.revoke",
            "fresh",
            owner(post(
                "/origins/revoke",
                json!({"originator": "owner.example"}),
            )),
        )
        .await;
    assert!(revoked.value("/grants/0/revokedAt").is_number());
    // An answer to a row the store does not hold.
    r.case(
        "owner.answer.noSuchPending",
        "fresh",
        owner(post(
            "/origins/answer",
            json!({"pendingId": 1, "answer": "deny"}),
        )),
    )
    .await;
    record_refusals(r).await;
}

/// The admission's refusals: a page, its preflight, a `Host` beyond
/// loopback, and no bearer under `AUTH_TOKEN`.
async fn record_refusals(r: &mut Recorder) {
    r.case(
        "owner.origins.page",
        "fresh",
        owner(get("/origins")).header("origin", ORIGIN),
    )
    .await;
    r.case(
        "owner.options.page",
        "fresh",
        owner(options("/origins/answer", "POST")).header("origin", ORIGIN),
    )
    .await;
    r.case(
        "owner.origins.remoteHost",
        "fresh",
        owner(get("/origins")).header("host", "wallet.evil.example"),
    )
    .await;
    r.case("owner.origins.noBearer", "auth", owner(get("/origins")))
        .await;
}
