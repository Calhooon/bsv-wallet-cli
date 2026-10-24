//! The configuration of the wallet's binaries: every setting read from the
//! process environment and the command line, read once at start, typed and
//! checked (#80).
//!
//! [`Config::load`] reads the environment once, before a command runs: each
//! variable `bsv-wallet` knows is parsed into its group ([`ServicesSettings`],
//! [`HostSettings`], [`ReconcileSettings`], [`VerifySettings`],
//! [`ServerSettings`], [`DaemonSettings`]), and the command line's `--db`,
//! `--testnet` and `--port` into [`WalletSettings`]. Everything after reads
//! the [`Config`] it was given: the request path never reads the
//! environment (the spend gate's settings are built from it once, at the
//! server's start). `bsv-wallet-relay` reads its own settings through
//! [`RelaySettings::load`]. This file is the crate's one reader of the
//! environment (the architecture rule A7, `tests/arch_config_env.rs`).
//!
//! The names do not change: they are the operators' interface (README's
//! table, `src/CLAUDE.md`). A number, an address or a URL that does not
//! parse is refused at start with a [`ConfigError`] naming the variable and
//! the shape it must have, never read as its default; unset or blank is the
//! default. A switch keeps the words it always had (each group's field
//! says which), and `CHAINTRACKS_URL` stays required where a command builds
//! its services ([`ServicesSettings::header_service`]), so a command that
//! asks no chain (`init`, `config`) runs without it.
//!
//! A key or a token is a [`Secret`]: its `Debug` and `Display` print
//! `<set>`. [`Config::knobs`] lists every setting with its effective value
//! and its [`Source`]; `bsv-wallet config` prints that list, and `daemon`
//! and `serve` log it as one line ([`Config::log_line`]) at start.

use std::collections::BTreeMap;
use std::fmt;
use std::net::IpAddr;
use std::str::FromStr;

use anyhow::Context;
use bsv_tracker::{Height, Timestamp};
use bsv_wallet_toolbox::Chain;
use serde::Serialize;

mod knobs;
#[cfg(test)]
mod tests;

pub use knobs::Knob;

/// The global flags the configuration takes, as [`Config::knobs`] names
/// them.
pub const FLAG_NAMES: &[&str] = &["--db", "--testnet", "--port"];

/// `bsv-wallet`'s log filter when `RUST_LOG` is unset or does not parse.
pub const DEFAULT_LOG_FILTER: &str = "bsv_wallet=info,tower_http=info";

/// The one read of the process environment, at start: every variable, as
/// a snapshot the settings are parsed from. A name or a value that is not
/// Unicode reads as unset, as `std::env::var` always made it.
fn environment() -> Vec<(String, String)> {
    std::env::vars_os()
        .filter_map(|(k, v)| Some((k.into_string().ok()?, v.into_string().ok()?)))
        .collect()
}

/// A key or a token: its `Debug` and `Display` print `<set>`, so a log
/// line, a panic or `bsv-wallet config` never carries it.
#[derive(Clone, PartialEq, Eq)]
pub struct Secret(String);

impl Secret {
    /// A secret holding `value`.
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    /// The value itself, for the one place that sends it.
    #[must_use]
    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for Secret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("<set>")
    }
}

impl fmt::Display for Secret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("<set>")
    }
}

/// A secret as the knob list shows it, from its variable as set and the
/// secret read from it: `<set>` when it holds a value, `empty` when the
/// variable is set but holds none in effect (`AUTH_TOKEN=` is no token at
/// the bind guard), `-` when it is unset.
pub(crate) fn shown_secret(set: Option<&str>, secret: Option<&Secret>) -> String {
    match (set, secret) {
        (_, Some(s)) if !s.expose().is_empty() => s.to_string(),
        (Some(_), _) => "empty".to_string(),
        (None, _) => "-".to_string(),
    }
}

/// Where a setting's effective value came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Source {
    /// The process environment (the variable is set, even to empty).
    Env,
    /// The command line.
    Flag,
    /// Neither: the built-in default.
    Default,
}

