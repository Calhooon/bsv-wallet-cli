//! Single source of truth for building `ServicesOptions` from the
//! configuration's [`ServicesSettings`] (read once at start by
//! `crate::config`, the one reader of the environment).
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
//! | `ARC_MODE` | The broadcaster mode: unset, blank or `arcade` (any case) is Arcade V2 (EF-only submit, SSE status stream, push proofs), the default by the owner's word of 2026-10-27 (#300); `classic` (any case) is the toolbox's classic ARC ladder, deprecated ("arc is just deprecated, arcade is the standard"), kept for the hermetic stub until #299, never a default or a failover; any other value is refused by name |
//! | `ARCADE` | The older switch, read when `ARC_MODE` is unset: `1`, `true` or `yes` is Arcade, `0`, `false` or `no` is classic; any other value is refused by name |
//! | `ARC_URL` | Override the broadcaster URL (blank is unset): the Arcade endpoint in Arcade mode (default `ARCADE_V2_MAINNET` on mainnet; no testnet default, so a testnet wallet without it is refused by name), the classic ARC URL in classic mode (default the chain's TAAL ARC) |
//! | `CALLBACK_TOKEN` | Override the per-wallet callback token (otherwise auto-generated and persisted next to the db) |
//! | `PUBLIC_CALLBACK_URL` | Public HTTPS URL Arcade should POST status webhooks to (`X-CallbackUrl`) |
//! | `TAAL_API_KEY` | Deprecated (#300): classic mode only, TAAL ARC key sent as `Authorization: Bearer <key>`; in Arcade mode it is unused and logged once at warn |
//! | `MAIN_TAAL_API_KEY` | Deprecated (#300): classic mode only, TAAL ARC key sent as raw `Authorization: <key>` (TAAL accepts no Bearer prefix); in Arcade mode it is unused and logged once at warn |
//! | `WOC_API_KEY` | WhatsOnChain key, sent as `woc-api-key` and `Authorization: Bearer` by the toolbox's provider and the CLI's own WhatsOnChain rung (optional) |
//! | `BITAILS_API_KEY` | Bitails key (optional) |
//! | `BANANABLOCKS_BASE` | BananaBlocks API base: unset is the chain's default (`https://bananablocks.com/api/v1`, `https://test.bananablocks.com/api/v1`), empty turns the rung off, a URL moves it (no key exists) |
//! | `WOC_BASE` | WhatsOnChain API base for the CLI's own rungs (the spend probe, `receive`'s courier, the broadcast verifier's chain index): unset or blank is the chain's default, a URL moves it (a fixture, a mirror). The toolbox's own WhatsOnChain provider keeps its default until it takes a base (bsv-wallet-cli #74) |
//! | `BITAILS_BASE` | Bitails API base for the broadcast verifier's chain index: unset or blank is the chain's default, a URL moves it. The toolbox's own Bitails provider keeps its default until it takes a base (#74) |
//! | `ARC_GORILLAPOOL_URL` | The GorillaPool ARC as the broadcast verifier's third-party store, never a broadcaster (#300): unset is the chain's default (mainnet only), empty leaves it out, a URL moves it |

use anyhow::{Context, Result};
use bsv_wallet_toolbox::services::providers::{
    bitails::{BITAILS_MAINNET_URL, BITAILS_TESTNET_URL},
    whatsonchain::{WOC_MAINNET_URL, WOC_TESTNET_URL},
};
use bsv_wallet_toolbox::{
    services::{ArcadeConfig, ARCADE_V2_MAINNET},
    ArcConfig, Chain, ServicesOptions,
};
use std::io::Write;
use std::path::PathBuf;
use std::sync::Once;

use crate::config::{
    shown_url, url_or, ArcMode, HeaderService, Reader, Secret, ServicesSettings, Value,
};

/// What an unset or empty `CHAINTRACKS_URL` is told. No explorer in the
/// proof path (P0-1c, bsv-stack-lean #48): the header source is the
/// operator's own service, and ours (`chaintracks-cloudflare`) documents no
/// public URL to default to.
pub const CHAINTRACKS_URL_REQUIRED: &str = "CHAINTRACKS_URL is not set: set it to the header \
     service that checks merkle roots (your chaintracks-cloudflare or rust-chaintracks \
     deployment), or to `off` to run with none (every merkle proof is then refused). There is \
     no default header service.";

