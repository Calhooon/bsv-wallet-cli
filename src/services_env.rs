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
//! | `CHAINTRACKS_URL` | Required: the header service that checks every merkle root (your `chaintracks-cloudflare` or rust-chaintracks deployment). Unset or empty is refused: there is no third-party default. `off` = no header service, so every proof is refused and nothing is marked proven |
//! | `BREAK_GLASS_EXPLORER_HEADERS` | Break-glass, off by default: `1`, `true` or `yes` lets the proof path ask WhatsOnChain (then Bitails) for headers when the header service gives no answer; every such call is logged at warn level (marker `break_glass_explorer_header`) |
//! | `ARC_URL` | Override the broadcaster URL (classic ARC, or the Arcade endpoint in Arcade mode) |
//! | `ARC_MODE=arcade` or `ARCADE=1` | Arcade V2 mode: EF-only submit, SSE status stream, push proofs |
//! | `CALLBACK_TOKEN` | Override the per-wallet callback token (otherwise auto-generated and persisted next to the db) |
//! | `PUBLIC_CALLBACK_URL` | Public HTTPS URL Arcade should POST status webhooks to (`X-CallbackUrl`) |
//! | `TAAL_API_KEY` | TAAL ARC key sent as `Authorization: Bearer <key>` |
//! | `MAIN_TAAL_API_KEY` | TAAL ARC key sent as raw `Authorization: <key>` (TAAL accepts no Bearer prefix) |
//! | `WOC_API_KEY` | WhatsOnChain key, sent as `woc-api-key` and `Authorization: Bearer` by the toolbox's provider and the CLI's own WhatsOnChain rung (optional) |
//! | `BITAILS_API_KEY` | Bitails key (optional) |
//! | `BANANABLOCKS_BASE` | BananaBlocks API base: unset is the chain's default (`https://bananablocks.com/api/v1`, `https://test.bananablocks.com/api/v1`), empty turns the rung off, a URL moves it (no key exists) |
//! | `WOC_BASE` | WhatsOnChain API base for the CLI's own rungs (the spend probe, `receive`'s courier, the broadcast verifier's chain index): unset or blank is the chain's default, a URL moves it (a fixture, a mirror). The toolbox's own WhatsOnChain provider keeps its default until it takes a base (bsv-wallet-cli #74) |
//! | `BITAILS_BASE` | Bitails API base for the broadcast verifier's chain index: unset or blank is the chain's default, a URL moves it. The toolbox's own Bitails provider keeps its default until it takes a base (#74) |
//! | `ARC_GORILLAPOOL_URL` | The GorillaPool ARC, the toolbox's second broadcaster and the verifier's third-party store: unset is the chain's default (mainnet only), empty leaves it out, a URL moves it |

use anyhow::{Context, Result};
use bsv_wallet_toolbox::services::providers::{
    arc::ARC_GORILLAPOOL,
    bitails::{BITAILS_MAINNET_URL, BITAILS_TESTNET_URL},
    whatsonchain::{WOC_MAINNET_URL, WOC_TESTNET_URL},
};
use bsv_wallet_toolbox::{
    services::{ArcadeConfig, ARCADE_V2_MAINNET},
    ArcConfig, Chain, ServicesOptions,
};
use std::io::Write;
use std::path::PathBuf;

use crate::config::{ConfigError, Raw, Secret};

/// What an unset or empty `CHAINTRACKS_URL` is told. No explorer in the
/// proof path (P0-1c, bsv-stack-lean #48): the header source is the
/// operator's own service, and ours (`chaintracks-cloudflare`) documents no
/// public URL to default to.
pub const CHAINTRACKS_URL_REQUIRED: &str = "CHAINTRACKS_URL is not set: set it to the header \
     service that checks merkle roots (your chaintracks-cloudflare or rust-chaintracks \
     deployment), or to `off` to run with none (every merkle proof is then refused). There is \
     no default header service.";

