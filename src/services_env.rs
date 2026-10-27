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
//! | `ARC_MODE` | The broadcaster mode: unset or `arcade` (any case) is Arcade V2 (EF-only submit, SSE status stream, push proofs), the default by the owner's word of 2026-10-27 (#300); `classic` (any case) is the toolbox's classic ARC ladder, the one opt-out, kept for the hermetic stub until #299; any other value is refused by name |
//! | `ARCADE` | The older switch, read when `ARC_MODE` is unset: `1`, `true` or `yes` is Arcade, `0`, `false` or `no` is classic; any other value is refused by name |
//! | `ARC_URL` | Override the broadcaster URL: the Arcade endpoint in Arcade mode (default `ARCADE_V2_MAINNET`), the classic ARC URL in classic mode (default the chain's TAAL ARC) |
//! | `CALLBACK_TOKEN` | Override the per-wallet callback token (otherwise auto-generated and persisted next to the db) |
//! | `PUBLIC_CALLBACK_URL` | Public HTTPS URL Arcade should POST status webhooks to (`X-CallbackUrl`) |
//! | `TAAL_API_KEY` | Deprecated (#300): classic mode only, TAAL ARC key sent as `Authorization: Bearer <key>`; in Arcade mode it is unused and logged once at warn |
//! | `MAIN_TAAL_API_KEY` | Deprecated (#300): classic mode only, TAAL ARC key sent as raw `Authorization: <key>` (TAAL accepts no Bearer prefix); in Arcade mode it is unused and logged once at warn |
//! | `WOC_API_KEY` | WhatsOnChain key, sent as `woc-api-key` and `Authorization: Bearer` by the toolbox's provider and the CLI's own WhatsOnChain rung (optional) |
//! | `BITAILS_API_KEY` | Bitails key (optional) |
//! | `BANANABLOCKS_BASE` | BananaBlocks API base: unset is the chain's default (`https://bananablocks.com/api/v1`, `https://test.bananablocks.com/api/v1`), empty turns the rung off, a URL moves it (no key exists) |

use anyhow::{Context, Result};
use bsv_wallet_toolbox::{
    services::{ArcadeConfig, ARCADE_V2_MAINNET},
    ArcConfig, Chain, ServicesOptions, PROVIDER_ARCADE_V2, PROVIDER_BITAILS,
    PROVIDER_GORILLAPOOL_ARC, PROVIDER_TAAL_ARC, PROVIDER_WHATSONCHAIN,
};
use std::io::Write;
use std::path::PathBuf;
use std::sync::Once;

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

/// The environment variable naming the broadcaster mode.
pub const ARC_MODE_ENV: &str = "ARC_MODE";

/// The older Arcade switch, read when `ARC_MODE` is unset.
pub const ARCADE_ENV: &str = "ARCADE";

/// The broadcaster mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArcMode {
    /// Arcade V2, the default (the owner's word of 2026-10-27, #300: "arc is
    /// old af... we use arcade now for everything").
    Arcade,
    /// The toolbox's classic ARC ladder: the one opt-out, kept for the
    /// hermetic stub, which answers classic ARC only, until #299 gives it an
    /// Arcade face. No production wallet runs it.
    Classic,
}

/// The broadcaster mode from `ARC_MODE` and `ARCADE` (blank is unset).
///
/// `ARC_MODE` wins when set: `arcade` or `classic` (any case), anything
/// else refused by name. Unset, `ARCADE` decides: `1`, `true` or `yes` is
/// Arcade, `0`, `false` or `no` is classic, anything else refused by name.
/// Both unset is Arcade.
pub fn arc_mode_for(arc_mode: Option<&str>, arcade: Option<&str>) -> Result<ArcMode> {
    fn blank_is_unset(v: Option<&str>) -> Option<&str> {
        v.map(str::trim).filter(|v| !v.is_empty())
    }
    if let Some(mode) = blank_is_unset(arc_mode) {
        return match mode.to_ascii_lowercase().as_str() {
            "arcade" => Ok(ArcMode::Arcade),
            "classic" => Ok(ArcMode::Classic),
            _ => anyhow::bail!(
                "{ARC_MODE_ENV}={mode} is not a broadcaster mode: set {ARC_MODE_ENV} to \
                 `arcade` (the default when unset) or `classic` (the hermetic stub's, #299)"
            ),
        };
    }
    match blank_is_unset(arcade)
        .map(str::to_ascii_lowercase)
        .as_deref()
    {
        None | Some("1" | "true" | "yes") => Ok(ArcMode::Arcade),
        Some("0" | "false" | "no") => Ok(ArcMode::Classic),
        Some(other) => anyhow::bail!(
            "{ARCADE_ENV}={other} is not a broadcaster switch: use {ARC_MODE_ENV}=arcade \
             (the default when unset) or {ARC_MODE_ENV}=classic, or {ARCADE_ENV}=1 or 0"
        ),
    }
}

