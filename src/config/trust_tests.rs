//! `TRUST_SETTINGS` read through the typed config (#331): the manager built
//! from it, and its refusal in the manager's words.

use super::tests::{config, refusal, row};
use super::*;

/// One trusted certifier at the reference's default threshold, its
/// description within BRC-100's 5 to 50 bytes and its icon named, so the
/// served `certifierInfo` equals the configured entry (#331).
const TRUST: &str = r#"{"trustLevel": 2, "trustedCertifiers": [{"name": "Example Certifier", "description": "Certifies the example's names", "identityKey": "0279be667ef9dcbbac55a06295ce870b07029bfcdb2dce28d959f2815b16f81798", "trust": 3, "iconUrl": "https://certifier.example/icon.png"}]}"#;

/// `TRUST_SETTINGS` (#331, toolbox #388) builds the wallet's settings
/// manager from the reference's trust settings JSON; unset, the wallet has
/// none and discovery uses the chain's default certifiers.
#[tokio::test]
async fn trust_settings_build_the_wallets_settings_manager() {
    let c = config(&[("TRUST_SETTINGS", TRUST)]);
    let trust = c.wallet.trust.as_ref().unwrap();
    assert_eq!(
        row(&c, "TRUST_SETTINGS"),
        ("1 trusted certifier(s), trust level 2".into(), Source::Env)
    );
    let manager = c.wallet.options().settings_manager.unwrap();
    let held = manager.get().await.unwrap().trust_settings;
    assert_eq!(held, trust.settings);
    assert_eq!(held.trusted_certifiers[0].name, "Example Certifier");
    assert_eq!(
        held.trusted_certifiers[0].icon_url.as_deref(),
        Some("https://certifier.example/icon.png")
    );
    let unset = Config::default();
    assert!(unset.wallet.trust.is_none());
    assert!(unset.wallet.options().settings_manager.is_none());
    assert_eq!(row(&unset, "TRUST_SETTINGS"), ("-".into(), Source::Default));
}

/// The bounds are the manager's alone (toolbox `settings_manager.rs`, the
/// reference's `WalletSettingsManager.ts:98-136`): its refusal, in its own
/// words, is the knob's, by name, before the wallet starts; a value that is
/// not the settings' JSON is refused with the parser's.
#[test]
fn a_trust_setting_the_manager_refuses_is_refused_by_name_in_its_words() {
    let level = TRUST.replace(r#""trustLevel": 2"#, r#""trustLevel": 0"#);
    let long = TRUST.replace("Certifies the example's names", &"d".repeat(501));
    let key = TRUST.replace("0279be66", "0479be66");
    for (value, words) in [
        (level.as_str(), "Invalid wallet settings: trustLevel must be an integer from 1 to 2560"),
        (
            long.as_str(),
            "Invalid wallet settings: trustedCertifiers[0].description must contain 1-500 UTF-8 bytes",
        ),
        (
            key.as_str(),
            "Invalid wallet settings: trustedCertifiers[0].identityKey must be a compressed public key",
        ),
    ] {
        let e = refusal(&[("TRUST_SETTINGS", value)]);
        assert_eq!(e.variable, "TRUST_SETTINGS");
        assert!(e.expected.starts_with("trust settings the wallet accepts ("), "{e}");
        assert!(e.expected.contains(words), "{e}");
        assert!(e.to_string().starts_with("TRUST_SETTINGS is "), "{e}");
    }
    let e = refusal(&[("TRUST_SETTINGS", "not json")]);
    assert!(
        e.expected
            .starts_with("the reference's trust settings JSON"),
        "{e}"
    );
    let refused = Config::from_pairs(&[("TRUST_SETTINGS", "not json")]).unwrap_err();
    assert!(refused.config.wallet.trust.is_none());
}
