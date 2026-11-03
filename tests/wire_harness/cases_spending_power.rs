//! The spending power (#414, the design's L6, sections (1) and (2), W6):
//! a page the seed does not list spends from the `spending` world, its
//! `createAction` held on a spending row and answered by the owner, each
//! answer as the door serves it. Each call asks for exactly a coin less the
//! toolbox's fee (`exact`), so no change output is drawn at random; the
//! toolbox takes the largest coin first, so the cases spend them in order.
//! Recorded last, so no file before them moves.
use serde_json::json;

use super::cases::exact;
use super::owner::Owner;
use super::recorder::{post, Recorder};
use super::worlds::P2PKH;

/// A page the seed does not list.
const PAGE: &str = "http://page.test";

/// The page's broadcast spend of `satoshis`.
fn spend(satoshis: u64) -> crate::wire_support::Req {
    post(
        "/createAction",
        json!({"description": "wire spending power",
            "outputs": [{"lockingScript": P2PKH, "satoshis": satoshis,
                "outputDescription": "an output"}],
            "options": {"acceptDelayedBroadcast": false, "randomizeOutputs": false}}),
    )
    .header("origin", PAGE)
}

fn answered(answer: &str, amount: Option<i64>) -> Option<Owner> {
    Some(Owner {
        after_ms: Some(200),
        answer: Some(answer.into()),
        amount,
        ..Owner::default()
    })
}

pub(crate) async fn record(r: &mut Recorder) {
    // No spending row: held on a row of its own, answered once; the door's
    // own 200, echoed, and no grant row written.
    let once = r
        .case_owned(
            "spending.createAction.heldOnce",
            "spending",
            spend(exact(0)),
            answered("once", None),
        )
        .await;
    assert!(once
        .body
        .as_ref()
        .is_some_and(|b| b.pointer("/txid").is_some()));
}