/// Resolve the header service from the `CHAINTRACKS_URL` value.
///
/// A URL is the header service. `off` (any case) returns `None`: the
/// wallet has no chain tracker, and the toolbox refuses every proof that
/// reaches it (webhook, SSE, monitor, relay) rather than take it on the
/// broadcaster's word, so nothing is marked proven; only ever right for an
/// offline or air-gapped run. Unset or empty is an error
/// ([`CHAINTRACKS_URL_REQUIRED`]): there is no default.
pub fn chaintracks_url_for(configured: Option<&str>) -> Result<Option<String>> {
    match configured.map(str::trim) {
        Some(v) if v.eq_ignore_ascii_case("off") => Ok(None),
        Some(v) if !v.is_empty() => Ok(Some(v.to_string())),
        _ => anyhow::bail!(CHAINTRACKS_URL_REQUIRED),
    }
}

/// The environment variable that turns the break-glass explorer header
/// fallback on (the toolbox's `ServicesOptions::break_glass_explorer_headers`).
pub const BREAK_GLASS_EXPLORER_HEADERS_ENV: &str = "BREAK_GLASS_EXPLORER_HEADERS";

/// Is the break-glass explorer header fallback asked for? `1`, `true` or
/// `yes` (any case); anything else, or unset, is off.
pub fn break_glass_explorer_headers(configured: Option<&str>) -> bool {
    matches!(
        configured.map(|v| v.trim().to_ascii_lowercase()).as_deref(),
        Some("1" | "true" | "yes")
    )
}

/// What a wallet with the break-glass setting on is told at startup.
pub const BREAK_GLASS_WARNING: &str = "BREAK_GLASS_EXPLORER_HEADERS is on: when the header \
     service gives no answer, merkle roots and block headers are asked of WhatsOnChain (then \
     Bitails); every such call is logged. Turn it off once the header service is back.";

/// What a wallet with `CHAINTRACKS_URL=off` is told at startup.
pub const CHAINTRACKS_OFF_WARNING: &str = "CHAINTRACKS_URL=off: no chain tracker, so every merkle \
     proof will be refused and nothing marked proven until CHAINTRACKS_URL is set";

/// Refuse a command whose job is to prove when no chain tracker is
/// configured: it could only refuse every proof it met, so it exits
/// non-zero instead of reporting a run that proved nothing.
pub fn require_chain_tracker_to_prove(command: &str, has_tracker: bool) -> Result<()> {
    if !has_tracker {
        anyhow::bail!(
            "{command} refused: no chain tracker is configured (CHAINTRACKS_URL=off); \
             every merkle proof would be refused. Set CHAINTRACKS_URL to your header service."
        );
    }
    Ok(())
}

/// The environment variable holding the WhatsOnChain API key.
pub const WOC_API_KEY_ENV: &str = "WOC_API_KEY";

/// The environment variable holding the Bitails API key.
pub const BITAILS_API_KEY_ENV: &str = "BITAILS_API_KEY";

/// The environment variable that moves (or, empty, turns off) BananaBlocks.
pub const BANANABLOCKS_BASE_ENV: &str = "BANANABLOCKS_BASE";

/// A key from its configured value: empty or blank is no key.
pub fn api_key_for(configured: Option<&str>) -> Option<String> {
    configured
        .map(str::trim)
        .filter(|k| !k.is_empty())
        .map(str::to_string)
}

/// The BananaBlocks base from its configured value, in the toolbox's
/// terms (`ServicesOptions::bananablocks_base`): unset is `None` (the
/// chain's default); set is the value as given, so an empty one is off.
pub fn bananablocks_base_for(configured: Option<&str>) -> Option<String> {
    configured.map(|v| v.trim().to_string())
}

/// `WOC_API_KEY`, when set and not blank.
pub fn woc_api_key_from_env() -> Option<String> {
    api_key_for(std::env::var(WOC_API_KEY_ENV).ok().as_deref())
}

