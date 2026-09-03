//! Post-broadcast verification — fail loudly when a broadcast was silently
//! dropped, and **never** when it was not.
//!
//! # Bug 1 — the silent data loss this module was built to close
//!
//! A `send` (CLI `send` or the served `/createAction` endpoint) delegates to
//! `Wallet::create_action`, which signs the tx and broadcasts it. For a
//! **monitor-less served wallet** (no chain monitor / chaintracks) the wallet
//! has never fetched merkle proofs for its *confirmed* ancestors, so the BEEF it
//! hands ARC carries the whole unconfirmed chain. ARC then charges the fee for
//! the **entire package** and rejects the tx with **error 465 "fee too low"**.
//!
//! In `bsv-wallet-toolbox-rs`, an ARC 465 is tagged `service_error = true`, so
//! `classify_broadcast_results` treats it as a *transient* `ServiceError`
//! (retryable) rather than a permanent `InvalidTx`. `create_action` therefore
//! returns `Ok` with a txid — a **phantom txid that never propagates**. The send
//! path reported success and exit 0 while the funds were never sent.
//!
//! # Bug 2 — the false negative this module *introduced* (fixed here)
//!
//! The first cut of this module polled a **hardcoded** source list and never
//! looked at which broadcaster the wallet had actually been configured to use.
//! It was plane-blind, and every one of its sources was marked authoritative for
//! absence. Three separate defects fell out of that:
//!
//! 1. **The plane that actually holds the answer was never asked.** A wallet in
//!    Arcade V2 mode (`ARC_MODE=arcade`) submits to the Arcade endpoint, and the
//!    verifier never queried it — so the one store that is *guaranteed* to have
//!    a record of our own submission contributed nothing.
//! 2. **`arc.gorillapool.io` was trusted for absence unconditionally.** It is a
//!    submission-scoped metamorph store, **not a chain index**: it answers 404
//!    for transactions that are mined with hundreds of thousands of
//!    confirmations. (Verified directly: `GET
//!    https://arc.gorillapool.io/v1/tx/<genesis coinbase txid>` → `404
//!    {"extraInfo":"transaction not found"}`.) Its 404 carries no information
//!    about a transaction it was never handed.
//! 3. **Only WhatsOnChain could ever vote `Present`, inside a ~7.5 s window.**
//!    So the verdict reduced to a coin flip on WoC's mempool-indexing latency.
//!
//! The module's own comment asserted that "ARC keeps recently submitted txs
//! queryable … so all default sources are authoritative here". That is true only
//! of the ARC instance you actually submitted to. That unchecked proposition was
//! the root cause: in the dHouse funder's entire history, all four `Rejected`
//! verdicts were **false negatives** — every one of those transactions was on
//! chain.
//!
//! # Bug 3: "present" is not "on the network" (2026-09-02)
//!
//! A 200 from the broadcaster we submitted through used to count as presence.
//! It is not network evidence: Arcade answers `GET /tx/{txid}` with a 200 and
//! `txStatus: RECEIVED` / `SENT_TO_NETWORK` for a transaction it holds but
//! that no node has seen, and with a 200 and `txStatus: REJECTED` for one it
//! will never relay. On 2026-09-02 four beta wallets sent EF children whose
//! 202'd parents had never propagated; the children were orphans forever,
//! and a verifier that read "200" as "present" could not tell.
//!
//! So a probe now reads the body. Every source yields one of: network-level
//! [`NetworkEvidence`] (`SEEN_ON_NETWORK` / `SEEN_MULTIPLE_NODES` / `MINED`
//! from an ARC-style store, any 200 from the chain index), *held* (the store
//! has the bytes, the network has not vouched: pre-gate statuses, an
//! orphan-pool hit), a *fatal* verdict from the broadcaster we submitted
//! through (`REJECTED` / `DOUBLE_SPEND_ATTEMPTED`), absence, or unknown.
//! [`BroadcastVerifier::verify_report`] surfaces all of it in a
//! [`PresenceReport`]; the served follow-up credits the wallet's broadcast
//! memory (`seen` for the tx AND its unproven ancestors: presence of the
//! child implies the parents connected) and the reconciler runs its absence
//! clock on it.
//!
//! # The model this module now implements
//!
//! Doctrine (`CLAUDE.md`): *"2xx is never success — truth = visible in our own
//! index / on chain"*; a **positive** answer may be trusted, an **absence** must
//! be chain-verified. Applied to the verifier itself: **absence from the wrong
//! plane is not truth.**
//!
//! * **Presence is trusted from anybody.** A store holding the transaction
//!   (held or seen) means the broadcast was not silently dropped. A
//!   freshly-minted txid we just created cannot be known to a third party
//!   unless it really propagated. So any held / seen answer → `Confirmed`.
//! * **Absence is trusted from almost nobody.** See [`AbsenceAuthority`]: a 404
//!   (or a fatal verdict) is evidence only from the broadcaster we personally
//!   submitted through (scope) or from a real chain+mempool index after its
//!   indexing window has elapsed (time), and we require **both** before
//!   declaring `Rejected`.
//! * **The broadcaster we used is consulted first**, so the happy path
//!   short-circuits to `Confirmed` on a single request.
//! * If we cannot satisfy that bar we return `Inconclusive`, and callers preserve
//!   prior behaviour — a down (or unidentifiable) confirmation service never
//!   turns a real send into a false failure.

use std::time::{Duration, Instant};

use bsv_wallet_toolbox::{
    services::ARCADE_V2_MAINNET, BroadcastStatus, Chain, BROADCAST_PROVIDER_CHAIN,
    BROADCAST_PROVIDER_NETWORK, PROVIDER_ARCADE_V2,
};
use reqwest::Client;

/// Default number of probe rounds before an absence may become definitive.
///
/// # Why not the original 6 × 1500 ms (~7.5 s)?
///
/// 7.5 s was never defensible as a *mempool-index* window. It is plenty for the
/// broadcaster we submitted through — that store knows about our submission the
/// instant it 200s our POST — but an independent index like WhatsOnChain only
/// learns of the transaction once it propagates to WoC's own node and WoC's
/// mempool ingestion picks it up. Normally that is a few seconds; under network
/// load, a provider hiccup, or an ARC→network relay delay it is routinely tens
/// of seconds. Declaring "the funds were NOT sent" on a 7.5 s WoC miss is
/// declaring a verdict on indexing latency, and that is exactly how the four
/// observed false negatives happened.
///
/// ~26 s of wall clock (see [`INITIAL_DELAY_MS`] for the schedule) gives the
/// independent index a realistic chance to catch up before its silence is
/// treated as evidence.
///
/// The cost is paid **only by transactions that really are absent everywhere**:
/// the happy path returns on the very first probe of the broadcaster, and the
/// caller's spending lock is already released before verification runs, so a
/// longer window does not serialize anything.
const DEFAULT_ATTEMPTS: u32 = 14;
/// Default CAP on the delay between probe rounds (ms). See [`INITIAL_DELAY_MS`].
const DEFAULT_DELAY_MS: u64 = 2500;
/// First inter-round delay (ms). The schedule is: probe immediately, then wait
/// 250 ms, 500 ms, 1 s, 2 s, then [`DEFAULT_DELAY_MS`] between every further
/// round. A cleanly accepted transaction is usually visible at the broadcaster
/// within a second, so the early rounds are cheap; the later rounds keep the
/// total window long enough for a lagging chain index. With the defaults the
/// gaps sum to 250+500+1000+2000 + 9×2500 = 26,250 ms.
const INITIAL_DELAY_MS: u64 = 250;
/// Per-request timeout for a single status probe. Deliberately shorter than the
/// inter-round delay so one slow source cannot stretch a round past the next.
const PROBE_TIMEOUT: Duration = Duration::from_secs(5);

/// Outcome of verifying that a just-broadcast tx actually reached the network.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BroadcastVerification {
    /// At least one source holds the tx (accepted / seen / mined).
    Confirmed,
    /// Both the broadcaster we actually submitted through **and** an independent
    /// chain index affirmatively report the tx absent (or, for the broadcaster,
    /// fatally rejected) after the full probe window, and no source holds it:
    /// the broadcast was silently dropped (classic ARC 465 fee-too-low on a
    /// deep unconfirmed BEEF, an Arcade `REJECTED`). The funds were NOT sent.
    Rejected,
    /// No source could give an answer that clears the evidence bar. Callers must
    /// NOT treat this as a failure (avoids false negatives when the confirmation
    /// service is unreachable, or when only the *wrong* plane reports absence).
    Inconclusive,
}

