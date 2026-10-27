//! The configuration as a list: every setting with its effective value and
//! its source, for `bsv-wallet config` and the start line of `daemon` and
//! `serve`. A secret's value is `<set>`, or `empty` when its variable is set
//! to nothing in effect; a setting with no value is `-` (`unset`, `off` or
//! `empty` where that is its word), `null` in the JSON; a switch is `on` or
//! `off`, a boolean in the JSON; a URL's userinfo and query are `<set>`
//! ([`shown_url`]). `CHAINTRACKS_URL=off` is a value, the header service set
//! off, the string `off` in the JSON; unset or blank it is `null`.

use std::fmt;

use bsv_wallet_toolbox::{services::ARCADE_V2_MAINNET, Chain, ServicesOptions};
use serde::{Serialize, Serializer};

use super::{shown_url, ArcMode, Config, HeaderService, Secret, Source};

/// A setting's effective value, as the table shows it ([`fmt::Display`])
/// and as the JSON carries it ([`Serialize`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Value {
    /// No value in effect: `null` in the JSON, the table's word for it
    /// (`-`, `unset`, `off`, `empty`).
    None(&'static str),
    /// A switch: a boolean in the JSON, `on` or `off` in the table.
    Switch(bool),
    /// A value (a secret's `<set>`, a URL as [`shown_url`] shows it).
    Text(String),
}

impl fmt::Display for Value {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.pad(match self {
            Self::None(word) => word,
            Self::Switch(on) => {
                if *on {
                    "on"
                } else {
                    "off"
                }
            }
            Self::Text(text) => text,
        })
    }
}

impl Serialize for Value {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self {
            Self::None(_) => serializer.serialize_none(),
            Self::Switch(on) => serializer.serialize_bool(*on),
            Self::Text(text) => serializer.serialize_str(text),
        }
    }
}

impl From<String> for Value {
    fn from(text: String) -> Self {
        Self::Text(text)
    }
}

/// One setting: its name (a variable, or a flag), its effective value and
/// where it came from.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Knob {
    /// The variable's name, or the flag (`--db`).
    pub name: &'static str,
    /// The effective value; `<set>` for a secret that is set, `empty` (no
    /// value) for one set to nothing.
    pub value: Value,
    /// Where the value came from.
    pub source: Source,
    /// The shape the value must have, when it was refused at start (the
    /// value is then as set, and the setting reads as its default).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub refused: Option<&'static str>,
}

/// A secret as the knob list shows it, from its variable as set and the
/// secret read from it: `<set>` when it holds a value, `empty` when the
/// variable is set but holds none in effect (`AUTH_TOKEN=` is no token at
/// the bind guard), `-` when it is unset.
fn shown_secret(set: Option<&str>, secret: Option<&Secret>) -> Value {
    match (set, secret) {
        (_, Some(s)) if !s.expose().is_empty() => Value::Text(s.to_string()),
        (Some(_), _) => Value::None("empty"),
        (None, _) => NONE,
    }
}

/// No value, `-` in the table.
pub(crate) const NONE: Value = Value::None("-");

fn or_none(value: Option<&str>) -> Value {
    value.map_or(NONE, |v| Value::Text(v.to_string()))
}

/// A URL as [`shown_url`] shows it, or `none` when there is none.
pub(crate) fn url_or(value: Option<&str>, none: Value) -> Value {
    value.map_or(none, |v| Value::Text(shown_url(v)))
}

impl Config {
    /// Every setting: the three flags, then every variable `bsv-wallet`
    /// reads.
    #[must_use]
    pub fn knobs(&self) -> Vec<Knob> {
        let mut values = self.wallet_values();
        values.extend(self.services_values());
        values.extend(self.host_values());
        values.extend(self.server_values());
        values
            .into_iter()
            .map(|(name, value)| {
                let refused = self.refused.iter().find(|e| e.variable == name);
                Knob {
                    name,
                    value: refused.map_or(value, |e| Value::Text(e.value.clone())),
                    source: self.source(name),
                    refused: refused.map(|e| e.expected),
                }
            })
            .collect()
    }

    /// Every setting as `NAME=value` on one line, secrets `<set>`: what
    /// `daemon` and `serve` log at start. A value with a space in it is
    /// quoted (`ARC_MODE="arcade (default)"`), so each setting is one word.
    #[must_use]
    pub fn log_line(&self) -> String {
        self.knobs()
            .iter()
            .map(|k| {
                let value = k.value.to_string();
                if value.contains(char::is_whitespace) {
                    format!("{}={value:?}", k.name)
                } else {
                    format!("{}={value}", k.name)
                }
            })
            .collect::<Vec<_>>()
            .join(" ")
    }

    /// A secret's row: [`shown_secret`] of its variable as read.
    fn secret(&self, name: &str, value: Option<&Secret>) -> Value {
        shown_secret(self.raw.get(name), value)
    }

    fn wallet_values(&self) -> Vec<(&'static str, Value)> {
        let w = &self.wallet;
        vec![
            ("--db", w.db.clone().into()),
            ("--testnet", Value::Switch(w.chain == Chain::Test)),
            ("--port", w.port.to_string().into()),
            ("ROOT_KEY", self.secret("ROOT_KEY", w.root_key.as_ref())),
            (
                "E2E_RNG_SEED",
                if w.rng_seed.is_some() {
                    Value::Text("<set>".into())
                } else {
                    NONE
                },
            ),
            ("RUST_LOG", w.log_filter().0.into()),
        ]
    }