impl fmt::Display for Source {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Env => "env",
            Self::Flag => "flag",
            Self::Default => "default",
        })
    }
}

/// A URL as it may be shown: its userinfo and its query (or fragment) are
/// `<set>`, its scheme, host, port and path as set. Read as text, so a
/// value that does not parse is shown the same way (a credential in a URL
/// is never printed, #80).
#[must_use]
pub fn shown_url(value: &str) -> String {
    let (scheme, rest) = value
        .split_once("://")
        .map_or(("", value), |(scheme, rest)| (scheme, rest));
    let (authority, tail) = rest.split_at(rest.find(['/', '?', '#']).unwrap_or(rest.len()));
    let mut shown = String::new();
    if !scheme.is_empty() {
        shown.push_str(scheme);
        shown.push_str("://");
    }
    match authority.rfind('@') {
        Some(at) => {
            shown.push_str("<set>@");
            shown.push_str(&authority[at + 1..]);
        }
        None => shown.push_str(authority),
    }
    match tail.find(['?', '#']) {
        Some(q) => {
            shown.push_str(&tail[..q]);
            shown.push_str(&tail[q..=q]);
            shown.push_str("<set>");
        }
        None => shown.push_str(tail),
    }
    shown
}

/// A setting refused at start: the variable, the value it holds (a URL as
/// [`shown_url`] shows it), and the shape it must have. Never built for a
/// [`Secret`], whose value is not parsed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfigError {
    /// The variable's name.
    pub variable: &'static str,
    /// The value it holds (a URL's credentials `<set>`).
    pub value: String,
    /// The shape it must have.
    pub expected: &'static str,
}

impl fmt::Display for ConfigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{} is {:?}, which is not {}: set it so, or unset it for the default",
            self.variable, self.value, self.expected
        )
    }
}

impl std::error::Error for ConfigError {}

/// The command line's part: the global flags and which were given.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Flags {
    /// `--db`, the wallet's database.
    pub db: String,
    /// `--testnet`.
    pub testnet: bool,
    /// `--port`, the served port.
    pub port: u16,
    /// The flags given on the command line (`--db`, `--testnet`, `--port`);
    /// any other was its default.
    pub given: Vec<&'static str>,
}

impl Default for Flags {
    fn default() -> Self {
        Self {
            db: "wallet.db".to_string(),
            testnet: false,
            port: 3322,
            given: Vec::new(),
        }
    }
}

/// The environment as it was read at start, by name: what each group
/// parses, what [`Source`] and the fleet's uniformity check read. Its
/// `Debug` prints how many variables it holds, never a name or a value.
#[derive(Clone, Default)]
pub(crate) struct Raw(BTreeMap<String, String>);

impl fmt::Debug for Raw {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Raw({} variables)", self.0.len())
    }
}

impl Raw {
    fn read(vars: impl IntoIterator<Item = (String, String)>) -> Self {
        Self(vars.into_iter().collect())
    }

    /// The value as set, when it is set.
    pub(crate) fn get(&self, name: &str) -> Option<&str> {
        self.0.get(name).map(String::as_str)
    }

    /// The value when it is set and not blank, trimmed.
    fn given(&self, name: &str) -> Option<&str> {
        self.get(name).map(str::trim).filter(|v| !v.is_empty())
    }

    /// The value as set, when it is set and not empty (blank is kept).
    fn non_empty(&self, name: &str) -> Option<&str> {
        self.get(name).filter(|v| !v.is_empty())
    }

    fn secret(&self, name: &str) -> Option<Secret> {
        self.non_empty(name).map(Secret::new)
    }

    /// A switch: `on(value)` when set, `unset` when not.
    fn switch(&self, name: &str, unset: bool, on: fn(&str) -> bool) -> bool {
        self.get(name).map_or(unset, on)
    }

    /// A number: unset or blank is `None`, anything else must parse.
    fn number<T: FromStr>(
        &self,
        name: &'static str,
        expected: &'static str,
    ) -> Result<Option<T>, ConfigError> {
        self.given(name)
            .map(|v| v.parse().map_err(|_| refused(name, v, expected)))
            .transpose()
    }