/// The CLI's explicit classic endpoints (#322). Toolbox beta.2 defaults to
/// Arcade, so a classic caller must never inherit its `arc_url` default.
/// Classic ARC is deprecated and remains for callers that select it.
#[must_use]
pub const fn classic_arc_url(chain: Chain) -> &'static str {
    match chain {
        Chain::Main => "https://arc.taal.com",
        Chain::Test => "https://arc-test.taal.com",
    }
}

/// Resolve the header service from `CHAINTRACKS_URL` as the configuration
/// read it.
///
/// A URL is the header service. `off` returns `None`: the
/// wallet has no chain tracker, and the toolbox refuses every proof that
/// reaches it (webhook, SSE, monitor, relay) rather than take it on the
/// broadcaster's word, so nothing is marked proven; only ever right for an
/// offline or air-gapped run. Unset or empty is an error
/// ([`CHAINTRACKS_URL_REQUIRED`]): there is no default.
pub fn chaintracks_url_for(configured: Option<&HeaderService>) -> Result<Option<String>> {
    match configured {
        Some(HeaderService::Url(url)) => Ok(Some(url.clone())),
        Some(HeaderService::Off) => Ok(None),
        None => anyhow::bail!(CHAINTRACKS_URL_REQUIRED),
    }
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

/// The bases of the explorers and the third-party store the CLI's own
/// rungs ask (`explorers.rs`, `broadcast_verify.rs`): `WhatsOnChain` and
/// Bitails always on, at the chain's default or where `WOC_BASE` /
/// `BITAILS_BASE` point; `GorillaPool`'s ARC at the chain's default (mainnet
/// only), where `ARC_GORILLAPOOL_URL` points, or left out when it is empty.
/// A hermetic run points all three at a local stub (#74).
///
/// The toolbox's own `WhatsOnChain` and Bitails providers do not take a base
/// (`ServicesOptions` has `bananablocks_base` alone); `GorillaPool`'s base
/// here is the broadcast verifier's alone: [`services_options`] never hands
/// it to the toolbox as a broadcaster (#300).
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
    /// The chain's defaults, the toolbox's: its constants, and for
    /// `GorillaPool` its options' own default (`ServicesOptions::mainnet`,
    /// `ServicesOptions::testnet`, which leaves it out).
    pub fn defaults(chain: Chain) -> Self {
        let (woc, bitails, options) = match chain {
            Chain::Main => (
                WOC_MAINNET_URL,
                BITAILS_MAINNET_URL,
                ServicesOptions::mainnet(),
            ),
            Chain::Test => (
                WOC_TESTNET_URL,
                BITAILS_TESTNET_URL,
                ServicesOptions::testnet(),
            ),
        };
        Self {
            woc: woc.trim_end_matches('/').to_string(),
            bitails: bitails.trim_end_matches('/').to_string(),
            gorillapool: options.arc_gorillapool_url,
        }
    }

    /// The bases from their values as `crate::config` read them (trimmed,
    /// checked, without a trailing slash): `None` is the default; for
    /// `GorillaPool`, `None` is the default and an empty value leaves it
    /// out.
    #[must_use]
    pub fn configured(
        chain: Chain,
        woc: Option<&str>,
        bitails: Option<&str>,
        gorillapool: Option<&str>,
    ) -> Self {
        let defaults = Self::defaults(chain);
        Self {
            woc: woc.map_or(defaults.woc, str::to_string),
            bitails: bitails.map_or(defaults.bitails, str::to_string),
            gorillapool: match gorillapool {
                None => defaults.gorillapool,
                Some(v) => Some(v.to_string()).filter(|v| !v.is_empty()),
            },
        }
    }
}

