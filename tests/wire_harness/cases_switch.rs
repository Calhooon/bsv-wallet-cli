//! The switch (#414, the design's L5b, `docs/design/origin-grants.md`
//! section (2)'s fixtures): a page the seed does not list, asked per power,
//! each answer as the door serves it. Served from the `pages` world (the
//! shutdown's from `shutdown`), so no earlier fixture's rows move; recorded
//! last, so no file before them moves.
use serde_json::json;

use super::owner::{Act, Owner};
use super::recorder::{get, post, Recorder, G};

/// A page the seed does not list.
const PAGE: &str = "http://page.test";

/// A page's listOutputs of `basket`.
fn listing(basket: &str) -> crate::wire_support::Req {
    post("/listOutputs", json!({"basket": basket})).header("origin", PAGE)
}

fn grant(kind: &str, key: &str, expiry: i64) -> Act {
    Act::Grant {
        originator: "page.test".into(),
        kind: kind.into(),
        key: Some(key.into()),
        expiry,
    }
}

fn answered(answer: &str) -> Option<Owner> {
    Some(Owner {
        after_ms: Some(200),
        answer: Some(answer.into()),
        ..Owner::default()
    })
}

pub(crate) async fn record(r: &mut Recorder) {
    // A held basket call the owner grants: the door's own 200, echoed.
    let granted = r
        .case_owned(
            "switch.listOutputs.heldGranted",
            "pages",
            listing("wire basket"),
            answered("grant"),
        )
        .await;
    assert!(granted
        .body
        .as_ref()
        .is_some_and(|b| b.pointer("/outputs").is_some()));
    // A revoked power: 400 ERR_PERMISSION_DENIED in the client's grouped
    // denial at once, no pending row.
    r.case_owned(
        "switch.listOutputs.revoked",
        "pages",
        listing("revoked basket"),
        Some(Owner {
            before: vec![
                grant("basket", r#"["revoked basket"]"#, 0),
                Act::Revoke {
                    originator: "page.test".into(),
                    kind: Some("basket".into()),
                    key: Some(r#"["revoked basket"]"#.into()),
                },
            ],
            ..Owner::default()
        }),
    )
    .await;
    // An expired grant: held as a renewal, granted, the door's answer.
    let renewed = r
        .case_owned(
            "switch.createSignature.expiredHeldGranted",
            "pages",
            post(
                "/createSignature",
                json!({"protocolID": [2, "wire renewal"], "keyID": "1", "counterparty": G,
                       "data": [1, 2, 3]}),
            )
            .header("origin", PAGE),
            Some(Owner {
                before: vec![grant(
                    "protocol",
                    &format!(r#"[false,2,"wire renewal","{G}"]"#),
                    1,
                )],
                after_ms: Some(200),
                answer: Some("grant".into()),
                ..Owner::default()
            }),
        )
        .await;
    assert!(renewed
        .body
        .as_ref()
        .is_some_and(|b| b.pointer("/signature").is_some()));
    record_open_and_refused(r).await;
}

/// W15 (i), the admin-reserved names, the cap, a host route and the
/// shutdown.
async fn record_open_and_refused(r: &mut Recorder) {
    // An open door, from a page holding no row: answered, echoed (W15).
    r.case(
        "switch.getVersion.open",
        "pages",
        get("/getVersion").header("origin", PAGE),
    )
    .await;
    // A level-0 protocol, from a page holding no row (W15, BRC-100 line 65).
    let open = r
        .case(
            "switch.createSignature.levelZero",
            "pages",
            post(
                "/createSignature",
                json!({"protocolID": [0, "wire open"], "keyID": "1", "counterparty": "self",
                       "data": [1, 2, 3]}),
            )
            .header("origin", PAGE),
        )
        .await;
    assert!(open
        .body
        .as_ref()
        .is_some_and(|b| b.pointer("/signature").is_some()));
    // An admin-reserved basket: refused at once in the reference's words.
    r.case(
        "switch.listOutputs.adminBasket",
        "pages",
        listing("default"),
    )
    .await;
    // The cap: two held calls of the page, the third answered 429 at once.
    r.case_owned(
        "switch.listOutputs.pendingLimit",
        "pages",
        listing("third basket"),
        Some(Owner {
            holding: vec![listing("first basket"), listing("second basket")],
            ..Owner::default()
        }),
    )
    .await;
    // A host route from a page the seed does not list: no kind names it.
    r.case(
        "switch.permission-audit.page",
        "pages",
        get("/permission-audit").header("origin", PAGE),
    )
    .await;
    // The server's shutdown while the page's call is held: 503
    // WALLET_BRIDGE_UNAVAILABLE in the port's carrier (W13).
    r.case_owned(
        "switch.listOutputs.heldAtShutdown",
        "shutdown",
        listing("wire basket"),
        answered("shutdown"),
    )
    .await;
}
