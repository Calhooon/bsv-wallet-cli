//! Toolbox #240's served privacy boundaries, after all existing fixture numbers.
use super::recorder::{post, Recorder, CERT_TYPE, G};
use crate::wire_support::BOUND;
use serde_json::json;

pub(crate) async fn record(r: &mut Recorder) {
    let acquired = r
        .case(
            "acquireCertificate.revealFields",
            "certificate-privacy",
            post(
                "/acquireCertificate",
                super::certificate_material::body(11, b"private"),
            ),
        )
        .await;
    assert_eq!(acquired.value("/type"), CERT_TYPE);
    let empty = r
        .case(
            "proveCertificate.emptyFields",
            "certificate-privacy",
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
            "certificate-privacy",
            post(
                "/proveCertificate",
                json!({
                    "certificate": certificate, "verifier": G, "fieldsToReveal": []
                }),
            ),
        )
        .await;
    assert_eq!(short.value("/code"), "VALIDATION_ERROR");
    assert_eq!(
        short.value("/message"),
        "Validation error: The certificate.serialNumber parameter must be at least 32 bytes"
    );
}
