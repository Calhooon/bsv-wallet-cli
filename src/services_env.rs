//! Single source of truth for building `ServicesOptions` from the environment.
//!
//! Previously `context.rs`, `commands/daemon.rs`, `commands/tick.rs`, and
//! `commands/services.rs` each rolled their own (subtly divergent) services
//! setup. This module unifies them and adds the Arcade V2 surface.
//!
//! # Environment variables
//!
//! | Var | Effect |
//! |-----|--------|
//! | `CHAINTRACKS_URL` | Chaintracks header service for proof validation (default: the public Babbage instance for the chain; `off` disables validation on purpose) |
//! | `ARC_URL` | Override the broadcaster URL (classic ARC, or the Arcade endpoint in Arcade mode) |
//! | `ARC_MODE=arcade` or `ARCADE=1` | Arcade V2 mode: EF-only submit, SSE status stream, push proofs |
//! | `CALLBACK_TOKEN` | Override the per-wallet callback token (otherwise auto-generated and persisted next to the db) |
//! | `PUBLIC_CALLBACK_URL` | Public HTTPS URL Arcade should POST status webhooks to (`X-CallbackUrl`) |
//! | `TAAL_API_KEY` | TAAL ARC key sent as `Authorization: Bearer <key>` |
//! | `MAIN_TAAL_API_KEY` | TAAL ARC key sent as raw `Authorization: <key>` (TAAL accepts no Bearer prefix) |

use anyhow::{Context, Result};
use bsv_wallet_toolbox::{
    services::{ArcadeConfig, ARCADE_V2_MAINNET},
    ArcConfig, Chain, ServicesOptions,
};
use std::io::Write;
use std::path::PathBuf;

/// Header service used to validate merkle proofs when `CHAINTRACKS_URL` is
/// unset: the public Babbage chaintracks for the chain, the same default the
/// TS toolbox and MetaNet Desktop ship with. The toolbox keeps a
/// WhatsOnChain header fallback behind it.
pub const DEFAULT_MAINNET_CHAINTRACKS_URL: &str = "https://mainnet-chaintracks.babbage.systems";
/// Testnet counterpart of [`DEFAULT_MAINNET_CHAINTRACKS_URL`].
pub const DEFAULT_TESTNET_CHAINTRACKS_URL: &str = "https://testnet-chaintracks.babbage.systems";

/// Resolve the header service from the `CHAINTRACKS_URL` value.
///
/// Unset or empty falls back to the chain's public default; `off` (any
/// case) returns `None` and the wallet stores proofs unvalidated, which is
/// only ever right for an offline or air-gapped run. Without a header
/// service every proof that reaches the wallet (webhook, SSE, monitor,
/// `tick`) would be taken on the broadcaster's word.
pub fn chaintracks_url_for(chain: Chain, configured: Option<&str>) -> Option<String> {
    match configured.map(str::trim) {
        Some(v) if v.eq_ignore_ascii_case("off") => None,
        Some(v) if !v.is_empty() => Some(v.to_string()),
        _ => Some(
            match chain {
                Chain::Main => DEFAULT_MAINNET_CHAINTRACKS_URL,
                Chain::Test => DEFAULT_TESTNET_CHAINTRACKS_URL,
            }
            .to_string(),
        ),
    }
}

/// Resolved Arcade V2 runtime settings (present only in Arcade mode).
#[derive(Debug, Clone)]
pub struct ArcadeRuntime {
    /// Arcade base URL (from `ARC_URL`, default [`ARCADE_V2_MAINNET`]).
    pub url: String,
    /// Per-wallet callback token (env override or persisted next to the db).
    pub callback_token: String,
    /// Public HTTPS webhook URL passed as `X-CallbackUrl` on submits.
    pub public_callback_url: Option<String>,
}

/// Whether Arcade V2 mode is selected via env (`ARC_MODE=arcade` or `ARCADE=1`).
pub fn arcade_mode_enabled() -> bool {
    if let Ok(mode) = std::env::var("ARC_MODE") {
        if mode.eq_ignore_ascii_case("arcade") {
            return true;
        }
    }
    if let Ok(v) = std::env::var("ARCADE") {
        let v = v.trim();
        return v == "1" || v.eq_ignore_ascii_case("true") || v.eq_ignore_ascii_case("yes");
    }
    false
}

/// Resolve the Arcade runtime settings, or `None` when not in Arcade mode.
///
/// `db_path` locates the persisted per-wallet callback token
/// (`<db>.callback-token`), giving each wallet db/port its own independent
/// SSE stream and webhook identity.
pub fn arcade_runtime(db_path: &str) -> Result<Option<ArcadeRuntime>> {
    if !arcade_mode_enabled() {
        return Ok(None);
    }
    let url = std::env::var("ARC_URL").unwrap_or_else(|_| ARCADE_V2_MAINNET.to_string());
    let callback_token = resolve_callback_token(db_path)?;
    let public_callback_url = std::env::var("PUBLIC_CALLBACK_URL")
        .ok()
        .filter(|s| !s.is_empty());
    Ok(Some(ArcadeRuntime {
        url,
        callback_token,
        public_callback_url,
    }))
}

