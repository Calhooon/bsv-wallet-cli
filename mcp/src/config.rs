//! The configuration of `bsv-wallet-mcp`: its five variables read once at
//! start, typed and checked (#171), in the shape of the main crate's
//! `src/config.rs` (#80).
//!
//! [`Config::load`] reads the environment once, in `main`, before the
//! server starts; everything after reads the [`Config`] it was given. This
//! file is the crate's one reader of the environment (the architecture
//! rule A7, `tests/arch_config_env.rs`, scans `mcp/src` too).
//!
//! A malformed setting is a typed [`ConfigError`], and every one is
//! refused together at start by name. `bsv-wallet-mcp --config` prints
//! every setting with its value and its [`Source`] ([`Config::table`]),
//! the refused rows marked, and exits 1 when one is refused. The relay's
//! admin token is a [`Secret`]: it prints `<set>`, or `empty` when the
//! variable is set to nothing; a URL's userinfo and query print `<set>`.

use std::fmt;

use reqwest::header::HeaderValue;
use reqwest::Url;
use tracing_subscriber::filter::LevelFilter;
use tracing_subscriber::EnvFilter;

/// `WALLET_URL` when unset: the wallet's default port.
pub const DEFAULT_WALLET_URL: &str = "http://localhost:3322";

/// `WALLET_ORIGIN` when unset: the `Origin` header every request carries.
pub const DEFAULT_WALLET_ORIGIN: &str = "http://localhost";

#[path = "../../src/config/words.rs"]
mod words;

use words::Raw;
pub use words::{shown_url, ConfigError, Secret};

/// The directive the log filter always holds, after `RUST_LOG`'s.
pub const LOG_DIRECTIVE: &str = "bsv_wallet_mcp=info";

/// The variables the configuration reads, in the order `--config` prints
/// them.
pub const NAMES: &[&str] = &[
    "WALLET_URL",
    "WALLET_ORIGIN",
    "RELAY_URL",
    "RELAY_ADMIN_TOKEN",
    "RUST_LOG",
];

/// The one read of the process environment, at start: each of [`NAMES`]
/// that is set. A value that is not Unicode reads as unset, as
/// `std::env::var` always made it.
fn environment() -> Vec<(String, String)> {
    NAMES
        .iter()
        .filter_map(|&name| {
            Some((
                name.to_string(),
                std::env::var_os(name)?.into_string().ok()?,
            ))
        })
        .collect()
}

/// Whether the command line asks for `--config`: print every setting with
/// its source, and exit (1 when one is refused). Any other argument is
/// ignored, as it always was.
#[must_use]
pub fn asked() -> bool {
    std::env::args().skip(1).any(|a| a == "--config")
}

/// Where a setting's effective value came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    /// The process environment (the variable is set, even to empty).
    Env,
    /// Not set: the built-in default.
    Default,
}

impl fmt::Display for Source {
    /// `env` or `default`, as `bsv-wallet config` prints a source.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(if *self == Self::Env { "env" } else { "default" })
    }
}

/// [`Config::load`]'s refusal: every setting refused, and the configuration
/// read with each of them at its default, for `--config`, which prints it
/// with the refused rows marked.
#[derive(Debug, Clone)]
pub struct Refused {
    /// The settings refused, in the order they are read.
    pub errors: Vec<ConfigError>,
    /// The configuration, each refused setting at its default.
    pub config: Box<Config>,
}

/// The configuration of `bsv-wallet-mcp`, read once at start.
#[derive(Debug, Clone)]
pub struct Config {
    /// `WALLET_URL`: the wallet's HTTP server, an http or https URL, as set
    /// (default `http://localhost:3322`).
    pub wallet_url: String,
    /// `WALLET_ORIGIN`: the `Origin` header every request carries, not
    /// blank (default `http://localhost`).
    pub wallet_origin: String,
    /// `RELAY_URL`: the callback relay (`bsv-wallet-relay`), an http or
    /// https URL; unset or empty is no relay.
    pub relay_url: Option<String>,
    /// `RELAY_ADMIN_TOKEN`: the relay's admin bearer for `register`, when
    /// set and not empty.
    pub relay_admin_token: Option<Secret>,
    /// `RUST_LOG`: the log filter's directives, when set and not empty
    /// (`error` otherwise); [`LOG_DIRECTIVE`] is added after them.
    pub rust_log: Option<String>,
    raw: Raw,
    refused: Vec<ConfigError>,
}

impl Config {
    /// The configuration from the environment: every malformed setting,
    /// together, as [`Refused`].
    pub fn load() -> Result<Self, Refused> {
        Self::from_vars(environment())
    }