/// `BANANABLOCKS_BASE` in the toolbox's terms ([`bananablocks_base_for`]).
pub fn bananablocks_base_from_env() -> Option<String> {
    bananablocks_base_for(std::env::var(BANANABLOCKS_BASE_ENV).ok().as_deref())
}

/// The environment variable that moves `WhatsOnChain`'s API base for the
/// CLI's own rungs (bsv-wallet-cli #74).
pub const WOC_BASE_ENV: &str = "WOC_BASE";

/// The environment variable that moves Bitails' API base for the CLI's own
/// rungs (#74).
pub const BITAILS_BASE_ENV: &str = "BITAILS_BASE";

/// The environment variable that moves (or, empty, leaves out) the
/// `GorillaPool` ARC (#74).
pub const ARC_GORILLAPOOL_URL_ENV: &str = "ARC_GORILLAPOOL_URL";

/// A base URL from its configured value: unset or blank is `None` (the
/// default stands), else the value without surrounding whitespace or a
/// trailing slash.
pub fn base_for(configured: Option<&str>) -> Option<String> {
    configured
        .map(|v| v.trim().trim_end_matches('/'))
        .filter(|v| !v.is_empty())
        .map(str::to_string)
}

/// The bases of the explorers and the third-party broadcaster the CLI's
/// own rungs ask (`explorers.rs`, `broadcast_verify.rs`): `WhatsOnChain` and
/// Bitails always on, at the chain's default or where `WOC_BASE` /
/// `BITAILS_BASE` point; `GorillaPool` at the chain's default (mainnet only),
/// where `ARC_GORILLAPOOL_URL` points, or left out when it is empty. A
/// hermetic run points all three at a local stub (#74).
///
/// The toolbox's own `WhatsOnChain` and Bitails providers do not take a base
/// (`ServicesOptions` has `bananablocks_base` alone); `GorillaPool`'s URL is
/// its `arc_gorillapool_url`, set from here by
/// [`services_options_from_env`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExplorerBases {
    /// `WhatsOnChain`'s API base, no trailing slash.
    pub woc: String,
    /// Bitails' API base, no trailing slash.
    pub bitails: String,
    /// The `GorillaPool` ARC's base, `None` when it is left out.
    pub gorillapool: Option<String>,
}

impl ExplorerBases {
    /// The chain's defaults, the toolbox's constants.
    pub fn defaults(chain: Chain) -> Self {
        let (woc, bitails, gorillapool) = match chain {
            Chain::Main => (WOC_MAINNET_URL, BITAILS_MAINNET_URL, Some(ARC_GORILLAPOOL)),
            // GorillaPool's testnet ARC is not commonly used; the toolbox
            // leaves it out too (`ServicesOptions::testnet`).
            Chain::Test => (WOC_TESTNET_URL, BITAILS_TESTNET_URL, None),
        };
        Self {
            woc: woc.trim_end_matches('/').to_string(),
            bitails: bitails.trim_end_matches('/').to_string(),
            gorillapool: gorillapool.map(str::to_string),
        }
    }

    /// The bases from their configured values: unset or blank is the
    /// default; for `GorillaPool`, unset is the default and an empty or blank
    /// value leaves it out.
    #[must_use]
    pub fn configured(
        chain: Chain,
        woc: Option<&str>,
        bitails: Option<&str>,
        gorillapool: Option<&str>,
    ) -> Self {
        let defaults = Self::defaults(chain);
        Self {
            woc: base_for(woc).unwrap_or(defaults.woc),
            bitails: base_for(bitails).unwrap_or(defaults.bitails),
            gorillapool: match gorillapool {
                None => defaults.gorillapool,
                Some(v) => base_for(Some(v)),
            },
        }
    }