impl BroadcastVerification {
    /// Map a verification into a `Result`, failing loudly only on a definitive
    /// `Rejected`. `Confirmed` and `Inconclusive` are both treated as "proceed".
    pub fn into_send_result(self, txid: &str) -> anyhow::Result<()> {
        match self {
            BroadcastVerification::Rejected => Err(anyhow::anyhow!(
                "broadcast rejected: transaction {txid} is absent from BOTH the broadcaster \
                 it was submitted to AND an independent chain index, after the full probe \
                 window. The broadcaster dropped it — most likely error 465 \"fee too low\", \
                 because a monitor-less wallet presented a deep unconfirmed BEEF and ARC \
                 charged the fee for the whole unconfirmed package. The funds were NOT sent. \
                 Fetch merkle proofs for the confirmed ancestors (run `bsv-wallet tick` with \
                 CHAINTRACKS_URL set) or fund from a confirmed UTXO, then retry."
            )),
            BroadcastVerification::Confirmed | BroadcastVerification::Inconclusive => Ok(()),
        }
    }
}

/// Network-level presence a source reported: the transaction was seen by a
/// node (so its parents connected), or mined.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum NetworkEvidence {
    /// `SEEN_ON_NETWORK` / `SEEN_MULTIPLE_NODES`, or an unconfirmed chain-index
    /// hit.
    Seen,
    /// `MINED`, or a chain-index hit with confirmations.
    Mined,
}

impl NetworkEvidence {
    /// The broadcast-memory status this evidence records.
    pub fn memory_status(self) -> &'static str {
        match self {
            NetworkEvidence::Seen => bsv_wallet_toolbox::BROADCAST_STATUS_SEEN,
            NetworkEvidence::Mined => bsv_wallet_toolbox::BROADCAST_STATUS_MINED,
        }
    }
}

/// What the chain index (WhatsOnChain) answered in the last probe round.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChainIndexAnswer {
    /// It holds the transaction (mempool or chain).
    Present(NetworkEvidence),
    /// It answered 404: not in its mempool, not on chain.
    Absent,
    /// Not asked, unreachable, or no chain index configured.
    Unknown,
}

/// Everything one verification learned, for callers that act on more than
/// the verdict (the broadcast memory, the absence clock).
///
/// Evidence is graded by where it came from. The chain index is the only
/// plane whose answer is chain evidence ([`PresenceReport::chain_index`]);
/// a broadcaster's `SEEN_MULTIPLE_NODES` is that provider's evidence (good
/// for its reduced sends, credited under its name) and nothing more: on
/// 2026-09-02 Arcade reported it two hours later for transactions the chain
/// index never saw.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PresenceReport {
    /// The verdict (`verify`'s answer).
    pub verification: BroadcastVerification,
    /// The best network-level evidence from any source, if any.
    pub evidence: Option<NetworkEvidence>,
    /// The broadcast-memory provider to credit with `evidence`:
    /// [`BROADCAST_PROVIDER_CHAIN`] for a chain-index hit,
    /// [`BROADCAST_PROVIDER_NETWORK`] for a third-party store (a peer node
    /// has it), [`PROVIDER_ARCADE_V2`] when only the Arcade plane reported
    /// it.
    pub evidence_provider: &'static str,
    /// The chain index's own answer.
    pub chain_index: ChainIndexAnswer,
    /// The broadcaster we submitted through reports a fatal verdict
    /// (`REJECTED` / `DOUBLE_SPEND_ATTEMPTED`).
    pub broadcaster_fatal: bool,
    /// In the last probe round the chain index answered absent, the
    /// broadcaster answered (held, seen, absent or fatal) and no third-party
    /// node vouched for the transaction: it is not on the network right
    /// now, whatever the broadcaster says. The reconciler's absence rule
    /// acts on it; it is NOT a verdict by itself.
    pub network_absent: bool,
}

impl PresenceReport {
    /// A report carrying only a verdict (tests, callers without a probe).
    pub fn from_verification(verification: BroadcastVerification) -> Self {
        Self {
            verification,
            evidence: None,
            evidence_provider: BROADCAST_PROVIDER_NETWORK,
            chain_index: ChainIndexAnswer::Unknown,
            broadcaster_fatal: false,
            network_absent: false,
        }
    }
}

/// Presence of a txid according to a single source.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Presence {
    /// The source holds the bytes but has not seen them on the network (an
    /// ARC/Arcade pre-gate status, an orphan-pool hit, a 200 without a
    /// readable status).
    Held,
    /// The source saw the tx on the network (or mined).
    Present(NetworkEvidence),
    /// The broadcaster we submitted through reports `REJECTED` /
    /// `DOUBLE_SPEND_ATTEMPTED`: a definitive negative from the scope that
    /// holds our submission. Counts as its absence vote.
    Fatal,
    /// Source definitively does not have the tx (HTTP 404 from a real handler).
    Absent,
    /// Source could not give a definitive answer (auth error, 5xx, network
    /// error, or a 404 that looks like "no such route" rather than "no such tx").
    Unknown,
}

/// What a source's **absence** (404) answer is worth.
///
/// Presence is trusted from every source; absence is a different question
/// entirely, and the answer depends on *why* that store would be expected to
/// hold the transaction.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AbsenceAuthority {
    /// **Worthless.** A submission-scoped store we did *not* submit to.
    ///
    /// ARC/metamorph instances index what was handed to *them*. They are not
    /// chain indexes: `arc.gorillapool.io` returns 404 for the Bitcoin genesis
    /// coinbase, a transaction with ~960,000 confirmations. A 404 from such a
    /// store tells us only that *it* never received the transaction — which is
    /// the expected answer whenever we broadcast somewhere else. These sources
    /// are kept purely as extra chances to observe presence.
    None,

    /// **Scope-authoritative.** This is the broadcaster we personally submitted
    /// through, so it *must* have a record of our own submission.
    ///
    /// This is the only store whose silence is meaningful immediately rather
    /// than eventually. It is still not sufficient on its own:
    ///   * in Arcade mode the toolbox keeps classic ARC as a failover provider,
    ///     so the transaction may legitimately have gone out through the other
    ///     provider and be unknown to the primary; and
    ///   * a misconfigured base URL turns "no such route" into a 404 that is
    ///     indistinguishable from "no such transaction" at the status-code level
    ///     (Arcade V2 answers `GET /tx/{txid}` with `application/json
    ///     {"error":"transaction not found"}` but answers the *wrong* path
    ///     `GET /v1/tx/{txid}` with `text/plain "404 page not found"`).
    ///
    /// Hence the content-type guard in [`probe`] and the conjunction below.
    Broadcaster,

    /// **Time-authoritative.** An independent chain + mempool index (WhatsOnChain).
    ///
    /// Unlike a metamorph store this really does index the whole chain, so its
    /// 404 is about the transaction and not about scope. Its weakness is
    /// *latency*, not coverage: mempool ingestion lags acceptance. So its
    /// absence counts only from the **final** probe round, after the window in
    /// [`DEFAULT_ATTEMPTS`] has elapsed.
    ChainIndex,
}

/// How a source's 200 body is read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SourceKind {
    /// Arcade V2: `{"txid","txStatus",...}`.
    Arcade,
    /// Classic ARC: `{"txid","txStatus",...}` with ARC's status vocabulary.
    ClassicArc,
    /// WhatsOnChain `/tx/hash/{txid}`: `{"confirmations",...}`.
    ChainIndex,
}

/// Absence votes gathered during one probe round, grouped by authority class.
///
/// A `Rejected` verdict requires the **conjunction**: the plane we submitted
/// through has no record of our submission (or rejected it) *and* an
/// independent chain index still cannot see the transaction after the full
/// window. Either one alone has a mundane innocent explanation (provider
/// failover; indexing lag), and acting on either one alone is precisely what
/// produced four false "funds were NOT sent" reports on transactions that were
/// on chain.
///
/// Consequence, stated honestly: a wallet whose broadcaster cannot be probed
/// (e.g. classic TAAL ARC with no API key, which answers 401 → `Unknown`) can
/// never reach `Rejected`. That is the intended trade. A missed drop is caught
/// downstream (the transaction simply never mines and the reconciler's
/// absence clock retires it), whereas a false `Rejected` reports lost funds
/// that were not lost, which is the more expensive error by far.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct AbsenceVotes {
    /// The broadcaster we submitted through answered 404 (or fatal).
    broadcaster: bool,
    /// An independent chain index answered 404.
    chain_index: bool,
}

impl AbsenceVotes {
    fn record(&mut self, authority: AbsenceAuthority) {
        match authority {
            AbsenceAuthority::Broadcaster => self.broadcaster = true,
            AbsenceAuthority::ChainIndex => self.chain_index = true,
            // A store we did not submit to has no opinion about absence.
            AbsenceAuthority::None => {}
        }
    }

    /// Absence is definitive only when both authority classes agree.
    fn is_definitive(self) -> bool {
        self.broadcaster && self.chain_index
    }
}