    /// [`Config::load`] from `vars` (the variables, by name and value).
    pub fn from_vars(vars: impl IntoIterator<Item = (String, String)>) -> Result<Self, Refused> {
        let raw = Raw::read(
            vars.into_iter()
                .filter(|(k, _)| NAMES.contains(&k.as_str())),
        );
        let mut refused = Vec::new();
        let get = |name: &str| raw.get(name);
        let mut kept = |r: Result<Option<String>, ConfigError>| {
            r.unwrap_or_else(|e| {
                refused.push(e);
                None
            })
        };
        let wallet_url = kept(get("WALLET_URL").map(|v| url("WALLET_URL", v)).transpose());
        let wallet_origin = kept(
            get("WALLET_ORIGIN")
                .map(|v| origin("WALLET_ORIGIN", v))
                .transpose(),
        );
        let relay_url = kept(
            get("RELAY_URL")
                .filter(|v| !v.is_empty())
                .map(|v| url("RELAY_URL", v))
                .transpose(),
        );
        let config = Self {
            wallet_url: wallet_url.unwrap_or_else(|| DEFAULT_WALLET_URL.to_string()),
            wallet_origin: wallet_origin.unwrap_or_else(|| DEFAULT_WALLET_ORIGIN.to_string()),
            relay_url,
            relay_admin_token: get("RELAY_ADMIN_TOKEN")
                .filter(|v| !v.is_empty())
                .map(Secret::new),
            rust_log: get("RUST_LOG")
                .filter(|v| !v.is_empty())
                .map(str::to_string),
            raw,
            refused,
        };
        if config.refused.is_empty() {
            Ok(config)
        } else {
            Err(Refused {
                errors: config.refused.clone(),
                config: Box::new(config),
            })
        }
    }

    /// The settings refused at start (empty for a configuration
    /// [`Config::load`] answered).
    #[must_use]
    pub fn refused(&self) -> &[ConfigError] {
        &self.refused
    }

    /// Where the setting `name` came from.
    #[must_use]
    pub fn source(&self, name: &str) -> Source {
        if self.raw.get(name).is_some() {
            Source::Env
        } else {
            Source::Default
        }
    }

    /// The log filter: `RUST_LOG`'s directives (those that parse), `error`
    /// when it is unset or holds none, then [`LOG_DIRECTIVE`].
    #[must_use]
    pub fn log_filter(&self) -> EnvFilter {
        let filter = EnvFilter::builder()
            .with_default_directive(LevelFilter::ERROR.into())
            .parse_lossy(self.rust_log.as_deref().unwrap_or_default());
        match LOG_DIRECTIVE.parse() {
            Ok(directive) => filter.add_directive(directive),
            Err(e) => unreachable!("{LOG_DIRECTIVE} is a directive: {e}"),
        }
    }

    /// Every setting as `(name, value, source, refused)`: the value in
    /// effect (a URL as [`shown_url`] shows it, the token `<set>`, `empty`
    /// or `-`), or the value as set when it was refused, with the shape it
    /// must have.
    #[must_use]
    pub fn knobs(
        &self,
    ) -> Vec<(
        &'static str,
        String,
        Source,
        Option<std::borrow::Cow<'static, str>>,
    )> {
        let none = || "-".to_string();
        NAMES
            .iter()
            .map(|&name| {
                let refused = self.refused.iter().find(|e| e.variable == name);
                let value = match (refused, name) {
                    (Some(e), _) => e.value.clone(),
                    (None, "WALLET_URL") => shown_url(&self.wallet_url),
                    (None, "WALLET_ORIGIN") => self.wallet_origin.clone(),
                    (None, "RELAY_URL") => self.relay_url.as_deref().map_or_else(none, shown_url),
                    (None, "RELAY_ADMIN_TOKEN") => {
                        match (self.raw.get(name), &self.relay_admin_token) {
                            (_, Some(token)) => token.to_string(),
                            (Some(_), None) => "empty".to_string(),
                            (None, None) => none(),
                        }
                    }
                    (None, _) => format!(
                        "{},{LOG_DIRECTIVE}",
                        self.rust_log.as_deref().unwrap_or("error")
                    ),
                };
                (
                    name,
                    value,
                    self.source(name),
                    refused.map(|e| e.expected.clone()),
                )
            })
            .collect()
    }

    /// The knobs as three aligned columns under a header, as
    /// `bsv-wallet config` prints its own.
    #[must_use]
    pub fn table(&self) -> String {
        let knobs = self.knobs();
        let name_width = knobs.iter().map(|k| k.0.len()).max().unwrap_or(0).max(4);
        let value_width = knobs.iter().map(|k| k.1.len()).max().unwrap_or(0).max(5);
        let header = format!(
            "{:<name_width$}  {:<value_width$}  SOURCE\n",
            "NAME", "VALUE"
        );
        let rows = knobs.into_iter().map(|(name, value, source, refused)| {
            let refused = refused.map_or_else(String::new, |e| format!("  REFUSED: not {e}"));
            format!("{name:<name_width$}  {value:<value_width$}  {source}{refused}\n")
        });
        std::iter::once(header).chain(rows).collect()
    }
}

/// An http or https URL with a host, kept as set.
fn url(name: &'static str, value: &str) -> Result<String, ConfigError> {
    let web = |u: Url| u.has_host() && ["http", "https"].contains(&u.scheme());
    if Url::parse(value).is_ok_and(web) {
        Ok(value.to_string())
    } else {
        Err(ConfigError {
            variable: name,
            value: shown_url(value),
            expected: "an http or https URL".into(),
        })
    }
}

/// An `Origin` header's value: not blank, and a value a header may hold.
fn origin(name: &'static str, value: &str) -> Result<String, ConfigError> {
    if !value.trim().is_empty() && HeaderValue::from_str(value).is_ok() {
        Ok(value.to_string())
    } else {
        Err(ConfigError {
            variable: name,
            value: value.to_string(),
            expected: "an Origin header's value".into(),
        })
    }
}