    /// The bases from `WOC_BASE`, `BITAILS_BASE` and `ARC_GORILLAPOOL_URL`.
    #[must_use]
    pub fn from_env(chain: Chain) -> Self {
        let woc = std::env::var(WOC_BASE_ENV).ok();
        let bitails = std::env::var(BITAILS_BASE_ENV).ok();
        let gorillapool = std::env::var(ARC_GORILLAPOOL_URL_ENV).ok();
        Self::configured(
            chain,
            woc.as_deref(),
            bitails.as_deref(),
            gorillapool.as_deref(),
        )
    }
}

/// The explorers' settings (#50, #74): their keys and the bases of the
/// CLI's own rungs, parsed here from the environment `crate::config` read
/// once at start (`WOC_API_KEY`, `BITAILS_API_KEY`, `BANANABLOCKS_BASE`,
/// `WOC_BASE`, `BITAILS_BASE`, `ARC_GORILLAPOOL_URL`).
#[derive(Debug, Clone)]
pub struct ExplorerSettings {
    /// `WOC_API_KEY`, trimmed, when not blank.
    pub woc_api_key: Option<Secret>,
    /// The Bitails key, `BITAILS_API_KEY`, trimmed, when not blank.
    pub bitails_api_key: Option<Secret>,
    /// The `BananaBlocks` base, trimmed, in the toolbox's terms
    /// (`ServicesOptions::bananablocks_base`): unset is `None` (the chain's
    /// default); set is the value as given, so an empty one is off.
    pub bananablocks_base: Option<String>,
    /// The bases of the CLI's own rungs on the configured chain.
    pub bases: ExplorerBases,
}

impl ExplorerSettings {
    /// The settings from the environment as read at start, on `chain`: a
    /// base that is set and not blank must be a URL.
    pub(crate) fn read(raw: &Raw, chain: Chain) -> Result<Self, ConfigError> {
        let key = |name| api_key_for(raw.get(name)).map(Secret::new);
        Ok(Self {
            woc_api_key: key("WOC_API_KEY"),
            bitails_api_key: key("BITAILS_API_KEY"),
            bananablocks_base: raw.present_url("BANANABLOCKS_BASE")?,
            bases: ExplorerBases::configured(
                chain,
                raw.url("WOC_BASE")?.as_deref(),
                raw.url("BITAILS_BASE")?.as_deref(),
                raw.get("ARC_GORILLAPOOL_URL"),
            ),
        })
    }

    /// The settings' rows for `crate::config::Config::knobs`, in their order.
    pub(crate) fn values(&self) -> Vec<(&'static str, String)> {
        let secret = |s: &Option<Secret>| s.as_ref().map_or("-".into(), ToString::to_string);
        vec![
            ("WOC_API_KEY", secret(&self.woc_api_key)),
            ("BITAILS_API_KEY", secret(&self.bitails_api_key)),
            (
                "BANANABLOCKS_BASE",
                match self.bananablocks_base.as_deref() {
                    None => "default",
                    Some("") => "off",
                    Some(v) => v,
                }
                .to_string(),
            ),
            ("WOC_BASE", self.bases.woc.clone()),
            ("BITAILS_BASE", self.bases.bitails.clone()),
        ]
    }
}