/// The broadcast plane the wallet is configured to submit through.
///
/// This mirrors `services_env::services_options_from_env` — the ONE place that
/// decides which broadcaster the wallet uses — so the verifier asks the same
/// endpoint the transaction was actually handed to.
#[derive(Debug, Clone, PartialEq, Eq)]
enum BroadcastPlane {
    /// Arcade V2 (`ARC_MODE=arcade` / `ARCADE=1`).
    ///
    /// Status endpoint is `GET {base}/tx/{txid}` — **no `/v1` prefix**. Verified
    /// two ways: `ArcadeV2Provider::get_tx_status` in `bsv-wallet-toolbox-rs`
    /// builds `format!("{}/tx/{}", self.url, txid)`, and the live endpoint
    /// answers that path with `application/json {"error":"transaction not
    /// found"}` while `/v1/tx/{txid}` answers `text/plain "404 page not found"`
    /// (i.e. the `/v1` path does not exist and its 404 is a routing artifact).
    /// Keyless: Arcade's status read needs no `Authorization` header.
    ArcadeV2 { base: String },
    /// Classic ARC. Status endpoint is `GET {base}/v1/tx/{txid}`, matching
    /// `ArcProvider::get_tx_status` in the toolbox.
    ClassicArc { base: String },
}

impl BroadcastPlane {
    /// Resolve the plane from explicit inputs (pure — unit-testable).
    ///
    /// `arcade_mode` and `arc_url` are read from the same env vars that
    /// `services_env` reads, so the verifier cannot drift from the broadcaster.
    fn resolve(chain: Chain, arcade_mode: bool, arc_url: Option<String>) -> Self {
        let arc_url = arc_url
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty());
        if arcade_mode {
            BroadcastPlane::ArcadeV2 {
                base: normalize_base(&arc_url.unwrap_or_else(|| ARCADE_V2_MAINNET.to_string())),
            }
        } else {
            BroadcastPlane::ClassicArc {
                base: normalize_base(&arc_url.unwrap_or_else(|| taal_arc_url(chain).to_string())),
            }
        }
    }

    fn from_env(chain: Chain) -> Self {
        Self::resolve(
            chain,
            crate::services_env::arcade_mode_enabled(),
            std::env::var("ARC_URL").ok(),
        )
    }

    fn base(&self) -> &str {
        match self {
            BroadcastPlane::ArcadeV2 { base } | BroadcastPlane::ClassicArc { base } => base,
        }
    }

    fn name(&self) -> &'static str {
        match self {
            BroadcastPlane::ArcadeV2 { .. } => "broadcaster(arcade-v2)",
            BroadcastPlane::ClassicArc { .. } => "broadcaster(arc)",
        }
    }

    fn kind(&self) -> SourceKind {
        match self {
            BroadcastPlane::ArcadeV2 { .. } => SourceKind::Arcade,
            BroadcastPlane::ClassicArc { .. } => SourceKind::ClassicArc,
        }
    }

    /// URL template with the literal `{txid}` placeholder.
    fn status_template(&self) -> String {
        match self {
            // Arcade V2: `/tx/{txid}`. `/v1/tx/{txid}` is NOT a route there.
            BroadcastPlane::ArcadeV2 { base } => format!("{base}/tx/{{txid}}"),
            // Classic ARC: `/v1/tx/{txid}`.
            BroadcastPlane::ClassicArc { base } => format!("{base}/v1/tx/{{txid}}"),
        }
    }
}

/// A network endpoint we can ask "do you know this txid?".
#[derive(Clone, Debug)]
struct StatusSource {
    /// Human-readable name (diagnostics only).
    name: &'static str,
    /// URL template containing the literal `{txid}` placeholder.
    url_template: String,
    /// Full `Authorization` header value, if the endpoint needs one.
    auth: Option<String>,
    /// What this source's 404 is worth. See [`AbsenceAuthority`].
    absence: AbsenceAuthority,
    /// How its 200 body is read. See [`SourceKind`].
    kind: SourceKind,
}

