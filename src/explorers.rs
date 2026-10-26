//! The explorer rungs the commands ask themselves (bsv-wallet-cli #50).
//!
//! The owner's word of 2026-10-22: each chain scanner has its own rate
//! limits, so when one fails we go to the next. WhatsOnChain, BananaBlocks
//! and Bitails are each other's fallbacks; a negative ("unspent",
//! "absent") needs a second provider's clean answer; "could not look" is
//! never "nothing there". The toolbox's services hold that rule for the
//! questions they answer (`get_raw_tx`, `get_merkle_path`,
//! `get_utxo_status`, `get_status_for_txids`, `is_utxo_with_spender` for
//! the spend probe, and `get_utxo_leads` for `sync`'s discovery, toolbox
//! 0.13.1). This module holds it for the break-glass reads the CLI still
//! makes itself:
//!
//! - a payment's bytes for `receive`: WhatsOnChain's `/tx/{txid}/beef`, then
//!   BananaBlocks' `/tx/{txid}/hex` (bytes that hash to the txid);
//! - BananaBlocks as a chain index of the broadcast verifier.
//!
//! The spend probe's "who spent it" left this module in 0.18.2: the
//! toolbox's `is_utxo_with_spender` (0.13.1, bsv-wallet-toolbox-rs #44)
//! names the spender its own ladder bound beside the verdict, so the probe
//! (`commands::cleanup_abandoned::probe_input_spend`) asks no spender route
//! of its own and costs one ladder pass.
//!
//! No rung sleeps on a 429: it is a fault returned at once, and the rung is
//! skipped without a request until its Retry-After passes (the toolbox's
//! `RateGate`; BananaBlocks also paces itself at 60 requests a minute).
//! The process's rungs spend one budget per host with the toolbox's
//! providers: the WhatsOnChain rung passes the toolbox WhatsOnChain
//! provider's gate (`WhatsOnChain::rate_gate`, the process's for its base
//! and key) and the BananaBlocks rung is the toolbox's provider, so a 429
//! either side meets closes the rung for both until Retry-After passes.
//! Every call is logged at info with its provider, status and milliseconds
//! (marker `explorer_call`), so an audit can see which rung answered.

use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};

use bsv_wallet_toolbox::services::providers::whatsonchain::WOC_API_KEY_HEADER;
use bsv_wallet_toolbox::services::providers::{BananaBlocks, RateGate, BANANABLOCKS_NAME};
use bsv_wallet_toolbox::services::GetRawTxResult;
use bsv_wallet_toolbox::{Chain, ServicesOptions};
use bsv_wallet_toolbox::{WhatsOnChain, WhatsOnChainConfig};
use reqwest::StatusCode;

/// The name WhatsOnChain answers under in this module's results and logs.
pub const PROVIDER_WHATSONCHAIN: &str = "WhatsOnChain";

/// The name BananaBlocks answers under (the toolbox's).
pub const PROVIDER_BANANABLOCKS: &str = BANANABLOCKS_NAME;

/// How long a WhatsOnChain 429 without a Retry-After closes a private rung
/// (`Explorers::at`; the toolbox's WhatsOnChain provider uses the same
/// second for the process's).
const WOC_DEFAULT_RETRY_AFTER: Duration = Duration::from_secs(1);

/// The longest one explorer request may take.
const EXPLORER_TIMEOUT: Duration = Duration::from_secs(30);

/// WhatsOnChain's API base for a chain.
pub fn woc_base(chain: Chain) -> &'static str {
    match chain {
        Chain::Main => "https://api.whatsonchain.com/v1/bsv/main",
        Chain::Test => "https://api.whatsonchain.com/v1/bsv/test",
    }
}

/// WhatsOnChain as the CLI asks it: its base, its key, its gate.
struct WocRung {
    base: String,
    api_key: Option<String>,
    /// The process's gate for the base and key ([`shared_woc_gate`]), or a
    /// private one ([`Explorers::at`]).
    gate: Arc<RateGate>,
}

