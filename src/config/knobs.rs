//! The configuration as a list: every setting with its effective value and
//! its source, for `bsv-wallet config` and the start line of `daemon` and
//! `serve`. A secret's value is `<set>`; a setting with no value is `-`.

use bsv_wallet_toolbox::{services::ARCADE_V2_MAINNET, Chain, ServicesOptions};
use serde::Serialize;

use super::{Config, Secret, Source};

/// One setting: its name (a variable, or a flag), its effective value and
/// where it came from.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Knob {
    /// The variable's name, or the flag (`--db`).
    pub name: &'static str,
    /// The effective value; `<set>` for a secret that is set, `-` for none.
    pub value: String,
    /// Where the value came from.
    pub source: Source,
}

const NONE: &str = "-";

fn on_off(on: bool) -> String {
    if on { "on" } else { "off" }.to_string()
}

fn or_none(value: Option<&str>) -> String {
    value.unwrap_or(NONE).to_string()
}

fn secret(value: Option<&Secret>) -> String {
    value.map_or_else(|| NONE.to_string(), ToString::to_string)
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
            .map(|(name, value)| Knob {
                name,
                value,
                source: self.source(name),
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

    fn wallet_values(&self) -> Vec<(&'static str, String)> {
        let w = &self.wallet;
        vec![
            ("--db", w.db.clone()),
            ("--testnet", on_off(w.chain == Chain::Test)),
            ("--port", w.port.to_string()),
            ("ROOT_KEY", secret(w.root_key.as_ref())),
            (
                "RUST_LOG",
                w.rust_log
                    .clone()
                    .unwrap_or_else(|| super::DEFAULT_LOG_FILTER.into()),
            ),
        ]
    }

    fn services_values(&self) -> Vec<(&'static str, String)> {
        let s = &self.services;
        let defaults = match self.chain() {
            Chain::Main => ServicesOptions::mainnet(),
            Chain::Test => ServicesOptions::testnet(),
        };
        let arc_url = match (&s.arc_url, s.arcade()) {
            (Some(url), _) => url.clone(),
            (None, true) => ARCADE_V2_MAINNET.to_string(),
            (None, false) => defaults.arc_url,
        };
        let mut values = vec![
            (
                "CHAINTRACKS_URL",
                s.chaintracks_url.clone().unwrap_or_else(|| "unset".into()),
            ),
            (
                "BREAK_GLASS_EXPLORER_HEADERS",
                on_off(s.break_glass_explorer_headers),
            ),
            ("ARC_URL", arc_url),
            (
                "ARC_MODE",
                if s.arcade() { "arcade" } else { "classic" }.to_string(),
            ),
            ("ARCADE", on_off(s.arcade_flag)),
            ("CALLBACK_TOKEN", secret(s.callback_token.as_ref())),
            (
                "PUBLIC_CALLBACK_URL",
                or_none(s.public_callback_url.as_deref()),
            ),
            ("TAAL_API_KEY", secret(s.taal_api_key.as_ref())),
            ("MAIN_TAAL_API_KEY", secret(s.main_taal_api_key.as_ref())),
        ];
        values.extend(s.explorers.values());
        let gorillapool = s.explorers.bases.gorillapool.clone();
        values.push((
            "ARC_GORILLAPOOL_URL",
            gorillapool.unwrap_or_else(|| "off".into()),
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
            ("AUTH_TOKEN", secret(s.auth_token.as_ref())),
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
            ("RELAY_URL", or_none(d.relay_url.as_deref())),
            ("RELAY_POLL_SECS", d.relay_poll_secs.to_string()),
        ]
    }
}