/// The explorers' settings (#50, #74): their keys and the bases of the
/// CLI's own rungs and of `GorillaPool`, as `crate::config` read them once at
/// start (`WOC_API_KEY`, `BITAILS_API_KEY`, `BANANABLOCKS_BASE`, `WOC_BASE`,
/// `BITAILS_BASE`, `ARC_GORILLAPOOL_URL`). The explorers' variables are named
/// here, to the configuration's reader, since the architecture rule A3 keeps
/// an explorer's name out of `src/config.rs`; every value is parsed there.
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
    /// The settings from the environment as read at start, on `chain`, with
    /// `GorillaPool`'s base as `crate::config` read it (`ARC_GORILLAPOOL_URL`:
    /// unset `None`, set the value checked, trimmed and without a trailing
    /// slash): a key is trimmed, and a base that is set and not blank must
    /// be a URL, kept without a trailing slash.
    pub(crate) fn read(raw: &Reader, chain: Chain, gorillapool: Option<&str>) -> Self {
        Self {
            woc_api_key: raw.key("WOC_API_KEY"),
            bitails_api_key: raw.key("BITAILS_API_KEY"),
            bananablocks_base: raw.present_url("BANANABLOCKS_BASE"),
            bases: ExplorerBases::configured(
                chain,
                raw.base("WOC_BASE").as_deref(),
                raw.base("BITAILS_BASE").as_deref(),
                gorillapool,
            ),
        }
    }

    /// The settings' rows for `crate::config::Config::knobs`, in their order,
    /// on `chain`. A key's row is `secret`'s (the knob list's, which tells an
    /// empty key from an unset one); `BananaBlocks`' row is the base in
    /// effect, as `WOC_BASE`'s is.
    pub(crate) fn values(
        &self,
        chain: Chain,
        secret: impl Fn(&'static str, Option<&Secret>) -> Value,
    ) -> Vec<(&'static str, Value)> {
        let key = |name, s: &Option<Secret>| (name, secret(name, s.as_ref()));
        let base = |v: &str| Value::Text(shown_url(v));
        vec![
            key("WOC_API_KEY", &self.woc_api_key),
            key("BITAILS_API_KEY", &self.bitails_api_key),
            (
                "BANANABLOCKS_BASE",
                url_or(
                    crate::explorers::bananablocks_base_in_effect(chain, self).as_deref(),
                    Value::None("off"),
                ),
            ),
            ("WOC_BASE", base(&self.bases.woc)),
            ("BITAILS_BASE", base(&self.bases.bitails)),
        ]
    }
}

/// Resolved Arcade V2 runtime settings (present only in Arcade mode).
#[derive(Debug, Clone)]
pub struct ArcadeRuntime {
    /// Arcade base URL ([`arcade_url_for`]: `ARC_URL`, default
    /// [`ARCADE_V2_MAINNET`] on mainnet).
    pub url: String,
    /// Per-wallet callback token (env override or persisted next to the db).
    pub callback_token: String,
    /// Public HTTPS webhook URL passed as `X-CallbackUrl` on submits.
    pub public_callback_url: Option<String>,
}

/// What a testnet wallet in Arcade mode without `ARC_URL` is told.
pub const NO_TESTNET_ARCADE_DEFAULT: &str =
    "no testnet Arcade default; set ARC_URL (or ARC_MODE=classic, deprecated)";

/// The Arcade endpoint for `chain` from `ARC_URL` as the configuration read
/// it (trimmed, blank none).
///
/// Unset on mainnet is [`ARCADE_V2_MAINNET`]. Unset on testnet is refused
/// by name: the toolbox has no testnet Arcade, and a testnet wallet must
/// never post to mainnet Arcade, nor fall back to classic ARC (#300). The
/// broadcast verifier reads its plane through this same rule.
///
/// # Errors
///
/// [`NO_TESTNET_ARCADE_DEFAULT`]: `chain` is the test chain and `arc_url`
/// is unset or blank.
pub fn arcade_url_for(chain: Chain, arc_url: Option<&str>) -> Result<String> {
    match (arc_url.map(str::trim).filter(|s| !s.is_empty()), chain) {
        (Some(url), _) => Ok(url.to_string()),
        (None, Chain::Main) => Ok(ARCADE_V2_MAINNET.to_string()),
        (None, Chain::Test) => anyhow::bail!("{NO_TESTNET_ARCADE_DEFAULT}"),
    }
}

