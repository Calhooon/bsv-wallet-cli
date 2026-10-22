//! The explorer rungs the commands ask themselves (bsv-wallet-cli #50).
//!
//! The owner's word of 2026-10-22: each chain scanner has its own rate
//! limits, so when one fails we go to the next. WhatsOnChain, BananaBlocks
//! and Bitails are each other's fallbacks; a negative ("unspent",
//! "absent") needs a second provider's clean answer; "could not look" is
//! never "nothing there". The toolbox's services hold that rule for the
//! questions they answer (`get_raw_tx`, `get_merkle_path`,
//! `get_utxo_status`, `get_status_for_txids`, and `get_utxo_leads` for
//! `sync`'s discovery, toolbox 0.12.0). This module
//! holds it for the break-glass reads the CLI still makes itself:
//!
//! - who spent an outpoint (the spend probe's step 1): WhatsOnChain's
//!   `/tx/{txid}/{vout}/spent` and BananaBlocks' `/txo/{txid}/{vout}`,
//!   a rotating start, a fault moving to the next rung at once;
//! - a payment's bytes for `receive`: WhatsOnChain's `/tx/{txid}/beef`, then
//!   BananaBlocks' `/tx/{txid}/hex` (bytes that hash to the txid);
//! - BananaBlocks as a chain index of the broadcast verifier.
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

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};

use bsv_wallet_toolbox::services::providers::whatsonchain::WOC_API_KEY_HEADER;
use bsv_wallet_toolbox::services::providers::{
    BananaBlocks, OutpointSpend, RateGate, BANANABLOCKS_NAME,
};
use bsv_wallet_toolbox::services::traits::convert_script_hash;
use bsv_wallet_toolbox::services::GetRawTxResult;
use bsv_wallet_toolbox::{Chain, GetUtxoStatusOutputFormat, ServicesOptions};
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

/// One rung of the spender question.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Rung {
    WhatsOnChain,
    BananaBlocks,
}

