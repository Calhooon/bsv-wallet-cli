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
    super::cases_certificate_privacy::record(r).await;
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
    // This signed 1 MiB text passes construction and verification. bsv-rs
    // 0.9.0-beta.3 src/auth/certificates/bounds.rs:26 defines the 64 KiB
    // plaintext bound applied at master.rs:325-329; its 786,384 plaintext
    // bytes exceed that bound during toolbox #374's keyring proof (#343).
    let at_bound = super::certificate_material::body(8, &vec![b'x'; 786_384]);
    assert_eq!(
        at_bound["fields"]["name"].as_str().unwrap().len(),
        1_048_576
    );
    for (name, body) in [
        ("fieldAtBound", at_bound),
        ("fieldAboveBound", {
            let mut body = super::certificate_material::body(9, b"wire");
            body["fields"]["name"] = json!("A".repeat(1_048_577));
            body
        }),
    ] {
        let refused = r
            .case(
                &format!("acquireCertificate.{name}"),
                "fresh",
                post("/acquireCertificate", body),
            )
            .await;
        assert_eq!(refused.value("/code"), "VALIDATION_ERROR");
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