    /// A URL: unset or blank is `None`, anything else must be an http or
    /// https URL with a host. The value is kept as set, trimmed.
    pub(crate) fn url(&self, name: &'static str) -> Result<Option<String>, ConfigError> {
        self.given(name).map(|v| checked_url(name, v)).transpose()
    }

    /// A URL whose empty value means something (off): unset is `None`, set
    /// is the value trimmed, which must be a URL unless it is blank.
    pub(crate) fn present_url(&self, name: &'static str) -> Result<Option<String>, ConfigError> {
        match self.get(name).map(str::trim) {
            Some(v) if !v.is_empty() => checked_url(name, v).map(Some),
            other => Ok(other.map(str::to_string)),
        }
    }
}

fn refused(variable: &'static str, value: &str, expected: &'static str) -> ConfigError {
    ConfigError {
        variable,
        value: value.to_string(),
        expected,
    }
}

fn checked_url(name: &'static str, value: &str) -> Result<String, ConfigError> {
    match url::Url::parse(value) {
        Ok(u) if matches!(u.scheme(), "http" | "https") && u.has_host() => Ok(value.to_string()),
        _ => Err(refused(name, &shown_url(value), "an http or https URL")),
    }
}

/// `1`, `true` or `yes`, any case, trimmed.
fn yes(v: &str) -> bool {
    matches!(v.trim().to_ascii_lowercase().as_str(), "1" | "true" | "yes")
}

/// Anything but `0` or `false` (any case), untrimmed.
fn not_zero_or_false(v: &str) -> bool {
    v != "0" && !v.eq_ignore_ascii_case("false")
}

/// The wallet's own settings: the global flags and the root key.
#[derive(Debug, Clone)]
pub struct WalletSettings {
    /// `--db`, default `wallet.db`.
    pub db: String,
    /// `--testnet`: the test chain, else the main chain.
    pub chain: Chain,
    /// `--port`, default 3322.
    pub port: u16,
    /// `ROOT_KEY`, as set (its hex is parsed where a wallet opens, so `init
    /// --force` can replace a damaged one).
    pub root_key: Option<Secret>,
    /// `RUST_LOG`, as set: the log filter (`tracing`'s directives), else
    /// [`DEFAULT_LOG_FILTER`]; one that does not parse is the default too,
    /// as `tracing_subscriber` always read it. `--verbose` overrides it.
    pub rust_log: Option<String>,
}

impl WalletSettings {
    fn read(flags: &Flags, raw: &Raw) -> Self {
        Self {
            db: flags.db.clone(),
            chain: if flags.testnet {
                Chain::Test
            } else {
                Chain::Main
            },
            port: flags.port,
            root_key: raw.get("ROOT_KEY").map(Secret::new),
            rust_log: raw.get("RUST_LOG").map(str::to_string),
        }
    }

    /// The root key's hex, or the refusal every wallet command gives
    /// without one.
    ///
    /// # Errors
    ///
    /// `ROOT_KEY` is not set.
    pub fn root_key(&self) -> anyhow::Result<&str> {
        self.root_key
            .as_ref()
            .map(Secret::expose)
            .ok_or(std::env::VarError::NotPresent)
            .context("ROOT_KEY not set. Run `bsv-wallet init` first.")
    }
}