/// The broadcaster mode from the process environment ([`arc_mode_for`]).
pub fn arc_mode_from_env() -> Result<ArcMode> {
    arc_mode_with(&process_env)
}

fn arc_mode_with(env: &dyn Fn(&str) -> Option<String>) -> Result<ArcMode> {
    arc_mode_for(env(ARC_MODE_ENV).as_deref(), env(ARCADE_ENV).as_deref())
}

/// Whether the wallet runs in Arcade V2 mode, the default ([`ArcMode`]).
///
/// For readers that cannot fail (the broadcast verifier's plane): a mode
/// value [`arc_mode_for`] refuses never gets this far, since
/// [`services_options_from_env`] refuses it before any command runs, and
/// reads here as the default.
pub fn arcade_mode_enabled() -> bool {
    !matches!(arc_mode_from_env(), Ok(ArcMode::Classic))
}

/// What a testnet wallet in Arcade mode without `ARC_URL` is told.
pub const NO_TESTNET_ARCADE_DEFAULT: &str =
    "no testnet Arcade default; set ARC_URL (or ARC_MODE=classic, deprecated)";

/// The Arcade endpoint for `chain` from `ARC_URL` (blank is unset).
///
/// Unset on mainnet is [`ARCADE_V2_MAINNET`]. Unset on testnet is refused
/// by name: the toolbox has no testnet Arcade, and a testnet wallet must
/// never post to mainnet Arcade, nor fall back to classic ARC (#300). The
/// broadcast verifier reads its plane through this same rule.
pub fn arcade_url_for(chain: Chain, arc_url: Option<&str>) -> Result<String> {
    match (arc_url.map(str::trim).filter(|s| !s.is_empty()), chain) {
        (Some(url), _) => Ok(url.to_string()),
        (None, Chain::Main) => Ok(ARCADE_V2_MAINNET.to_string()),
        (None, Chain::Test) => anyhow::bail!("{NO_TESTNET_ARCADE_DEFAULT}"),
    }
}

/// Resolve the Arcade runtime settings, or `None` in classic mode.
///
/// `db_path` locates the persisted per-wallet callback token
/// (`<db>.callback-token`), giving each wallet db/port its own independent
/// SSE stream and webhook identity. A mode [`arc_mode_for`] refuses, or a
/// testnet wallet without `ARC_URL` ([`arcade_url_for`]), is an error here
/// too.
pub fn arcade_runtime(chain: Chain, db_path: &str) -> Result<Option<ArcadeRuntime>> {
    arcade_runtime_with(chain, db_path, &process_env)
}

fn arcade_runtime_with(
    chain: Chain,
    db_path: &str,
    env: &dyn Fn(&str) -> Option<String>,
) -> Result<Option<ArcadeRuntime>> {
    if arc_mode_with(env)? == ArcMode::Classic {
        return Ok(None);
    }
    let url = arcade_url_for(chain, env("ARC_URL").as_deref())?;
    let callback_token = resolve_callback_token(db_path, env)?;
    let public_callback_url = env("PUBLIC_CALLBACK_URL").filter(|s| !s.is_empty());
    Ok(Some(ArcadeRuntime {
        url,
        callback_token,
        public_callback_url,
    }))
}

/// One variable of the process environment, `None` when unset or not UTF-8.
fn process_env(key: &str) -> Option<String> {
    std::env::var(key).ok()
}

