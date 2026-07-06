//! Post-broadcast verification — fail loudly when a broadcast was silently dropped.
//!
//! # The silent-data-loss bug this closes
//!
//! A `send` (CLI `send` or the served `/createAction` endpoint) delegates to
//! `Wallet::create_action`, which signs the tx and broadcasts it through ARC.
//! For a **monitor-less served wallet** (the LOW e2e runs `bsv-wallet serve`
//! with no chain monitor / chaintracks) the wallet has never fetched merkle
//! proofs for its *confirmed* ancestors, so the BEEF it hands ARC carries the
//! whole unconfirmed chain. ARC then charges the fee for the **entire package**
//! and rejects the tx with **error 465 "fee too low"**.
//!
//! In `bsv-wallet-toolbox-rs`, an ARC 465 is tagged `service_error = true`, so
//! `classify_broadcast_results` treats it as a *transient* `ServiceError`
//! (retryable) rather than a permanent `InvalidTx`. `create_action` therefore
//! returns `Ok` with a txid — a **phantom txid that never propagates**
//! (WhatsOnChain 404 forever). The send path reported success and exit 0 while
//! the funds were never sent. That stranded funds in a LOW mainnet e2e rebalance.
//!
//! # What this module does (part 1 — fail loud)
//!
//! After a broadcast, ask the network whether the txid actually exists. We only
//! declare a **`Rejected`** (hard failure) when a broadcaster/indexer that would
//! hold the tx *if it had been accepted* affirmatively reports it **absent**,
//! and **no** source reports it present. If we cannot reach any source we return
//! `Inconclusive` and callers preserve prior behaviour — so a down confirmation
//! service never turns a real send into a false failure (zero added flakiness).
//!
//! A legitimately-accepted tx is present on ARC immediately (ARC keeps recently
//! submitted txs queryable) and indexed by WoC within seconds, so the happy path
//! short-circuits to `Confirmed` on the first probe — no meaningful added latency
//! for a wallet whose ancestors are already proven/shallow.

use std::time::Duration;

use bsv_wallet_toolbox::Chain;
use reqwest::Client;

/// Default number of probe rounds before declaring a definitive absence.
const DEFAULT_ATTEMPTS: u32 = 6;
/// Default delay between probe rounds (ms). Only rejected txs pay the full cost;
/// an accepted tx short-circuits on the first `Present`.
const DEFAULT_DELAY_MS: u64 = 1500;
/// Per-request timeout for a single status probe.
const PROBE_TIMEOUT: Duration = Duration::from_secs(10);

/// Outcome of verifying that a just-broadcast tx actually reached the network.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BroadcastVerification {
    /// At least one source confirms the tx exists (accepted / seen / mined).
    Confirmed,
    /// A source that would hold the tx if it had been accepted affirmatively
    /// reports it absent, and no source reports it present — the broadcast was
    /// silently dropped (classic ARC 465 fee-too-low on a deep unconfirmed BEEF).
    /// The funds were NOT sent.
    Rejected,
    /// No source could be reached to confirm either way. Callers must NOT treat
    /// this as a failure (avoids false negatives when the confirmation service
    /// itself is unreachable).
    Inconclusive,
}

impl BroadcastVerification {
    /// Map a verification into a `Result`, failing loudly only on a definitive
    /// `Rejected`. `Confirmed` and `Inconclusive` are both treated as "proceed".
    pub fn into_send_result(self, txid: &str) -> anyhow::Result<()> {
        match self {
            BroadcastVerification::Rejected => Err(anyhow::anyhow!(
                "broadcast rejected: transaction {txid} is not present on the network. \
                 The broadcaster (ARC) dropped it — most likely error 465 \"fee too low\", \
                 because a monitor-less wallet presented a deep unconfirmed BEEF and ARC \
                 charged the fee for the whole unconfirmed package. The funds were NOT sent. \
                 Fetch merkle proofs for the confirmed ancestors (run `bsv-wallet tick` with \
                 CHAINTRACKS_URL set) or fund from a confirmed UTXO, then retry."
            )),
            BroadcastVerification::Confirmed | BroadcastVerification::Inconclusive => Ok(()),
        }
    }
}

/// Presence of a txid according to a single source.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Presence {
    /// Source has the tx (HTTP 200).
    Present,
    /// Source definitively does not have the tx (HTTP 404).
    Absent,
    /// Source could not give a definitive answer (auth error, 5xx, network error).
    Unknown,
}

/// A network endpoint we can ask "do you know this txid?".
#[derive(Clone)]
struct StatusSource {
    /// Human-readable name (diagnostics only).
    name: &'static str,
    /// URL template containing the literal `{txid}` placeholder.
    url_template: String,
    /// Full `Authorization` header value, if the endpoint needs one.
    auth: Option<String>,
    /// Whether a 404 from this source is trustworthy evidence of absence.
    /// ARC keeps recently-submitted txs queryable and WoC indexes mempool txs
    /// within the probe window, so all default sources are authoritative here;
    /// the probe window guards against indexing lag by short-circuiting on the
    /// first `Present`.
    authoritative_absence: bool,
}

