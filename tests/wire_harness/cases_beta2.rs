//! Additional witnesses for the published beta.2 pin (#277). Existing rows
//! keep their numbers; recording these comes after the original corpus.
use super::recorder::{post, Recorder, CERT_TYPE, G};
use super::worlds::{puzzle_parent, P2PKH, PUZZLE};
use crate::wire_support::BOUND;
use serde_json::json;

pub(crate) async fn record(r: &mut Recorder) {
    sign_bound(r).await;
    labels_and_remittance(r).await;
    certificate_bounds(r).await;
}

async fn sign_bound(r: &mut Recorder) {
    let (puzzle, beef) = puzzle_parent();
    let body = json!({"description": "wire ten byte declaration", "inputBEEF": beef,
        "inputs": [{"outpoint": format!("{}.0", puzzle.id()), "unlockingScriptLength": 10,
            "inputDescription": "the puzzle"}],
        "outputs": [{"lockingScript": P2PKH, "satoshis": PUZZLE.2 - 10, "outputDescription": "an output"}],
        "options": {"signAndProcess": false, "randomizeOutputs": false}});
    let template = r
        .case(
            "createAction.tenByteDeclaration",
            "pin-spend",
            post("/createAction", body.clone()),
        )
        .await;
    let overlong = r.case("signAction.spendLongerThanDeclared", "pin-spend", post("/signAction",
        json!({"reference": BOUND, "spends": {"0": {"unlockingScript": "51".repeat(11)}}, "options": {"noSend": true}}))
        .bind("/reference", &template, "/signableTransaction/reference")).await;
    assert_eq!(overlong.value("/code"), "VALIDATION_ERROR");
    r.case(
        "listActions.afterOverlongSpend",
        "pin-spend",
        post(
            "/listActions",
            json!({"labels": [], "includeInputs": true, "includeInputUnlockingScripts": true}),
        ),
    )
    .await;
    r.case("signAction.shorterThanDeclared", "pin-spend", post("/signAction",
        json!({"reference": BOUND, "spends": {"0": {"unlockingScript": "51"}}, "options": {"noSend": true}}))
        .bind("/reference", &template, "/signableTransaction/reference")).await;
    let review = r
        .case(
            "createAction.doubleSpendReview",
            "review-race",
            post("/createAction", body),
        )
        .await;
    assert_eq!(review.value("/code"), 5);
    assert_eq!(review.value("/isError"), true);
    assert!(review.value("/reviewActionResults").is_array());
    r.case(
        "listActions.afterDoubleSpendReview",
        "review-race",
        post("/listActions", json!({"labels": []})),
    )
    .await;
}

async fn labels_and_remittance(r: &mut Recorder) {
    for (name, labels) in [
        ("expiryUnsupported", vec!["p nosend expiry seconds 300"]),
        ("expiryBatch", vec!["p nosend batch"]),
        ("expiryNever", vec!["p nosend expiry never"]),
        (
            "expiryDuplicate",
            vec!["p nosend expiry seconds 1", "p nosend expiry seconds 2"],
        ),
    ] {
        r.case(&format!("createAction.{name}"), "fresh", post("/createAction", json!({
            "description": "wire expiry grammar", "labels": labels,
            "outputs": [{"lockingScript": P2PKH, "satoshis": 1000, "outputDescription": "an output"}]}))).await;
    }
    for (name, output) in [
        (
            "missingRemittance",
            json!({"outputIndex": 0, "protocol": "basket insertion"}),
        ),
        (
            "exclusiveRemittance",
            json!({"outputIndex": 0, "protocol": "basket insertion",
            "insertionRemittance": {"basket": "wire"}, "paymentRemittance": {
                "derivationPrefix": "eA==", "derivationSuffix": "eA==", "senderIdentityKey": G}}),
        ),
    ] {
        let refused = r
            .case(
                &format!("internalizeAction.{name}"),
                "fresh",
                post(
                    "/internalizeAction",
                    json!({"tx": [], "description": "wire remittance guards", "outputs": [output]}),
                ),
            )
            .await;
        assert_eq!(refused.value("/code"), "VALIDATION_ERROR");
    }
}

async fn certificate_bounds(r: &mut Recorder) {
    // Direct acquisition validates the shape but does not call Certificate's
    // writer or decrypt fields (toolbox #374). Pin that boundary explicitly.
    for (name, n, serial) in [
        ("fieldAtBound", 1_048_576, 8),
        ("fieldAboveBound", 1_048_577, 9),
    ] {
        let certificate = r.case(&format!("acquireCertificate.{name}"), "fresh", post("/acquireCertificate",
            json!({"type": CERT_TYPE, "certifier": G, "acquisitionProtocol": "direct",
                "serialNumber": bsv_sdk::primitives::to_base64(&[serial; 32]),
                "signature": "3006020101020101", "revocationOutpoint": format!("{}.0", "33".repeat(32)),
                "fields": {"name": "A".repeat(n)}, "keyringRevealer": "certifier", "keyringForSubject": {}}))).await;
        assert_eq!(certificate.value("/type"), CERT_TYPE);
    }
    let refused = r
        .case(
            "acquireCertificate.signatureAboveBound",
            "fresh",
            post(
                "/acquireCertificate",
                json!({"type": CERT_TYPE, "certifier": G, "acquisitionProtocol": "direct",
            "serialNumber": bsv_sdk::primitives::to_base64(&[10; 32]),
            "signature": "01".repeat(73), "revocationOutpoint": format!("{}.0", "33".repeat(32)),
            "fields": {}, "keyringRevealer": "certifier", "keyringForSubject": {}}),
            ),
        )
        .await;
    assert_eq!(refused.value("/code"), "VALIDATION_ERROR");
}
