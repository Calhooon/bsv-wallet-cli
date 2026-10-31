//! The trusted and stranger worlds' trust settings (#331, toolbox #388): the
//! wallet's options read through the CLI's typed config, and the harness
//! certificate each holds before any case.

use super::fixture::NORMALIZER;
use super::worlds::stranger_key;
use bsv_sdk::primitives::PrivateKey;
use bsv_wallet_toolbox::{Services, StorageSqlx, Wallet};

/// The world's wallet, its options as the CLI's typed configuration builds
/// them: `trusted` and `stranger` read `TRUST_SETTINGS` (#331, toolbox
/// #388), every other world none (the chain's default certifiers).
pub(crate) async fn wallet(
    name: &str,
    key: PrivateKey,
    storage: StorageSqlx,
    services: Services,
) -> Wallet<StorageSqlx, Services> {
    let options = match name {
        "trusted" | "stranger" => {
            bsv_wallet_cli::config::Config::from_pairs(&[("TRUST_SETTINGS", &settings(name))])
                .unwrap()
                .wallet
                .options()
        }
        _ => bsv_wallet_toolbox::WalletOptions::default(),
    };
    Wallet::with_options(Some(key), storage, services, options)
        .await
        .unwrap()
}

/// The harness's certificate (certifier [`super::recorder::G`], the
/// wallet its subject), acquired through the served door before any case
/// is recorded, so discovery has a certificate to answer.
pub(crate) async fn hold_the_harness_certificate(name: &str, router: &axum::Router) {
    if !matches!(name, "trusted" | "stranger") {
        return;
    }
    let acquire = super::recorder::post(
        "/acquireCertificate",
        super::certificate_material::body(7, b"wire"),
    );
    let (resp, _) = NORMALIZER.serve(router, &acquire).await;
    assert_eq!(
        resp.status,
        200,
        "{:?}",
        resp.json.map(|j| serde_json::to_string(&j).unwrap())
    );
}

/// The trust settings of `trusted` and `stranger` (#331): one certifier at
/// the reference's default threshold (2), its trust 3, its description
/// within BRC-100's 5 to 50 bytes and its icon named, so the served
/// `certifierInfo` is the configured entry (toolbox `discovery_trust.rs`
/// cuts a longer description and fills a missing icon). `trusted` names
/// the harness certificate's certifier; `stranger` names another key, so
/// the same certificate is a stranger's.
fn settings(world: &str) -> String {
    let (name, key) = match world {
        "trusted" => ("Wire Certifier", super::recorder::G.to_string()),
        _ => ("Other Certifier", stranger_key().public_key().to_hex()),
    };
    serde_json::json!({"trustLevel": 2, "trustedCertifiers": [{"name": name,
        "description": "The wire harness's certifier", "identityKey": key, "trust": 3,
        "iconUrl": "https://wire.test/certifier.png"}]})
    .to_string()
}