/// The toolbox WhatsOnChain provider's gate for `chain` and `api_key`: the
/// process's for that base and key (`RateGate::shared` inside
/// `WhatsOnChain::new`), the one every `Services` of the process passes, so
/// the CLI's rung and the toolbox's ladders spend one budget and a 429
/// either side meets closes both until its Retry-After passes.
fn shared_woc_gate(chain: Chain, api_key: Option<String>) -> Arc<RateGate> {
    match WhatsOnChain::new(
        chain,
        WhatsOnChainConfig {
            api_key,
            timeout_secs: None,
        },
    ) {
        Ok(provider) => Arc::clone(provider.rate_gate()),
        Err(e) => {
            tracing::warn!(error = %e, "WhatsOnChain's shared gate could not be read; the rung keeps a gate of its own");
            Arc::new(RateGate::new(
                PROVIDER_WHATSONCHAIN,
                WOC_DEFAULT_RETRY_AFTER,
            ))
        }
    }
}

/// The explorer rungs of one process (or one test).
pub struct Explorers {
    client: reqwest::Client,
    woc: Option<WocRung>,
    bananablocks: Option<Arc<BananaBlocks>>,
}

impl Explorers {
    /// The process's rungs for `chain`, built once from the environment
    /// (`WOC_API_KEY`, `BANANABLOCKS_BASE`; see `services_env`), so a
    /// Retry-After one command or pass met holds for the next.
    pub fn from_env(chain: Chain) -> Arc<Self> {
        static MAIN: OnceLock<Arc<Explorers>> = OnceLock::new();
        static TEST: OnceLock<Arc<Explorers>> = OnceLock::new();
        let cell = match chain {
            Chain::Main => &MAIN,
            Chain::Test => &TEST,
        };
        cell.get_or_init(|| {
            Arc::new(Self::configured(
                chain,
                crate::services_env::woc_api_key_from_env(),
                crate::services_env::bananablocks_base_from_env(),
            ))
        })
        .clone()
    }

    /// The rungs for `chain`: WhatsOnChain at its base, BananaBlocks at
    /// `bananablocks_base` (`None` the chain's default, empty off: the
    /// toolbox's `ServicesOptions::bananablocks_base_for`).
    pub fn configured(
        chain: Chain,
        woc_api_key: Option<String>,
        bananablocks_base: Option<String>,
    ) -> Self {
        let options = ServicesOptions {
            bananablocks_base,
            ..ServicesOptions::default()
        };
        let bananablocks = options.bananablocks_base_for(chain);
        let woc_api_key = woc_api_key.filter(|k| !k.trim().is_empty());
        let gate = shared_woc_gate(chain, woc_api_key.clone());
        Self::with_woc_gate(
            Some((woc_base(chain), gate)),
            bananablocks.as_deref(),
            woc_api_key,
        )
    }

    /// The rungs at explicit bases (a fixture in a test, a mirror): `None`
    /// leaves that rung out. The WhatsOnChain rung's gate is its own, never
    /// the process's, so a test's 429 never carries into another case.
    #[allow(dead_code)] // the binary builds through `from_env`; the tests and the library's callers call this
    pub fn at(woc: Option<&str>, bananablocks: Option<&str>, woc_api_key: Option<String>) -> Self {
        let woc = woc.map(|base| {
            (
                base,
                Arc::new(RateGate::new(
                    PROVIDER_WHATSONCHAIN,
                    WOC_DEFAULT_RETRY_AFTER,
                )),
            )
        });
        Self::with_woc_gate(woc, bananablocks, woc_api_key)
    }

    /// The rungs with the WhatsOnChain rung at `woc`'s base behind its gate.
    fn with_woc_gate(
        woc: Option<(&str, Arc<RateGate>)>,
        bananablocks: Option<&str>,
        woc_api_key: Option<String>,
    ) -> Self {
        let bananablocks = bananablocks.and_then(|base| match BananaBlocks::with_base(base, None) {
            Ok(provider) => Some(Arc::new(provider)),
            Err(e) => {
                tracing::warn!(base, error = %e, "BananaBlocks could not be set up; the rung is left out");
                None
            }
        });
        Self {
            client: reqwest::Client::builder()
                .timeout(EXPLORER_TIMEOUT)
                .build()
                .unwrap_or_default(),
            woc: woc.map(|(base, gate)| WocRung {
                base: base.trim().trim_end_matches('/').to_string(),
                api_key: woc_api_key.filter(|k| !k.trim().is_empty()),
                gate,
            }),
            bananablocks,
        }
    }