/// The services: the header service, the broadcasters and the explorers
/// (see `services_env`, which builds the toolbox's options from them).
#[derive(Debug, Clone)]
pub struct ServicesSettings {
    /// `CHAINTRACKS_URL`, trimmed: a URL, or `off` (any case); unset or
    /// blank is refused where services are built.
    pub chaintracks_url: Option<String>,
    /// `BREAK_GLASS_EXPLORER_HEADERS`: `1`, `true` or `yes`; else off.
    pub break_glass_explorer_headers: bool,
    /// `ARC_URL`: the broadcaster (the Arcade endpoint in Arcade mode).
    pub arc_url: Option<String>,
    /// `ARC_MODE=arcade` (any case).
    pub arc_mode_arcade: bool,
    /// `ARCADE`: `1`, `true` or `yes`.
    pub arcade_flag: bool,
    /// `CALLBACK_TOKEN`, when not empty.
    pub callback_token: Option<Secret>,
    /// `PUBLIC_CALLBACK_URL`: the webhook Arcade posts to.
    pub public_callback_url: Option<String>,
    /// `TAAL_API_KEY`, when not empty (`Authorization: Bearer <key>`).
    pub taal_api_key: Option<Secret>,
    /// `MAIN_TAAL_API_KEY`, when not empty (raw `Authorization: <key>`).
    pub main_taal_api_key: Option<Secret>,
    /// The explorers' keys and bases, read by `services_env`
    /// ([`crate::services_env::ExplorerSettings`]).
    pub explorers: crate::services_env::ExplorerSettings,
    /// `ARC_GORILLAPOOL_URL`, trimmed: unset the toolbox's default, empty
    /// leaves `GorillaPool` out.
    pub arc_gorillapool_url: Option<String>,
}

impl ServicesSettings {
    fn read(raw: &Raw, chain: Chain) -> Result<Self, ConfigError> {
        let chaintracks_url = match raw.given("CHAINTRACKS_URL") {
            Some(v) if v.eq_ignore_ascii_case("off") => Some(v.to_string()),
            Some(v) => Some(checked_url("CHAINTRACKS_URL", v)?),
            None => None,
        };
        Ok(Self {
            chaintracks_url,
            break_glass_explorer_headers: raw.switch("BREAK_GLASS_EXPLORER_HEADERS", false, |v| {
                crate::services_env::break_glass_explorer_headers(Some(v))
            }),
            arc_url: raw.url("ARC_URL")?,
            arc_mode_arcade: raw.switch("ARC_MODE", false, |v| v.eq_ignore_ascii_case("arcade")),
            arcade_flag: raw.switch("ARCADE", false, yes),
            callback_token: raw.secret("CALLBACK_TOKEN"),
            public_callback_url: raw.url("PUBLIC_CALLBACK_URL")?,
            taal_api_key: raw.secret("TAAL_API_KEY"),
            main_taal_api_key: raw.secret("MAIN_TAAL_API_KEY"),
            explorers: crate::services_env::ExplorerSettings::read(raw, chain)?,
            arc_gorillapool_url: raw.present_url("ARC_GORILLAPOOL_URL")?,
        })
    }

    /// Arcade V2 mode (`ARC_MODE=arcade` or `ARCADE=1`).
    #[must_use]
    pub fn arcade(&self) -> bool {
        self.arc_mode_arcade || self.arcade_flag
    }

    /// The header service: `Some(url)`, or `None` for `off`.
    ///
    /// # Errors
    ///
    /// `CHAINTRACKS_URL` is unset or blank: there is no default.
    pub fn header_service(&self) -> anyhow::Result<Option<String>> {
        crate::services_env::chaintracks_url_for(self.chaintracks_url.as_deref())
    }
}

/// The host's parameters of the spend gate and the tracker (the stack's
/// wallet charter, section 6: named once).
#[derive(Debug, Clone)]
pub struct HostSettings {
    /// `SPEND_DEPTH`, default 1; below 1 reads as 1 (R3 (b)).
    pub spend_depth: Height,
    /// `SPEND_OWN_CHANGE`: `0`, `false`, `no` or `off` (any case) turn it
    /// off; anything else, or unset, leaves it on (R1 (b)).
    pub spend_own_change: bool,
    /// `TRACKER_AGE_SECS`, default 600, at least 1.
    pub tracker_age_secs: Timestamp,
    /// `TRACKER_MAX_ASKS`, default 20.
    pub tracker_max_asks: usize,
}

