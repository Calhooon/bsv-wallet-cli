//! Public deterministic certificate material for the verified direct door (#374).
use super::recorder::{CERT_TYPE, G};
use crate::wire_support::ROOT_KEY;
use bsv_sdk::auth::certificates::Certificate;
use bsv_sdk::primitives::{to_base64, PrivateKey, SymmetricKey};
use bsv_sdk::wallet::Outpoint;
use serde_json::{json, Value};

// Field key [13; 32], encrypted by scalar 1 to ROOT_KEY, nonce [15; 32].
const KEYRING: &str = "Dw8PDw8PDw8PDw8PDw8PDw8PDw8PDw8PDw8PDw8PDw+yjKuVcDbO9efkiFXVtspIbhATgrV7c2cLwYAsCCu84144ctRgR9bnD68C86pkbKM=";

pub(crate) fn body(serial: u8, plaintext: &[u8]) -> Value {
    let key = SymmetricKey::from_bytes(&[13; 32]).unwrap();
    let field = to_base64(&key.encrypt_with_iv(&[14; 32], plaintext).unwrap());
    let certifier = PrivateKey::from_hex(&format!("{:064x}", 1)).unwrap();
    let subject = PrivateKey::from_hex(ROOT_KEY).unwrap().public_key();
    let mut certificate = Certificate::new([6; 32], [serial; 32], subject, certifier.public_key())
        .with_revocation_outpoint(Some(Outpoint::new([0x33; 32], 0)))
        .with_fields([("name".into(), field.clone())].into_iter().collect());
    certificate.sign(&certifier).unwrap();
    assert!(certificate.verify().unwrap());
    json!({"type": CERT_TYPE, "certifier": G, "acquisitionProtocol": "direct",
        "serialNumber": to_base64(&[serial; 32]), "revocationOutpoint": format!("{}.0", "33".repeat(32)),
        "signature": hex::encode(certificate.signature.unwrap()), "fields": {"name": field},
        "keyringRevealer": "certifier", "keyringForSubject": {"name": KEYRING}})
}