    /// The broadcaster's rows (#300): `ARC_URL` in effect (else the mode's
    /// default on the chain; a testnet wallet in Arcade mode has none,
    /// refused where services are built, `services_env::arcade_url_for`),
    /// the mode in effect (`ARC_MODE`'s, or `ARCADE`'s when it is unset),
    /// and `ARCADE` as set (blank is no value).
    fn broadcaster_values(&self) -> [(&'static str, Value); 3] {
        let s = &self.services;
        let arc_url = match (&s.arc_url, s.arc_mode, self.chain()) {
            (Some(url), _, _) => Value::Text(shown_url(url)),
            (None, ArcMode::Arcade, Chain::Main) => Value::Text(ARCADE_V2_MAINNET.to_string()),
            (None, ArcMode::Arcade, Chain::Test) => Value::None("unset"),
            (None, ArcMode::Classic, Chain::Main) => {
                Value::Text(ServicesOptions::mainnet().arc_url)
            }
            (None, ArcMode::Classic, Chain::Test) => {
                Value::Text(ServicesOptions::testnet().arc_url)
            }
        };
        let mode = match s.arc_mode {
            ArcMode::Arcade => "arcade (default)",
            ArcMode::Classic => "classic",
        };
        let arcade = match self.raw.get("ARCADE") {
            Some(v) if v.trim().is_empty() => Value::None("empty"),
            set => or_none(set),
        };
        [
            ("ARC_URL", arc_url),
            ("ARC_MODE", Value::Text(mode.to_string())),
            ("ARCADE", arcade),
        ]
    }

    fn services_values(&self) -> Vec<(&'static str, Value)> {
        let s = &self.services;
        let mut values = vec![
            (
                "CHAINTRACKS_URL",
                match &s.chaintracks_url {
                    None => Value::None("unset"),
                    Some(HeaderService::Off) => Value::Text("off".to_string()),
                    Some(HeaderService::Url(url)) => Value::Text(shown_url(url)),
                },
            ),
            (
                "BREAK_GLASS_EXPLORER_HEADERS",
                Value::Switch(s.break_glass_explorer_headers),
            ),
        ];
        values.extend(self.broadcaster_values());
        values.extend([
            (
                "CALLBACK_TOKEN",
                self.secret("CALLBACK_TOKEN", s.callback_token.as_ref()),
            ),
            (
                "PUBLIC_CALLBACK_URL",
                url_or(s.public_callback_url.as_deref(), NONE),
            ),
            (
                "TAAL_API_KEY",
                self.secret("TAAL_API_KEY", s.taal_api_key.as_ref()),
            ),
            (
                "MAIN_TAAL_API_KEY",
                self.secret("MAIN_TAAL_API_KEY", s.main_taal_api_key.as_ref()),
            ),
        ]);
        values.extend(
            s.explorers
                .values(self.chain(), |name, key| self.secret(name, key)),
        );
        values.push((
            "ARC_GORILLAPOOL_URL",
            url_or(s.explorers.bases.gorillapool.as_deref(), Value::None("off")),
        ));
        values
    }

    fn host_values(&self) -> Vec<(&'static str, Value)> {
        let (h, r, v) = (&self.host, &self.reconcile, &self.verify);
        vec![
            ("SPEND_DEPTH", h.spend_depth.to_string().into()),
            ("SPEND_OWN_CHANGE", Value::Switch(h.spend_own_change)),
            ("TRACKER_AGE_SECS", h.tracker_age_secs.to_string().into()),
            ("TRACKER_MAX_ASKS", h.tracker_max_asks.to_string().into()),
            ("BROADCAST_RECONCILE", Value::Switch(r.enabled)),
            (
                "BROADCAST_RECONCILE_INTERVAL_SECS",
                r.interval_secs.to_string().into(),
            ),
            (
                "BROADCAST_ABSENCE_MINUTES",
                r.absence_minutes.to_string().into(),
            ),
            ("BSV_WALLET_SKIP_BROADCAST_VERIFY", Value::Switch(v.skip)),
            (
                "BSV_WALLET_BROADCAST_VERIFY_ATTEMPTS",
                v.attempts.to_string().into(),
            ),
            (
                "BSV_WALLET_BROADCAST_VERIFY_DELAY_MS",
                v.delay_ms.to_string().into(),
            ),
        ]
    }

    fn server_values(&self) -> Vec<(&'static str, Value)> {
        let (s, d) = (&self.server, &self.daemon);
        vec![
            ("BIND_ADDR", s.bind_addr.to_string().into()),
            (
                "AUTH_TOKEN",
                self.secret("AUTH_TOKEN", s.auth_token.as_ref()),
            ),
            ("TLS_CERT_PATH", or_none(s.tls_cert_path.as_deref())),
            ("TLS_KEY_PATH", or_none(s.tls_key_path.as_deref())),
            (
                "MONITOR_CHECK_PROOFS_INTERVAL_SECS",
                d.check_proofs_interval_secs
                    .map_or(NONE, |s| s.to_string().into()),
            ),
            ("MIN_UTXOS", d.min_utxos.to_string().into()),
            ("RECONCILE_ABANDONED", Value::Switch(d.reconcile_abandoned)),
            (
                "RECONCILE_ABANDONED_MIN_AGE_SECS",
                d.reconcile_abandoned_min_age_secs.to_string().into(),
            ),
            ("RELAY_URL", url_or(d.relay_url.as_deref(), NONE)),
            ("RELAY_POLL_SECS", d.relay_poll_secs.to_string().into()),
        ]
    }
}