impl HostSettings {
    fn read(raw: &Raw) -> Result<Self, ConfigError> {
        use crate::tracker_host::{DEFAULT_AGE_SECS, DEFAULT_MAX_ASKS, DEFAULT_SPEND_DEPTH};
        let off = |v: &str| {
            !matches!(
                v.trim().to_ascii_lowercase().as_str(),
                "0" | "false" | "no" | "off"
            )
        };
        Ok(Self {
            spend_depth: raw
                .number("SPEND_DEPTH", "a block depth (a whole number)")?
                .unwrap_or(DEFAULT_SPEND_DEPTH)
                .max(DEFAULT_SPEND_DEPTH),
            spend_own_change: raw.switch(
                "SPEND_OWN_CHANGE",
                crate::spend_gate::DEFAULT_SPEND_OWN_CHANGE,
                off,
            ),
            tracker_age_secs: raw
                .number("TRACKER_AGE_SECS", "a number of seconds")?
                .unwrap_or(DEFAULT_AGE_SECS)
                .max(1),
            tracker_max_asks: raw
                .number("TRACKER_MAX_ASKS", "a whole number")?
                .unwrap_or(DEFAULT_MAX_ASKS),
        })
    }
}

/// The served reconcile loop's settings.
#[derive(Debug, Clone)]
pub struct ReconcileSettings {
    /// `BROADCAST_RECONCILE`: `0` or `false` (any case) turn the served
    /// loop off; on otherwise.
    pub enabled: bool,
    /// `BROADCAST_RECONCILE_INTERVAL_SECS`, default 60, at least 5.
    pub interval_secs: u64,
    /// `BROADCAST_ABSENCE_MINUTES`, default 30.
    pub absence_minutes: i64,
}

impl ReconcileSettings {
    fn read(raw: &Raw) -> Result<Self, ConfigError> {
        use crate::broadcast_reconcile::{DEFAULT_ABSENCE_MINUTES, DEFAULT_INTERVAL_SECS};
        Ok(Self {
            enabled: raw.switch("BROADCAST_RECONCILE", true, not_zero_or_false),
            interval_secs: raw
                .number("BROADCAST_RECONCILE_INTERVAL_SECS", "a number of seconds")?
                .unwrap_or(DEFAULT_INTERVAL_SECS)
                .max(5),
            absence_minutes: raw
                .number("BROADCAST_ABSENCE_MINUTES", "a number of minutes")?
                .unwrap_or(DEFAULT_ABSENCE_MINUTES),
        })
    }
}

/// The post-broadcast verifier's settings.
#[derive(Debug, Clone)]
pub struct VerifySettings {
    /// `BSV_WALLET_SKIP_BROADCAST_VERIFY`: `1`, `true`, `yes` or `on`.
    pub skip: bool,
    /// `BSV_WALLET_BROADCAST_VERIFY_ATTEMPTS`, default 14; 0 is the default.
    pub attempts: u32,
    /// `BSV_WALLET_BROADCAST_VERIFY_DELAY_MS`, default 2500.
    pub delay_ms: u64,
}

impl VerifySettings {
    fn read(raw: &Raw) -> Result<Self, ConfigError> {
        use crate::broadcast_verify::{DEFAULT_ATTEMPTS, DEFAULT_DELAY_MS};
        let truthy = |v: &str| {
            matches!(
                v.trim().to_ascii_lowercase().as_str(),
                "1" | "true" | "yes" | "on"
            )
        };
        Ok(Self {
            skip: raw.switch("BSV_WALLET_SKIP_BROADCAST_VERIFY", false, truthy),
            attempts: raw
                .number("BSV_WALLET_BROADCAST_VERIFY_ATTEMPTS", "a whole number")?
                .filter(|n| *n > 0)
                .unwrap_or(DEFAULT_ATTEMPTS),
            delay_ms: raw
                .number(
                    "BSV_WALLET_BROADCAST_VERIFY_DELAY_MS",
                    "a number of milliseconds",
                )?
                .unwrap_or(DEFAULT_DELAY_MS),
        })
    }
}

/// The HTTP server's settings.
#[derive(Debug, Clone)]
pub struct ServerSettings {
    /// `BIND_ADDR`, default `127.0.0.1`.
    pub bind_addr: IpAddr,
    /// `AUTH_TOKEN`, as set (an empty one counts as none at the bind
    /// guard).
    pub auth_token: Option<Secret>,
    /// `TLS_CERT_PATH`.
    pub tls_cert_path: Option<String>,
    /// `TLS_KEY_PATH`.
    pub tls_key_path: Option<String>,
}

