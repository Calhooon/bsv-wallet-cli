//! The configuration as a list: every setting with its effective value and
//! its source, for `bsv-wallet config` and the start line of `daemon` and
//! `serve`. A secret's value is `<set>`, or `empty` when its variable is set
//! to nothing in effect; a setting with no value is `-`; a
//! URL's userinfo and query are `<set>` ([`shown_url`]).

use bsv_wallet_toolbox::{services::ARCADE_V2_MAINNET, Chain, ServicesOptions};
use serde::Serialize;

use super::{Config, Secret, Source};

/// One setting: its name (a variable, or a flag), its effective value and
/// where it came from.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Knob {
    /// The variable's name, or the flag (`--db`).
    pub name: &'static str,
    /// The effective value; `<set>` for a secret that is set, `empty` for
    /// one set to nothing, `-` for none.
    pub value: String,
    /// Where the value came from.
    pub source: Source,
    /// The shape the value must have, when it was refused at start (the
    /// value is then as set, and the setting reads as its default).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub refused: Option<&'static str>,
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

const NONE: &str = "-";

fn on_off(on: bool) -> String {
    if on { "on" } else { "off" }.to_string()
}

fn or_none(value: Option<&str>) -> String {
    value.unwrap_or(NONE).to_string()
}

fn url_or_none(value: Option<&str>) -> String {
    value.map_or_else(|| NONE.to_string(), shown_url)
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
                    value: refused.map_or(value, |e| e.value.clone()),
                    source: self.source(name),
                    refused: refused.map(|e| e.expected),
                }
            })
            .collect()
    }

    /// Every setting as `NAME=value` on one line, secrets `<set>`: what
    /// `daemon` and `serve` log at start.
    #[must_use]
    pub fn log_line(&self) -> String {
        self.knobs()
            .iter()
            .map(|k| format!("{}={}", k.name, k.value))
            .collect::<Vec<_>>()
            .join(" ")
    }

    /// A secret's row: [`super::shown_secret`] of its variable as read.
    fn secret(&self, name: &str, value: Option<&Secret>) -> String {
        shown_secret(self.raw.get(name), value)
    }

    fn wallet_values(&self) -> Vec<(&'static str, String)> {
        let w = &self.wallet;
        vec![
            ("--db", w.db.clone()),
            ("--testnet", on_off(w.chain == Chain::Test)),
            ("--port", w.port.to_string()),
            ("ROOT_KEY", self.secret("ROOT_KEY", w.root_key.as_ref())),
            ("RUST_LOG", w.log_filter().0),
        ]
    }

    fn services_values(&self) -> Vec<(&'static str, String)> {
        let s = &self.services;
        let defaults = match self.chain() {
            Chain::Main => ServicesOptions::mainnet(),
            Chain::Test => ServicesOptions::testnet(),
        };
        let arc_url = match (&s.arc_url, s.arcade()) {
            (Some(url), _) => shown_url(url),
            (None, true) => ARCADE_V2_MAINNET.to_string(),
            (None, false) => defaults.arc_url,
        };
        let mut values = vec![
            (
                "CHAINTRACKS_URL",
                s.chaintracks_url
                    .as_deref()
                    .map_or_else(|| "unset".into(), shown_url),
            ),
            (
                "BREAK_GLASS_EXPLORER_HEADERS",
                on_off(s.break_glass_explorer_headers),
            ),
            ("ARC_URL", arc_url),
            // ARC_MODE's own value as set: the mode in effect is this or
            // ARCADE's, each on its own row.
            ("ARC_MODE", or_none(self.raw.get("ARC_MODE"))),
            ("ARCADE", on_off(s.arcade_flag)),
            (
                "CALLBACK_TOKEN",
                self.secret("CALLBACK_TOKEN", s.callback_token.as_ref()),
            ),
            (
                "PUBLIC_CALLBACK_URL",
                url_or_none(s.public_callback_url.as_deref()),
            ),
            (
                "TAAL_API_KEY",
                self.secret("TAAL_API_KEY", s.taal_api_key.as_ref()),
            ),
            (
                "MAIN_TAAL_API_KEY",
                self.secret("MAIN_TAAL_API_KEY", s.main_taal_api_key.as_ref()),
            ),
        ];
        values.extend(s.explorers.values(&self.raw));
        let gorillapool = s.explorers.bases.gorillapool.as_deref();
        values.push((
            "ARC_GORILLAPOOL_URL",
            gorillapool.map_or_else(|| "off".into(), shown_url),
        ));
        values
    }

    fn host_values(&self) -> Vec<(&'static str, String)> {
        let (h, r, v) = (&self.host, &self.reconcile, &self.verify);
        vec![
            ("SPEND_DEPTH", h.spend_depth.to_string()),
            ("SPEND_OWN_CHANGE", on_off(h.spend_own_change)),
            ("TRACKER_AGE_SECS", h.tracker_age_secs.to_string()),
            ("TRACKER_MAX_ASKS", h.tracker_max_asks.to_string()),
            ("BROADCAST_RECONCILE", on_off(r.enabled)),
            (
                "BROADCAST_RECONCILE_INTERVAL_SECS",
                r.interval_secs.to_string(),
            ),
            ("BROADCAST_ABSENCE_MINUTES", r.absence_minutes.to_string()),
            ("BSV_WALLET_SKIP_BROADCAST_VERIFY", on_off(v.skip)),
            (
                "BSV_WALLET_BROADCAST_VERIFY_ATTEMPTS",
                v.attempts.to_string(),
            ),
            (
                "BSV_WALLET_BROADCAST_VERIFY_DELAY_MS",
                v.delay_ms.to_string(),
            ),
        ]
    }

    fn server_values(&self) -> Vec<(&'static str, String)> {
        let (s, d) = (&self.server, &self.daemon);
        vec![
            ("BIND_ADDR", s.bind_addr.to_string()),
            (
                "AUTH_TOKEN",
                self.secret("AUTH_TOKEN", s.auth_token.as_ref()),
            ),
            ("TLS_CERT_PATH", or_none(s.tls_cert_path.as_deref())),
            ("TLS_KEY_PATH", or_none(s.tls_key_path.as_deref())),
            (
                "MONITOR_CHECK_PROOFS_INTERVAL_SECS",
                d.check_proofs_interval_secs
                    .map_or_else(|| NONE.into(), |s| s.to_string()),
            ),
            ("MIN_UTXOS", d.min_utxos.to_string()),
            ("RECONCILE_ABANDONED", on_off(d.reconcile_abandoned)),
            (
                "RECONCILE_ABANDONED_MIN_AGE_SECS",
                d.reconcile_abandoned_min_age_secs.to_string(),
            ),
            ("RELAY_URL", url_or_none(d.relay_url.as_deref())),
            ("RELAY_POLL_SECS", d.relay_poll_secs.to_string()),
        ]
    }
}
