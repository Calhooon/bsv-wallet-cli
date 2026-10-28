//! Beta.3 package facts: argument refusals and a decryptable certificate bound.
use super::recorder::{post, Recorder, CERT_TYPE, G};
use super::worlds::{stranger_key, stranger_payment, Payment, P2PKH, PAYMENT_PREFIX};
use crate::wire_support::ROOT_KEY;
use serde_json::json;

pub(crate) async fn record(r: &mut Recorder) {
    crypto_arguments(r).await;
    description_arguments(r).await;
    internalize_order(r).await;
    linkage_aliases(r).await;
    // bsv-rs 0.9.0-beta.3 src/auth/certificates/bounds.rs:26 and
    // master.rs:325-329: exactly MAX_DECRYPTED_FIELD_BYTES is accepted (#343).
    let acquired = r
        .case(
            "acquireCertificate.decryptedFieldAtBound",
            "fresh",
            post(
                "/acquireCertificate",
                super::certificate_material::body(12, &vec![b'x'; 65_536]),
            ),
        )
        .await;
    assert_eq!(acquired.value("/type"), CERT_TYPE);
    let stored = r
        .case(
            "listCertificates.decryptedFieldAtBound",
            "fresh",
            post(
                "/listCertificates",
                json!({"certifiers": [G], "types": [CERT_TYPE]}),
            ),
        )
        .await;
    assert_eq!(stored.value("/totalCertificates"), 1);
}

async fn crypto_arguments(r: &mut Recorder) {
    for method in [
        "createSignature",
        "verifySignature",
        "createHmac",
        "verifyHmac",
    ] {
        let mut body =
            json!({"protocolID": [2, "a"], "keyID": "1", "counterparty": G, "data": [1]});
        if method == "verifySignature" {
            body["signature"] = json!([48, 6, 2, 1, 1, 2, 1, 1]);
        }
        if method == "verifyHmac" {
            body["hmac"] = json!(vec![0; 32]);
        }
        let refused = r
            .case(
                &format!("{method}.shortProtocol"),
                "fresh",
                post(&format!("/{method}"), body),
            )
            .await;
        assert_eq!(refused.value("/code"), "VALIDATION_ERROR");
    }
    let refused = r.case("verifySignature.malformedDer", "fresh", post("/verifySignature",
        json!({"protocolID": [2,"wire test"], "keyID": "1", "counterparty": G, "data": [1], "signature": [1,2,3]}))).await;
    assert_eq!(refused.value("/code"), "VALIDATION_ERROR");
}

async fn description_arguments(r: &mut Recorder) {
    for (name, length) in [("descriptionTooShort", 4), ("descriptionTooLong", 2001)] {
        let refused = r
            .case(
                &format!("createAction.{name}"),
                "fresh",
                post(
                    "/createAction",
                    json!({"description": "x".repeat(length), "outputs": [{"lockingScript": P2PKH,
                "satoshis":1000,"outputDescription":"an output"}]}),
                ),
            )
            .await;
        assert_eq!(refused.value("/code"), "VALIDATION_ERROR");
    }
}

async fn internalize_order(r: &mut Recorder) {
    let key = bsv_sdk::primitives::PrivateKey::from_hex(ROOT_KEY).unwrap();
    let (_, atomic) = stranger_payment(Payment::Wallet, &key).await;
    let refused = r.case("internalizeAction.badLabelAndDerivation", "funded", post("/internalizeAction",
        json!({"tx":atomic, "description":"wire validator order", "labels":["p nosend expiry never"],
            "outputs":[{"outputIndex":0,"protocol":"wallet payment","paymentRemittance":{
                "derivationPrefix":PAYMENT_PREFIX,"derivationSuffix":"d3Jvbmc=",
                "senderIdentityKey":stranger_key().public_key().to_hex()}}]}))).await;
    assert_eq!(refused.value("/code"), "VALIDATION_ERROR");
    let (_, atomic) = stranger_payment(Payment::Wallet, &key).await;
    let fault = r.case("internalizeAction.trackerTransportFault", "tracker-fault", post("/internalizeAction",
        json!({"tx":atomic,"description":"wire typed tracker fault","outputs":[{
            "outputIndex":0,"protocol":"wallet payment","paymentRemittance":{
                "derivationPrefix":super::worlds::PAYMENT_PREFIX,"derivationSuffix":super::worlds::PAYMENT_SUFFIX,
                "senderIdentityKey":stranger_key().public_key().to_hex()}}]}))).await;
    assert_eq!(fault.value("/code"), "SERVICE_ERROR");
    assert_eq!(refused.value("/message"), "Validation error: The labels parameter must be BRC-177 noSend expiry labels only on outgoing createAction requests");
}

async fn linkage_aliases(r: &mut Recorder) {
    let identity = bsv_sdk::primitives::PrivateKey::from_hex(ROOT_KEY)
        .unwrap()
        .public_key()
        .to_hex();
    for (alias, expected) in [("anyone", G.to_string()), ("self", identity)] {
        let result = r.case(&format!("revealSpecificKeyLinkage.{alias}"), "fresh", post("/revealSpecificKeyLinkage",
            json!({"protocolID":[2,"wire test"],"keyID":"1","counterparty":alias,"verifier":G}))).await;
        assert_eq!(result.value("/counterparty"), expected);
    }
    let refused = r
        .case(
            "internalizeAction.protocolBeforeDescription",
            "fresh",
            post(
                "/internalizeAction",
                json!({"tx":[],"description":"x","outputs":[{"outputIndex":0,"protocol":"foo"}]}),
            ),
        )
        .await;
    assert_eq!(refused.value("/code"), "VALIDATION_ERROR");
    assert_eq!(
        refused.value("/message"),
        "Validation error: The protocol parameter must be 'basket insertion' or 'wallet payment'"
    );
}