impl ServerSettings {
    fn read(raw: &Raw) -> Result<Self, ConfigError> {
        Ok(Self {
            bind_addr: raw
                .number("BIND_ADDR", "an IP address")?
                .unwrap_or(IpAddr::from([127, 0, 0, 1])),
            auth_token: raw.get("AUTH_TOKEN").map(Secret::new),
            tls_cert_path: raw.get("TLS_CERT_PATH").map(str::to_string),
            tls_key_path: raw.get("TLS_KEY_PATH").map(str::to_string),
        })
    }
}

/// The daemon's own settings (`daemon`, `serve-fleet --daemon`).
#[derive(Debug, Clone)]
pub struct DaemonSettings {
    /// `MONITOR_CHECK_PROOFS_INTERVAL_SECS`: the monitor's proof check
    /// period; unset or 0 keeps the toolbox's.
    pub check_proofs_interval_secs: Option<u64>,
    /// `MIN_UTXOS`, default 3: below it the daemon warns.
    pub min_utxos: usize,
    /// `RECONCILE_ABANDONED`: `0` or `false` (any case) turn the abandoned
    /// reconcile off; on otherwise.
    pub reconcile_abandoned: bool,
    /// `RECONCILE_ABANDONED_MIN_AGE_SECS`, default 3600.
    pub reconcile_abandoned_min_age_secs: i64,
    /// `RELAY_URL`, when not empty: a `bsv-wallet-relay` to drain.
    pub relay_url: Option<String>,
    /// `RELAY_POLL_SECS`, default 10.
    pub relay_poll_secs: u64,
}

impl DaemonSettings {
    fn read(raw: &Raw) -> Result<Self, ConfigError> {
        let seconds = "a number of seconds";
        Ok(Self {
            check_proofs_interval_secs: raw
                .number("MONITOR_CHECK_PROOFS_INTERVAL_SECS", seconds)?
                .filter(|s| *s > 0),
            min_utxos: raw.number("MIN_UTXOS", "a whole number")?.unwrap_or(3),
            reconcile_abandoned: raw.switch("RECONCILE_ABANDONED", true, not_zero_or_false),
            reconcile_abandoned_min_age_secs: raw
                .number("RECONCILE_ABANDONED_MIN_AGE_SECS", seconds)?
                .unwrap_or(3600),
            relay_url: raw.url("RELAY_URL")?,
            relay_poll_secs: raw.number("RELAY_POLL_SECS", seconds)?.unwrap_or(10),
        })
    }
}

/// The configuration of `bsv-wallet`, read once at start.
#[derive(Debug, Clone)]
pub struct Config {
    /// The flags and the root key.
    pub wallet: WalletSettings,
    /// The header service, the broadcasters and the explorers.
    pub services: ServicesSettings,
    /// The spend gate's and the tracker's parameters.
    pub host: HostSettings,
    /// The served reconcile loop.
    pub reconcile: ReconcileSettings,
    /// The post-broadcast verifier.
    pub verify: VerifySettings,
    /// The HTTP server.
    pub server: ServerSettings,
    /// The daemon.
    pub daemon: DaemonSettings,
    given: Vec<&'static str>,
    raw: Raw,
}

impl Config {
    /// The configuration from the environment and `flags`.
    ///
    /// # Errors
    ///
    /// A variable whose value does not have its shape ([`ConfigError`]).
    pub fn load(flags: &Flags) -> Result<Self, ConfigError> {
        Self::from_vars(flags, environment())
    }