/// Verifies that a broadcast tx actually reached the network.
///
/// Cheap to clone (shares the reqwest connection pool). Built once and shared
/// via an axum extension on the served path, or per-command on the CLI path.
#[derive(Clone)]
pub struct BroadcastVerifier {
    client: Client,
    sources: Vec<StatusSource>,
    attempts: u32,
    delay: Duration,
    /// When false (env opt-out) `verify` short-circuits to `Inconclusive`.
    enabled: bool,
}

impl BroadcastVerifier {
    /// Build a verifier for `chain`, reading optional overrides from the env:
    /// - `BSV_WALLET_SKIP_BROADCAST_VERIFY=1` disables verification entirely.
    /// - `BSV_WALLET_BROADCAST_VERIFY_ATTEMPTS` overrides the probe-round count.
    /// - `BSV_WALLET_BROADCAST_VERIFY_DELAY_MS` overrides the inter-round delay.
    /// - `TAAL_API_KEY` / `MAIN_TAAL_API_KEY` authenticate the TAAL ARC probe.
    pub fn from_env(chain: Chain) -> Self {
        let enabled = !env_truthy("BSV_WALLET_SKIP_BROADCAST_VERIFY");
        let attempts = std::env::var("BSV_WALLET_BROADCAST_VERIFY_ATTEMPTS")
            .ok()
            .and_then(|v| v.parse::<u32>().ok())
            .filter(|n| *n > 0)
            .unwrap_or(DEFAULT_ATTEMPTS);
        let delay_ms = std::env::var("BSV_WALLET_BROADCAST_VERIFY_DELAY_MS")
            .ok()
            .and_then(|v| v.parse::<u64>().ok())
            .unwrap_or(DEFAULT_DELAY_MS);

        // TAAL ARC uses a raw `Authorization: <key>` header (no "Bearer " prefix).
        let taal_key = std::env::var("TAAL_API_KEY")
            .ok()
            .filter(|k| !k.is_empty())
            .or_else(|| {
                std::env::var("MAIN_TAAL_API_KEY")
                    .ok()
                    .filter(|k| !k.is_empty())
            });

        let mut sources = vec![
            // TAAL ARC — the primary broadcaster. Authenticated GET when a key is
            // present; without a key it may 401 (→ Unknown, harmless).
            StatusSource {
                name: "arc-taal",
                url_template: format!("{}/v1/tx/{{txid}}", taal_arc_url(chain)),
                auth: taal_key.clone(),
                authoritative_absence: taal_key.is_some(),
            },
            // WhatsOnChain — keyless, reliable 200/404. Presence + absence signal.
            StatusSource {
                name: "whatsonchain",
                url_template: format!("{}/tx/hash/{{txid}}", woc_base(chain)),
                auth: None,
                authoritative_absence: true,
            },
        ];
        // GorillaPool ARC — keyless fallback broadcaster (mainnet only).
        if let Some(gp) = gorillapool_arc_url(chain) {
            sources.push(StatusSource {
                name: "arc-gorillapool",
                url_template: format!("{}/v1/tx/{{txid}}", gp),
                auth: None,
                authoritative_absence: true,
            });
        }

        Self {
            client: Client::new(),
            sources,
            attempts,
            delay: Duration::from_millis(delay_ms),
            enabled,
        }
    }

    /// Probe the network for `txid`, returning as soon as any source confirms it
    /// present, otherwise after the full probe window.
    pub async fn verify(&self, txid: &str) -> BroadcastVerification {
        if !self.enabled || self.sources.is_empty() {
            return BroadcastVerification::Inconclusive;
        }

        let mut last_round_saw_authoritative_absence = false;
        for attempt in 0..self.attempts {
            let mut any_present = false;
            let mut authoritative_absence = false;
            for src in &self.sources {
                match probe(&self.client, src, txid).await {
                    Presence::Present => any_present = true,
                    Presence::Absent if src.authoritative_absence => authoritative_absence = true,
                    _ => {}
                }
            }
            if any_present {
                return BroadcastVerification::Confirmed;
            }
            last_round_saw_authoritative_absence = authoritative_absence;
            if attempt + 1 < self.attempts {
                tokio::time::sleep(self.delay).await;
            }
        }

        if last_round_saw_authoritative_absence {
            BroadcastVerification::Rejected
        } else {
            BroadcastVerification::Inconclusive
        }
    }
}

/// Probe a single source for a txid's presence.
async fn probe(client: &Client, src: &StatusSource, txid: &str) -> Presence {
    let url = src.url_template.replace("{txid}", txid);
    let mut req = client.get(&url).timeout(PROBE_TIMEOUT);
    if let Some(auth) = &src.auth {
        req = req.header("Authorization", auth);
    }
    match req.send().await {
        Ok(resp) => match resp.status().as_u16() {
            200 => Presence::Present,
            404 => Presence::Absent,
            other => {
                tracing::debug!(
                    source = src.name,
                    status = other,
                    "broadcast probe inconclusive"
                );
                Presence::Unknown
            }
        },
        Err(e) => {
            tracing::debug!(source = src.name, error = %e, "broadcast probe request failed");
            Presence::Unknown
        }
    }
}

