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
    bitails::{BITAILS_MAINNET_URL, BITAILS_TESTNET_URL},
    whatsonchain::{WOC_MAINNET_URL, WOC_TESTNET_URL},
};
use bsv_wallet_toolbox::{
    services::{ArcadeConfig, ARCADE_V2_MAINNET},
    ArcConfig, Chain, ServicesOptions,
};
use std::io::Write;
use std::path::PathBuf;

use crate::config::{shown_url, url_or, HeaderService, Reader, Secret, ServicesSettings, Value};

/// What an unset or empty `CHAINTRACKS_URL` is told. No explorer in the
/// proof path (P0-1c, bsv-stack-lean #48): the header source is the
/// operator's own service, and ours (`chaintracks-cloudflare`) documents no
/// public URL to default to.
pub const CHAINTRACKS_URL_REQUIRED: &str = "CHAINTRACKS_URL is not set: set it to the header \
     service that checks merkle roots (your chaintracks-cloudflare or rust-chaintracks \
     deployment), or to `off` to run with none (every merkle proof is then refused). There is \
     no default header service.";

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
/// (`ServicesOptions` has `bananablocks_base` alone); `GorillaPool`'s base
/// here is the toolbox's `arc_gorillapool_url`, set from it by
/// [`services_options`].
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
    /// unset `None`, set the value checked and trimmed): a key is trimmed,
    /// and a base that is set and not blank must be a URL.
    pub(crate) fn read(raw: &Reader, chain: Chain, gorillapool: Option<&str>) -> Self {
        Self {
            woc_api_key: raw.key("WOC_API_KEY"),
            bitails_api_key: raw.key("BITAILS_API_KEY"),
            bananablocks_base: raw.present_url("BANANABLOCKS_BASE"),
            bases: ExplorerBases::configured(
                chain,
                raw.url("WOC_BASE").as_deref(),
                raw.url("BITAILS_BASE").as_deref(),
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
    /// Arcade base URL (from `ARC_URL`, default [`ARCADE_V2_MAINNET`]).
    pub url: String,
    /// Per-wallet callback token (env override or persisted next to the db).
    pub callback_token: String,
    /// Public HTTPS webhook URL passed as `X-CallbackUrl` on submits.
    pub public_callback_url: Option<String>,
}

/// Resolve the Arcade runtime settings, or `None` when not in Arcade mode
/// (`ARC_MODE=arcade` or `ARCADE=1`, [`ServicesSettings::arcade`]).
///
/// `db_path` locates the persisted per-wallet callback token
/// (`<db>.callback-token`), giving each wallet db/port its own independent
/// SSE stream and webhook identity.
pub fn arcade_runtime(settings: &ServicesSettings, db_path: &str) -> Result<Option<ArcadeRuntime>> {
    if !settings.arcade() {
        return Ok(None);
    }
    let url = settings
        .arc_url
        .clone()
        .unwrap_or_else(|| ARCADE_V2_MAINNET.to_string());
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
/// Priority: `CALLBACK_TOKEN` (`configured`) → persisted
/// `<db>.callback-token` file → auto-generate a random 32-hex token and
/// persist it (0600 on unix). The token is NEVER logged.
pub fn resolve_callback_token(configured: Option<&str>, db_path: &str) -> Result<String> {
    if let Some(tok) = configured.map(str::trim).filter(|t| !t.is_empty()) {
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

/// Build `ServicesOptions` from the configuration (the ONE shared helper).
///
/// `db_path` is used only in Arcade mode, to resolve the persisted callback
/// token.
pub fn services_options(
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
    // GorillaPool, the toolbox's second broadcaster: the toolbox's default
    // unless `ARC_GORILLAPOOL_URL` moves it or, empty, leaves it out (#74);
    // the base the knob shows and the verifier asks.
    opts.arc_gorillapool_url
        .clone_from(&explorers.bases.gorillapool);

    // TAAL ARC auth (applies to the classic ARC provider — in Arcade mode
    // that provider is the failover behind Arcade).
    // - TAAL_API_KEY       → `Authorization: Bearer <key>`
    // - MAIN_TAAL_API_KEY  → raw `Authorization: <key>` (TAAL accepts the key
    //   WITHOUT the Bearer prefix; kept for backward compatibility with
    //   existing daemon deployments).
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

    if let Some(runtime) = arcade_runtime(settings, db_path)? {
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
        let arc_url = settings
            .arc_url
            .clone()
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

    /// `ARC_GORILLAPOOL_URL` (#74, #172): the one base, the knob's and the
    /// toolbox's. Unset is `ServicesOptions`' own default for either chain;
    /// empty or blank leaves `GorillaPool` out; a URL moves it.
    #[test]
    fn the_toolbox_takes_the_gorillapool_base_the_knob_shows() {
        let options = |pairs: &[(&str, &str)], chain| {
            let mut pairs = pairs.to_vec();
            pairs.push(("CHAINTRACKS_URL", "off"));
            let flags = crate::config::Flags {
                testnet: chain == Chain::Test,
                ..crate::config::Flags::default()
            };
            let vars = pairs
                .iter()
                .map(|(k, v)| ((*k).to_string(), (*v).to_string()));
            let config = crate::config::Config::from_vars(&flags, vars).unwrap();
            let opts = services_options(&config.services, chain, "unused.db").unwrap();
            (
                opts.arc_gorillapool_url,
                config.services.explorers.bases.gorillapool,
            )
        };
        for (chain, base) in [
            (Chain::Main, ServicesOptions::mainnet()),
            (Chain::Test, ServicesOptions::testnet()),
        ] {
            let (toolbox, shown) = options(&[], chain);
            assert_eq!(toolbox, base.arc_gorillapool_url, "{chain:?}");
            assert_eq!(shown, toolbox, "{chain:?}");
        }
        for blank in ["", "  "] {
            let (toolbox, shown) = options(&[("ARC_GORILLAPOOL_URL", blank)], Chain::Main);
            assert_eq!((toolbox, shown), (None, None), "{blank:?}");
        }
        let (toolbox, shown) = options(
            &[("ARC_GORILLAPOOL_URL", " http://127.0.0.1:3380/arc/ ")],
            Chain::Main,
        );
        assert_eq!(toolbox.as_deref(), Some("http://127.0.0.1:3380/arc"));
        assert_eq!(shown, toolbox);
        assert_eq!(
            ServicesOptions::mainnet().arc_gorillapool_url.as_deref(),
            Some("https://arc.gorillapool.io")
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

        // No CALLBACK_TOKEN configured; read/created from file.
        let tok = resolve_callback_token(None, db).unwrap();
        assert_eq!(tok.len(), 32);
        assert!(tok.chars().all(|c| c.is_ascii_hexdigit()));

        // Stable on re-read; a configured token wins, trimmed.
        let tok2 = resolve_callback_token(Some("  "), db).unwrap();
        assert_eq!(
            resolve_callback_token(Some(" t "), db).unwrap(),
            "t".to_string()
        );
        assert_eq!(tok, tok2);

        // Persisted next to the db.
        assert!(callback_token_path(db).exists());
    }
}