/// Build the ordered source list for a plane (pure — unit-testable).
///
/// Ordering is load-bearing: **index 0 is always the broadcaster we submitted
/// through**, because it is both the fastest and the most authoritative answer
/// available, and `verify` returns on the first presence.
fn build_sources(
    chain: Chain,
    plane: &BroadcastPlane,
    taal_key: Option<String>,
) -> Vec<StatusSource> {
    let mut sources = vec![StatusSource {
        name: plane.name(),
        url_template: plane.status_template(),
        // Arcade's status read is keyless; classic ARC (TAAL) wants the key.
        auth: match plane {
            BroadcastPlane::ArcadeV2 { .. } => None,
            BroadcastPlane::ClassicArc { .. } => taal_key.clone(),
        },
        absence: AbsenceAuthority::Broadcaster,
        kind: plane.kind(),
    }];

    // The independent chain + mempool index. Keyless, reliable 200/404, and the
    // only source here that indexes the chain rather than its own inbox.
    sources.push(StatusSource {
        name: "whatsonchain",
        url_template: format!("{}/tx/hash/{{txid}}", woc_base(chain)),
        auth: None,
        absence: AbsenceAuthority::ChainIndex,
        kind: SourceKind::ChainIndex,
    });

    // Third-party ARC stores: extra chances to observe presence, never a vote
    // for absence (see AbsenceAuthority::None). Skipped when they *are* the
    // broadcaster — that row is already at index 0 with real authority.
    if let Some(gp) = gorillapool_arc_url(chain) {
        if normalize_base(gp) != plane.base() {
            sources.push(StatusSource {
                name: "arc-gorillapool",
                url_template: format!("{gp}/v1/tx/{{txid}}"),
                auth: None,
                absence: AbsenceAuthority::None,
                kind: SourceKind::ClassicArc,
            });
        }
    }
    // TAAL only when we hold a key — keyless it answers 401 (`Unknown`), which
    // is pure latency for zero information.
    if let Some(key) = taal_key {
        let taal = taal_arc_url(chain);
        if normalize_base(taal) != plane.base() {
            sources.push(StatusSource {
                name: "arc-taal",
                url_template: format!("{taal}/v1/tx/{{txid}}"),
                auth: Some(key),
                absence: AbsenceAuthority::None,
                kind: SourceKind::ClassicArc,
            });
        }
    }

    sources
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
    /// Build a verifier for `chain`, reading the broadcast plane and optional
    /// overrides from the env:
    /// - `ARC_MODE=arcade` / `ARCADE=1` + `ARC_URL` select the plane probed first.
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

        let plane = BroadcastPlane::from_env(chain);
        tracing::debug!(plane = ?plane, "broadcast verifier plane");

        Self {
            client: Client::new(),
            sources: build_sources(chain, &plane, taal_key),
            attempts,
            delay: Duration::from_millis(delay_ms),
            enabled,
        }
    }

    /// Wall-clock ceiling for the absence determination. A source that hangs
    /// must not be able to stretch the window without bound, so rounds stop once
    /// the nominal window (plus one probe timeout of slack) has elapsed.
    /// ONE probe pass over every source (no retry window) — the verdict the
    /// abandoned-tx reconcile needs (2026-08-29, THE RELEASE RULE): a
    /// transaction is abandoned ONLY on DEFINITIVE absence (the broadcaster
    /// it was submitted to answers a JSON 404 AND the chain index answers
    /// 404, with no other source holding it). A lone index miss is
    /// `Inconclusive` and must keep the tx: a fresh Arcade/GorillaPool-only
    /// tx is a WoC 404 for minutes while a peer's orphan pool still holds it.
    /// Honours `BSV_WALLET_SKIP_BROADCAST_VERIFY` like `from_env` — under it
    /// every verdict is `Inconclusive`, so nothing is ever abandoned (the
    /// fail-safe direction).
    pub fn single_pass(chain: Chain) -> Self {
        let mut v = Self::from_env(chain);
        v.attempts = 1;
        v.delay = Duration::ZERO;
        v
    }

    fn absence_window(&self) -> Duration {
        (1..self.attempts)
            .map(|round| self.delay_before_round(round))
            .sum::<Duration>()
            + PROBE_TIMEOUT
    }

    /// The pause before probe round `round` (1-based; round 0 is immediate):
    /// [`INITIAL_DELAY_MS`] doubling each round, capped at the configured
    /// delay (`BSV_WALLET_BROADCAST_VERIFY_DELAY_MS`, default
    /// [`DEFAULT_DELAY_MS`]). A cap below the initial delay simply flattens the
    /// schedule to the cap.
    fn delay_before_round(&self, round: u32) -> Duration {
        let exponent = round.saturating_sub(1).min(16);
        let grown = Duration::from_millis(INITIAL_DELAY_MS.saturating_mul(1u64 << exponent));
        grown.min(self.delay)
    }

    /// Probe the network for `txid`, returning as soon as any source reports it
    /// present, otherwise after the full probe window.
    pub async fn verify(&self, txid: &str) -> BroadcastVerification {
        self.verify_report(txid).await.verification
    }

    /// [`BroadcastVerifier::verify`] with everything the probes learned: the
    /// network evidence (and which plane gave it), the chain index's own
    /// answer, a fatal verdict from the broadcaster, and whether the
    /// transaction is absent from the network right now.
    ///
    /// A chain-index hit ends the verification at once (chain evidence
    /// settles everything). A round in which a store holds or has seen the
    /// tx ends the verification too (the verdict is `Confirmed` and cannot
    /// become `Rejected`), but only after the chain index has been asked in
    /// that round: a broadcaster's `SEEN` never stands in for the chain
    /// index. Absence keeps probing until the window ends.
    pub async fn verify_report(&self, txid: &str) -> PresenceReport {
        let mut report = PresenceReport::from_verification(BroadcastVerification::Inconclusive);
        if !self.enabled || self.sources.is_empty() {
            return report;
        }

        let deadline = Instant::now() + self.absence_window();
        // The LAST COMPLETED round decides. Using the last round (rather than
        // any round) is what makes the chain-index vote time-authoritative: its
        // silence only counts once the indexing window has actually elapsed.
        let mut last: Option<RoundResult> = None;

        for attempt in 0..self.attempts {
            let mut round = RoundResult::default();
            for src in &self.sources {
                match probe(&self.client, src, txid).await {
                    Presence::Present(evidence) => {
                        if src.kind == SourceKind::ChainIndex {
                            // Chain evidence: the answer for everyone.
                            report.verification = BroadcastVerification::Confirmed;
                            report.evidence = Some(evidence);
                            report.evidence_provider = BROADCAST_PROVIDER_CHAIN;
                            report.chain_index = ChainIndexAnswer::Present(evidence);
                            return report;
                        }
                        if src.absence == AbsenceAuthority::Broadcaster {
                            round.broadcaster_answered = true;
                            let provider = if src.kind == SourceKind::Arcade {
                                PROVIDER_ARCADE_V2
                            } else {
                                BROADCAST_PROVIDER_NETWORK
                            };
                            round.broadcaster_evidence = Some((evidence, provider));
                        } else {
                            // A peer node we did not submit to holds it as a
                            // non-orphan: the network has it.
                            round.third_party_evidence = Some(evidence);
                        }
                    }
                    Presence::Held => {
                        round.held = true;
                        if src.absence == AbsenceAuthority::Broadcaster {
                            round.broadcaster_answered = true;
                        }
                    }
                    Presence::Fatal => {
                        round.fatal = true;
                        round.broadcaster_answered = true;
                        round.votes.record(src.absence);
                    }
                    Presence::Absent => {
                        if src.absence == AbsenceAuthority::Broadcaster {
                            round.broadcaster_answered = true;
                        }
                        round.votes.record(src.absence);
                    }
                    Presence::Unknown => {}
                }
            }
            let settled = round.held
                || round.broadcaster_evidence.is_some()
                || round.third_party_evidence.is_some();
            last = Some(round);

            // A store holding (or having seen) the tx settles the verdict
            // (Confirmed): the window exists to give absence time to become
            // definitive, and nothing about a held transaction is absent.
            if settled {
                break;
            }

            if attempt + 1 < self.attempts {
                if Instant::now() >= deadline {
                    // Slow sources already consumed the window; further rounds
                    // would only extend the caller's wait, not the evidence.
                    break;
                }
                tokio::time::sleep(self.delay_before_round(attempt + 1)).await;
            }
        }

        if let Some(round) = last {
            report.broadcaster_fatal = round.fatal;
            report.chain_index = if round.votes.chain_index {
                ChainIndexAnswer::Absent
            } else {
                ChainIndexAnswer::Unknown
            };
            // A peer node's evidence is more independent than the
            // broadcaster's own: prefer it, and let it block the absence.
            if let Some(evidence) = round.third_party_evidence {
                report.evidence = Some(evidence);
                report.evidence_provider = BROADCAST_PROVIDER_NETWORK;
            } else if let Some((evidence, provider)) = round.broadcaster_evidence {
                report.evidence = Some(evidence);
                report.evidence_provider = provider;
            }
            report.network_absent = round.votes.chain_index
                && round.broadcaster_answered
                && round.third_party_evidence.is_none();
            let held = round.held
                || round.broadcaster_evidence.is_some()
                || round.third_party_evidence.is_some();
            report.verification = if held {
                BroadcastVerification::Confirmed
            } else if round.votes.is_definitive() {
                BroadcastVerification::Rejected
            } else {
                BroadcastVerification::Inconclusive
            };
        }
        report
    }

    /// A verifier over an explicit broadcaster and chain index (tests and
    /// tools; the binary reaches it through the library): one round, no
    /// delay. `arcade` selects the Arcade status
    /// path (`{base}/tx/{txid}`) and body vocabulary; classic ARC uses
    /// `{base}/v1/tx/{txid}`. The chain index is probed at
    /// `{base}/tx/hash/{txid}`.
    #[allow(dead_code)]
    pub fn explicit(arcade: bool, broadcaster_base: &str, chain_index_base: Option<&str>) -> Self {
        let plane = if arcade {
            BroadcastPlane::ArcadeV2 {
                base: normalize_base(broadcaster_base),
            }
        } else {
            BroadcastPlane::ClassicArc {
                base: normalize_base(broadcaster_base),
            }
        };
        let mut sources = vec![StatusSource {
            name: plane.name(),
            url_template: plane.status_template(),
            auth: None,
            absence: AbsenceAuthority::Broadcaster,
            kind: plane.kind(),
        }];
        if let Some(base) = chain_index_base {
            sources.push(StatusSource {
                name: "chain-index",
                url_template: format!("{}/tx/hash/{{txid}}", normalize_base(base)),
                auth: None,
                absence: AbsenceAuthority::ChainIndex,
                kind: SourceKind::ChainIndex,
            });
        }
        Self {
            client: Client::new(),
            sources,
            attempts: 1,
            delay: Duration::ZERO,
            enabled: true,
        }
    }
}