    /// The BananaBlocks provider, when the rung is on.
    pub fn bananablocks(&self) -> Option<&Arc<BananaBlocks>> {
        self.bananablocks.as_ref()
    }

    /// WhatsOnChain's `/tx/{txid}/beef`: a payment's BEEF, hex. Anything
    /// but a 200 with hex is a fault.
    pub async fn woc_beef(&self, txid: &str) -> Result<Vec<u8>, String> {
        // Break-glass (Rule 28, C2): a courier of a payment nobody handed
        // us; what it delivers is checked against the header service when
        // it is internalized, so it is trusted for delivery alone.
        let (status, body) = self.woc_get(&format!("/tx/{txid}/beef"), "beef").await?;
        if status != StatusCode::OK {
            return Err(format!("{PROVIDER_WHATSONCHAIN}: HTTP {status}"));
        }
        hex::decode(body.trim())
            .map_err(|e| format!("{PROVIDER_WHATSONCHAIN}: the BEEF is not hex: {e}"))
    }

    /// BananaBlocks' `/tx/{txid}/hex`: the transaction's bytes, which the
    /// toolbox's provider binds to the txid (bytes that do not hash to it
    /// are a fault). "No such transaction" is a fault here too: this rung
    /// delivers or it does not.
    pub async fn bananablocks_raw_tx(&self, txid: &str) -> Result<Vec<u8>, String> {
        let Some(bb) = &self.bananablocks else {
            return Err(format!("{PROVIDER_BANANABLOCKS}: off"));
        };
        let started = Instant::now();
        let answer = bb.get_raw_tx(txid).await;
        let status = match &answer {
            Ok(GetRawTxResult {
                raw_tx: Some(_), ..
            }) => "200".to_string(),
            Ok(_) => "404".to_string(),
            Err(e) => fault_status(e),
        };
        log_call(PROVIDER_BANANABLOCKS, "hex", &status, started);
        match answer {
            Ok(GetRawTxResult {
                raw_tx: Some(raw), ..
            }) => Ok(raw),
            Ok(_) => Err(format!("{PROVIDER_BANANABLOCKS}: no such transaction")),
            Err(e) => Err(format!("{PROVIDER_BANANABLOCKS}: {e}")),
        }
    }

    /// One GET at WhatsOnChain through its gate: no request inside a
    /// Retry-After window; a 429 closes the rung and is the fault returned.
    async fn woc_get(
        &self,
        path: &str,
        call: &'static str,
    ) -> Result<(StatusCode, String), String> {
        let Some(woc) = &self.woc else {
            return Err(format!("{PROVIDER_WHATSONCHAIN}: off"));
        };
        if let Err(e) = woc.gate.admit() {
            log_call(
                PROVIDER_WHATSONCHAIN,
                call,
                "skipped (rate limited)",
                Instant::now(),
            );
            return Err(format!("{PROVIDER_WHATSONCHAIN}: {e}"));
        }
        // Break-glass (Rule 28, C2): the one WhatsOnChain request line of
        // the CLI's own rungs (the courier's BEEF); the route says why at
        // its caller.
        let mut request = self.client.get(format!("{}{}", woc.base, path));
        if let Some(key) = &woc.api_key {
            request = request
                .header(WOC_API_KEY_HEADER, key)
                .header("Authorization", format!("Bearer {key}"));
        }
        let started = Instant::now();
        let response = match request.send().await {
            Ok(response) => response,
            Err(e) => {
                log_call(PROVIDER_WHATSONCHAIN, call, "error", started);
                return Err(format!("{PROVIDER_WHATSONCHAIN}: {e}"));
            }
        };
        let status = response.status();
        log_call(PROVIDER_WHATSONCHAIN, call, status.as_str(), started);
        woc.gate
            .check(status, response.headers())
            .map_err(|e| format!("{PROVIDER_WHATSONCHAIN}: {e}"))?;
        let body = response
            .text()
            .await
            .map_err(|e| format!("{PROVIDER_WHATSONCHAIN}: {e}"))?;
        Ok((status, body))
    }
}

