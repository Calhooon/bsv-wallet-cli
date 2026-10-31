//! `TRUST_SETTINGS`, discovery's trust section (#331, toolbox #388).

use super::ConfigError;

/// `TRUST_SETTINGS` as read (#331): the reference's trust settings JSON,
/// `{"trustLevel": n, "trustedCertifiers": [{"name", "description",
/// "identityKey", "trust", "iconUrl"}]}` (toolbox `managers/settings_manager.rs`,
/// the reference's `WalletSettingsManager.ts`), and the settings manager the
/// wallet reads on every discovery call. The bounds are the manager's alone:
/// it validates once, here, and its words are the knob's refusal.
#[derive(Debug, Clone)]
pub struct Trust {
    /// The trust settings the manager accepted.
    pub settings: bsv_wallet_toolbox::managers::TrustSettings,
    /// The manager built from them.
    pub manager: std::sync::Arc<bsv_wallet_toolbox::WalletSettingsManager>,
}

impl Trust {
    /// The variable's name.
    pub const VARIABLE: &'static str = "TRUST_SETTINGS";

    /// Parse `value` and build the manager: a value that is not the
    /// settings' JSON, or that the manager refuses, is refused by the
    /// knob's name with the parser's or the manager's own words.
    pub(super) fn read(value: &str) -> Result<Self, ConfigError> {
        use bsv_wallet_toolbox::managers::{TrustSettings, WalletSettings};
        let refused = |expected: String| ConfigError {
            variable: Self::VARIABLE,
            value: value.to_string(),
            expected: expected.into(),
        };
        let settings: TrustSettings = serde_json::from_str(value).map_err(|e| {
            refused(format!(
                "the reference's trust settings JSON, {{\"trustLevel\", \"trustedCertifiers\"}} ({e})"
            ))
        })?;
        let manager = bsv_wallet_toolbox::WalletSettingsManager::new(Some(
            bsv_wallet_toolbox::WalletSettingsManagerConfig {
                default_settings: WalletSettings {
                    trust_settings: settings.clone(),
                    theme: None,
                    currency: None,
                    permission_mode: None,
                },
            },
        ))
        .map_err(|e| refused(format!("trust settings the wallet accepts ({e})")))?;
        Ok(Self {
            settings,
            manager: std::sync::Arc::new(manager),
        })
    }

    /// The knob list's value: how many certifiers, and the threshold.
    #[must_use]
    pub fn shown(&self) -> String {
        format!(
            "{} trusted certifier(s), trust level {}",
            self.settings.trusted_certifiers.len(),
            self.settings.trust_level
        )
    }
}