/// What one probe round learned when the chain index did not hold the tx.
#[derive(Debug, Clone, Copy, Default)]
struct RoundResult {
    votes: AbsenceVotes,
    /// Some source holds the tx (no network evidence).
    held: bool,
    /// The broadcaster we submitted through answered (held, seen, absent or
    /// fatal).
    broadcaster_answered: bool,
    /// The broadcaster reported a fatal verdict.
    fatal: bool,
    /// The broadcaster reported network-level presence (its plane's word,
    /// with the provider to credit).
    broadcaster_evidence: Option<(NetworkEvidence, &'static str)>,
    /// A store we did not submit to reported network-level presence.
    third_party_evidence: Option<NetworkEvidence>,
}

/// Read a 200 body according to the source kind.
fn presence_of_body(src: &StatusSource, body: &str) -> Presence {
    let json: Option<serde_json::Value> = serde_json::from_str(body).ok();
    match src.kind {
        SourceKind::ChainIndex => {
            let confirmations = json
                .as_ref()
                .and_then(|v| v.get("confirmations"))
                .and_then(|c| c.as_i64())
                .unwrap_or(0);
            if confirmations >= 1 {
                Presence::Present(NetworkEvidence::Mined)
            } else {
                Presence::Present(NetworkEvidence::Seen)
            }
        }
        SourceKind::Arcade | SourceKind::ClassicArc => {
            let Some(tx_status) = json
                .as_ref()
                .and_then(|v| v.get("txStatus"))
                .and_then(|s| s.as_str())
            else {
                return Presence::Held;
            };
            let status = match src.kind {
                SourceKind::Arcade => BroadcastStatus::from_arcade_status(tx_status),
                _ => BroadcastStatus::from_arc_status(tx_status),
            };
            match status {
                BroadcastStatus::Seen => Presence::Present(NetworkEvidence::Seen),
                BroadcastStatus::Mined => Presence::Present(NetworkEvidence::Mined),
                BroadcastStatus::Rejected => {
                    if src.absence == AbsenceAuthority::Broadcaster {
                        Presence::Fatal
                    } else {
                        // A store we did not submit to rejecting a copy it
                        // was handed by someone says nothing about ours.
                        Presence::Unknown
                    }
                }
                BroadcastStatus::Accepted | BroadcastStatus::Unknown => Presence::Held,
            }
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
        Ok(resp) => {
            let status = resp.status().as_u16();
            match status {
                200 => {
                    let body = resp.text().await.unwrap_or_default();
                    let presence = presence_of_body(src, &body);
                    tracing::debug!(source = src.name, ?presence, "broadcast probe");
                    presence
                }
                404 => {
                    // A 404 has two very different meanings: "I have no such
                    // transaction" (a real answer from the ARC/Arcade handler,
                    // always a JSON problem document) and "I have no such route"
                    // (a misconfigured base URL — Go/edge routers answer
                    // `text/plain "404 page not found"`). Only the former is
                    // evidence, and only for a source whose absence we would act
                    // on. Downgrading the routing artifact to `Unknown` keeps a
                    // typo in `ARC_URL` from being reported as lost funds.
                    if src.absence == AbsenceAuthority::Broadcaster && !is_json(&resp) {
                        tracing::debug!(
                            source = src.name,
                            url = %url,
                            "broadcaster 404 is not a JSON tx-status body — treating as \
                             route-not-found (check ARC_URL / path shape), not absence"
                        );
                        return Presence::Unknown;
                    }
                    Presence::Absent
                }
                other => {
                    tracing::debug!(
                        source = src.name,
                        status = other,
                        "broadcast probe inconclusive"
                    );
                    Presence::Unknown
                }
            }
        }
        Err(e) => {
            tracing::debug!(source = src.name, error = %e, "broadcast probe request failed");
            Presence::Unknown
        }
    }
}

/// Whether a response carries a JSON body (the shape every ARC/Arcade status
/// handler returns, including for "transaction not found").
fn is_json(resp: &reqwest::Response) -> bool {
    resp.headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .map(|ct| ct.to_ascii_lowercase().contains("json"))
        .unwrap_or(false)
}

fn normalize_base(url: &str) -> String {
    url.trim().trim_end_matches('/').to_string()
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

    // ---- synthetic values only (never a real txid / URL from any wallet) ----
    const TXID: &str = "0000000000000000000000000000000000000000000000000000000000000001";
    const SYNTHETIC_ARCADE: &str = "https://arcade.invalid";
    const SYNTHETIC_ARC: &str = "https://arc.invalid";
    const SYNTHETIC_KEY: &str = "test-key-not-a-real-credential";

    // =====================================================================
    // Source selection: which plane do we ask, and with what path shape?
    // =====================================================================

    /// THE RELEASE RULE's verdict source: one attempt, no retry window, and
    /// with probing disabled every verdict is Inconclusive — a sweep that
    /// cannot look can never abandon anything.
    #[tokio::test]
    async fn single_pass_is_one_attempt_and_disabled_means_inconclusive() {
        let v = BroadcastVerifier::single_pass(Chain::Main);
        assert_eq!(v.attempts, 1);
        assert_eq!(v.delay, Duration::ZERO);
        let off = BroadcastVerifier {
            enabled: false,
            ..v
        };
        assert_eq!(
            off.verify(&"cd".repeat(32)).await,
            BroadcastVerification::Inconclusive
        );
    }

    #[test]
    fn arcade_plane_uses_bare_tx_path_not_v1() {
        // Arcade V2's status route is `/tx/{txid}`. `/v1/tx/{txid}` is not a
        // route on Arcade at all (it answers with the router's text/plain 404),
        // which would have made every Arcade tx look "absent".
        let plane = BroadcastPlane::resolve(
            Chain::Main,
            /* arcade_mode */ true,
            Some(SYNTHETIC_ARCADE.to_string()),
        );
        assert_eq!(
            plane.status_template(),
            format!("{SYNTHETIC_ARCADE}/tx/{{txid}}")
        );
        assert!(
            !plane.status_template().contains("/v1/"),
            "Arcade V2 must NOT be probed on the classic ARC /v1 path"
        );
        assert_eq!(plane.kind(), SourceKind::Arcade);
    }

    #[test]
    fn classic_arc_plane_uses_v1_tx_path() {
        let plane = BroadcastPlane::resolve(
            Chain::Main,
            /* arcade_mode */ false,
            Some(SYNTHETIC_ARC.to_string()),
        );
        assert_eq!(
            plane.status_template(),
            format!("{SYNTHETIC_ARC}/v1/tx/{{txid}}")
        );
        assert_eq!(plane.kind(), SourceKind::ClassicArc);
    }

    #[test]
    fn arcade_mode_defaults_to_the_arcade_endpoint_when_arc_url_is_unset() {
        let plane = BroadcastPlane::resolve(Chain::Main, true, None);
        assert_eq!(plane.base(), ARCADE_V2_MAINNET.trim_end_matches('/'));
    }

    #[test]
    fn classic_mode_defaults_to_taal_and_respects_chain() {
        assert_eq!(
            BroadcastPlane::resolve(Chain::Main, false, None).base(),
            "https://arc.taal.com"
        );
        assert_eq!(
            BroadcastPlane::resolve(Chain::Test, false, None).base(),
            "https://arc-test.taal.com"
        );
    }

    #[test]
    fn empty_arc_url_falls_back_to_the_default_rather_than_an_empty_base() {
        let plane = BroadcastPlane::resolve(Chain::Main, true, Some("   ".to_string()));
        assert_eq!(plane.base(), ARCADE_V2_MAINNET.trim_end_matches('/'));
    }

    #[test]
    fn trailing_slash_in_arc_url_does_not_produce_a_double_slash() {
        let plane = BroadcastPlane::resolve(
            Chain::Main,
            true,
            Some(format!("{SYNTHETIC_ARCADE}/").to_string()),
        );
        assert_eq!(
            plane.status_template(),
            format!("{SYNTHETIC_ARCADE}/tx/{{txid}}")
        );
    }

    #[test]
    fn the_broadcaster_we_used_is_always_the_first_source_consulted() {
        // This is the whole point of the fix: the plane that actually holds the
        // answer must be asked FIRST, in both modes.
        for plane in [
            BroadcastPlane::resolve(Chain::Main, true, Some(SYNTHETIC_ARCADE.to_string())),
            BroadcastPlane::resolve(Chain::Main, false, Some(SYNTHETIC_ARC.to_string())),
        ] {
            let sources = build_sources(Chain::Main, &plane, None);
            assert_eq!(sources[0].absence, AbsenceAuthority::Broadcaster);
            assert_eq!(sources[0].kind, plane.kind());
            assert!(
                sources[0].url_template.starts_with(plane.base()),
                "source 0 ({}) must be the configured broadcaster {}",
                sources[0].url_template,
                plane.base()
            );
        }
    }

    #[test]
    fn arcade_broadcaster_probe_is_keyless_even_when_a_taal_key_exists() {
        let plane = BroadcastPlane::resolve(Chain::Main, true, Some(SYNTHETIC_ARCADE.to_string()));
        let sources = build_sources(Chain::Main, &plane, Some(SYNTHETIC_KEY.to_string()));
        assert!(sources[0].auth.is_none());
    }

    #[test]
    fn classic_broadcaster_probe_carries_the_taal_key_when_present() {
        let plane = BroadcastPlane::resolve(Chain::Main, false, None);
        let sources = build_sources(Chain::Main, &plane, Some(SYNTHETIC_KEY.to_string()));
        assert_eq!(sources[0].auth.as_deref(), Some(SYNTHETIC_KEY));
    }

    #[test]
    fn keyless_taal_is_not_probed_at_all() {
        // Without a key TAAL answers 401 → Unknown: pure latency, zero signal.
        let plane = BroadcastPlane::resolve(Chain::Main, true, Some(SYNTHETIC_ARCADE.to_string()));
        let sources = build_sources(Chain::Main, &plane, None);
        assert!(!sources.iter().any(|s| s.name == "arc-taal"));
    }

    #[test]
    fn a_store_is_never_listed_twice_when_it_is_also_the_broadcaster() {
        // Broadcasting through GorillaPool in classic mode must not add a second
        // (presence-only) GorillaPool row.
        let plane = BroadcastPlane::resolve(
            Chain::Main,
            false,
            Some("https://arc.gorillapool.io".to_string()),
        );
        let sources = build_sources(Chain::Main, &plane, None);
        let gp_rows: Vec<_> = sources
            .iter()
            .filter(|s| s.url_template.contains("arc.gorillapool.io"))
            .collect();
        assert_eq!(gp_rows.len(), 1);
        assert_eq!(gp_rows[0].absence, AbsenceAuthority::Broadcaster);
    }

    // =====================================================================
    // Absence authority: whose 404 may be believed, and when?
    // =====================================================================

    #[test]
    fn a_third_party_arc_store_is_never_authoritative_for_absence() {
        // arc.gorillapool.io 404s for the genesis coinbase (~960k confirmations).
        // It is a submission-scoped metamorph store, not a chain index: when we
        // broadcast through Arcade, its 404 is the EXPECTED answer and carries
        // no information. Marking it authoritative caused false "funds not sent".
        let plane = BroadcastPlane::resolve(Chain::Main, true, Some(SYNTHETIC_ARCADE.to_string()));
        let sources = build_sources(Chain::Main, &plane, Some(SYNTHETIC_KEY.to_string()));
        for s in sources.iter().filter(|s| s.name.starts_with("arc-")) {
            assert_eq!(
                s.absence,
                AbsenceAuthority::None,
                "{} is not the broadcaster; its absence must carry no weight",
                s.name
            );
        }
    }

    #[test]
    fn whatsonchain_is_the_chain_index_authority() {
        let plane = BroadcastPlane::resolve(Chain::Main, true, Some(SYNTHETIC_ARCADE.to_string()));
        let sources = build_sources(Chain::Main, &plane, None);
        let woc = sources.iter().find(|s| s.name == "whatsonchain").unwrap();
        assert_eq!(woc.absence, AbsenceAuthority::ChainIndex);
        assert_eq!(woc.kind, SourceKind::ChainIndex);
    }

    #[test]
    fn absence_is_definitive_only_when_broadcaster_and_chain_index_agree() {
        let mut none = AbsenceVotes::default();
        assert!(!none.is_definitive(), "no votes is not evidence");

        // A store we did not submit to voting absent changes nothing.
        none.record(AbsenceAuthority::None);
        assert!(!none.is_definitive());

        let mut broadcaster_only = AbsenceVotes::default();
        broadcaster_only.record(AbsenceAuthority::Broadcaster);
        assert!(
            !broadcaster_only.is_definitive(),
            "the primary may 404 while the tx went out through the failover provider"
        );

        let mut index_only = AbsenceVotes::default();
        index_only.record(AbsenceAuthority::ChainIndex);
        assert!(
            !index_only.is_definitive(),
            "a chain index can simply be lagging its mempool ingestion"
        );

        let mut both = AbsenceVotes::default();
        both.record(AbsenceAuthority::Broadcaster);
        both.record(AbsenceAuthority::ChainIndex);
        assert!(both.is_definitive());
    }

    // =====================================================================
    // Reading a 200: held, seen, mined, fatal.
    // =====================================================================

    fn src_of(kind: SourceKind, absence: AbsenceAuthority) -> StatusSource {
        StatusSource {
            name: "test",
            url_template: "http://127.0.0.1:1/tx/{txid}".to_string(),
            auth: None,
            absence,
            kind,
        }
    }

    #[test]
    fn a_200_body_is_read_by_source_kind() {
        let arcade = src_of(SourceKind::Arcade, AbsenceAuthority::Broadcaster);
        assert_eq!(
            presence_of_body(&arcade, r#"{"txid":"x","txStatus":"RECEIVED"}"#),
            Presence::Held,
            "a pre-gate status is held, not network evidence"
        );
        assert_eq!(
            presence_of_body(&arcade, r#"{"txid":"x","txStatus":"ACCEPTED_BY_NETWORK"}"#),
            Presence::Held
        );
        assert_eq!(
            presence_of_body(&arcade, r#"{"txid":"x","txStatus":"SEEN_ON_NETWORK"}"#),
            Presence::Present(NetworkEvidence::Seen)
        );
        assert_eq!(
            presence_of_body(&arcade, r#"{"txid":"x","txStatus":"MINED"}"#),
            Presence::Present(NetworkEvidence::Mined)
        );
        assert_eq!(
            presence_of_body(&arcade, r#"{"txid":"x","txStatus":"REJECTED"}"#),
            Presence::Fatal
        );
        assert_eq!(
            presence_of_body(&arcade, "{}"),
            Presence::Held,
            "a 200 without a readable status still means the store holds it"
        );
        assert_eq!(presence_of_body(&arcade, "not json"), Presence::Held);

        let arc = src_of(SourceKind::ClassicArc, AbsenceAuthority::None);
        assert_eq!(
            presence_of_body(&arc, r#"{"txStatus":"SEEN_IN_ORPHAN_MEMPOOL"}"#),
            Presence::Held,
            "an orphan-pool hit is held: the node lacks the parent"
        );
        assert_eq!(
            presence_of_body(&arc, r#"{"txStatus":"SEEN_ON_NETWORK"}"#),
            Presence::Present(NetworkEvidence::Seen)
        );
        assert_eq!(
            presence_of_body(&arc, r#"{"txStatus":"REJECTED"}"#),
            Presence::Unknown,
            "a third-party rejection of somebody's copy is no vote"
        );

        let woc = src_of(SourceKind::ChainIndex, AbsenceAuthority::ChainIndex);
        assert_eq!(
            presence_of_body(&woc, r#"{"txid":"x","confirmations":0}"#),
            Presence::Present(NetworkEvidence::Seen)
        );
        assert_eq!(
            presence_of_body(&woc, r#"{"txid":"x","confirmations":3}"#),
            Presence::Present(NetworkEvidence::Mined)
        );
        assert_eq!(
            presence_of_body(&woc, r#"{"txid":"x"}"#),
            Presence::Present(NetworkEvidence::Seen)
        );
    }

    // =====================================================================
    // End-to-end verdicts against local mock sources.
    // =====================================================================

    /// Local mock answering every status path (`/tx/{txid}` and `/v1/tx/{txid}`)
    /// with `code`. Returns the base URL (`http://127.0.0.1:PORT`).
    async fn mock_status_server(code: StatusCode) -> String {
        mock_status_server_full(code, Some("application/json"), "{}").await
    }

    /// As [`mock_status_server`], with an explicit `Content-Type` (or none).
    async fn mock_status_server_ct(code: StatusCode, content_type: Option<&'static str>) -> String {
        mock_status_server_full(code, content_type, "{}").await
    }

    /// A 200 with this JSON body on every status path.
    async fn mock_status_server_body(body: &'static str) -> String {
        mock_status_server_full(StatusCode::OK, Some("application/json"), body).await
    }

    async fn mock_status_server_full(
        code: StatusCode,
        content_type: Option<&'static str>,
        body: &'static str,
    ) -> String {
        let handler = move || async move {
            let mut resp = axum::response::Response::new(axum::body::Body::from(body));
            *resp.status_mut() = code;
            if let Some(ct) = content_type {
                resp.headers_mut()
                    .insert(reqwest::header::CONTENT_TYPE.as_str(), ct.parse().unwrap());
            } else {
                resp.headers_mut()
                    .remove(reqwest::header::CONTENT_TYPE.as_str());
            }
            resp
        };
        let app = Router::new()
            .route("/tx/{txid}", get(handler))
            .route("/v1/tx/{txid}", get(handler))
            .route("/tx/hash/{txid}", get(handler));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr: SocketAddr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(listener, app).await.ok();
        });
        format!("http://{}", addr)
    }

    fn source(name: &'static str, base: &str, absence: AbsenceAuthority) -> StatusSource {
        source_kind(name, base, absence, SourceKind::ClassicArc)
    }

    fn source_kind(
        name: &'static str,
        base: &str,
        absence: AbsenceAuthority,
        kind: SourceKind,
    ) -> StatusSource {
        StatusSource {
            name,
            url_template: format!("{base}/tx/{{txid}}"),
            auth: None,
            absence,
            kind,
        }
    }

    /// Verifier over an explicit source list (fast: 2 rounds, no delay).
    fn verifier_with(sources: Vec<StatusSource>) -> BroadcastVerifier {
        BroadcastVerifier {
            client: Client::new(),
            sources,
            attempts: 2,
            delay: Duration::from_millis(0),
            enabled: true,
        }
    }

    #[tokio::test]
    async fn rejected_when_broadcaster_and_chain_index_both_report_absent() {
        // The original purpose of the module (ARC 465 fee-too-low) still fires:
        // the plane we submitted to has no record AND the chain index cannot see
        // it after the window.
        let base = mock_status_server(StatusCode::NOT_FOUND).await;
        let verifier = verifier_with(vec![
            source("broadcaster", &base, AbsenceAuthority::Broadcaster),
            source("chain-index", &base, AbsenceAuthority::ChainIndex),
        ]);

        let report = verifier.verify_report(TXID).await;
        assert_eq!(report.verification, BroadcastVerification::Rejected);
        assert!(report.network_absent);
        assert!(!report.broadcaster_fatal);
        assert_eq!(report.evidence, None);
        assert!(
            report.verification.into_send_result(TXID).is_err(),
            "a Rejected verification must map to Err so the send fails loudly"
        );
    }

    #[tokio::test]
    async fn the_false_negative_that_motivated_this_fix_is_now_inconclusive() {
        // Exactly the observed regression: the broadcaster we used is never
        // asked (or is unreachable), a third-party ARC store 404s because we
        // never submitted to it, and the chain index has not indexed the mempool
        // entry yet. Old code: Rejected ("the funds were NOT sent"). Every such
        // transaction was actually on chain.
        let absent = mock_status_server(StatusCode::NOT_FOUND).await;
        let verifier = verifier_with(vec![
            // Broadcaster unreachable → Unknown, not a vote.
            source(
                "broadcaster",
                "http://127.0.0.1:1",
                AbsenceAuthority::Broadcaster,
            ),
            source("chain-index", &absent, AbsenceAuthority::ChainIndex),
            source("arc-third-party", &absent, AbsenceAuthority::None),
        ]);
        let report = verifier.verify_report(TXID).await;
        assert_eq!(report.verification, BroadcastVerification::Inconclusive);
        assert!(
            !report.network_absent,
            "the absence clock does not run while the broadcaster is unreachable"
        );
    }

    #[tokio::test]
    async fn third_party_absence_alone_never_rejects() {
        let base = mock_status_server(StatusCode::NOT_FOUND).await;
        let verifier = verifier_with(vec![
            source("arc-third-party-a", &base, AbsenceAuthority::None),
            source("arc-third-party-b", &base, AbsenceAuthority::None),
        ]);
        assert_eq!(
            verifier.verify(TXID).await,
            BroadcastVerification::Inconclusive
        );
    }

    #[tokio::test]
    async fn broadcaster_absence_alone_never_rejects() {
        // The toolbox keeps a failover provider behind the primary, so the tx
        // may legitimately have gone out through the other plane.
        let absent = mock_status_server(StatusCode::NOT_FOUND).await;
        let verifier = verifier_with(vec![
            source("broadcaster", &absent, AbsenceAuthority::Broadcaster),
            // Chain index unreachable → Unknown.
            source(
                "chain-index",
                "http://127.0.0.1:1",
                AbsenceAuthority::ChainIndex,
            ),
        ]);
        assert_eq!(
            verifier.verify(TXID).await,
            BroadcastVerification::Inconclusive
        );
    }

    #[tokio::test]
    async fn chain_index_absence_alone_never_rejects() {
        let absent = mock_status_server(StatusCode::NOT_FOUND).await;
        let verifier = verifier_with(vec![
            // Broadcaster answers 401 (keyless TAAL) → Unknown.
            source("broadcaster", &absent, AbsenceAuthority::Broadcaster),
            source("chain-index", &absent, AbsenceAuthority::ChainIndex),
        ]);
        // Sanity: with both absent it WOULD reject...
        assert_eq!(verifier.verify(TXID).await, BroadcastVerification::Rejected);

        // ...but with the broadcaster unreachable, the chain index alone must not.
        let unauth = mock_status_server(StatusCode::UNAUTHORIZED).await;
        let verifier = verifier_with(vec![
            source("broadcaster", &unauth, AbsenceAuthority::Broadcaster),
            source("chain-index", &absent, AbsenceAuthority::ChainIndex),
        ]);
        assert_eq!(
            verifier.verify(TXID).await,
            BroadcastVerification::Inconclusive
        );
    }

    #[tokio::test]
    async fn presence_from_any_source_confirms_even_when_others_say_absent() {
        // Doctrine: a positive answer may be trusted; an absence may not.
        let present = mock_status_server(StatusCode::OK).await;
        let absent = mock_status_server(StatusCode::NOT_FOUND).await;
        let verifier = verifier_with(vec![
            source("broadcaster", &absent, AbsenceAuthority::Broadcaster),
            source("chain-index", &absent, AbsenceAuthority::ChainIndex),
            source("arc-third-party", &present, AbsenceAuthority::None),
        ]);
        let outcome = verifier.verify(TXID).await;
        assert_eq!(outcome, BroadcastVerification::Confirmed);
        assert!(outcome.into_send_result(TXID).is_ok());
    }

    #[tokio::test]
    async fn confirmed_broadcast_succeeds() {
        let base = mock_status_server(StatusCode::OK).await;
        let verifier = verifier_with(vec![source(
            "broadcaster",
            &base,
            AbsenceAuthority::Broadcaster,
        )]);
        let outcome = verifier.verify(TXID).await;
        assert_eq!(outcome, BroadcastVerification::Confirmed);
        assert!(outcome.into_send_result(TXID).is_ok());
    }

    #[tokio::test]
    async fn unreachable_source_is_inconclusive_not_a_failure() {
        // 503 from every probe → we cannot confirm either way → Inconclusive,
        // which must NOT be a failure (no false negatives when the service is down).
        let base = mock_status_server(StatusCode::SERVICE_UNAVAILABLE).await;
        let verifier = verifier_with(vec![
            source("broadcaster", &base, AbsenceAuthority::Broadcaster),
            source("chain-index", &base, AbsenceAuthority::ChainIndex),
        ]);
        let outcome = verifier.verify(TXID).await;
        assert_eq!(outcome, BroadcastVerification::Inconclusive);
        assert!(outcome.into_send_result(TXID).is_ok());
    }

    #[tokio::test]
    async fn a_routing_404_from_the_broadcaster_is_not_absence() {
        // A wrong base URL / path shape yields `text/plain "404 page not found"`.
        // That must never be read as "the funds were NOT sent".
        let text_404 = mock_status_server_ct(StatusCode::NOT_FOUND, Some("text/plain")).await;
        let json_404 = mock_status_server(StatusCode::NOT_FOUND).await;
        let verifier = verifier_with(vec![
            source("broadcaster", &text_404, AbsenceAuthority::Broadcaster),
            source("chain-index", &json_404, AbsenceAuthority::ChainIndex),
        ]);
        assert_eq!(
            verifier.verify(TXID).await,
            BroadcastVerification::Inconclusive
        );
    }

    #[tokio::test]
    async fn disabled_verifier_is_inconclusive() {
        let base = mock_status_server(StatusCode::NOT_FOUND).await;
        let mut verifier = verifier_with(vec![
            source("broadcaster", &base, AbsenceAuthority::Broadcaster),
            source("chain-index", &base, AbsenceAuthority::ChainIndex),
        ]);
        verifier.enabled = false;
        assert_eq!(
            verifier.verify(TXID).await,
            BroadcastVerification::Inconclusive
        );
    }

    // ---- the 2026-09-02 lesson: a 200 is not the network ---------------------

    #[tokio::test]
    async fn seen_on_network_from_the_arcade_plane_is_network_evidence_for_arcade() {
        let seen = mock_status_server_body(r#"{"txid":"x","txStatus":"SEEN_ON_NETWORK"}"#).await;
        let verifier = verifier_with(vec![source_kind(
            "broadcaster",
            &seen,
            AbsenceAuthority::Broadcaster,
            SourceKind::Arcade,
        )]);
        let report = verifier.verify_report(TXID).await;
        assert_eq!(report.verification, BroadcastVerification::Confirmed);
        assert_eq!(report.evidence, Some(NetworkEvidence::Seen));
        assert_eq!(report.evidence_provider, PROVIDER_ARCADE_V2);
        assert_eq!(report.chain_index, ChainIndexAnswer::Unknown);
        assert!(!report.network_absent && !report.broadcaster_fatal);
    }

    #[tokio::test]
    async fn a_broadcasters_seen_with_a_chain_index_miss_is_network_absent() {
        // The 2026-09-02 phantom roots: Arcade still says SEEN_MULTIPLE_NODES
        // two hours later, the chain index has never seen them. The
        // broadcaster's word is its own evidence (credited to Arcade), not
        // the chain's: the report says absent.
        let seen =
            mock_status_server_body(r#"{"txid":"x","txStatus":"SEEN_MULTIPLE_NODES"}"#).await;
        let absent = mock_status_server(StatusCode::NOT_FOUND).await;
        let verifier = verifier_with(vec![
            source_kind(
                "broadcaster",
                &seen,
                AbsenceAuthority::Broadcaster,
                SourceKind::Arcade,
            ),
            source_kind(
                "chain-index",
                &absent,
                AbsenceAuthority::ChainIndex,
                SourceKind::ChainIndex,
            ),
        ]);
        let report = verifier.verify_report(TXID).await;
        assert_eq!(report.verification, BroadcastVerification::Confirmed);
        assert_eq!(report.evidence, Some(NetworkEvidence::Seen));
        assert_eq!(report.evidence_provider, PROVIDER_ARCADE_V2);
        assert_eq!(report.chain_index, ChainIndexAnswer::Absent);
        assert!(
            report.network_absent,
            "the chain index was asked and said no"
        );
        assert!(!report.broadcaster_fatal);

        // The explicit constructor builds exactly that pair.
        let explicit = BroadcastVerifier::explicit(true, &seen, Some(&absent));
        assert_eq!(explicit.sources.len(), 2);
        assert_eq!(explicit.sources[0].kind, SourceKind::Arcade);
        assert_eq!(explicit.sources[1].kind, SourceKind::ChainIndex);
        let report = explicit.verify_report(TXID).await;
        assert!(report.network_absent);
        assert_eq!(report.chain_index, ChainIndexAnswer::Absent);
    }

    #[tokio::test]
    async fn a_peer_nodes_seen_blocks_the_absence() {
        // A third-party store holding the tx as a non-orphan means a node of
        // the network has it: the chain index is merely lagging.
        let held = mock_status_server_body(r#"{"txid":"x","txStatus":"RECEIVED"}"#).await;
        let absent = mock_status_server(StatusCode::NOT_FOUND).await;
        let peer = mock_status_server_body(r#"{"txid":"x","txStatus":"SEEN_ON_NETWORK"}"#).await;
        let verifier = verifier_with(vec![
            source_kind(
                "broadcaster",
                &held,
                AbsenceAuthority::Broadcaster,
                SourceKind::Arcade,
            ),
            source_kind(
                "chain-index",
                &absent,
                AbsenceAuthority::ChainIndex,
                SourceKind::ChainIndex,
            ),
            source_kind(
                "arc-third-party",
                &peer,
                AbsenceAuthority::None,
                SourceKind::ClassicArc,
            ),
        ]);
        let report = verifier.verify_report(TXID).await;
        assert_eq!(report.verification, BroadcastVerification::Confirmed);
        assert_eq!(report.evidence, Some(NetworkEvidence::Seen));
        assert_eq!(report.evidence_provider, BROADCAST_PROVIDER_NETWORK);
        assert_eq!(report.chain_index, ChainIndexAnswer::Absent);
        assert!(!report.network_absent);
    }

    #[tokio::test]
    async fn a_pre_gate_status_is_held_only_and_the_absence_clock_runs() {
        // The incident shape: Arcade holds the tx (RECEIVED) but no node has
        // seen it and the chain index cannot find it. Not a rejection (the
        // store holds it), no network evidence, and the clock advances.
        let held = mock_status_server_body(r#"{"txid":"x","txStatus":"RECEIVED"}"#).await;
        let absent = mock_status_server(StatusCode::NOT_FOUND).await;
        let verifier = verifier_with(vec![
            source_kind(
                "broadcaster",
                &held,
                AbsenceAuthority::Broadcaster,
                SourceKind::Arcade,
            ),
            source_kind(
                "chain-index",
                &absent,
                AbsenceAuthority::ChainIndex,
                SourceKind::ChainIndex,
            ),
        ]);
        let report = verifier.verify_report(TXID).await;
        assert_eq!(report.verification, BroadcastVerification::Confirmed);
        assert_eq!(report.evidence, None);
        assert!(report.network_absent);
        assert!(!report.broadcaster_fatal);
    }

    #[tokio::test]
    async fn a_fatal_verdict_from_the_broadcaster_with_an_index_miss_is_rejected() {
        let fatal = mock_status_server_body(r#"{"txid":"x","txStatus":"REJECTED"}"#).await;
        let absent = mock_status_server(StatusCode::NOT_FOUND).await;
        let verifier = verifier_with(vec![
            source_kind(
                "broadcaster",
                &fatal,
                AbsenceAuthority::Broadcaster,
                SourceKind::Arcade,
            ),
            source_kind(
                "chain-index",
                &absent,
                AbsenceAuthority::ChainIndex,
                SourceKind::ChainIndex,
            ),
        ]);
        let report = verifier.verify_report(TXID).await;
        assert_eq!(report.verification, BroadcastVerification::Rejected);
        assert!(report.broadcaster_fatal);
        assert!(report.network_absent);

        // A fatal verdict alone (index unreachable) is still not definitive.
        let verifier = verifier_with(vec![
            source_kind(
                "broadcaster",
                &fatal,
                AbsenceAuthority::Broadcaster,
                SourceKind::Arcade,
            ),
            source_kind(
                "chain-index",
                "http://127.0.0.1:1",
                AbsenceAuthority::ChainIndex,
                SourceKind::ChainIndex,
            ),
        ]);
        let report = verifier.verify_report(TXID).await;
        assert_eq!(report.verification, BroadcastVerification::Inconclusive);
        assert!(report.broadcaster_fatal);
        assert!(!report.network_absent);
    }

    #[tokio::test]
    async fn a_chain_index_hit_is_network_evidence_for_everyone() {
        let held = mock_status_server_body(r#"{"txid":"x","txStatus":"SENT_TO_NETWORK"}"#).await;
        let mined = mock_status_server_body(r#"{"txid":"x","confirmations":2}"#).await;
        let verifier = verifier_with(vec![
            source_kind(
                "broadcaster",
                &held,
                AbsenceAuthority::Broadcaster,
                SourceKind::Arcade,
            ),
            source_kind(
                "chain-index",
                &mined,
                AbsenceAuthority::ChainIndex,
                SourceKind::ChainIndex,
            ),
        ]);
        let report = verifier.verify_report(TXID).await;
        assert_eq!(report.verification, BroadcastVerification::Confirmed);
        assert_eq!(report.evidence, Some(NetworkEvidence::Mined));
        assert_eq!(report.evidence_provider, BROADCAST_PROVIDER_CHAIN);
        assert_eq!(
            report.chain_index,
            ChainIndexAnswer::Present(NetworkEvidence::Mined)
        );
        assert!(!report.network_absent);
    }

    #[tokio::test]
    async fn a_third_party_rejection_alone_is_inconclusive() {
        let fatal = mock_status_server_body(r#"{"txid":"x","txStatus":"REJECTED"}"#).await;
        let verifier = verifier_with(vec![source_kind(
            "arc-third-party",
            &fatal,
            AbsenceAuthority::None,
            SourceKind::ClassicArc,
        )]);
        let report = verifier.verify_report(TXID).await;
        assert_eq!(report.verification, BroadcastVerification::Inconclusive);
        assert!(!report.broadcaster_fatal);
    }

    #[test]
    fn absence_window_is_bounded_and_reflects_the_configured_rounds() {
        let v = BroadcastVerifier {
            client: Client::new(),
            sources: vec![],
            attempts: DEFAULT_ATTEMPTS,
            delay: Duration::from_millis(DEFAULT_DELAY_MS),
            enabled: true,
        };
        // 13 gaps: 250+500+1000+2000 then 9 × 2.5 s, plus 5 s slack — long
        // enough for a real mempool index to catch up, and hard-bounded so a
        // hung source cannot extend it.
        assert_eq!(
            v.absence_window(),
            Duration::from_millis(26_250) + PROBE_TIMEOUT
        );
    }

    #[test]
    fn probe_schedule_starts_short_grows_and_caps() {
        // A clean tx is usually present within a second: the first re-probes
        // come quickly, then the gaps grow to the cap so the total window stays
        // long enough for a lagging chain index.
        let v = BroadcastVerifier {
            client: Client::new(),
            sources: vec![],
            attempts: DEFAULT_ATTEMPTS,
            delay: Duration::from_millis(DEFAULT_DELAY_MS),
            enabled: true,
        };
        let gaps: Vec<u64> = (1..v.attempts)
            .map(|r| v.delay_before_round(r).as_millis() as u64)
            .collect();
        assert_eq!(
            gaps,
            vec![250, 500, 1000, 2000, 2500, 2500, 2500, 2500, 2500, 2500, 2500, 2500, 2500]
        );
        assert!(gaps.windows(2).all(|w| w[0] <= w[1]), "never shrinks");
        assert!(
            gaps.iter().all(|g| *g <= DEFAULT_DELAY_MS),
            "never exceeds the cap"
        );

        // An env override below the initial delay flattens the schedule.
        let tight = BroadcastVerifier {
            delay: Duration::from_millis(100),
            ..v
        };
        assert!(
            (1..tight.attempts).all(|r| tight.delay_before_round(r) == Duration::from_millis(100))
        );

        // single_pass has no gaps at all.
        let one = BroadcastVerifier::single_pass(Chain::Main);
        assert_eq!(one.absence_window(), PROBE_TIMEOUT);
    }

    #[tokio::test]
    async fn a_present_tx_is_confirmed_on_the_first_probe_without_waiting() {
        // The served handler's ambiguous path and the CLI send bar both call
        // verify inline: presence must be answered by the immediate first
        // round, never after a sleep.
        let present = mock_status_server(StatusCode::OK).await;
        let verifier = BroadcastVerifier {
            client: Client::new(),
            sources: vec![source(
                "broadcaster",
                &present,
                AbsenceAuthority::Broadcaster,
            )],
            attempts: DEFAULT_ATTEMPTS,
            delay: Duration::from_millis(DEFAULT_DELAY_MS),
            enabled: true,
        };
        let started = std::time::Instant::now();
        assert_eq!(
            verifier.verify(TXID).await,
            BroadcastVerification::Confirmed
        );
        assert!(
            started.elapsed() < Duration::from_millis(INITIAL_DELAY_MS),
            "took {:?}",
            started.elapsed()
        );
    }

    #[tokio::test]
    async fn an_absent_tx_is_retried_on_the_growing_schedule() {
        // 4 rounds against an absent broadcaster + index under a 200 ms cap: the
        // 250/500/1000 ms schedule flattens to 3 gaps of 200 ms, so the verdict
        // must arrive after ~600 ms — and only after every round has run.
        let absent = mock_status_server(StatusCode::NOT_FOUND).await;
        let verifier = BroadcastVerifier {
            client: Client::new(),
            sources: vec![
                source("broadcaster", &absent, AbsenceAuthority::Broadcaster),
                source("chain-index", &absent, AbsenceAuthority::ChainIndex),
            ],
            attempts: 4,
            delay: Duration::from_millis(200),
            enabled: true,
        };
        // Schedule under a 200 ms cap: 250→200, 500→200, 1000→200.
        assert!((1..4).all(|r| verifier.delay_before_round(r) == Duration::from_millis(200)));
        let started = std::time::Instant::now();
        assert_eq!(verifier.verify(TXID).await, BroadcastVerification::Rejected);
        let elapsed = started.elapsed();
        assert!(
            elapsed >= Duration::from_millis(600) && elapsed < Duration::from_millis(2_000),
            "took {:?}",
            elapsed
        );
    }
}