/// The status an explorer fault is logged under: a rate limit apart from
/// every other fault.
pub fn fault_status(e: &bsv_wallet_toolbox::Error) -> String {
    match e {
        bsv_wallet_toolbox::Error::RateLimited { .. } => "rate limited".to_string(),
        _ => "fault".to_string(),
    }
}

/// The one log line of an explorer call (marker `explorer_call`): which
/// provider, which call, its status and how long it took.
pub fn log_call(provider: &str, call: &str, status: &str, started: Instant) {
    tracing::info!(
        marker = "explorer_call",
        provider,
        call,
        status,
        ms = started.elapsed().as_millis() as u64,
        "explorer call"
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::Fixture;

    const SRC: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

    /// `BANANABLOCKS_BASE` unset is the chain's default, empty is off.
    #[test]
    fn bananablocks_is_on_by_default_and_off_when_empty() {
        let main = Explorers::configured(Chain::Main, None, None);
        assert_eq!(
            main.bananablocks().map(|bb| bb.base_url().to_string()),
            Some("https://bananablocks.com/api/v1".to_string())
        );
        let test = Explorers::configured(Chain::Test, None, None);
        assert_eq!(
            test.bananablocks().map(|bb| bb.base_url().to_string()),
            Some("https://test.bananablocks.com/api/v1".to_string())
        );
        let off = Explorers::configured(Chain::Main, None, Some(String::new()));
        assert!(off.bananablocks().is_none());
    }

    /// 0.18.1 (toolbox #44): the spend probe's spender rungs left this
    /// module; what remains asks for bytes (`/beef`, `/hex`) and for a
    /// chain index's word, never for a spender.
    #[test]
    fn the_module_holds_no_spender_route() {
        let source = include_str!("explorers.rs");
        let code = source.split("#[cfg(test)]").next().unwrap();
        assert!(!code.contains("/spent\""), "a spent request");
        assert!(!code.contains("get_txo("), "a BananaBlocks txo read");
        assert!(!code.contains("get_spend("), "a BananaBlocks spend read");
    }

    /// A 429 is a fault at once, and the rung is skipped without a request
    /// until its Retry-After passes.
    #[tokio::test]
    async fn a_whatsonchain_429_closes_the_rung_for_its_retry_after() {
        let route = format!("/tx/{SRC}/beef");
        let fixture =
            Fixture::start_with_headers(&[(&route, 429, "", &[("retry-after", "30")])], 500).await;
        let explorers = Explorers::at(Some(&fixture.base), None, None);
        let started = Instant::now();
        let first = explorers.woc_beef(SRC).await;
        assert!(first.unwrap_err().contains("Rate limited"));
        let second = explorers.woc_beef(SRC).await;
        assert!(second.unwrap_err().contains("Rate limited"));
        assert_eq!(fixture.count(&route), 1, "the closed rung was asked again");
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "a 429 was slept on"
        );
    }

    /// M2 of the review: the process's WhatsOnChain rung passes the toolbox
    /// provider's gate for its base and key, so a 429 either side meets
    /// closes the rung for both: one the toolbox's gate took closes the
    /// CLI's rung without a request, and one the CLI's rung met closes the
    /// toolbox provider's. A key is its own budget, and `Explorers::at`'s
    /// gate is its own. Red at the base: the rung held a second gate.
    #[tokio::test]
    async fn the_whatsonchain_rung_shares_the_toolbox_providers_gate() {
        let toolbox = |key: &str| {
            WhatsOnChain::new(
                Chain::Main,
                WhatsOnChainConfig {
                    api_key: Some(key.to_string()),
                    timeout_secs: None,
                },
            )
            .unwrap()
        };
        let mut retry_after = reqwest::header::HeaderMap::new();
        retry_after.insert("retry-after", "30".parse().unwrap());

        // The toolbox's side met the 429: the CLI's rung is closed and
        // asks nothing (a fixture at its base would have counted it).
        let key = "m2-toolbox-side";
        let process =
            Explorers::configured(Chain::Main, Some(key.to_string()), Some(String::new()));
        let gate = Arc::clone(&process.woc.as_ref().unwrap().gate);
        assert!(Arc::ptr_eq(&gate, toolbox(key).rate_gate()));
        let fixture = Fixture::start(&[], 404).await;
        let rung = Explorers::with_woc_gate(Some((&fixture.base, Arc::clone(&gate))), None, None);
        let _ = toolbox(key).rate_gate().close(&retry_after);
        let closed = rung.woc_beef(SRC).await;
        assert!(closed.unwrap_err().contains("Rate limited"));
        assert_eq!(fixture.total(), 0, "the closed rung made a request");

        // The CLI's rung met the 429: the toolbox provider's gate is closed.
        let key = "m2-cli-side";
        let gate = Arc::clone(
            &Explorers::configured(Chain::Main, Some(key.to_string()), Some(String::new()))
                .woc
                .unwrap()
                .gate,
        );
        let route = format!("/tx/{SRC}/beef");
        let limited =
            Fixture::start_with_headers(&[(&route, 429, "", &[("retry-after", "30")])], 500).await;
        let rung = Explorers::with_woc_gate(Some((&limited.base, gate)), None, None);
        assert!(toolbox(key).rate_gate().admit().is_ok());
        let met = rung.woc_beef(SRC).await;
        assert!(met.unwrap_err().contains("Rate limited"));
        assert!(
            toolbox(key).rate_gate().admit().is_err(),
            "the toolbox's gate is open after the CLI's 429"
        );

        // Another key is another budget; a test's rungs share nothing.
        assert!(toolbox("m2-other-key").rate_gate().admit().is_ok());
        let private = Explorers::at(Some(&limited.base), None, Some(key.to_string()));
        assert!(!Arc::ptr_eq(
            &private.woc.as_ref().unwrap().gate,
            toolbox(key).rate_gate()
        ));
    }

    /// A configured key is sent as `woc-api-key` and as a bearer token.
    #[tokio::test]
    async fn the_woc_api_key_is_sent() {
        let fixture = Fixture::start(&[], 404).await;
        let explorers = Explorers::at(Some(&fixture.base), None, Some("k-123".to_string()));
        let _ = explorers.woc_beef(SRC).await;
        let headers = fixture.headers();
        let sent = headers.last().expect("a request");
        assert_eq!(
            sent.get(WOC_API_KEY_HEADER).map(String::as_str),
            Some("k-123")
        );
        assert_eq!(
            sent.get("authorization").map(String::as_str),
            Some("Bearer k-123")
        );
    }

    /// BananaBlocks' bytes must hash to the txid: other bytes are a fault.
    #[tokio::test]
    async fn bananablocks_bytes_are_bound_to_the_txid() {
        use crate::test_support::{p2pkh, raw_tx, txid_of};
        let raw = raw_tx(&[(SRC, 0)], &[(1, p2pkh([7; 20]))]);
        let txid = txid_of(&raw);
        let wrong = "dd".repeat(32);
        let fixture = Fixture::start(
            &[
                (&format!("/tx/{txid}/hex"), 200, &hex::encode(&raw)),
                (&format!("/tx/{wrong}/hex"), 200, &hex::encode(&raw)),
            ],
            404,
        )
        .await;
        let explorers = Explorers::at(None, Some(&fixture.base), None);
        assert_eq!(explorers.bananablocks_raw_tx(&txid).await, Ok(raw));
        assert!(explorers.bananablocks_raw_tx(&wrong).await.is_err());
        assert!(explorers
            .bananablocks_raw_tx(&"ee".repeat(32))
            .await
            .unwrap_err()
            .contains("no such transaction"));
    }
}