/// `GorillaPool` on `ServicesOptions` from `ARC_GORILLAPOOL_URL`'s configured
/// value: unset leaves the options' own default untouched (the toolbox's,
/// never a copy of it); empty or blank leaves it out; a URL moves it.
pub fn apply_gorillapool(opts: &mut ServicesOptions, configured: Option<&str>) {
    if let Some(v) = configured {
        opts.arc_gorillapool_url = base_for(Some(v));
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
    match chaintracks_url_for(configured.as_deref())? {
        Some(url) => opts = opts.with_chaintracks_url(url),
        None => tracing::warn!("{}", CHAINTRACKS_OFF_WARNING),
    }
    let break_glass = std::env::var(BREAK_GLASS_EXPLORER_HEADERS_ENV).ok();
    if break_glass_explorer_headers(break_glass.as_deref()) {
        tracing::warn!(
            marker = "break_glass_explorer_header",
            "{}",
            BREAK_GLASS_WARNING
        );
        opts = opts.with_break_glass_explorer_headers(true);
    }

    // The explorers (bsv-wallet-cli #50): WhatsOnChain, BananaBlocks and
    // Bitails, each the others' fallback in the toolbox's ladders.
    if let Some(key) = woc_api_key_from_env() {
        opts = opts.with_woc_api_key(key);
    }
    if let Some(key) = api_key_for(std::env::var(BITAILS_API_KEY_ENV).ok().as_deref()) {
        opts = opts.with_bitails_api_key(key);
    }
    opts.bananablocks_base = bananablocks_base_from_env();
    // GorillaPool, the toolbox's second broadcaster: the toolbox's default
    // unless `ARC_GORILLAPOOL_URL` moves it or, empty, leaves it out (#74).
    apply_gorillapool(
        &mut opts,
        std::env::var(ARC_GORILLAPOOL_URL_ENV).ok().as_deref(),
    );

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
    fn chaintracks_has_no_default() {
        for unset in [None, Some(""), Some("   ")] {
            let err = chaintracks_url_for(unset).unwrap_err();
            assert!(
                err.to_string().contains("CHAINTRACKS_URL is not set"),
                "{err}"
            );
        }
        assert!(!CHAINTRACKS_URL_REQUIRED.contains("babbage"));
    }

    #[test]
    fn chaintracks_honours_an_explicit_url() {
        assert_eq!(
            chaintracks_url_for(Some("https://ct.example/v1"))
                .unwrap()
                .as_deref(),
            Some("https://ct.example/v1")
        );
    }

    #[test]
    fn chaintracks_off_disables_validation_on_purpose() {
        assert_eq!(chaintracks_url_for(Some("off")).unwrap(), None);
        assert_eq!(chaintracks_url_for(Some("OFF")).unwrap(), None);
    }

    #[test]
    fn break_glass_is_off_unless_asked_for() {
        for off in [
            None,
            Some(""),
            Some("0"),
            Some("false"),
            Some("no"),
            Some("on"),
        ] {
            assert!(!break_glass_explorer_headers(off), "{off:?}");
        }
        for on in [Some("1"), Some("true"), Some("YES"), Some(" yes ")] {
            assert!(break_glass_explorer_headers(on), "{on:?}");
        }
    }

    #[test]
    fn the_off_warning_says_proofs_are_refused() {
        assert!(CHAINTRACKS_OFF_WARNING.contains("every merkle proof will be refused"));
        assert!(!CHAINTRACKS_OFF_WARNING.contains("stored"));
    }

    #[test]
    fn a_proving_command_needs_a_chain_tracker() {
        let err = require_chain_tracker_to_prove("tick", false).unwrap_err();
        assert!(err.to_string().starts_with("tick refused"), "{err}");
        assert!(err.to_string().contains("CHAINTRACKS_URL=off"), "{err}");
        assert!(require_chain_tracker_to_prove("tick", true).is_ok());
    }

    /// `BANANABLOCKS_BASE`: unset the chain's default, empty off, a URL
    /// moves it; the toolbox reads the value so.
    #[test]
    fn bananablocks_base_unset_is_the_default_and_empty_is_off() {
        let opts = |configured: Option<&str>| ServicesOptions {
            bananablocks_base: bananablocks_base_for(configured),
            ..ServicesOptions::mainnet()
        };
        assert_eq!(
            opts(None).bananablocks_base_for(Chain::Main).as_deref(),
            Some("https://bananablocks.com/api/v1")
        );
        assert_eq!(
            opts(None).bananablocks_base_for(Chain::Test).as_deref(),
            Some("https://test.bananablocks.com/api/v1")
        );
        for off in [Some(""), Some("  ")] {
            assert_eq!(
                opts(off).bananablocks_base_for(Chain::Main),
                None,
                "{off:?}"
            );
        }
        assert_eq!(
            opts(Some(" https://mirror.example/api/v1 "))
                .bananablocks_base_for(Chain::Main)
                .as_deref(),
            Some("https://mirror.example/api/v1")
        );
    }

    /// `ARC_GORILLAPOOL_URL` unset leaves `ServicesOptions`' own default
    /// for either chain; empty or blank leaves `GorillaPool` out; a URL moves it.
    #[test]
    fn gorillapool_unset_leaves_the_toolbox_default() {
        for base in [ServicesOptions::mainnet(), ServicesOptions::testnet()] {
            let mut opts = base.clone();
            apply_gorillapool(&mut opts, None);
            assert_eq!(opts.arc_gorillapool_url, base.arc_gorillapool_url);
            for blank in ["", "  "] {
                let mut opts = base.clone();
                apply_gorillapool(&mut opts, Some(blank));
                assert_eq!(opts.arc_gorillapool_url, None, "{blank:?}");
            }
            let mut opts = base.clone();
            apply_gorillapool(&mut opts, Some(" http://127.0.0.1:3380/arc/ "));
            assert_eq!(
                opts.arc_gorillapool_url.as_deref(),
                Some("http://127.0.0.1:3380/arc")
            );
        }
        assert_eq!(
            ServicesOptions::mainnet().arc_gorillapool_url.as_deref(),
            Some("https://arc.gorillapool.io")
        );
    }

    #[test]
    fn a_blank_api_key_is_no_key() {
        assert_eq!(api_key_for(None), None);
        assert_eq!(api_key_for(Some("  ")), None);
        assert_eq!(api_key_for(Some(" k ")).as_deref(), Some("k"));
    }

    /// `WOC_BASE` and `BITAILS_BASE` (#74): unset or blank is the chain's
    /// default, a URL moves the rung, a trailing slash is dropped.
    #[test]
    fn explorer_bases_default_and_move() {
        let main = ExplorerBases::defaults(Chain::Main);
        assert_eq!(main.woc, "https://api.whatsonchain.com/v1/bsv/main");
        assert_eq!(main.bitails, "https://api.bitails.io");
        assert_eq!(
            main.gorillapool.as_deref(),
            Some("https://arc.gorillapool.io")
        );
        let test = ExplorerBases::defaults(Chain::Test);
        assert_eq!(test.woc, "https://api.whatsonchain.com/v1/bsv/test");
        assert_eq!(test.bitails, "https://test-api.bitails.io");
        assert_eq!(test.gorillapool, None);

        for blank in [None, Some(""), Some("  ")] {
            assert_eq!(
                ExplorerBases::configured(Chain::Main, blank, blank, None),
                main,
                "{blank:?}"
            );
        }
        let moved = ExplorerBases::configured(
            Chain::Main,
            Some(" http://127.0.0.1:3380/woc/ "),
            Some("http://127.0.0.1:3380/bitails"),
            Some("http://127.0.0.1:3380/arc"),
        );
        assert_eq!(moved.woc, "http://127.0.0.1:3380/woc");
        assert_eq!(moved.bitails, "http://127.0.0.1:3380/bitails");
        assert_eq!(
            moved.gorillapool.as_deref(),
            Some("http://127.0.0.1:3380/arc")
        );
    }

    /// `ARC_GORILLAPOOL_URL` (#74): unset is the chain's default, empty or
    /// blank leaves `GorillaPool` out, a URL moves it.
    #[test]
    fn gorillapool_unset_is_the_default_and_empty_is_out() {
        let at =
            |v: Option<&str>| ExplorerBases::configured(Chain::Main, None, None, v).gorillapool;
        assert_eq!(at(None).as_deref(), Some("https://arc.gorillapool.io"));
        assert_eq!(at(Some("")), None);
        assert_eq!(at(Some("  ")), None);
        assert_eq!(
            at(Some("https://gp.example/")).as_deref(),
            Some("https://gp.example")
        );
        assert_eq!(base_for(None), None);
        assert_eq!(base_for(Some(" / ")), None);
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
