//! Toolbox #240's served privacy boundaries, after all existing fixture numbers.
use super::recorder::{post, Recorder, CERT_TYPE, G};
use crate::wire_support::BOUND;
use serde_json::json;

pub(crate) async fn record(r: &mut Recorder) {
    // Public fixture material: AES-GCM with fixed 32-byte nonces, field key
    // [13; 32], and the wire world's root. The master key uses BRC-42 protocol
    // [2, "certificate field encryption"], key ID "name", counterparty G.
    let acquired = r.case("acquireCertificate.revealFields", "fresh", post("/acquireCertificate", json!({
        "type": CERT_TYPE, "serialNumber": bsv_sdk::primitives::to_base64(&[11; 32]),
        "certifier": G, "acquisitionProtocol": "direct", "keyringRevealer": "certifier",
        "revocationOutpoint": format!("{}.0", "33".repeat(32)), "signature": "3006020101020101",
        "fields": {"name": "Dg4ODg4ODg4ODg4ODg4ODg4ODg4ODg4ODg4ODg4ODg682dOrgyFyWensRFxTzrqpydQXlSJQWqZPBQA2aQ=="},
        "keyringForSubject": {"name": "Dw8PDw8PDw8PDw8PDw8PDw8PDw8PDw8PDw8PDw8PDw+yjKuVcDbO9efkiFXVtspIbhATgrV7c2cLwYAsCCu84144ctRgR9bnD68C86pkbKM="}
    }))).await;
    assert_eq!(acquired.value("/type"), CERT_TYPE);
    let empty = r
        .case(
            "proveCertificate.emptyFields",
            "fresh",
            post(
                "/proveCertificate",
                json!({
                    "certificate": BOUND, "verifier": G, "fieldsToReveal": []
                }),
            )
            .bind("/certificate", &acquired, ""),
        )
        .await;
    assert_eq!(empty.value("/keyringForVerifier"), json!({}));

    let mut certificate = acquired.value("");
    certificate["serialNumber"] = json!(bsv_sdk::primitives::to_base64(&[11; 31]));
    let short = r
        .case(
            "proveCertificate.shortSerial",
            "fresh",
            post(
                "/proveCertificate",
                json!({
                    "certificate": certificate, "verifier": G, "fieldsToReveal": []
                }),
            ),
        )
        .await;
    assert_eq!(short.value("/code"), "VALIDATION_ERROR");
    assert_eq!(short.value("/message"), "wallet error: Validation error: The certificate.serialNumber parameter must be at least 32 bytes");
}