/// Resolve the per-wallet callback token.
///
/// Priority: `CALLBACK_TOKEN` env → persisted `<db>.callback-token` file →
/// auto-generate a random 32-hex token and persist it (0600 on unix).
/// The token is NEVER logged.
pub fn resolve_callback_token(db_path: &str) -> Result<String> {
    if let Ok(tok) = std::env::var("CALLBACK_TOKEN") {
        let tok = tok.trim().to_string();
        if !tok.is_empty() {
            return Ok(tok);
        }
    }

    let token_path = callback_token_path(db_path);
    if token_path.exists() {
        let tok = std::fs::read_to_string(&token_path)
            .with_context(|| format!("reading {}", token_path.display()))?
            .trim()
            .to_string();
        if !tok.is_empty() {
            return Ok(tok);
        }
    }

    // Generate: 16 random bytes → 32 hex chars. PrivateKey::random() is the
    // CSPRNG already in the dependency tree; we take half its bytes.
    let tok: String = bsv_sdk::primitives::PrivateKey::random()
        .to_hex()
        .chars()
        .take(32)
        .collect();

    let mut opts = std::fs::OpenOptions::new();
    opts.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600);
    }
    let mut f = opts
        .open(&token_path)
        .with_context(|| format!("creating {}", token_path.display()))?;
    f.write_all(tok.as_bytes())?;
    tracing::info!(path = %token_path.display(), "generated per-wallet callback token");
    Ok(tok)
}

/// Where the per-wallet callback token lives: `<db>.callback-token`
/// (covered by the repo's `*.db*` gitignore pattern).
pub fn callback_token_path(db_path: &str) -> PathBuf {
    PathBuf::from(format!("{}.callback-token", db_path))
}

/// Build `ServicesOptions` from the environment (the ONE shared helper).
///
/// `db_path` is used only in Arcade mode, to resolve the persisted callback
/// token.
pub fn services_options_from_env(chain: Chain, db_path: &str) -> Result<ServicesOptions> {
    let mut opts = match chain {
        Chain::Main => ServicesOptions::mainnet(),
        Chain::Test => ServicesOptions::testnet(),
    };

    let configured = std::env::var("CHAINTRACKS_URL").ok();
    match chaintracks_url_for(chain, configured.as_deref()) {
        Some(url) => opts = opts.with_chaintracks_url(url),
        None => tracing::warn!(
            "CHAINTRACKS_URL=off: merkle proofs will be stored without header validation"
        ),
    }

    // TAAL ARC auth (applies to the classic ARC provider — in Arcade mode
    // that provider is the failover behind Arcade).
    // - TAAL_API_KEY       → `Authorization: Bearer <key>`
    // - MAIN_TAAL_API_KEY  → raw `Authorization: <key>` (TAAL accepts the key
    //   WITHOUT the Bearer prefix; kept for backward compatibility with
    //   existing daemon deployments).
    let mut arc_config: Option<ArcConfig> = None;
    if let Ok(key) = std::env::var("TAAL_API_KEY") {
        if !key.is_empty() {
            arc_config = Some(ArcConfig::with_api_key(key));
        }
    }
    if let Ok(key) = std::env::var("MAIN_TAAL_API_KEY") {
        if !key.is_empty() {
            let mut headers = std::collections::HashMap::new();
            headers.insert("Authorization".to_string(), key);
            let mut cfg = arc_config.unwrap_or_default();
            cfg.headers = Some(headers);
            arc_config = Some(cfg);
        }
    }

    if let Some(runtime) = arcade_runtime(db_path)? {
        // Arcade V2 mode: explicit flag on ServicesOptions (never inferred
        // from the URL). Classic ARC config still applies to the TAAL
        // failover provider.
        let arcade_config = ArcadeConfig {
            callback_token: Some(runtime.callback_token.clone()),
            callback_url: runtime.public_callback_url.clone(),
            ..Default::default()
        };
        opts.arc_config = arc_config;
        opts = opts.with_arcade(runtime.url, Some(arcade_config));
    } else {
        // Classic ARC: honor ARC_URL override, else keep the chain default.
        let arc_url = std::env::var("ARC_URL")
            .ok()
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| opts.arc_url.clone());
        opts = opts.with_arc(arc_url, arc_config);
    }

    Ok(opts)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chaintracks_defaults_to_the_public_instance_for_the_chain() {
        assert_eq!(
            chaintracks_url_for(Chain::Main, None).as_deref(),
            Some(DEFAULT_MAINNET_CHAINTRACKS_URL)
        );
        assert_eq!(
            chaintracks_url_for(Chain::Test, Some("")).as_deref(),
            Some(DEFAULT_TESTNET_CHAINTRACKS_URL)
        );
        assert_eq!(
            chaintracks_url_for(Chain::Main, Some("   ")).as_deref(),
            Some(DEFAULT_MAINNET_CHAINTRACKS_URL)
        );
    }

    #[test]
    fn chaintracks_honours_an_explicit_url() {
        assert_eq!(
            chaintracks_url_for(Chain::Main, Some("https://ct.example/v1")).as_deref(),
            Some("https://ct.example/v1")
        );
    }

    #[test]
    fn chaintracks_off_disables_validation_on_purpose() {
        assert_eq!(chaintracks_url_for(Chain::Main, Some("off")), None);
        assert_eq!(chaintracks_url_for(Chain::Test, Some("OFF")), None);
    }

    #[test]
    fn callback_token_path_is_next_to_db() {
        let p = callback_token_path("/tmp/wallet.db");
        assert_eq!(p, PathBuf::from("/tmp/wallet.db.callback-token"));
    }

    #[test]
    fn generated_token_is_32_hex_and_persisted() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("w.db");
        let db = db.to_str().unwrap();

        // No env override in effect for this name; read/created from file.
        std::env::remove_var("CALLBACK_TOKEN");
        let tok = resolve_callback_token(db).unwrap();
        assert_eq!(tok.len(), 32);
        assert!(tok.chars().all(|c| c.is_ascii_hexdigit()));

        // Stable on re-read.
        let tok2 = resolve_callback_token(db).unwrap();
        assert_eq!(tok, tok2);

        // Persisted next to the db.
        assert!(callback_token_path(db).exists());
    }
}
