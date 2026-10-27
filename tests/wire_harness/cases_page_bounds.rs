//! The five paging doors' bounds and the unchanged unsigned type refusals.

use super::recorder::{post, Recorder, G};
use serde_json::{json, Value};

/// Append the bounds and type-refusal cases after the existing corpus.
pub(crate) async fn record_page_bounds(r: &mut Recorder) {
    record_limits(r, "listActions", json!({"labels": []})).await;
    record_limits(r, "listOutputs", json!({"basket": "default"})).await;
    r.case(
        "listOutputs.limitZero",
        "fresh",
        post("/listOutputs", json!({"basket": "default", "limit": 0})),
    )
    .await;
    r.case(
        "listOutputs.offsetNegative",
        "fresh",
        post("/listOutputs", json!({"basket": "default", "offset": -1})),
    )
    .await;
    record_limits(
        r,
        "listCertificates",
        json!({"certifiers": [], "types": []}),
    )
    .await;
    record_limits(r, "discoverByIdentityKey", json!({"identityKey": G})).await;
    record_limits(
        r,
        "discoverByAttributes",
        json!({"attributes": {"name": "wire"}}),
    )
    .await;
    r.case(
        "signAction.sequenceNumberAboveType",
        "fresh",
        post(
            "/signAction",
            json!({"spends": {"0": {"unlockingScript": "00", "sequenceNumber": 4_294_967_296u64}}, "reference": "bm8tc3VjaC1yZWZlcmVuY2U="}),
        ),
    )
    .await;
    r.case(
        "listActions.offsetAboveType",
        "fresh",
        post(
            "/listActions",
            json!({"labels": [], "offset": 4_294_967_296u64}),
        ),
    )
    .await;
}

async fn record_limits(r: &mut Recorder, call: &str, args: Value) {
    for (limit, suffix) in [(10000, "limitAtBound"), (10001, "limitAboveBound")] {
        let mut body = args.clone();
        body["limit"] = json!(limit);
        r.case(
            &format!("{call}.{suffix}"),
            "fresh",
            post(&format!("/{call}"), body),
        )
        .await;
    }
}