    /// The configuration from `vars` (the variables, by name and value) and
    /// `flags`: [`Config::load`] without the process environment.
    ///
    /// # Errors
    ///
    /// As [`Config::load`].
    pub fn from_vars(
        flags: &Flags,
        vars: impl IntoIterator<Item = (String, String)>,
    ) -> Result<Self, ConfigError> {
        let raw = Raw::read(vars);
        let wallet = WalletSettings::read(flags, &raw);
        Ok(Self {
            services: ServicesSettings::read(&raw, wallet.chain)?,
            wallet,
            host: HostSettings::read(&raw)?,
            reconcile: ReconcileSettings::read(&raw)?,
            verify: VerifySettings::read(&raw)?,
            server: ServerSettings::read(&raw)?,
            daemon: DaemonSettings::read(&raw)?,
            given: flags.given.clone(),
            raw,
        })
    }

    /// The configuration with the default flags and these variables set,
    /// every other unset (a test's, or a tool's).
    ///
    /// # Errors
    ///
    /// As [`Config::load`].
    pub fn from_pairs(pairs: &[(&str, &str)]) -> Result<Self, ConfigError> {
        let vars = pairs
            .iter()
            .map(|(k, v)| ((*k).to_string(), (*v).to_string()));
        Self::from_vars(&Flags::default(), vars)
    }

    /// The chain the flags name.
    #[must_use]
    pub fn chain(&self) -> Chain {
        self.wallet.chain
    }

    /// A variable's value as it was read at start, or `None` when it was
    /// unset (`serve-fleet` compares a seat's `.env` against it).
    #[must_use]
    pub fn raw(&self, name: &str) -> Option<&str> {
        self.raw.get(name)
    }

    /// Where the setting `name` (a variable, or `--db`, `--testnet`,
    /// `--port`) came from.
    #[must_use]
    pub fn source(&self, name: &str) -> Source {
        if self.given.contains(&name) {
            Source::Flag
        } else if FLAG_NAMES.contains(&name) || self.raw.get(name).is_none() {
            Source::Default
        } else {
            Source::Env
        }
    }
}

impl Default for Config {
    /// The configuration of an empty environment and the default flags.
    fn default() -> Self {
        match Self::from_vars(&Flags::default(), []) {
            Ok(config) => config,
            Err(e) => unreachable!("an empty environment is refused: {e}"),
        }
    }
}

/// The settings of `bsv-wallet-relay`, read once at its start.
#[derive(Debug, Clone)]
pub struct RelaySettings {
    /// `RELAY_DB`, default `relay.db`.
    pub db: String,
    /// `RELAY_PORT`, default 3390.
    pub port: u16,
    /// `BIND_ADDR`, default `0.0.0.0` (the relay is the public receiver).
    pub bind_addr: IpAddr,
    /// `RELAY_ADMIN_TOKEN`, when not empty: enables `POST /register`.
    pub admin_token: Option<Secret>,
    /// `RELAY_REQUIRE_REGISTER`: `1` or `true` (any case) turn
    /// auto-registration off.
    pub require_register: bool,
    /// `RUST_LOG`, as set: the log filter, else `info`.
    pub rust_log: Option<String>,
}

impl RelaySettings {
    /// The relay's settings from the environment.
    ///
    /// # Errors
    ///
    /// A variable whose value does not have its shape ([`ConfigError`]).
    pub fn load() -> Result<Self, ConfigError> {
        Self::from_vars(environment())
    }

    /// [`RelaySettings::load`] from `vars` (by name and value).
    ///
    /// # Errors
    ///
    /// As [`RelaySettings::load`].
    pub fn from_vars(
        vars: impl IntoIterator<Item = (String, String)>,
    ) -> Result<Self, ConfigError> {
        let raw = Raw::read(vars);
        Ok(Self {
            db: raw
                .get("RELAY_DB")
                .map_or_else(|| "relay.db".to_string(), str::to_string),
            port: raw.number("RELAY_PORT", "a port")?.unwrap_or(3390),
            bind_addr: raw
                .number("BIND_ADDR", "an IP address")?
                .unwrap_or(IpAddr::from([0, 0, 0, 0])),
            admin_token: raw.secret("RELAY_ADMIN_TOKEN"),
            require_register: raw.switch("RELAY_REQUIRE_REGISTER", false, |v| {
                v == "1" || v.eq_ignore_ascii_case("true")
            }),
            rust_log: raw.get("RUST_LOG").map(str::to_string),
        })
    }
}
