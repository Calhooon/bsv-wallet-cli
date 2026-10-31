//! The certificate endpoints: listing, the full lifecycle, the unknown certificate.

use crate::support::{post_json, post_json_ok, setup};
use serde_json::{json, Value};

/// A signed certificate and decryptable keyring for the served identity.
pub(crate) async fn signed_acquisition(
    client: &reqwest::Client,
    base: &str,
    serial: [u8; 32],
) -> Value {
    use bsv_sdk::auth::certificates::{Certificate, MasterCertificate};
    use bsv_sdk::primitives::{to_base64, PrivateKey, PublicKey};
    use bsv_sdk::wallet::{Outpoint, ProtoWallet};

    let identity = post_json_ok(
        client,
        &format!("{base}/getPublicKey"),
        json!({"identityKey":true}),
    )
    .await;
    let subject = PublicKey::from_hex(identity["publicKey"].as_str().unwrap()).unwrap();
    let issuer = PrivateKey::from_hex(&format!("{:064x}", 1)).unwrap();
    let (fields, keyring) = MasterCertificate::create_certificate_fields(
        &ProtoWallet::new(Some(issuer.clone())),
        &subject,
        [("name".into(), "test".into())].into_iter().collect(),
        None,
        None,
        None,
    )
    .await
    .unwrap();
    let mut cert = Certificate::new([6; 32], serial, subject, issuer.public_key())
        .with_revocation_outpoint(Some(Outpoint::new([0x33; 32], 0)))
        .with_fields(fields.clone());
    cert.sign(&issuer).unwrap();
    assert!(cert.verify().unwrap());
    json!({"type":to_base64(&[6;32]),"certifier":issuer.public_key().to_hex(),
        "acquisitionProtocol":"direct","fields":fields,"serialNumber":to_base64(&serial),
        "revocationOutpoint":format!("{}.0","33".repeat(32)),
        "signature":hex::encode(cert.signature.unwrap()),
        "keyringRevealer":"certifier","keyringForSubject":keyring})
}

// =============================================================================
// Batch 5: Certificates
// =============================================================================

#[tokio::test]
async fn test_list_certificates_empty() {
    let (base, client, _tmp) = setup().await;
    let resp = post_json(
        &client,
        &format!("{base}/listCertificates"),
        json!({"certifiers": [], "types": []}),
    )
    .await;
    assert_eq!(resp.status(), 200);
    let body: Value = resp.json().await.unwrap();
    assert_eq!(body["totalCertificates"], 0);
    assert!(body["certificates"].is_array());
}

// =============================================================================
// Batch 9: Certificates — acquireCertificate, proveCertificate,
//          relinquishCertificate
// =============================================================================