/// Resolve the Arcade runtime settings, or `None` in classic mode
/// ([`ServicesSettings::arcade`]).
///
/// `db_path` locates the persisted per-wallet callback token
/// (`<db>.callback-token`), giving each wallet db/port its own independent
/// SSE stream and webhook identity. A testnet wallet without `ARC_URL`
/// ([`arcade_url_for`]) is an error here.
pub fn arcade_runtime(
    settings: &ServicesSettings,
    chain: Chain,
    db_path: &str,
) -> Result<Option<ArcadeRuntime>> {
    if settings.arc_mode == ArcMode::Classic {
        return Ok(None);
    }
    let url = arcade_url_for(chain, settings.arc_url.as_deref())?;
    let callback_token = resolve_callback_token(
        settings
            .callback_token
            .as_ref()
            .map(crate::config::Secret::expose),
        db_path,
    )?;
    Ok(Some(ArcadeRuntime {
        url,
        callback_token,
        public_callback_url: settings.public_callback_url.clone(),
    }))
}

/// Resolve the per-wallet callback token.
///
/// Priority: `CALLBACK_TOKEN` (`configured`, as `crate::config` read it:
/// trimmed, blank none) → persisted `<db>.callback-token` file →
/// auto-generate a random 32-hex token and persist it (0600 on unix). The
/// token is NEVER logged.
pub fn resolve_callback_token(configured: Option<&str>, db_path: &str) -> Result<String> {
    if let Some(tok) = configured {
        return Ok(tok.to_string());
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

/// What a wallet in Arcade mode is told, once, when a TAAL key is set.
pub const TAAL_KEY_UNUSED_WARNING: &str = "is set but unused in Arcade mode (arc is just \
     deprecated, arcade is the standard); no classic ARC provider runs in Arcade mode; \
     remove the knob";

/// Build `ServicesOptions` from the configuration (the ONE shared helper).
///
/// `db_path` is used only in Arcade mode, to resolve the persisted callback
/// token. The first build logs the postBeef providers once
/// (`post_beef_ladder`, [`crate::broadcast_verify::post_beef_ladder`]).
pub fn services_options(
    settings: &ServicesSettings,
    chain: Chain,
    db_path: &str,
) -> Result<ServicesOptions> {
    static LADDER: Once = Once::new();
    let opts = services_options_quiet(settings, chain, db_path)?;
    LADDER.call_once(|| {
        tracing::info!(
            marker = "post_beef_ladder",
            mode = if opts.arcade_v2 { "arcade" } else { "classic" },
            broadcaster = %opts.arc_url,
            providers = %crate::broadcast_verify::post_beef_ladder(&opts).join(","),
            "broadcast ladder"
        );
    });
    Ok(opts)
}

fn services_options_quiet(
    settings: &ServicesSettings,
    chain: Chain,
    db_path: &str,
) -> Result<ServicesOptions> {
    let mut opts = match chain {
        Chain::Main => ServicesOptions::mainnet(),
        Chain::Test => ServicesOptions::testnet(),
    };

    match settings.header_service()? {
        Some(url) => opts = opts.with_chaintracks_url(url),
        None => tracing::warn!("{}", CHAINTRACKS_OFF_WARNING),
    }
    if settings.break_glass_explorer_headers {
        tracing::warn!(
            marker = "break_glass_explorer_header",
            "{}",
            BREAK_GLASS_WARNING
        );
        opts = opts.with_break_glass_explorer_headers(true);
    }

    // The explorers (bsv-wallet-cli #50): WhatsOnChain, BananaBlocks and
    // Bitails, each the others' fallback in the toolbox's ladders.
    let explorers = &settings.explorers;
    if let Some(key) = &explorers.woc_api_key {
        opts = opts.with_woc_api_key(key.expose().to_string());
    }
    if let Some(key) = &explorers.bitails_api_key {
        opts = opts.with_bitails_api_key(key.expose().to_string());
    }
    opts.bananablocks_base = explorers.bananablocks_base.clone();

    // GorillaPool's ARC is never a rung, in either mode: the owner's word of
    // 2026-10-27 ("arc is old af... we use arcade now for everyting") and
    // the keeper's reading of it (#300: GorillaPool's ARC is a rung of no
    // gate and no courier; it held every submission of the 0.18.2 gate at
    // ANNOUNCED for over thirteen minutes). The toolbox's mainnet default
    // registers it whenever this is Some. `ARC_GORILLAPOOL_URL` stays the
    // broadcast verifier's third-party store; BananaBlocks, its explorer,
    // keeps its explorer rung above: neither is a broadcaster.
    opts.arc_gorillapool_url = None;

    if let Some(runtime) = arcade_runtime(settings, chain, db_path)? {
        // Arcade V2 mode: explicit flag on ServicesOptions (never inferred
        // from the URL). The TAAL keys are unused in Arcade mode (arc is just
        // deprecated, arcade is the standard): no classic ARC config is
        // passed. Toolbox #317 builds no classic ARC provider in this mode.
        warn_unused_taal_keys(settings);
        let arcade_config = ArcadeConfig {
            callback_token: Some(runtime.callback_token.clone()),
            callback_url: runtime.public_callback_url.clone(),
            ..Default::default()
        };
        opts.arc_config = None;
        opts = opts.with_arcade(runtime.url, Some(arcade_config));
    } else {
        // The CLI names its own deprecated classic endpoint. The toolbox's
        // default is Arcade at beta.2, including when ARC_URL is unset.
        let arc_url = settings
            .arc_url
            .clone()
            .unwrap_or_else(|| classic_arc_url(chain).to_string());
        #[allow(deprecated)] // Explicit classic mode, including the hermetic ARC stub (#322).
        {
            opts = opts.with_classic_arc(arc_url, classic_arc_config(settings));
        }
    }

    Ok(opts)
}

/// TAAL ARC auth, for the classic mode only (deprecated, #300):
/// - `TAAL_API_KEY`       → `Authorization: Bearer <key>`
/// - `MAIN_TAAL_API_KEY`  → raw `Authorization: <key>` (TAAL accepts the key
///   WITHOUT the Bearer prefix; kept for backward compatibility with
///   existing daemon deployments).
fn classic_arc_config(settings: &ServicesSettings) -> Option<ArcConfig> {
    let mut arc_config: Option<ArcConfig> = settings
        .taal_api_key
        .as_ref()
        .map(|key| ArcConfig::with_api_key(key.expose().to_string()));
    if let Some(key) = &settings.main_taal_api_key {
        let mut headers = std::collections::HashMap::new();
        headers.insert("Authorization".to_string(), key.expose().to_string());
        let mut cfg = arc_config.unwrap_or_default();
        cfg.headers = Some(headers);
        arc_config = Some(cfg);
    }
    arc_config
}

/// In Arcade mode a set TAAL key is unused: said once, at warn.
fn warn_unused_taal_keys(settings: &ServicesSettings) {
    static TAAL_UNUSED: Once = Once::new();
    TAAL_UNUSED.call_once(|| {
        for (name, value) in [
            ("TAAL_API_KEY", &settings.taal_api_key),
            ("MAIN_TAAL_API_KEY", &settings.main_taal_api_key),
        ] {
            if value.is_some() {
                tracing::warn!("{name} {TAAL_KEY_UNUSED_WARNING}");
            }
        }
    });
}

#[cfg(test)]
mod arc_mode_tests;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chaintracks_has_no_default() {
        let err = chaintracks_url_for(None).unwrap_err();
        assert!(
            err.to_string().contains("CHAINTRACKS_URL is not set"),
            "{err}"
        );
        assert!(!CHAINTRACKS_URL_REQUIRED.contains("babbage"));
    }

    #[test]
    fn chaintracks_honours_an_explicit_url() {
        let url = HeaderService::Url("https://ct.example/v1".to_string());
        assert_eq!(
            chaintracks_url_for(Some(&url)).unwrap().as_deref(),
            Some("https://ct.example/v1")
        );
    }

    #[test]
    fn chaintracks_off_disables_validation_on_purpose() {
        assert_eq!(
            chaintracks_url_for(Some(&HeaderService::Off)).unwrap(),
            None
        );
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
        let opts = |pairs: &[(&str, &str)]| ServicesOptions {
            bananablocks_base: crate::config::Config::from_pairs(pairs)
                .unwrap()
                .services
                .explorers
                .bananablocks_base,
            ..ServicesOptions::mainnet()
        };
        assert_eq!(
            opts(&[]).bananablocks_base_for(Chain::Main).as_deref(),
            Some("https://bananablocks.com/api/v1")
        );
        assert_eq!(
            opts(&[]).bananablocks_base_for(Chain::Test).as_deref(),
            Some("https://test.bananablocks.com/api/v1")
        );
        for off in ["", "  "] {
            let base = [("BANANABLOCKS_BASE", off)];
            assert_eq!(
                opts(&base).bananablocks_base_for(Chain::Main),
                None,
                "{off:?}"
            );
        }
        assert_eq!(
            opts(&[("BANANABLOCKS_BASE", " https://mirror.example/api/v1 ")])
                .bananablocks_base_for(Chain::Main)
                .as_deref(),
            Some("https://mirror.example/api/v1")
        );
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

        assert_eq!(
            ExplorerBases::configured(Chain::Main, None, None, None),
            main
        );
        let moved = ExplorerBases::configured(
            Chain::Main,
            Some("http://127.0.0.1:3380/woc"),
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

    /// `ARC_GORILLAPOOL_URL` (#74): unset is the chain's default, empty
    /// leaves `GorillaPool` out, a URL moves it (`crate::config` has trimmed
    /// it, blank to empty, and dropped its trailing slash).
    #[test]
    fn gorillapool_unset_is_the_default_and_empty_is_out() {
        let at =
            |v: Option<&str>| ExplorerBases::configured(Chain::Main, None, None, v).gorillapool;
        assert_eq!(at(None).as_deref(), Some("https://arc.gorillapool.io"));
        assert_eq!(at(Some("")), None);
        assert_eq!(
            at(Some("https://gp.example")).as_deref(),
            Some("https://gp.example")
        );
    }

    /// `CALLBACK_TOKEN` (#172): the token in effect is the one the
    /// configuration read, trimmed; a blank one is none (its knob shows
    /// `empty`), so the persisted token stands, never the blank.
    #[test]
    fn the_callback_token_in_effect_is_the_configurations() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("w.db");
        let db = db.to_str().unwrap();
        let persisted = resolve_callback_token(None, db).unwrap();
        let in_effect = |token: &str| {
            let config =
                crate::config::Config::from_pairs(&[("ARCADE", "1"), ("CALLBACK_TOKEN", token)])
                    .unwrap();
            let runtime = arcade_runtime(&config.services, Chain::Main, db)
                .unwrap()
                .unwrap();
            let shown = config
                .knobs()
                .into_iter()
                .find(|k| k.name == "CALLBACK_TOKEN")
                .unwrap()
                .value
                .to_string();
            (runtime.callback_token, shown)
        };
        assert_eq!(in_effect(" t "), ("t".to_string(), "<set>".to_string()));
        for blank in ["", "   "] {
            assert_eq!(
                in_effect(blank),
                (persisted.clone(), "empty".to_string()),
                "{blank:?}"
            );
        }
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

        // No CALLBACK_TOKEN configured; read/created from file.
        let tok = resolve_callback_token(None, db).unwrap();
        assert_eq!(tok.len(), 32);
        assert!(tok.chars().all(|c| c.is_ascii_hexdigit()));

        // Stable on re-read; a configured token wins, as configured.
        let tok2 = resolve_callback_token(None, db).unwrap();
        assert_eq!(
            resolve_callback_token(Some("t"), db).unwrap(),
            "t".to_string()
        );
        assert_eq!(tok, tok2);

        // Persisted next to the db.
        assert!(callback_token_path(db).exists());
    }
}