impl Rung {
    /// The provider's name, as logged and reported.
    pub fn name(self) -> &'static str {
        match self {
            Rung::WhatsOnChain => PROVIDER_WHATSONCHAIN,
            Rung::BananaBlocks => PROVIDER_BANANABLOCKS,
        }
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
    /// Which spender rung is asked first, advanced once per question.
    rotation: AtomicUsize,
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
    #[allow(dead_code)]
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
            rotation: AtomicUsize::new(0),
        }
    }

    /// The BananaBlocks provider, when the rung is on.
    pub fn bananablocks(&self) -> Option<&Arc<BananaBlocks>> {
        self.bananablocks.as_ref()
    }

    /// The spender rungs for one question, the start one on from the last
    /// question's (so one explorer is not the fixed first word, and one
    /// rate-limited explorer does not stall every check).
    pub fn spender_rungs(&self) -> Vec<Rung> {
        let mut rungs = Vec::new();
        if self.woc.is_some() {
            rungs.push(Rung::WhatsOnChain);
        }
        if self.bananablocks.is_some() {
            rungs.push(Rung::BananaBlocks);
        }
        if !rungs.is_empty() {
            let start = self.rotation.fetch_add(1, Ordering::Relaxed) % rungs.len();
            rungs.rotate_left(start);
        }
        rungs
    }

    /// Who spent `txid:vout`, by one rung. `Ok(Some(name))`: the rung
    /// names a spender (a word; its bytes bind it). `Ok(None)`: the rung
    /// knows no spend (a word, not an answer: it is also what an index
    /// behind, or a parent that does not exist, says). `Err`: could not
    /// look (a 429, a closed rung, a timeout, a 5xx, a malformed body).
    /// With `script`, BananaBlocks' answer is about the asked output only
    /// when its `script_pubkey` hashes to the script's hash; another
    /// script, the empty one included, is a fault.
    pub async fn spender(
        &self,
        rung: Rung,
        txid: &str,
        vout: u32,
        script: Option<&[u8]>,
    ) -> Result<Option<String>, String> {
        match rung {
            Rung::WhatsOnChain => {
                // Break-glass (Rule 28, C7): who spent an outpoint. No
                // header or proof says an output is spent by someone else,
                // and our outputs table knows only our own spends; the name
                // is a claim the spender's own bytes and proof must bear out.
                let (status, body) = self
                    .woc_get(&format!("/tx/{txid}/{vout}/spent"), "spent")
                    .await?;
                match status {
                    StatusCode::OK => {
                        let named = serde_json::from_str::<serde_json::Value>(&body)
                            .ok()
                            .and_then(|v| {
                                v.get("txid").and_then(|t| t.as_str()).map(str::to_string)
                            })
                            .filter(|t| is_txid(t));
                        match named {
                            Some(spender) => Ok(Some(spender.to_ascii_lowercase())),
                            None => Err(format!(
                                "{PROVIDER_WHATSONCHAIN}: a 200 that names no spender"
                            )),
                        }
                    }
                    StatusCode::NOT_FOUND => Ok(None),
                    status => Err(format!("{PROVIDER_WHATSONCHAIN}: HTTP {status}")),
                }
            }
            Rung::BananaBlocks => {
                let Some(bb) = &self.bananablocks else {
                    return Err(format!("{PROVIDER_BANANABLOCKS}: off"));
                };
                let started = Instant::now();
                let answer = bb.get_txo(txid, vout).await;
                let status = match &answer {
                    Ok(_) => "200".to_string(),
                    Err(e) => fault_status(e),
                };
                log_call(PROVIDER_BANANABLOCKS, "spend", &status, started);
                let txo = answer.map_err(|e| format!("{PROVIDER_BANANABLOCKS}: {e}"))?;
                if let Some(script) = script {
                    let asked = convert_script_hash(
                        &hex::encode(script),
                        Some(GetUtxoStatusOutputFormat::Script),
                    )
                    .map_err(|e| format!("{PROVIDER_BANANABLOCKS}: {e}"))?;
                    if txo.script_hash != asked {
                        return Err(format!(
                            "{PROVIDER_BANANABLOCKS}: txo {txid}.{vout}: its script hashes to {}, \
                             not the asked {asked}",
                            txo.script_hash
                        ));
                    }
                }
                match txo.spend {
                    OutpointSpend::SpentBy(spender) => Ok(Some(spender.to_ascii_lowercase())),
                    OutpointSpend::Unspent => Ok(None),
                }
            }
        }
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
        // Break-glass (Rule 28, C2 and C7): the one WhatsOnChain request
        // line of the CLI's own rungs (the courier's BEEF, the spender's
        // name); each route says why at its caller.
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

fn is_txid(s: &str) -> bool {
    s.len() == 64 && s.bytes().all(|b| b.is_ascii_hexdigit())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::Fixture;

    const SRC: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

    /// The start rotates over the rungs that are on.
    #[test]
    fn the_spender_start_rotates_over_the_rungs_that_are_on() {
        let both = Explorers::at(Some("http://127.0.0.1:1"), Some("http://127.0.0.1:1"), None);
        assert_eq!(
            both.spender_rungs(),
            vec![Rung::WhatsOnChain, Rung::BananaBlocks]
        );
        assert_eq!(
            both.spender_rungs(),
            vec![Rung::BananaBlocks, Rung::WhatsOnChain]
        );
        assert_eq!(
            both.spender_rungs(),
            vec![Rung::WhatsOnChain, Rung::BananaBlocks]
        );
        let woc_only = Explorers::at(Some("http://127.0.0.1:1"), None, None);
        assert_eq!(woc_only.spender_rungs(), vec![Rung::WhatsOnChain]);
        assert_eq!(woc_only.spender_rungs(), vec![Rung::WhatsOnChain]);
    }

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
        assert_eq!(off.spender_rungs(), vec![Rung::WhatsOnChain]);
    }

    /// WhatsOnChain's spent route, read: a name, "no spend known", and the
    /// faults (a 200 naming nothing, a 5xx) kept apart from both.
    #[tokio::test]
    async fn whatsonchains_spent_route_is_read_three_ways() {
        let spender = "bb".repeat(32);
        let named = format!(r#"{{"txid":"{}","vin":0}}"#, spender.to_uppercase());
        let fixture = Fixture::start(
            &[
                (&format!("/tx/{SRC}/0/spent"), 200, &named),
                (&format!("/tx/{SRC}/1/spent"), 404, ""),
                (&format!("/tx/{SRC}/2/spent"), 200, r#"{"vin":0}"#),
            ],
            503,
        )
        .await;
        let explorers = Explorers::at(Some(&fixture.base), None, None);
        let ask = |vout| explorers.spender(Rung::WhatsOnChain, SRC, vout, None);
        assert_eq!(ask(0).await, Ok(Some(spender)));
        assert_eq!(ask(1).await, Ok(None));
        assert!(ask(2).await.unwrap_err().contains("names no spender"));
        assert!(ask(3).await.unwrap_err().contains("503"));
    }

    /// BananaBlocks' `/txo/{txid}/{vout}` through the toolbox's provider:
    /// the recorded shape of the 2026-10-22 probe. With the asked script,
    /// an output under another script is a fault, never an answer.
    #[tokio::test]
    async fn bananablocks_txo_route_is_read_through_the_toolbox() {
        let spender = "cc".repeat(32);
        let script = crate::test_support::p2pkh([0xdb; 20]);
        let held = hex::encode(&script);
        let other = hex::encode(crate::test_support::p2pkh([0xdc; 20]));
        let fixture = Fixture::start(
            &[
                (
                    &format!("/txo/{SRC}/0"),
                    200,
                    &format!(
                        r#"{{"script_pubkey":"{held}","spent":true,"spent_txid":"{spender}","spend_scope":"mempool"}}"#
                    ),
                ),
                (
                    &format!("/txo/{SRC}/1"),
                    200,
                    &format!(r#"{{"script_pubkey":"{held}","spent":false}}"#),
                ),
                (&format!("/txo/{SRC}/2"), 404, ""),
                (
                    &format!("/txo/{SRC}/3"),
                    200,
                    &format!(
                        r#"{{"script_pubkey":"{other}","spent":true,"spent_txid":"{spender}"}}"#
                    ),
                ),
                (
                    &format!("/txo/{SRC}/4"),
                    200,
                    r#"{"script_pubkey":"","spent":false}"#,
                ),
            ],
            500,
        )
        .await;
        let explorers = Explorers::at(None, Some(&fixture.base), None);
        let ask = |vout| explorers.spender(Rung::BananaBlocks, SRC, vout, Some(&script));
        assert_eq!(ask(0).await, Ok(Some(spender)));
        assert_eq!(ask(1).await, Ok(None));
        assert!(ask(2).await.is_err(), "a 404 is a fault, never unspent");
        assert!(
            ask(3).await.unwrap_err().contains("not the asked"),
            "another script is a fault"
        );
        assert!(
            ask(4).await.unwrap_err().contains("not the asked"),
            "the empty script is a fault"
        );
    }

    /// A 429 is a fault at once, and the rung is skipped without a request
    /// until its Retry-After passes.
    #[tokio::test]
    async fn a_whatsonchain_429_closes_the_rung_for_its_retry_after() {
        let route = format!("/tx/{SRC}/0/spent");
        let fixture =
            Fixture::start_with_headers(&[(&route, 429, "", &[("retry-after", "30")])], 500).await;
        let explorers = Explorers::at(Some(&fixture.base), None, None);
        let started = Instant::now();
        let first = explorers.spender(Rung::WhatsOnChain, SRC, 0, None).await;
        assert!(first.unwrap_err().contains("Rate limited"));
        let second = explorers.spender(Rung::WhatsOnChain, SRC, 0, None).await;
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
        let closed = rung.spender(Rung::WhatsOnChain, SRC, 0, None).await;
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
        let route = format!("/tx/{SRC}/0/spent");
        let limited =
            Fixture::start_with_headers(&[(&route, 429, "", &[("retry-after", "30")])], 500).await;
        let rung = Explorers::with_woc_gate(Some((&limited.base, gate)), None, None);
        assert!(toolbox(key).rate_gate().admit().is_ok());
        let met = rung.spender(Rung::WhatsOnChain, SRC, 0, None).await;
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
        let _ = explorers.spender(Rung::WhatsOnChain, SRC, 0, None).await;
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