fn taal_arc_url(chain: Chain) -> &'static str {
    match chain {
        Chain::Main => "https://arc.taal.com",
        Chain::Test => "https://arc-test.taal.com",
    }
}

fn gorillapool_arc_url(chain: Chain) -> Option<&'static str> {
    match chain {
        Chain::Main => Some("https://arc.gorillapool.io"),
        // GorillaPool testnet ARC is not commonly used; omit it.
        Chain::Test => None,
    }
}

fn woc_base(chain: Chain) -> &'static str {
    match chain {
        Chain::Main => "https://api.whatsonchain.com/v1/bsv/main",
        Chain::Test => "https://api.whatsonchain.com/v1/bsv/test",
    }
}

fn env_truthy(key: &str) -> bool {
    std::env::var(key)
        .map(|v| {
            let v = v.trim().to_ascii_lowercase();
            v == "1" || v == "true" || v == "yes" || v == "on"
        })
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::StatusCode;
    use axum::routing::get;
    use axum::Router;
    use std::net::SocketAddr;

    /// Spin up a local mock that answers `GET /v1/tx/{txid}` with `code` for
    /// every txid. Returns the base URL (`http://127.0.0.1:PORT`).
    async fn mock_status_server(code: StatusCode) -> String {
        let app = Router::new().route("/v1/tx/{txid}", get(move || async move { code }));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr: SocketAddr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(listener, app).await.ok();
        });
        format!("http://{}", addr)
    }

    /// Build a verifier pointing at a single mock source (fast: 2 attempts, no delay).
    fn verifier_for(base: &str, authoritative_absence: bool) -> BroadcastVerifier {
        BroadcastVerifier {
            client: Client::new(),
            sources: vec![StatusSource {
                name: "mock",
                url_template: format!("{}/v1/tx/{{txid}}", base),
                auth: None,
                authoritative_absence,
            }],
            attempts: 2,
            delay: Duration::from_millis(0),
            enabled: true,
        }
    }

    const TXID: &str = "0000000000000000000000000000000000000000000000000000000000000001";

    // The core of the fix: a 465/rejected broadcast (the tx is absent from every
    // source that would hold it) must resolve to Rejected → an ERROR, never success.
    #[tokio::test]
    async fn rejected_broadcast_is_an_error_not_success() {
        let base = mock_status_server(StatusCode::NOT_FOUND).await;
        let verifier = verifier_for(&base, /* authoritative */ true);

        let outcome = verifier.verify(TXID).await;
        assert_eq!(
            outcome,
            BroadcastVerification::Rejected,
            "an absent (404-everywhere) tx must be classified Rejected"
        );
        // And the send path must surface it as a hard error, not exit 0.
        assert!(
            outcome.into_send_result(TXID).is_err(),
            "a Rejected verification must map to Err so the send fails loudly"
        );
    }

    #[tokio::test]
    async fn confirmed_broadcast_succeeds() {
        let base = mock_status_server(StatusCode::OK).await;
        let verifier = verifier_for(&base, true);

        let outcome = verifier.verify(TXID).await;
        assert_eq!(outcome, BroadcastVerification::Confirmed);
        assert!(outcome.into_send_result(TXID).is_ok());
    }

    #[tokio::test]
    async fn unreachable_source_is_inconclusive_not_a_failure() {
        // 503 from every probe → we cannot confirm either way → Inconclusive,
        // which must NOT be a failure (no false negatives when the service is down).
        let base = mock_status_server(StatusCode::SERVICE_UNAVAILABLE).await;
        let verifier = verifier_for(&base, true);

        let outcome = verifier.verify(TXID).await;
        assert_eq!(outcome, BroadcastVerification::Inconclusive);
        assert!(outcome.into_send_result(TXID).is_ok());
    }

    #[tokio::test]
    async fn non_authoritative_absence_is_inconclusive() {
        // A 404 from a presence-only source (not authoritative for absence) must
        // not by itself trigger a Rejected — it stays Inconclusive.
        let base = mock_status_server(StatusCode::NOT_FOUND).await;
        let verifier = verifier_for(&base, /* authoritative */ false);

        assert_eq!(
            verifier.verify(TXID).await,
            BroadcastVerification::Inconclusive
        );
    }

    #[tokio::test]
    async fn disabled_verifier_is_inconclusive() {
        let base = mock_status_server(StatusCode::NOT_FOUND).await;
        let mut verifier = verifier_for(&base, true);
        verifier.enabled = false;
        assert_eq!(
            verifier.verify(TXID).await,
            BroadcastVerification::Inconclusive
        );
    }
}
