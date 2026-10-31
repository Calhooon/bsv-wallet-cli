//! Beta.4 package facts: discovery under the wallet's trust settings (#331,
//! toolbox #388).
use super::recorder::{post, Recorder};
use crate::wire_support::ROOT_KEY;
use bsv_sdk::primitives::PrivateKey;
use serde_json::json;

pub(crate) async fn record(r: &mut Recorder) {
    // The wallet's own identity is the harness certificate's subject; the
    // local leg answers it. `trusted`'s TRUST_SETTINGS names its certifier,
    // so the certificate is answered with the configured certifierInfo;
    // `stranger`'s names another key, so the same certificate is dropped.
    let subject = PrivateKey::from_hex(ROOT_KEY)
        .unwrap()
        .public_key()
        .to_hex();
    let trusted = r
        .case(
            "discoverByIdentityKey.trusted",
            "trusted",
            post("/discoverByIdentityKey", json!({"identityKey": subject})),
        )
        .await;
    assert_eq!(trusted.value("/totalCertificates"), 1);
    let stranger = r
        .case(
            "discoverByIdentityKey.stranger",
            "stranger",
            post("/discoverByIdentityKey", json!({"identityKey": subject})),
        )
        .await;
    assert_eq!(stranger.value("/totalCertificates"), 0);
}