/// Resolve the per-wallet callback token.
///
/// Priority: `CALLBACK_TOKEN` env → persisted `<db>.callback-token` file →
/// auto-generate a random 32-hex token and persist it (0600 on unix).
/// The token is NEVER logged.
fn resolve_callback_token(db_path: &str, env: &dyn Fn(&str) -> Option<String>) -> Result<String> {
    if let Some(tok) = env("CALLBACK_TOKEN") {
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

/// What a wallet in Arcade mode is told, once, when a TAAL key is set.
pub const TAAL_KEY_UNUSED_WARNING: &str = "is set but unused in Arcade mode (arc is just \
     deprecated, arcade is the standard); the toolbox's TAAL provider posts unauthenticated \
     until toolbox #317; remove the knob";

/// The postBeef ladder the toolbox builds from `opts`, in order, by its
/// provider names: the rule of `Services::with_options` in toolbox 0.13.1
/// (`services.rs` 775-803), which logs no provider list of its own. In
/// Arcade mode the classic TAAL provider stays behind Arcade, pointed at
/// TAAL's URL, until toolbox #317 drops it.
pub fn post_beef_ladder(opts: &ServicesOptions) -> Vec<&'static str> {
    let mut ladder = Vec::new();
    if opts.arcade_v2 {
        ladder.push(PROVIDER_ARCADE_V2);
    }
    ladder.push(PROVIDER_TAAL_ARC);
    if opts.arc_gorillapool_url.is_some() {
        ladder.push(PROVIDER_GORILLAPOOL_ARC);
    }
    ladder.push(PROVIDER_BITAILS);
    ladder.push(PROVIDER_WHATSONCHAIN);
    ladder
}

/// Build `ServicesOptions` from the environment (the ONE shared helper).
///
/// `db_path` is used only in Arcade mode, to resolve the persisted callback
/// token. An `ARC_MODE` (or `ARCADE`) value [`arc_mode_for`] refuses is an
/// error here, before any daemon or command runs; the subcommands that
/// build no services (`init`, `backup`, `ui`) do not read it.
pub fn services_options_from_env(chain: Chain, db_path: &str) -> Result<ServicesOptions> {
    let opts = services_options_with(chain, db_path, &process_env)?;
    static LADDER: Once = Once::new();
    LADDER.call_once(|| {
        tracing::info!(
            marker = "post_beef_ladder",
            mode = if opts.arcade_v2 { "arcade" } else { "classic" },
            broadcaster = %opts.arc_url,
            providers = %post_beef_ladder(&opts).join(","),
            "broadcast ladder"
        );
    });
    Ok(opts)
}

fn services_options_with(
    chain: Chain,
    db_path: &str,
    env: &dyn Fn(&str) -> Option<String>,
) -> Result<ServicesOptions> {
    let mut opts = match chain {
        Chain::Main => ServicesOptions::mainnet(),
        Chain::Test => ServicesOptions::testnet(),
    };

    let configured = env("CHAINTRACKS_URL");
    match chaintracks_url_for(configured.as_deref())? {
        Some(url) => opts = opts.with_chaintracks_url(url),
        None => tracing::warn!("{}", CHAINTRACKS_OFF_WARNING),
    }
    let break_glass = env(BREAK_GLASS_EXPLORER_HEADERS_ENV);
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
    if let Some(key) = api_key_for(env(WOC_API_KEY_ENV).as_deref()) {
        opts = opts.with_woc_api_key(key);
    }
    if let Some(key) = api_key_for(env(BITAILS_API_KEY_ENV).as_deref()) {
        opts = opts.with_bitails_api_key(key);
    }
    opts.bananablocks_base = bananablocks_base_for(env(BANANABLOCKS_BASE_ENV).as_deref());

    // GorillaPool's ARC is never a rung, in either mode: the owner's word of
    // 2026-10-27 ("arc is old af... we use arcade now for everything") and
    // the keeper's reading of it (#300: GorillaPool's ARC is a rung of no
    // gate and no courier; it held every submission of the 0.18.2 gate at
    // ANNOUNCED for over thirteen minutes). The toolbox's mainnet default
    // registers it whenever this is Some. BananaBlocks, its explorer, keeps
    // its explorer rung above: it is not ARC.
    opts.arc_gorillapool_url = None;

    // TAAL ARC auth, for the classic mode only (deprecated, #300):
    // - TAAL_API_KEY       → `Authorization: Bearer <key>`
    // - MAIN_TAAL_API_KEY  → raw `Authorization: <key>` (TAAL accepts the key
    //   WITHOUT the Bearer prefix; kept for backward compatibility with
    //   existing daemon deployments).
    let taal_api_key = env("TAAL_API_KEY").filter(|k| !k.is_empty());
    let main_taal_api_key = env("MAIN_TAAL_API_KEY").filter(|k| !k.is_empty());

    if let Some(runtime) = arcade_runtime_with(chain, db_path, env)? {
        // Arcade V2 mode: explicit flag on ServicesOptions (never inferred
        // from the URL). The TAAL keys are unused in Arcade mode (arc is just
        // deprecated, arcade is the standard): no classic ARC config is
        // passed. The toolbox's TAAL provider still sits behind Arcade
        // (services.rs 660-673) and posts unauthenticated until toolbox #317
        // drops it.
        static TAAL_UNUSED: Once = Once::new();
        TAAL_UNUSED.call_once(|| {
            for (name, value) in [
                ("TAAL_API_KEY", &taal_api_key),
                ("MAIN_TAAL_API_KEY", &main_taal_api_key),
            ] {
                if value.is_some() {
                    tracing::warn!("{name} {TAAL_KEY_UNUSED_WARNING}");
                }
            }
        });
        let arcade_config = ArcadeConfig {
            callback_token: Some(runtime.callback_token.clone()),
            callback_url: runtime.public_callback_url.clone(),
            ..Default::default()
        };
        opts.arc_config = None;
        opts = opts.with_arcade(runtime.url, Some(arcade_config));
    } else {
        let mut arc_config: Option<ArcConfig> = taal_api_key.map(ArcConfig::with_api_key);
        if let Some(key) = main_taal_api_key {
            let mut headers = std::collections::HashMap::new();
            headers.insert("Authorization".to_string(), key);
            let mut cfg = arc_config.unwrap_or_default();
            cfg.headers = Some(headers);
            arc_config = Some(cfg);
        }
        // Classic ARC: honor ARC_URL override, else keep the chain default.
        let arc_url = env("ARC_URL")
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

    /// An environment of the given pairs alone: the builder reads it as it
    /// reads the process's, with no process variable set or raced.
    fn env_of(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<String> {
        let pairs: Vec<(String, String)> = pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        move |key| pairs.iter().find(|(k, _)| k == key).map(|(_, v)| v.clone())
    }

    /// The options the builder makes from `pairs`, with `CHAINTRACKS_URL=off`,
    /// a callback token (so no file is written) and a scratch db path.
    fn built(chain: Chain, pairs: &[(&str, &str)]) -> Result<ServicesOptions> {
        let mut all = vec![("CHAINTRACKS_URL", "off"), ("CALLBACK_TOKEN", "t0")];
        all.extend_from_slice(pairs);
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("w.db");
        services_options_with(chain, db.to_str().unwrap(), &env_of(&all))
    }

    #[test]
    fn arcade_is_the_default_mode() {
        assert_eq!(arc_mode_for(None, None).unwrap(), ArcMode::Arcade);
        assert_eq!(arc_mode_for(Some(" "), Some("")).unwrap(), ArcMode::Arcade);
        let opts = built(Chain::Main, &[]).unwrap();
        assert!(opts.arcade_v2);
        assert_eq!(opts.arc_url, ARCADE_V2_MAINNET);
    }

    #[test]
    fn classic_is_the_one_opt_out() {
        for (arc_mode, arcade) in [
            (Some("classic"), None),
            (Some("CLASSIC"), None),
            (None, Some("0")),
            (None, Some("false")),
            (None, Some("No")),
        ] {
            assert_eq!(
                arc_mode_for(arc_mode, arcade).unwrap(),
                ArcMode::Classic,
                "{arc_mode:?} {arcade:?}"
            );
        }
        for pairs in [[("ARC_MODE", "classic")], [("ARCADE", "0")]] {
            let opts = built(Chain::Main, &pairs).unwrap();
            assert!(!opts.arcade_v2, "{pairs:?}");
            assert_eq!(opts.arc_url, "https://arc.taal.com", "{pairs:?}");
        }
    }

    #[test]
    fn arcade_named_in_any_case_is_arcade() {
        for (arc_mode, arcade) in [
            (Some("arcade"), None),
            (Some("ARCADE"), None),
            (Some(" Arcade "), Some("0")),
            (None, Some("1")),
            (None, Some("TRUE")),
            (None, Some("yes")),
        ] {
            assert_eq!(
                arc_mode_for(arc_mode, arcade).unwrap(),
                ArcMode::Arcade,
                "{arc_mode:?} {arcade:?}"
            );
        }
        for pairs in [[("ARC_MODE", "arcade")], [("ARCADE", "1")]] {
            assert!(built(Chain::Main, &pairs).unwrap().arcade_v2, "{pairs:?}");
        }
    }

    #[test]
    fn an_unknown_mode_is_refused_by_name() {
        let err = arc_mode_for(Some("bogus"), None).unwrap_err().to_string();
        assert!(
            err.starts_with("ARC_MODE=bogus is not a broadcaster mode"),
            "{err}"
        );
        assert!(
            err.contains("`arcade`") && err.contains("`classic`"),
            "{err}"
        );
        let err = arc_mode_for(None, Some("maybe")).unwrap_err().to_string();
        assert!(err.starts_with("ARCADE=maybe"), "{err}");
        // The builder refuses it too, before any service is made.
        let err = built(Chain::Main, &[("ARC_MODE", "bogus")])
            .unwrap_err()
            .to_string();
        assert!(err.starts_with("ARC_MODE=bogus"), "{err}");
    }

    #[test]
    fn gorillapool_arc_is_never_a_rung() {
        assert!(ServicesOptions::mainnet().arc_gorillapool_url.is_some());
        for chain in [Chain::Main, Chain::Test] {
            // A testnet wallet in Arcade mode names its endpoint.
            let arcade: &[(&str, &str)] = match chain {
                Chain::Main => &[],
                Chain::Test => &[("ARC_URL", "https://arcade-test.invalid")],
            };
            for pairs in [arcade, &[("ARC_MODE", "classic")][..]] {
                let opts = built(chain, pairs).unwrap();
                assert_eq!(opts.arc_gorillapool_url, None, "{chain:?} {pairs:?}");
            }
        }
        // BananaBlocks, its explorer, keeps its rung.
        let opts = built(Chain::Main, &[]).unwrap();
        assert_eq!(
            opts.bananablocks_base_for(Chain::Main).as_deref(),
            Some("https://bananablocks.com/api/v1")
        );
    }

    #[test]
    fn arcade_mode_passes_no_classic_arc_config() {
        let keys = [("TAAL_API_KEY", "k1"), ("MAIN_TAAL_API_KEY", "k2")];
        assert!(built(Chain::Main, &keys).unwrap().arc_config.is_none());
        let classic = built(
            Chain::Main,
            &[("ARC_MODE", "classic"), ("TAAL_API_KEY", "k1")],
        )
        .unwrap();
        assert_eq!(
            classic.arc_config.and_then(|c| c.api_key).as_deref(),
            Some("k1")
        );
        let classic = built(
            Chain::Main,
            &[("ARC_MODE", "classic"), ("MAIN_TAAL_API_KEY", "k2")],
        )
        .unwrap();
        assert_eq!(
            classic
                .arc_config
                .and_then(|c| c.headers)
                .and_then(|h| h.get("Authorization").cloned())
                .as_deref(),
            Some("k2")
        );
    }

    #[test]
    fn arc_url_moves_the_broadcaster_in_either_mode() {
        let arcade = built(Chain::Main, &[("ARC_URL", "https://arcade.invalid")]).unwrap();
        assert!(arcade.arcade_v2);
        assert_eq!(arcade.arc_url, "https://arcade.invalid");
        let classic = built(
            Chain::Main,
            &[("ARC_MODE", "classic"), ("ARC_URL", "https://arc.invalid")],
        )
        .unwrap();
        assert!(!classic.arcade_v2);
        assert_eq!(classic.arc_url, "https://arc.invalid");
    }

    #[test]
    fn a_blank_arc_url_is_the_arcade_default() {
        for blank in ["", "  "] {
            let opts = built(Chain::Main, &[("ARC_URL", blank)]).unwrap();
            assert!(opts.arcade_v2, "{blank:?}");
            assert_eq!(opts.arc_url, ARCADE_V2_MAINNET, "{blank:?}");
            assert_eq!(post_beef_ladder(&opts)[0], PROVIDER_ARCADE_V2, "{blank:?}");
        }
        assert_eq!(
            arcade_url_for(Chain::Main, Some(" ")).unwrap(),
            ARCADE_V2_MAINNET
        );
    }

    #[test]
    fn testnet_arcade_without_arc_url_is_refused_by_name() {
        for pairs in [
            &[][..],
            &[("ARC_URL", " ")][..],
            &[("ARC_MODE", "arcade")][..],
        ] {
            let err = built(Chain::Test, pairs).unwrap_err().to_string();
            assert_eq!(err, NO_TESTNET_ARCADE_DEFAULT, "{pairs:?}");
        }
        assert_eq!(
            arcade_url_for(Chain::Test, None).unwrap_err().to_string(),
            "no testnet Arcade default; set ARC_URL (or ARC_MODE=classic, deprecated)"
        );
        // ARC_URL set: it runs, on that endpoint, Arcade first.
        let opts = built(Chain::Test, &[("ARC_URL", "https://arcade-test.invalid")]).unwrap();
        assert!(opts.arcade_v2);
        assert_eq!(opts.arc_url, "https://arcade-test.invalid");
        // The deprecated classic knob is the chain's TAAL testnet ARC.
        let opts = built(Chain::Test, &[("ARC_MODE", "classic")]).unwrap();
        assert!(!opts.arcade_v2);
        assert_eq!(opts.arc_url, "https://arc-test.taal.com");
    }

    #[test]
    fn the_taal_warning_names_the_provider_that_still_posts() {
        assert!(TAAL_KEY_UNUSED_WARNING.contains("arc is just deprecated, arcade is the standard"));
        assert!(TAAL_KEY_UNUSED_WARNING.contains("toolbox #317"));
        assert!(!TAAL_KEY_UNUSED_WARNING.contains("no classic ARC rung runs"));
    }

    #[test]
    fn the_post_ladder_reads_arcade_first() {
        assert_eq!(
            post_beef_ladder(&built(Chain::Main, &[]).unwrap()),
            [
                PROVIDER_ARCADE_V2,
                PROVIDER_TAAL_ARC,
                PROVIDER_BITAILS,
                PROVIDER_WHATSONCHAIN
            ]
        );
        assert_eq!(
            post_beef_ladder(&built(Chain::Main, &[("ARC_MODE", "classic")]).unwrap()),
            [PROVIDER_TAAL_ARC, PROVIDER_BITAILS, PROVIDER_WHATSONCHAIN]
        );
        // The toolbox's collection holds as many providers as the ladder
        // names: GorillaPool's presence alone would make them differ.
        for pairs in [&[][..], &[("ARC_MODE", "classic")][..]] {
            let opts = built(Chain::Main, pairs).unwrap();
            let ladder = post_beef_ladder(&opts);
            let services = bsv_wallet_toolbox::Services::with_options(Chain::Main, opts).unwrap();
            assert_eq!(
                services.post_beef_count().unwrap(),
                ladder.len(),
                "{pairs:?}"
            );
        }
        // The toolbox's own default, for the rule the ladder mirrors.
        assert_eq!(
            post_beef_ladder(&ServicesOptions::mainnet()),
            [
                PROVIDER_TAAL_ARC,
                PROVIDER_GORILLAPOOL_ARC,
                PROVIDER_BITAILS,
                PROVIDER_WHATSONCHAIN
            ]
        );
    }

    #[test]
    fn a_blank_api_key_is_no_key() {
        assert_eq!(api_key_for(None), None);
        assert_eq!(api_key_for(Some("  ")), None);
        assert_eq!(api_key_for(Some(" k ")).as_deref(), Some("k"));
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
        let env = env_of(&[]);
        let tok = resolve_callback_token(db, &env).unwrap();
        assert_eq!(tok.len(), 32);
        assert!(tok.chars().all(|c| c.is_ascii_hexdigit()));

        // Stable on re-read.
        let tok2 = resolve_callback_token(db, &env).unwrap();
        assert_eq!(tok, tok2);

        // Persisted next to the db.
        assert!(callback_token_path(db).exists());
    }
}