/// Full certificate lifecycle: acquire → list (find it) → relinquish → list (verify gone).
/// All operations are DB-only — no chain interaction or funding needed.
#[tokio::test]
async fn test_certificate_full_lifecycle() {
    let (base, client, _tmp) = setup().await;
    let cert_type = "BgYGBgYGBgYGBgYGBgYGBgYGBgYGBgYGBgYGBgYGBgY="; // base64("test")
    let certifier = "0279be667ef9dcbbac55a06295ce870b07029bfcdb2dce28d959f2815b16f81798";

    // Step 1: Acquire certificate (direct protocol)
    let cert = post_json_ok(
        &client,
        &format!("{base}/acquireCertificate"),
        signed_acquisition(&client, &base, [7; 32]).await,
    )
    .await;
    assert!(cert["type"].is_string(), "should have certificateType");
    assert!(cert["certifier"].is_string(), "should have certifier");
    assert!(cert["subject"].is_string(), "should have subject");
    let serial = cert["serialNumber"]
        .as_str()
        .expect("cert should have serialNumber");

    // Step 2: List certificates — should find our cert
    let list = post_json_ok(
        &client,
        &format!("{base}/listCertificates"),
        json!({
            "certifiers": [certifier],
            "types": [cert_type]
        }),
    )
    .await;
    assert!(
        list["totalCertificates"].as_u64().unwrap() >= 1,
        "should find at least 1 certificate after acquisition"
    );
    let certs = list["certificates"].as_array().expect("certificates array");
    // Each entry has { certificate: { serialNumber, ... }, verifier: "..." }
    assert!(
        certs
            .iter()
            .any(|c| c["serialNumber"].as_str() == Some(serial)),
        "listed certificates should include our acquired cert (serial={})",
        serial
    );

    // Step 3: Relinquish the certificate
    let relinquish = post_json_ok(
        &client,
        &format!("{base}/relinquishCertificate"),
        json!({
            "type": cert_type,
            "serialNumber": serial,
            "certifier": certifier
        }),
    )
    .await;
    assert_eq!(
        relinquish["relinquished"], true,
        "certificate should be relinquished successfully"
    );

    // Step 4: List again — cert should be gone
    let list2 = post_json_ok(
        &client,
        &format!("{base}/listCertificates"),
        json!({
            "certifiers": [certifier],
            "types": [cert_type]
        }),
    )
    .await;
    assert_eq!(
        list2["totalCertificates"].as_u64().unwrap(),
        0,
        "certificate count should be 0 after relinquishing"
    );
}

/// POST /proveCertificate with a made-up certificate — should return 400.
#[tokio::test]
async fn test_prove_certificate_not_found() {
    let (base, client, _tmp) = setup().await;
    let resp = post_json(
        &client,
        &format!("{base}/proveCertificate"),
        json!({
            "certificate": {
                "type": "BgYGBgYGBgYGBgYGBgYGBgYGBgYGBgYGBgYGBgYGBgY=",
                "subject": "0279be667ef9dcbbac55a06295ce870b07029bfcdb2dce28d959f2815b16f81798",
                "serialNumber": "BwcHBwcHBwcHBwcHBwcHBwcHBwcHBwcHBwcHBwcHBwc=",
                "certifier": "0279be667ef9dcbbac55a06295ce870b07029bfcdb2dce28d959f2815b16f81798",
                "revocationOutpoint": "deadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeef.0",
                "signature": "3006020101020101",
                "fields": {"name": "test"}
            },
            "fieldsToReveal": ["name"],
            "verifier": "0279be667ef9dcbbac55a06295ce870b07029bfcdb2dce28d959f2815b16f81798"
        }),
    )
    .await;
    assert_eq!(
        resp.status(),
        400,
        "proving nonexistent cert should return 400"
    );
    let body: Value = resp.json().await.unwrap();
    assert!(
        body["message"].is_string(),
        "error should have message field"
    );
}

/// POST /relinquishCertificate with a nonexistent cert: since toolbox
/// 0.14.0-beta.4 (#430, CLI #344) the wallet refuses before changing
/// storage with the reference's words, class `BadRequest`, served 400
/// `VALIDATION_ERROR`, where it answered `{relinquished: true}`.
#[tokio::test]
async fn test_relinquish_certificate_not_found() {
    let (base, client, _tmp) = setup().await;
    let resp = post_json(
        &client,
        &format!("{base}/relinquishCertificate"),
        json!({
            "type": "BgYGBgYGBgYGBgYGBgYGBgYGBgYGBgYGBgYGBgYGBgY=",
            "serialNumber": "BwcHBwcHBwcHBwcHBwcHBwcHBwcHBwcHBwcHBwcHBwc=",
            "certifier": "0279be667ef9dcbbac55a06295ce870b07029bfcdb2dce28d959f2815b16f81798"
        }),
    )
    .await;
    assert_eq!(resp.status().as_u16(), 400);
    let body: Value = resp.json().await.unwrap();
    assert_eq!(
        body,
        json!({"code": "VALIDATION_ERROR", "message": "Result must exist and be unique."})
    );
}
