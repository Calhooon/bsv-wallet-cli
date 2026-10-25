//! The CLI as a host of the transaction tracker (`bsv-tracker`).
//!
//! The tracker holds one word per transaction, the evidence behind it, the
//! hints it has heard and the next re-ask. Only headers and proofs change a
//! word; every broadcaster word is a hint that schedules a re-ask. The crate
//! fetches nothing, reads no clock and stores nothing: the host supplies the
//! four things below, runs the re-asks, and keeps the state.
//!
//! # The four host things
//!
//! | the tracker's trait | here | what it is |
//! |---|---|---|
//! | `Headers` | [`HeaderSnapshot`] | the header service's tip and the headers at the heights a pass needs, read once per pass through the wallet's services; a height not read is "no answer", a fault is a fault |
//! | `ProofFetcher` | [`Courier`] | `get_merkle_path` for one named transaction; what it carries is a raw path, checked by the tracker against the snapshot |
//! | `HintSource` | [`MemoryHints`] | the broadcast memory rows of the wallet's storage (what a broadcaster answered, what Arcade pushed over SSE or the webhook), each word fed once |
//! | `Clock` | [`SystemClock`] | seconds since the epoch |
//!
//! The tracker's traits are synchronous and the wallet's transports are
//! not, so a pass reads first (headers, memory rows, proofs for the re-asks
//! that are due) and steps second.
//!
//! # The state, in the CLI's storage
//!
//! One row per tracked transaction in `tracker_states`, in the wallet's own
//! database. A row holds inputs, never a trusted word: whether the host
//! built it, the hints in the order heard, and the raw merkle path. Loading
//! a row replays those inputs, so a stored path is checked again against
//! the active header every time it is read (the tracker has no
//! deserializer for a checked proof, by design). The `word`, `height` and
//! `reask` columns are what the tracker said at the last step, kept for
//! reading and for the indexes; nothing is decided from them.
//!
//! # What a pass asks, and what it never asks
//!
//! A pass ([`tick`]) names transactions; it never walks the chain and
//! never asks a chain index. It loads the rows that are not yet mined, feeds
//! the new hints, ticks the clock, and runs the re-asks that are due: one
//! proof request for one transaction, with a doubling pause between two
//! requests for the same transaction. A word whose root moved
//! (`Reask::Reorg`) is asked ahead of the rest, at most [`REORG_ASKS_PER_PASS`] a pass.
//! Under the daemon, which holds the header service's feed, a mined row is
//! read again only when a chain event names it or a spend names it
//! ([`spend_guard`]): the recheck of a mined word against the headers is
//! the crate's own step, once (bsv-tracker 0.7.0, ruling (5) of 2026-10-20
//! 19:4xZ, bsv-stack-lean `docs/RULINGS-2026-10-12.md`). The spend guard
//! rechecks only a word not marked suspect; a suspect word (a fork or a
//! split at or below its height, a reorg's open recheck) is refused as it
//! stands, and only the tracker's own steps clear the mark (#164). Under `serve`,
//! which holds no feed, the pass keeps the last twelve tips and reads the
//! mined rows at or above a header that moved.
//!
//! # What the header service pushes
//!
//! The daemon's chain-event feed ([`crate::chain_feed`]) hands each
//! envelope to [`deliver_chain_event`]: the rows the event names by index
//! (the mined rows at or above a fork, reorg or split height, the rows
//! marked by an earlier event for a tip, the rows on an invalidated
//! block's hash) are loaded into a [`Tracker`], and `Tracker::on_chain`
//! rechecks each against the new branch's headers by its stored root. A
//! word whose root is still the active header's at its height stands, its
//! marks cleared, and nothing is fetched; a word whose root is gone is
//! `stale` with `Reask::Reorg`, the only one the pass asks a proof for; a
//! height where the service still answers a header the reorg deactivated
//! has no answer, so the word stands suspect with `Reask::Recheck` until
//! the next tip. An event that left a row anything but a plain checked
//! `mined` is kept on the row as one more input to replay (the `chain`
//! column), and the stored proof is replayed against the header the reorg
//! took away, so a `stale` or a suspect mark survives the next load; a new
//! proof, or a stored path dropped because it no longer checks, clears it.
//! The cursor is stored in the same transaction.
//!
//! # A `stale` word at the broadcasters
//!
//! A reorg is absorbed below the user (ruling 7 of 2026-10-20 20:1xZ,
//! bsv-stack-lean `docs/DECISIONS-2026-10-19.md` section 8c): a `stale`
//! word is served as `stale` with its tier, the word its hints give (read
//! as "confirming"), and its re-ask. Once per `stale` episode the pass
//! reads the transaction's status at the broadcasters; one that knows it
//! ends the matter, and only when every one that answered does not know it
//! is it announced, once ([`REORG_ASKS_PER_PASS`] words a pass at most). A
//! refusal the node gave writes the node's verdict on the row. The door
//! (what a coin whose creator is `stale` may do) is the gate's: a `stale`
//! creator reads as `seen`, offered under the rule a pending creator's coin
//! is and never with no rule (ruling 7a, [`crate::spend_gate::hold_reason`]).
//! The action's status is the toolbox's.
//!
//! One rule, not two: where the toolbox's monitor runs its reorg and review
//! tasks (the daemon, [`TickOptions::monitor_runs`]), the follow-up of the
//! episode is the toolbox's (bsv-wallet-toolbox-rs 0.10.0
//! `reask_lost_proof`, run once right after the demotion that starts the
//! episode), and this pass marks the row `monitor` and reads no status and
//! announces nothing. Where no monitor runs (`serve`, `tracker-tick`), the
//! follow-up is this pass's. Either way one status read and at most one
//! announce per episode.
//!
//! One re-ask is an announce, not a proof request: a transaction a
//! broadcaster called not final (a forced `gift-claim` before its lock
//! time) is kept with its BEEF and handed to the broadcasters again at the
//! lock time ([`heard`], and the re-announce step of [`tick`]).

use std::collections::{BTreeMap, HashMap, VecDeque};
use std::convert::Infallible;

use anyhow::Result;
use bsv_sdk::transaction::{Beef, MerklePath};
use bsv_sdk::wallet::{InternalizeActionArgs, WalletInterface};
use bsv_tracker::{
    decode_envelope, ChainEnvelope, ChainEvent, CheckError, Clock, Evidence, Header, HeaderError,
    Headers, Height, Hint, HintSource, HintStatus, HostAction, Input, NodeVerdict, Params, Proof,
    ProofFetcher, Reask, State, Timestamp, Tracker, TxId, Word,
};
use bsv_wallet_toolbox::services::PostBeefResult;
use bsv_wallet_toolbox::{BroadcastStatus, StorageSqlx, WalletServices};
use serde::{Deserialize, Serialize};
use sqlx::Row as _;

/// Default seconds without a new hint before an announced or seen
/// transaction is asked for its proof (`TRACKER_AGE_SECS`): one block
/// interval.
pub const DEFAULT_AGE_SECS: u64 = 600;
/// Default re-asks one pass runs (`TRACKER_MAX_ASKS`).
pub const DEFAULT_MAX_ASKS: usize = 20;
/// The pause between two re-asks of one transaction doubles up to this
/// many times (600 s, then 1200 s, up to 64 times the age threshold).
const MAX_BACKOFF_DOUBLINGS: u32 = 6;
/// A transaction asked this many times in a row with no proof is not asked
/// again until a new hint names it (about five days at the default age).
const MAX_ASKS_PER_TX: u32 = 16;
/// Recent tips kept to see a moved header (without the feed).
const TIP_RING: usize = 12;
/// Words whose root moved (`Reask::Reorg`) asked for a proof, and
/// followed up at the broadcasters, per pass at most: a few, the pacing
/// the host's (ruling (5); the bsv-tracker 0.7.0 migration note).
pub const REORG_ASKS_PER_PASS: usize = 4;
/// Default spend depth (`SPEND_DEPTH`), and its floor: a coin is offered
/// when the block holding its transaction is at or below the tip.
pub const DEFAULT_SPEND_DEPTH: Height = 1;

// ---------------------------------------------------------------------------
// Headers
// ---------------------------------------------------------------------------

/// One pass's view of the active chain: the tip and the headers at the
/// heights the pass named, read from the header service through the
/// wallet's services. Fails closed: a height that was not read has no
/// answer, a read that failed is a fault, and with no tip nothing checks.
#[derive(Debug, Clone)]
pub struct HeaderSnapshot {
    tip: std::result::Result<(Height, String), String>,
    headers: BTreeMap<Height, std::result::Result<Header, String>>,
}

impl HeaderSnapshot {
    /// Read the tip and the headers at `heights`.
    pub async fn take<V: WalletServices + ?Sized>(
        services: &V,
        heights: impl IntoIterator<Item = Height>,
    ) -> Self {
        let tip = services
            .get_chain_tip_header()
            .await
            .map(|h| (h.height, h.hash.to_ascii_lowercase()))
            .map_err(|e| e.to_string());
        let mut snapshot = Self {
            tip,
            headers: BTreeMap::new(),
        };
        snapshot.extend(services, heights).await;
        snapshot
    }

    /// A snapshot of nothing: no tip, no header. Every check fails closed.
    pub fn unread() -> Self {
        Self {
            tip: Err("the header service was not read".to_string()),
            headers: BTreeMap::new(),
        }
    }

    /// Read the headers at `heights` that this snapshot does not hold yet.
    pub async fn extend<V: WalletServices + ?Sized>(
        &mut self,
        services: &V,
        heights: impl IntoIterator<Item = Height>,
    ) {
        for height in heights {
            if self.headers.contains_key(&height) {
                continue;
            }
            let answer = match services.get_header_for_height(height).await {
                Ok(bytes) => header_of(&bytes),
                Err(e) => Err(e.to_string()),
            };
            self.headers.insert(height, answer);
        }
    }

    /// The tip's height and hash, when the header service gave one.
    pub fn tip(&self) -> Option<(Height, &str)> {
        self.tip.as_ref().ok().map(|(h, hash)| (*h, hash.as_str()))
    }
}

impl Headers for HeaderSnapshot {
    fn header_at(&self, height: Height) -> std::result::Result<Option<Header>, HeaderError> {
        match self.headers.get(&height) {
            Some(Ok(header)) => Ok(Some(header.clone())),
            Some(Err(fault)) => Err(HeaderError(fault.clone())),
            None => Ok(None),
        }
    }

    fn tip_height(&self) -> std::result::Result<Height, HeaderError> {
        self.tip
            .as_ref()
            .map(|(height, _)| *height)
            .map_err(|fault| HeaderError(fault.clone()))
    }
}

/// The tracker's header projection of an 80-byte block header: its hash
/// and its merkle root, both as displayed.
fn header_of(bytes: &[u8]) -> std::result::Result<Header, String> {
    if bytes.len() != 80 {
        return Err(format!("a header of {} bytes", bytes.len()));
    }
    let display = |raw: &[u8]| {
        let mut reversed = raw.to_vec();
        reversed.reverse();
        hex::encode(reversed)
    };
    Ok(Header {
        hash: display(&bsv_sdk::primitives::hash::sha256d(bytes)),
        merkle_root: display(&bytes[36..68]),
    })
}

// ---------------------------------------------------------------------------
// Clock
// ---------------------------------------------------------------------------

/// The host's clock: seconds since the epoch.
#[derive(Debug, Clone, Copy, Default)]
pub struct SystemClock;

impl Clock for SystemClock {
    fn now(&self) -> Timestamp {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0)
    }
}

// ---------------------------------------------------------------------------
// Hints
// ---------------------------------------------------------------------------

/// The hints of one pass: the broadcast memory rows of the tracked
/// transactions whose `(provider, word)` the row has not heard yet. A word
/// a broadcaster repeats every minute is one hint, so a refreshed `seen`
/// never resets the age clock.
#[derive(Debug, Default)]
pub struct MemoryHints {
    queue: VecDeque<(TxId, Hint)>,
}

impl MemoryHints {
    async fn read(storage: &StorageSqlx, rows: &[TrackedRow]) -> Result<Self> {
        let txids: Vec<String> = rows.iter().map(|r| r.txid.clone()).collect();
        if txids.is_empty() {
            return Ok(Self::default());
        }
        let mut records = storage.broadcast_records(None, &txids).await?;
        records.sort_by_key(|r| r.seen_at);
        let mut queue = VecDeque::new();
        for record in records {
            let Some(status) = record.ladder_status() else {
                continue;
            };
            let txid = record.txid.to_ascii_lowercase();
            let Some(row) = rows.iter().find(|r| r.txid == txid) else {
                continue;
            };
            // A broadcaster's "mined" carries no height here and is no
            // proof: it reads as a word that disagrees and asks for one.
            let (label, status) = match status {
                BroadcastStatus::Accepted => ("accepted", HintStatus::Accepted),
                BroadcastStatus::Seen => ("seen", HintStatus::Seen),
                BroadcastStatus::Mined => ("mined", HintStatus::Unknown),
                BroadcastStatus::Rejected => (
                    "rejected",
                    HintStatus::Rejected {
                        reason: record.status.clone(),
                    },
                ),
                BroadcastStatus::Unknown => ("unknown", HintStatus::Unknown),
            };
            let source = format!("{}|{}", record.provider, label);
            if row.hints.iter().any(|h| h.source == source) {
                continue;
            }
            let observed = record.seen_at.timestamp().max(0) as Timestamp;
            queue.push_back((txid, Hint::new(source, status, observed)));
        }
        Ok(Self { queue })
    }
}

impl HintSource for MemoryHints {
    type Error = Infallible;

    fn next_hint(&mut self) -> std::result::Result<Option<(TxId, Hint)>, Self::Error> {
        Ok(self.queue.pop_front())
    }
}

// ---------------------------------------------------------------------------
// Proofs
// ---------------------------------------------------------------------------

/// The host's proof transport: `get_merkle_path` for the transactions whose
/// re-ask is due, read before the tracker steps. What it hands over is a
/// raw path bound to its txid; the tracker checks it.
#[derive(Debug, Default)]
pub struct Courier {
    carried: HashMap<TxId, std::result::Result<Option<Proof>, String>>,
}

impl Courier {
    /// Ask for the proof of each of `txids`, once.
    pub async fn carry<V: WalletServices + ?Sized>(services: &V, txids: &[TxId]) -> Self {
        let mut carried = HashMap::new();
        for txid in txids {
            let answer = match services.get_merkle_path(txid, false).await {
                Ok(found) => match found.merkle_path {
                    Some(hex) => MerklePath::from_hex(&hex)
                        .map_err(|e| format!("not a merkle path: {e}"))
                        .and_then(|path| Proof::new(txid.clone(), path).map_err(|e| e.to_string()))
                        .map(Some),
                    None => Ok(None),
                },
                Err(e) => Err(e.to_string()),
            };
            carried.insert(txid.clone(), answer);
        }
        Self { carried }
    }

    fn heights(&self) -> Vec<Height> {
        self.carried
            .values()
            .filter_map(|answer| answer.as_ref().ok()?.as_ref().map(Proof::height))
            .collect()
    }
}

impl ProofFetcher for Courier {
    type Error = String;

    fn fetch(
        &mut self,
        txid: &str,
        _reask: &Reask,
    ) -> std::result::Result<Option<Proof>, Self::Error> {
        self.carried.remove(txid).unwrap_or(Ok(None))
    }
}

// ---------------------------------------------------------------------------
// The stored row
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct StoredHint {
    source: String,
    status: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    height: Option<Height>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    reason: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    competitors: Vec<TxId>,
    /// The courier of a `mempool_present` hint (tracker 0.5.0).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    courier: Option<String>,
    observed: Timestamp,
}

impl StoredHint {
    fn of(hint: &Hint) -> Self {
        let mut courier = None;
        let (status, height, reason, competitors) = match &hint.status {
            HintStatus::Accepted => ("accepted", None, None, vec![]),
            HintStatus::Seen => ("seen", None, None, vec![]),
            HintStatus::Mined { height } => ("mined", Some(*height), None, vec![]),
            HintStatus::StaleBlock => ("stale_block", None, None, vec![]),
            HintStatus::Rejected { reason } => ("rejected", None, Some(reason.clone()), vec![]),
            HintStatus::DoubleSpend { competitors } => {
                ("double_spend", None, None, competitors.clone())
            }
            HintStatus::OrphanMempool => ("orphan_mempool", None, None, vec![]),
            HintStatus::Unknown => ("unknown", None, None, vec![]),
            HintStatus::MempoolPresent { source } => {
                courier = Some(source.clone());
                ("mempool_present", None, None, vec![])
            }
        };
        Self {
            source: hint.source.clone(),
            status: status.to_string(),
            height,
            reason,
            competitors,
            courier,
            observed: hint.observed,
        }
    }

    fn hint(&self) -> Hint {
        let status = match self.status.as_str() {
            "accepted" => HintStatus::Accepted,
            "seen" => HintStatus::Seen,
            "mined" => self
                .height
                .map_or(HintStatus::Unknown, |height| HintStatus::Mined { height }),
            "stale_block" => HintStatus::StaleBlock,
            "rejected" => HintStatus::Rejected {
                reason: self.reason.clone().unwrap_or_default(),
            },
            "double_spend" => HintStatus::DoubleSpend {
                competitors: self.competitors.clone(),
            },
            "orphan_mempool" => HintStatus::OrphanMempool,
            "mempool_present" => self.courier.clone().map_or(HintStatus::Unknown, |source| {
                HintStatus::MempoolPresent { source }
            }),
            _ => HintStatus::Unknown,
        };
        Hint::new(self.source.clone(), status, self.observed)
    }
}

/// One tracked transaction as the CLI's storage holds it: inputs to replay.
#[derive(Debug, Clone, PartialEq, Eq)]
struct TrackedRow {
    txid: TxId,
    built: bool,
    /// Oldest first.
    hints: Vec<StoredHint>,
    /// The raw merkle path, hex. Checked again on every load.
    bump: Option<String>,
    asks: u32,
    next_ask_at: Option<Timestamp>,
    /// The BEEF to hand to the broadcasters again, and when.
    reannounce_beef: Option<Vec<u8>>,
    reannounce_at: Option<Timestamp>,
    /// What the wallet records once a broadcaster takes it: the
    /// `internalizeAction` arguments, JSON.
    on_accept: Option<String>,
    /// Chain events that left the checked word other than a plain
    /// `mined` (stale, or suspect), oldest first, as their envelopes.
    /// Replayed after the proof on every load.
    chain: Vec<String>,
    /// The node's verdict, written when a broadcaster refused the one
    /// announce of a `stale` episode (ruling 7d (c)). Replayed last.
    verdict: Option<StoredVerdict>,
    /// The follow-up of the current `stale` episode: `read` once the
    /// broadcasters were asked for the transaction's status, `announced`
    /// once it was handed to them, `monitor` when the toolbox's monitor
    /// holds it. Cleared when the word leaves `stale`.
    follow_up: Option<String>,
}

/// A node's verdict as the row keeps it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
enum StoredVerdict {
    Rejected { reason: String },
    Conflicted { competitors: Vec<TxId> },
}

impl StoredVerdict {
    fn of(verdict: &NodeVerdict) -> Self {
        match verdict {
            NodeVerdict::Rejected { reason } => Self::Rejected {
                reason: reason.clone(),
            },
            NodeVerdict::Conflicted { competitors } => Self::Conflicted {
                competitors: competitors.clone(),
            },
        }
    }

    fn verdict(&self) -> NodeVerdict {
        match self {
            Self::Rejected { reason } => NodeVerdict::Rejected {
                reason: reason.clone(),
            },
            Self::Conflicted { competitors } => NodeVerdict::Conflicted {
                competitors: competitors.clone(),
            },
        }
    }
}

/// No headers: the view for replaying the hints alone.
struct NoHeaders;

impl Headers for NoHeaders {
    fn header_at(&self, _height: Height) -> std::result::Result<Option<Header>, HeaderError> {
        Ok(None)
    }

    fn tip_height(&self) -> std::result::Result<Height, HeaderError> {
        Ok(0)
    }
}

impl TrackedRow {
    fn new(txid: &str) -> Self {
        Self {
            txid: txid.to_ascii_lowercase(),
            built: true,
            hints: vec![],
            bump: None,
            asks: 0,
            next_ask_at: None,
            reannounce_beef: None,
            reannounce_at: None,
            on_accept: None,
            chain: vec![],
            verdict: None,
            follow_up: None,
        }
    }

    /// The headers the stored reorgs deactivated, by height (the latest
    /// event's header at a height it shares with an earlier one).
    fn deactivated(&self) -> BTreeMap<Height, Header> {
        self.chain_events()
            .iter()
            .flat_map(|event| deactivated_of(event, Some(self)).into_iter())
            .collect()
    }

    /// The word the hints alone give (`built`, `announced`, `seen`, ...):
    /// the tier a `stale` word stands on, its status before its proof
    /// (ruling 7b (b): the payment reads "confirming").
    fn tier(&self) -> &'static str {
        let params = Params {
            age_threshold: DEFAULT_AGE_SECS,
        };
        let mut state = State::new(self.txid.clone());
        if self.built {
            let _ = state.step(&params, &NoHeaders, Input::Host(HostAction::Build));
        }
        for hint in &self.hints {
            let _ = state.step(&params, &NoHeaders, Input::Hint(hint.hint()));
        }
        word_label(state.word())
    }

    /// The stored chain events, decoded. They were decoded once before
    /// they were stored; one that no longer decodes is reported and not
    /// replayed.
    fn chain_events(&self) -> Vec<ChainEvent> {
        self.chain
            .iter()
            .filter_map(|json| match decode_envelope(json.as_bytes()) {
                Ok(event) => Some(event),
                Err(fault) => {
                    tracing::warn!(txid = %self.txid, %fault, "a stored chain event does not decode");
                    None
                }
            })
            .collect()
    }

    /// Keep `event` on the row when it left the state anything but a
    /// plain checked `mined`; forget every kept event when it did. One
    /// `tip` is kept at most (each rechecks the same suspect mark).
    fn keep_chain(&mut self, state: &State, event: &ChainEvent) -> Result<()> {
        let plain = matches!(state.word(), Word::Mined(_)) && !state.suspect();
        if plain && state.reask().is_none() {
            self.chain.clear();
            return Ok(());
        }
        if matches!(event, ChainEvent::Tip { .. } | ChainEvent::TipAge { .. }) {
            self.chain.retain(|json| {
                !matches!(
                    decode_envelope(json.as_bytes()),
                    Ok(ChainEvent::Tip { .. } | ChainEvent::TipAge { .. })
                )
            });
        }
        self.chain
            .push(serde_json::to_string(&ChainEnvelope::new(event.clone()))?);
        Ok(())
    }

    /// Feed the stored inputs to `tracker`, as [`TrackedRow::replay`] feeds
    /// them to one state; `deactivated` adds the headers the event being
    /// delivered takes away.
    fn replay_into<H: Headers>(
        &self,
        tracker: &mut Tracker,
        headers: &H,
        deactivated: &BTreeMap<Height, Header>,
    ) {
        let mut gone = self.deactivated();
        gone.extend(deactivated.iter().map(|(h, header)| (*h, header.clone())));
        let old = OldBranch {
            live: headers,
            deactivated: &gone,
        };
        let new = NewBranch {
            live: headers,
            deactivated: &gone,
        };
        tracker.track(self.txid.clone());
        if self.built {
            let _ = tracker.apply(&self.txid, &new, Input::Host(HostAction::Build));
        }
        for hint in &self.hints {
            let _ = tracker.apply(&self.txid, &new, Input::Hint(hint.hint()));
        }
        if let Some(Ok(proof)) = self.proof() {
            let _ = tracker.apply(&self.txid, &old, Input::Evidence(Evidence::Proof(proof)));
        }
        for event in self.chain_events() {
            let _ = tracker.apply(&self.txid, &new, Input::Evidence(Evidence::Chain(event)));
        }
        if let Some(verdict) = &self.verdict {
            let _ = tracker.apply(
                &self.txid,
                &new,
                Input::Evidence(Evidence::Verdict(verdict.verdict())),
            );
        }
    }

    fn proof(&self) -> Option<std::result::Result<Proof, CheckError>> {
        let hex = self.bump.as_deref()?;
        Some(
            MerklePath::from_hex(hex)
                .map_err(|e| CheckError::InvalidProof(e.to_string()))
                .and_then(|path| Proof::new(self.txid.clone(), path)),
        )
    }

    fn proof_height(&self) -> Option<Height> {
        self.proof()?.ok().map(|p| p.height())
    }

    /// Replay the stored inputs. The stored path is checked against
    /// `headers` like any other proof, except at a height a stored reorg
    /// deactivated, where it is checked against the header it was checked
    /// against then, so the reorg's recheck runs again on the new branch
    /// and a `stale` word stands with its `Reask::Reorg` until a new proof.
    /// A path that no longer checks leaves the hint-tier word and a re-ask,
    /// and is returned as the fault.
    fn replay<H: Headers>(&self, params: &Params, headers: &H) -> (State, Option<CheckError>) {
        let gone = self.deactivated();
        let old = OldBranch {
            live: headers,
            deactivated: &gone,
        };
        let new = NewBranch {
            live: headers,
            deactivated: &gone,
        };
        let mut state = State::new(self.txid.clone());
        if self.built {
            let _ = state.step(params, &new, Input::Host(HostAction::Build));
        }
        for hint in &self.hints {
            let _ = state.step(params, &new, Input::Hint(hint.hint()));
        }
        let fault = match self.proof() {
            Some(Ok(proof)) => state
                .step(params, &old, Input::Evidence(Evidence::Proof(proof)))
                .err(),
            Some(Err(fault)) => Some(fault),
            None => None,
        };
        for event in self.chain_events() {
            let _ = state.step(params, &new, Input::Evidence(Evidence::Chain(event)));
        }
        if let Some(verdict) = &self.verdict {
            let _ = state.step(
                params,
                &new,
                Input::Evidence(Evidence::Verdict(verdict.verdict())),
            );
        }
        (state, fault)
    }
}

/// The tracker's word, as one lowercase label.
pub fn word_label(word: &Word) -> &'static str {
    match word {
        Word::Unknown => "unknown",
        Word::Built => "built",
        Word::Announced => "announced",
        Word::Seen => "seen",
        Word::Mined(_) => "mined",
        Word::Stale => "stale",
        Word::Rejected { .. } => "rejected",
        Word::Conflicted { .. } => "conflicted",
        Word::Abandoned => "abandoned",
    }
}

fn reask_label(reask: &Reask) -> String {
    match reask {
        Reask::Reorg { fork_height } => format!("reorg@{fork_height}"),
        Reask::Fork { height } => format!("fork@{height}"),
        Reask::Hint(hint) => format!("hint:{}", hint.source),
        Reask::ProofFailed { height } => format!("proof_failed@{height}"),
        Reask::Recheck => "recheck".to_string(),
        Reask::Age => "age".to_string(),
        Reask::Spend => "spend".to_string(),
    }
}

async fn ensure_tables(storage: &StorageSqlx) -> Result<()> {
    // Read first: a store that has every table and column sends nothing to
    // the writer (the read paths call this too).
    let reader = storage.pool();
    if crate::store::has_column(reader, "tracker_states", "follow_up").await?
        && crate::store::has_column(reader, "tracker_states", "verdict").await?
        && crate::store::has_column(reader, "tracker_states", "chain").await?
        && crate::store::has_column(reader, "tracker_meta", "value").await?
    {
        return Ok(());
    }
    let mut writer = crate::store::writer(storage).await?;
    for sql in [
        "CREATE TABLE IF NOT EXISTS tracker_states (\
            txid TEXT PRIMARY KEY, \
            built INTEGER NOT NULL DEFAULT 1, \
            hints TEXT NOT NULL DEFAULT '[]', \
            bump TEXT, \
            word TEXT NOT NULL DEFAULT 'unknown', \
            height INTEGER, \
            header_hash TEXT, \
            reask TEXT, \
            asks INTEGER NOT NULL DEFAULT 0, \
            next_ask_at INTEGER, \
            reannounce_beef BLOB, \
            reannounce_at INTEGER, \
            on_accept TEXT, \
            updated_at INTEGER NOT NULL DEFAULT 0)",
        "CREATE INDEX IF NOT EXISTS tracker_states_word ON tracker_states (word)",
        "CREATE INDEX IF NOT EXISTS tracker_states_height ON tracker_states (height)",
        "CREATE TABLE IF NOT EXISTS tracker_meta (key TEXT PRIMARY KEY, value TEXT NOT NULL)",
    ] {
        sqlx::query(sql).execute(&mut *writer).await?;
    }
    // Added with the chain-event feed (0.9.0); a table made before it
    // gains the column empty.
    let has_chain: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM pragma_table_info('tracker_states') WHERE name = 'chain'",
    )
    .fetch_one(&mut *writer)
    .await?;
    if has_chain == 0 {
        sqlx::query("ALTER TABLE tracker_states ADD COLUMN chain TEXT NOT NULL DEFAULT '[]'")
            .execute(&mut *writer)
            .await?;
    }
    // Added with the follow-up of a `stale` word (0.14.0, ruling 7d (c)).
    for (column, sql) in [
        (
            "verdict",
            "ALTER TABLE tracker_states ADD COLUMN verdict TEXT",
        ),
        (
            "follow_up",
            "ALTER TABLE tracker_states ADD COLUMN follow_up TEXT",
        ),
    ] {
        let has: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM pragma_table_info('tracker_states') WHERE name = ?",
        )
        .bind(column)
        .fetch_one(&mut *writer)
        .await?;
        if has == 0 {
            sqlx::query(sql).execute(&mut *writer).await?;
        }
    }
    Ok(())
}

const ROW_COLUMNS: &str = "txid, built, hints, bump, asks, next_ask_at, reannounce_beef, \
     reannounce_at, on_accept, chain, verdict, follow_up";

fn row_of(row: &sqlx::sqlite::SqliteRow) -> TrackedRow {
    let hints: String = row.get("hints");
    let chain: String = row.get("chain");
    TrackedRow {
        txid: row.get("txid"),
        built: row.get::<i64, _>("built") != 0,
        hints: serde_json::from_str(&hints).unwrap_or_default(),
        bump: row.get("bump"),
        asks: row.get::<i64, _>("asks").max(0) as u32,
        next_ask_at: row
            .get::<Option<i64>, _>("next_ask_at")
            .map(|t| t.max(0) as Timestamp),
        reannounce_beef: row.get("reannounce_beef"),
        reannounce_at: row
            .get::<Option<i64>, _>("reannounce_at")
            .map(|t| t.max(0) as Timestamp),
        on_accept: row.get("on_accept"),
        chain: serde_json::from_str(&chain).unwrap_or_default(),
        verdict: row
            .get::<Option<String>, _>("verdict")
            .and_then(|v| serde_json::from_str(&v).ok()),
        follow_up: row.get("follow_up"),
    }
}

async fn load_row(storage: &StorageSqlx, txid: &str) -> Result<Option<TrackedRow>> {
    let row = sqlx::query(&format!(
        "SELECT {ROW_COLUMNS} FROM tracker_states WHERE txid = ?"
    ))
    .bind(txid.to_ascii_lowercase())
    .fetch_optional(storage.pool())
    .await?;
    Ok(row.as_ref().map(row_of))
}

async fn store_row(
    storage: &StorageSqlx,
    row: &TrackedRow,
    state: &State,
    now: Timestamp,
) -> Result<()> {
    store_row_in(&mut *crate::store::writer(storage).await?, row, state, now).await
}

async fn store_row_in<'e, E: sqlx::Executor<'e, Database = sqlx::Sqlite>>(
    executor: E,
    row: &TrackedRow,
    state: &State,
    now: Timestamp,
) -> Result<()> {
    let (height, header_hash) = match state.word() {
        Word::Mined(mined) => (
            Some(mined.height() as i64),
            Some(mined.checked().header().hash.clone()),
        ),
        _ => (None, None),
    };
    sqlx::query(
        "INSERT INTO tracker_states \
            (txid, built, hints, bump, word, height, header_hash, reask, asks, next_ask_at, \
             reannounce_beef, reannounce_at, on_accept, chain, verdict, follow_up, updated_at) \
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?) \
         ON CONFLICT(txid) DO UPDATE SET built = excluded.built, hints = excluded.hints, \
            bump = excluded.bump, word = excluded.word, height = excluded.height, \
            header_hash = excluded.header_hash, reask = excluded.reask, asks = excluded.asks, \
            next_ask_at = excluded.next_ask_at, reannounce_beef = excluded.reannounce_beef, \
            reannounce_at = excluded.reannounce_at, on_accept = excluded.on_accept, \
            chain = excluded.chain, verdict = excluded.verdict, follow_up = excluded.follow_up, \
            updated_at = excluded.updated_at",
    )
    .bind(&row.txid)
    .bind(row.built as i64)
    .bind(serde_json::to_string(&row.hints)?)
    .bind(&row.bump)
    .bind(word_label(state.word()))
    .bind(height)
    .bind(header_hash)
    .bind(state.reask().map(reask_label))
    .bind(row.asks as i64)
    .bind(row.next_ask_at.map(|t| t as i64))
    .bind(&row.reannounce_beef)
    .bind(row.reannounce_at.map(|t| t as i64))
    .bind(&row.on_accept)
    .bind(serde_json::to_string(&row.chain)?)
    .bind(
        row.verdict
            .as_ref()
            .map(serde_json::to_string)
            .transpose()?,
    )
    // The follow-up belongs to one `stale` episode.
    .bind(
        row.follow_up
            .as_deref()
            .filter(|_| matches!(state.word(), Word::Stale)),
    )
    .bind(now as i64)
    .execute(executor)
    .await?;
    Ok(())
}

/// What the storage holds for `txid`: the word, the mined height and the
/// pending re-ask the tracker gave at its last step. A view for a person;
/// nothing is decided from it.
pub async fn stored_word(
    storage: &StorageSqlx,
    txid: &str,
) -> Result<Option<(String, Option<Height>, Option<String>)>> {
    ensure_tables(storage).await?;
    let row = sqlx::query("SELECT word, height, reask FROM tracker_states WHERE txid = ?")
        .bind(txid.to_ascii_lowercase())
        .fetch_optional(storage.pool())
        .await?;
    Ok(row.map(|r| {
        (
            r.get("word"),
            r.get::<Option<i64>, _>("height").map(|h| h as Height),
            r.get("reask"),
        )
    }))
}

/// The tier a stored `stale` word stands on: the word its hints alone give
/// (`announced`, `seen`), its status before its proof. `None` for any
/// other word, and for a txid the tracker holds no row for. Ruling 7b (b)
/// of 2026-10-20 20:1xZ (bsv-stack-lean `docs/DECISIONS-2026-10-19.md`
/// section 8c): a payment whose transaction went `stale` returns to its
/// pre-proof status and reads "confirming".
pub async fn stored_tier(storage: &StorageSqlx, txid: &str) -> Result<Option<&'static str>> {
    ensure_tables(storage).await?;
    let word: Option<String> = sqlx::query_scalar("SELECT word FROM tracker_states WHERE txid = ?")
        .bind(txid.to_ascii_lowercase())
        .fetch_optional(storage.pool())
        .await?;
    if word.as_deref() != Some("stale") {
        return Ok(None);
    }
    Ok(load_row(storage, txid).await?.map(|row| row.tier()))
}

// ---------------------------------------------------------------------------
// What a broadcaster answered to a transaction the host just handed over
// ---------------------------------------------------------------------------

/// The broadcasters' answer to one post, read as a hint.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PostAnswer {
    /// A broadcaster took it.
    Accepted { provider: String },
    /// None took it and one called it not final: its lock time has not
    /// passed for the node behind that broadcaster. Transient.
    NotFinal { provider: String, said: String },
    /// None took it, for some other reason (or none answered).
    Refused { said: String },
}

/// Does a broadcaster's refusal say "not final"? The node's word is
/// `non-final`; ARC and Arcade carry it under code 476, which the toolbox
/// reads as retryable (bsv-wallet-toolbox-rs `ARCADE_CODE_NON_FINAL`).
pub fn says_not_final(text: &str) -> bool {
    let lower = text.to_ascii_lowercase();
    ["non-final", "nonfinal", "non final", "not final"]
        .iter()
        .any(|word| lower.contains(word))
        || lower
            .split(|c: char| !c.is_ascii_alphanumeric())
            .any(|token| token == "476")
}

/// Read the results of one `post_beef`.
pub fn read_post(results: &[PostBeefResult]) -> PostAnswer {
    if let Some(accepted) = results.iter().find(|r| r.is_success()) {
        return PostAnswer::Accepted {
            provider: accepted.name.clone(),
        };
    }
    let said: Vec<(String, String)> = results
        .iter()
        .map(|r| {
            let detail = r
                .error
                .clone()
                .or_else(|| r.txid_results.iter().find_map(|t| t.data.clone()))
                .unwrap_or_else(|| r.status.clone());
            (r.name.clone(), detail)
        })
        .collect();
    if let Some((provider, detail)) = said.iter().find(|(_, detail)| says_not_final(detail)) {
        return PostAnswer::NotFinal {
            provider: provider.clone(),
            said: detail.clone(),
        };
    }
    PostAnswer::Refused {
        said: if said.is_empty() {
            "none answered".to_string()
        } else {
            said.iter()
                .map(|(name, detail)| format!("{name}: {detail}"))
                .collect::<Vec<_>>()
                .join("; ")
        },
    }
}

/// A BEEF to hand to the broadcasters again, and when.
#[derive(Debug, Clone)]
pub struct Reannounce {
    /// The BEEF, as it was posted.
    pub beef: Vec<u8>,
    /// Not before this time (seconds since the epoch).
    pub at: Timestamp,
    /// What the wallet records when a broadcaster takes it then.
    pub on_accept: Option<InternalizeActionArgs>,
}

/// Track a transaction this wallet built and just handed to a broadcaster,
/// and feed what the broadcaster said, as a hint. `not_before` holds every
/// re-ask until that time (a transaction that cannot be mined before its
/// lock time has no proof to ask for before it); `reannounce` keeps the
/// BEEF to hand over again then. Returns the tracker's word and its
/// pending re-ask.
pub async fn heard<C: Clock>(
    storage: &StorageSqlx,
    clock: &C,
    opts: &TickOptions,
    txid: &str,
    hint: Hint,
    not_before: Option<Timestamp>,
    reannounce: Option<Reannounce>,
) -> Result<(String, Option<String>)> {
    ensure_tables(storage).await?;
    let mut row = match load_row(storage, txid).await? {
        // A row with a stored path is past this: its word is read, never
        // rewritten from a hint.
        Some(row) if row.bump.is_some() => {
            let stored = stored_word(storage, txid).await?.unwrap_or_default();
            return Ok((stored.0, stored.2));
        }
        Some(row) => row,
        None => TrackedRow::new(txid),
    };
    let params = opts.params();
    let headers = HeaderSnapshot::unread();
    let (mut state, _) = row.replay(&params, &headers);
    if !row.hints.iter().any(|h| h.source == hint.source) {
        row.hints.push(StoredHint::of(&hint));
        let _ = state.step(&params, &headers, Input::Hint(hint));
    }
    row.asks = 0;
    row.next_ask_at = not_before;
    match reannounce {
        Some(again) => {
            row.reannounce_beef = Some(again.beef);
            row.reannounce_at = Some(again.at);
            row.on_accept = match again.on_accept {
                Some(args) => Some(serde_json::to_string(&args)?),
                None => None,
            };
        }
        None => {
            row.reannounce_beef = None;
            row.reannounce_at = None;
            row.on_accept = None;
        }
    }
    store_row(storage, &row, &state, clock.now()).await?;
    Ok((
        word_label(state.word()).to_string(),
        state.reask().map(reask_label),
    ))
}

// ---------------------------------------------------------------------------
// The pass
// ---------------------------------------------------------------------------

/// The host's parameters of a pass.
#[derive(Debug, Clone)]
pub struct TickOptions {
    /// Seconds without a new hint before an announced or seen transaction
    /// is asked for its proof (the tracker's `Params::age_threshold`).
    pub age_threshold: Timestamp,
    /// Re-asks one pass runs.
    pub max_asks: usize,
    /// How deep the block holding a coin's transaction must be before the
    /// coin is offered (`SPEND_DEPTH`): the tip is depth 1. The stack's
    /// wallet charter R3 (b), ruled 2026-10-12 (bsv-stack-lean
    /// `lean/Wallet.lean`, `guardDepth`): the spend depth is the host's
    /// parameter, named in its config, with 1 the floor; a number below 1
    /// reads as 1. Read by [`spend_guard`], where the host decides whether
    /// a coin may be spent.
    pub spend_depth: Height,
    /// The daemon holds the header service's chain-event feed
    /// ([`crate::chain_feed`]): a chain event reaches the tracked rows
    /// through [`deliver_chain_event`] and the crate's own recheck, so the
    /// pass keeps no ring of tips and reads no mined row again (ruling (5)
    /// of 2026-10-20 19:4xZ: the recheck lives in the crate's step, once,
    /// not in a host). `false` under `serve`, which holds no feed.
    pub feed_held: bool,
    /// The toolbox's monitor runs its reorg or review task in this process
    /// (the daemon): a `stale` episode's status read and its one announce
    /// are the toolbox's (`reask_lost_proof`, after its demotion), and the
    /// pass makes neither. `false` under `serve` and `tracker-tick`, which
    /// run no monitor: the pass's own follow-up runs.
    pub monitor_runs: bool,
}

impl Default for TickOptions {
    /// The defaults: what [`TickOptions::from_config`] gives with
    /// `TRACKER_AGE_SECS`, `TRACKER_MAX_ASKS` and `SPEND_DEPTH` unset.
    fn default() -> Self {
        Self {
            age_threshold: DEFAULT_AGE_SECS,
            max_asks: DEFAULT_MAX_ASKS,
            spend_depth: DEFAULT_SPEND_DEPTH,
            feed_held: false,
            monitor_runs: false,
        }
    }
}

impl TickOptions {
    /// `TRACKER_AGE_SECS`, `TRACKER_MAX_ASKS` and `SPEND_DEPTH` as the
    /// configuration read them at start (`crate::config::HostSettings`).
    #[must_use]
    pub fn from_config(config: &crate::config::Config) -> Self {
        let host = &config.host;
        Self {
            age_threshold: host.tracker_age_secs,
            max_asks: host.tracker_max_asks,
            spend_depth: host.spend_depth,
            ..Self::default()
        }
        .floored()
    }

    /// The options with the spend depth at its floor of 1 or above (R3).
    #[must_use]
    pub fn floored(mut self) -> Self {
        self.spend_depth = self.spend_depth.max(DEFAULT_SPEND_DEPTH);
        self
    }

    fn params(&self) -> Params {
        Params {
            age_threshold: self.age_threshold,
        }
    }
}

/// What one pass did.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct TickReport {
    /// Rows read this pass (never the mined ones, unless a header moved).
    pub tracked: usize,
    /// The wallet's own unproven transactions tracked for the first time.
    pub adopted: usize,
    /// Proven creators of coins we hold, tracked for the first time from
    /// the toolbox's stored proof replayed against the headers of the
    /// moment: `mined` when its root checks.
    pub adopted_mined: Vec<String>,
    /// The same, whose stored root does not meet the header at its height:
    /// tracked with the re-ask `proofFailed`, no word written.
    pub adopted_proof_failed: Vec<String>,
    /// New hints fed.
    pub hints: usize,
    /// Re-asks run: one proof request each.
    pub asked: usize,
    /// Transactions whose proof checked this pass: the word is `mined`.
    pub mined: Vec<String>,
    /// Re-asks that brought no proof; each is asked again after its pause.
    pub no_proof: Vec<String>,
    /// Mined rows whose stored path no longer meets the active header.
    pub moved: Vec<String>,
    /// The lowest height whose header moved since the last pass, if any.
    pub moved_from: Option<Height>,
    /// Transactions handed to the broadcasters again and accepted.
    pub reannounced: Vec<String>,
    /// Transactions handed over again and still called not final; each is
    /// handed over again after a pause.
    pub not_final: Vec<String>,
    /// What the wallet is to record for each transaction accepted on a
    /// re-announce (`txid`, the `internalizeAction` arguments): the caller
    /// that holds the wallet runs [`record_accepted`].
    #[serde(skip)]
    pub on_accept: Vec<(String, String)>,
    /// `stale` words a broadcaster knows, read once this episode: no
    /// announce (ruling 7d (c)).
    pub stale_known: Vec<String>,
    /// `stale` words no broadcaster knew, announced once and taken.
    pub stale_announced: Vec<String>,
    /// `stale` words whose follow-up is the toolbox's monitor's
    /// ([`TickOptions::monitor_runs`]): no status read, no announce here.
    pub stale_to_monitor: Vec<String>,
    /// The node's verdicts written on a refused announce: `txid: word`.
    pub verdicts: Vec<String>,
    /// Faults the host reports: `txid: what failed`.
    pub faults: Vec<String>,
    /// What the wallet's storage did with each checked proof.
    pub stored: Vec<String>,
}

impl TickReport {
    /// Nothing moved and nothing was asked.
    pub fn is_quiet(&self) -> bool {
        self.adopted == 0
            && self.adopted_mined.is_empty()
            && self.adopted_proof_failed.is_empty()
            && self.hints == 0
            && self.asked == 0
            && self.reannounced.is_empty()
            && self.not_final.is_empty()
            && self.moved.is_empty()
            && self.stale_known.is_empty()
            && self.stale_announced.is_empty()
            && self.stale_to_monitor.is_empty()
            && self.verdicts.is_empty()
            && self.faults.is_empty()
    }

    /// One line.
    pub fn summary(&self) -> String {
        format!(
            "tracker tick: {} tracked ({} new), {} proven creator(s) adopted ({} mined, {} proof failed), {} hint(s), {} announced again ({} still not final), {} re-ask(s): {} mined, {} without a proof, {} moved, {} stale followed up ({} known, {} announced, {} verdict(s), {} the monitor's), {} fault(s)",
            self.tracked,
            self.adopted,
            self.adopted_mined.len() + self.adopted_proof_failed.len(),
            self.adopted_mined.len(),
            self.adopted_proof_failed.len(),
            self.hints,
            self.reannounced.len(),
            self.not_final.len(),
            self.asked,
            self.mined.len(),
            self.no_proof.len(),
            self.moved.len(),
            self.stale_known.len()
                + self.stale_announced.len()
                + self.verdicts.len()
                + self.stale_to_monitor.len(),
            self.stale_known.len(),
            self.stale_announced.len(),
            self.verdicts.len(),
            self.stale_to_monitor.len(),
            self.faults.len(),
        )
    }
}

/// The recent tips, newest last, read from and written to `tracker_meta`.
async fn read_ring(storage: &StorageSqlx) -> Vec<(Height, String)> {
    sqlx::query("SELECT value FROM tracker_meta WHERE key = 'tips'")
        .fetch_optional(storage.pool())
        .await
        .ok()
        .flatten()
        .and_then(|r| serde_json::from_str(&r.get::<String, _>("value")).ok())
        .unwrap_or_default()
}

async fn write_ring(storage: &StorageSqlx, ring: &[(Height, String)]) -> Result<()> {
    sqlx::query(
        "INSERT INTO tracker_meta (key, value) VALUES ('tips', ?) \
         ON CONFLICT(key) DO UPDATE SET value = excluded.value",
    )
    .bind(serde_json::to_string(ring)?)
    .execute(&mut *crate::store::writer(storage).await?)
    .await?;
    Ok(())
}

/// The lowest height whose header is no longer the one a past pass saw,
/// and the ring with the moved entries dropped and the tip appended. One
/// header read when the tip advanced, none when it did not, and one more
/// per replaced block. Deeper than the ring, the ring's lowest height is
/// the answer and the host says so.
async fn moved_since<V: WalletServices + ?Sized>(
    services: &V,
    snapshot: &mut HeaderSnapshot,
    mut ring: Vec<(Height, String)>,
) -> (Option<Height>, Vec<(Height, String)>) {
    let Some((tip_height, tip_hash)) = snapshot.tip().map(|(h, hash)| (h, hash.to_string())) else {
        return (None, ring);
    };
    let mut moved = None;
    while let Some((height, hash)) = ring.last().cloned() {
        let active = if height == tip_height {
            Some(tip_hash.clone())
        } else if height > tip_height {
            None
        } else {
            snapshot.extend(services, [height]).await;
            match snapshot.header_at(height) {
                Ok(Some(header)) => Some(header.hash),
                // Could not look: nothing is concluded this pass.
                _ => return (moved, ring),
            }
        };
        if active.as_deref() == Some(hash.as_str()) {
            break;
        }
        moved = Some(height);
        ring.pop();
    }
    if ring.last().map(|(h, _)| *h) != Some(tip_height) {
        ring.push((tip_height, tip_hash));
    }
    if ring.len() > TIP_RING {
        let extra = ring.len() - TIP_RING;
        ring.drain(..extra);
    }
    (moved, ring)
}

/// One pass of the host. See the module docs.
pub async fn tick<V: WalletServices + ?Sized, C: Clock>(
    storage: &StorageSqlx,
    services: &V,
    clock: &C,
    opts: &TickOptions,
) -> Result<TickReport> {
    ensure_tables(storage).await?;
    let params = opts.params();
    let now = clock.now();
    let mut report = TickReport::default();

    // The wallet's own transactions that are out and unproven, tracked
    // from the first pass that sees them. Never anyone else's.
    let adopted = sqlx::query(
        "INSERT OR IGNORE INTO tracker_states (txid, word, updated_at) \
         SELECT DISTINCT lower(txid), 'built', ? FROM transactions \
         WHERE txid IS NOT NULL AND status IN ('unproven', 'sending')",
    )
    .bind(now as i64)
    .execute(&mut *crate::store::writer(storage).await?)
    .await?;
    report.adopted = adopted.rows_affected() as usize;

    // Without the feed: did a header we stood on move? Then the mined rows
    // at or above it are read again, and no other mined row is. With the
    // feed the crate's own step rechecks them on the event (ruling (5)),
    // so the pass reads no ring and no mined row.
    let mut snapshot = HeaderSnapshot::take(services, []).await;
    let (moved_from, ring) = if opts.feed_held {
        (None, None)
    } else {
        let (moved_from, ring) =
            moved_since(services, &mut snapshot, read_ring(storage).await).await;
        (moved_from, Some(ring))
    };
    report.moved_from = moved_from;

    let mut rows: Vec<TrackedRow> = sqlx::query(&format!(
        "SELECT {ROW_COLUMNS} FROM tracker_states \
         WHERE word IN ('unknown', 'built', 'announced', 'seen', 'stale') \
            OR (word = 'mined' AND ? IS NOT NULL AND height >= ?) \
         ORDER BY txid"
    ))
    .bind(moved_from.map(|h| h as i64))
    .bind(moved_from.map(|h| h as i64))
    .fetch_all(storage.pool())
    .await?
    .iter()
    .map(row_of)
    .collect();
    report.tracked = rows.len();

    // Read: the headers the stored paths name, and the new hints.
    snapshot
        .extend(services, rows.iter().filter_map(TrackedRow::proof_height))
        .await;
    let mut hints = MemoryHints::read(storage, &rows).await?;

    // Step: replay each row, feed its hints, tick its clock.
    let mut states: HashMap<TxId, State> = HashMap::new();
    for row in rows.iter_mut() {
        let (state, fault) = row.replay(&params, &snapshot);
        if let Some(fault) = fault {
            // The stored path does not check against the active header:
            // the row is back at its hint-tier word with a re-ask, due now.
            if matches!(fault, CheckError::RootMismatch(_)) {
                // The path proves inclusion in a block that is no longer
                // active: it is dropped, and a new one is asked for.
                row.bump = None;
                row.chain.clear();
                report.moved.push(row.txid.clone());
                tracing::warn!(
                    marker = "tracker_stored_proof_moved",
                    txid = %row.txid,
                    "a stored merkle path no longer meets the active header; asking again"
                );
            } else {
                report.faults.push(format!("{}: {fault}", row.txid));
            }
            row.asks = 0;
            row.next_ask_at = None;
        }
        states.insert(row.txid.clone(), state);
    }
    while let Ok(Some((txid, hint))) = hints.next_hint() {
        let (Some(state), Some(row)) = (
            states.get_mut(&txid),
            rows.iter_mut().find(|r| r.txid == txid),
        ) else {
            continue;
        };
        row.hints.push(StoredHint::of(&hint));
        let _ = state.step(&params, &snapshot, Input::Hint(hint));
        report.hints += 1;
        // A word that disagrees is asked about now, not after the pause
        // an earlier re-ask earned.
        if matches!(state.reask(), Some(Reask::Hint(_))) {
            row.asks = 0;
            row.next_ask_at = None;
        }
    }
    // Announce again what a broadcaster called not final, at its time.
    // The lock time passing on our clock is not the node's rule passing
    // (the node reads the median time past, about an hour behind; the rule
    // is the sibling's, bsv-script-lean `docs/BLOCK-CONSENSUS.md` section
    // 2.2), so "not final" again is expected and waits one more pause.
    for row in rows.iter_mut() {
        let (Some(beef), Some(at)) = (row.reannounce_beef.clone(), row.reannounce_at) else {
            continue;
        };
        let Some(state) = states.get_mut(&row.txid) else {
            continue;
        };
        if at > now || matches!(state.word(), Word::Mined(_)) {
            continue;
        }
        let pause = Some(now.saturating_add(opts.age_threshold));
        let answer = services
            .post_beef(&beef, std::slice::from_ref(&row.txid))
            .await
            .map(|results| read_post(&results));
        match answer {
            Ok(PostAnswer::Accepted { provider }) => {
                let hint = Hint::new(format!("{provider}|accepted"), HintStatus::Accepted, now);
                row.hints.push(StoredHint::of(&hint));
                let _ = state.step(&params, &snapshot, Input::Hint(hint));
                row.reannounce_beef = None;
                row.reannounce_at = None;
                row.asks = 0;
                row.next_ask_at = pause;
                report.reannounced.push(row.txid.clone());
                if let Some(args) = row.on_accept.take() {
                    report.on_accept.push((row.txid.clone(), args));
                }
            }
            Ok(PostAnswer::NotFinal { .. }) => {
                row.asks = row.asks.saturating_add(1);
                row.next_ask_at = pause;
                if row.asks >= MAX_ASKS_PER_TX {
                    row.reannounce_at = None;
                    report.faults.push(format!(
                        "{}: still not final after {MAX_ASKS_PER_TX} announces; not announced again",
                        row.txid
                    ));
                } else {
                    row.reannounce_at = pause;
                    report.not_final.push(row.txid.clone());
                }
            }
            Ok(PostAnswer::Refused { said }) => {
                row.reannounce_at = None;
                row.next_ask_at = pause;
                report.faults.push(format!(
                    "{}: refused when announced again: {said}",
                    row.txid
                ));
            }
            Err(e) => {
                // Could not look: the same time again, one pause on.
                row.reannounce_at = pause;
                report
                    .faults
                    .push(format!("{}: could not announce again: {e}", row.txid));
            }
        }
    }
    follow_up_stale(
        storage,
        services,
        opts.monitor_runs,
        (&params, &snapshot, now),
        &mut rows,
        &mut states,
        &mut report,
    )
    .await;
    for state in states.values_mut() {
        let _ = state.step(&params, &snapshot, Input::Host(HostAction::Tick(now)));
    }

    // Read again: a proof for each re-ask that is due, and its header. A
    // word whose root moved (`Reask::Reorg`) goes ahead, a few a pass;
    // every other re-ask fills the pass after them.
    let is_due = |row: &&TrackedRow| {
        states[&row.txid].reask().is_some()
            && row.asks < MAX_ASKS_PER_TX
            && row.next_ask_at.is_none_or(|at| at <= now)
    };
    let root_moved =
        |row: &&TrackedRow| matches!(states[&row.txid].reask(), Some(Reask::Reorg { .. }));
    let due: Vec<TxId> = rows
        .iter()
        .filter(is_due)
        .filter(root_moved)
        .take(REORG_ASKS_PER_PASS.min(opts.max_asks))
        .chain(rows.iter().filter(is_due).filter(|row| !root_moved(row)))
        .take(opts.max_asks)
        .map(|row| row.txid.clone())
        .collect();
    let mut courier = Courier::carry(services, &due).await;
    snapshot.extend(services, courier.heights()).await;

    // Step again: each proof through the tracker's check.
    for txid in &due {
        let (Some(state), Some(row)) = (
            states.get_mut(txid),
            rows.iter_mut().find(|r| &r.txid == txid),
        ) else {
            continue;
        };
        report.asked += 1;
        let reask = state.reask().cloned().unwrap_or(Reask::Age);
        let fetched = courier.fetch(txid, &reask);
        let proven = match fetched {
            Ok(Some(proof)) => {
                let hex = proof.path().to_hex();
                match state.step(&params, &snapshot, Input::Evidence(Evidence::Proof(proof))) {
                    Ok(()) => {
                        row.bump = Some(hex);
                        row.chain.clear();
                        true
                    }
                    Err(fault) => {
                        report.faults.push(format!("{txid}: {fault}"));
                        false
                    }
                }
            }
            Ok(None) => {
                report.no_proof.push(txid.clone());
                false
            }
            Err(fault) => {
                report
                    .faults
                    .push(format!("{txid}: could not ask for a proof: {fault}"));
                false
            }
        };
        if proven {
            row.asks = 0;
            row.next_ask_at = None;
            report.mined.push(txid.clone());
        } else {
            let pause = opts
                .age_threshold
                .saturating_mul(1 << row.asks.min(MAX_BACKOFF_DOUBLINGS));
            row.asks = row.asks.saturating_add(1);
            row.next_ask_at = Some(now.saturating_add(pause));
        }
    }

    // Keep the state, and hand each checked proof to the wallet's own
    // storage through its one proof funnel.
    for row in &rows {
        let state = &states[&row.txid];
        store_row(storage, row, state, now).await?;
        if let (true, Word::Mined(mined)) = (report.mined.contains(&row.txid), state.word()) {
            let checked = mined.checked();
            let outcome = storage
                .ingest_merkle_proof(
                    &row.txid,
                    &checked.proof().path().to_binary(),
                    checked.height(),
                    &checked.header().hash,
                    Some(checked.root()),
                )
                .await;
            report.stored.push(match outcome {
                Ok(outcome) => format!("{}: {outcome:?}", row.txid),
                Err(e) => format!("{}: not stored: {e}", row.txid),
            });
        }
    }
    adopt_proven(storage, services, &mut snapshot, &params, now, &mut report).await?;
    if let Some(ring) = ring {
        write_ring(storage, &ring).await?;
    }
    Ok(report)
}

/// The follow-up of a `stale` word at the broadcasters, ruling 7d (c) of
/// 2026-10-20 20:1xZ (bsv-stack-lean `docs/DECISIONS-2026-10-19.md`
/// section 8c): once per `stale` episode, the status read at the
/// broadcasters (`get_broadcaster_statuses`: Arcade `GET /tx/{txid}`,
/// classic ARC `GET /v1/tx/{txid}`). A broadcaster that knows the
/// transaction ends the matter, its word kept as a hint. When every one
/// that answered does not know it, the transaction is announced once, its
/// stored BEEF handed over (the transaction as bytes, its parents with
/// their paths); never a second time in the same episode. A refusal the
/// node gave writes the node's verdict (`rejected`, or `conflicted` with
/// the competitors it names). No answer at all announces nothing. Hints
/// are kept on the row and replay before the proof on the next load, so
/// the word stays `stale` with its `Reask::Reorg` until a new proof.
///
/// Load: one status read and at most one announce per `stale` word, for
/// at most [`REORG_ASKS_PER_PASS`] words a pass; bounded by our own
/// transactions, never a sweep.
///
/// With `monitor_runs` the episode's follow-up is the toolbox's
/// (`reask_lost_proof` after its demotion): the row is marked `monitor`,
/// once, and nothing is read or announced here, so the daemon makes one
/// status read and at most one announce per episode, not two.
async fn follow_up_stale<V: WalletServices + ?Sized, H: Headers>(
    storage: &StorageSqlx,
    services: &V,
    monitor_runs: bool,
    (params, headers, observed): (&Params, &H, Timestamp),
    rows: &mut [TrackedRow],
    states: &mut HashMap<TxId, State>,
    report: &mut TickReport,
) {
    let mut followed = 0;
    for row in rows.iter_mut() {
        if followed >= REORG_ASKS_PER_PASS {
            break;
        }
        let Some(state) = states.get_mut(&row.txid) else {
            continue;
        };
        if !matches!(state.word(), Word::Stale) || row.follow_up.is_some() {
            continue;
        }
        if monitor_runs {
            row.follow_up = Some("monitor".to_string());
            report.stale_to_monitor.push(row.txid.clone());
            continue;
        }
        followed += 1;
        let statuses = services.get_broadcaster_statuses(&row.txid).await;
        row.follow_up = Some("read".to_string());
        let known: Vec<(String, &'static str, HintStatus)> = statuses
            .iter()
            .filter_map(|(provider, status)| {
                let (label, hint) = match status {
                    BroadcastStatus::Accepted => ("accepted", HintStatus::Accepted),
                    BroadcastStatus::Seen => ("seen", HintStatus::Seen),
                    BroadcastStatus::Mined => ("mined", HintStatus::Unknown),
                    BroadcastStatus::Rejected => (
                        "rejected",
                        HintStatus::Rejected {
                            reason: "rejected".to_string(),
                        },
                    ),
                    BroadcastStatus::Unknown => return None,
                };
                Some((provider.clone(), label, hint))
            })
            .collect();
        if !known.is_empty() {
            for (provider, label, status) in known {
                let source = format!("{provider}|{label}");
                if !row.hints.iter().any(|h| h.source == source) {
                    row.hints
                        .push(StoredHint::of(&Hint::new(source, status, observed)));
                }
            }
            report.stale_known.push(row.txid.clone());
            continue;
        }
        if statuses.is_empty() {
            report.faults.push(format!(
                "{}: stale, and no broadcaster answered the status read; not announced",
                row.txid
            ));
            continue;
        }
        // Every broadcaster that answered does not know it: one announce.
        row.follow_up = Some("announced".to_string());
        let beef = match stored_beef(storage, &row.txid).await {
            Ok(Some(beef)) => beef,
            Ok(None) => {
                report.faults.push(format!(
                    "{}: stale and unknown to the broadcasters, with no stored transaction to announce",
                    row.txid
                ));
                continue;
            }
            Err(e) => {
                report
                    .faults
                    .push(format!("{}: the stored BEEF did not read: {e}", row.txid));
                continue;
            }
        };
        let results = match services
            .post_beef(&beef, std::slice::from_ref(&row.txid))
            .await
        {
            Ok(results) => results,
            Err(e) => {
                report
                    .faults
                    .push(format!("{}: could not announce: {e}", row.txid));
                continue;
            }
        };
        if let Some(verdict) = node_verdict(&results, &row.txid) {
            let _ = state.step(
                params,
                headers,
                Input::Evidence(Evidence::Verdict(verdict.clone())),
            );
            row.verdict = Some(StoredVerdict::of(&verdict));
            report
                .verdicts
                .push(format!("{}: {}", row.txid, word_label(state.word())));
            continue;
        }
        match read_post(&results) {
            PostAnswer::Accepted { provider } => {
                let source = format!("{provider}|accepted");
                if !row.hints.iter().any(|h| h.source == source) {
                    row.hints.push(StoredHint::of(&Hint::new(
                        source,
                        HintStatus::Accepted,
                        observed,
                    )));
                }
                report.stale_announced.push(row.txid.clone());
            }
            PostAnswer::NotFinal { said, .. } | PostAnswer::Refused { said } => {
                report.faults.push(format!(
                    "{}: announced once, not taken and no node verdict: {said}",
                    row.txid
                ));
            }
        }
    }
}

/// The node's verdict a broadcaster relayed on `txid`, when no broadcaster
/// accepted it: a double spend (the competitors named, `conflicted`), or a
/// refusal of the transaction itself (`rejected`). A service fault, a
/// missing parent (orphan mempool) and "not final" are no verdict.
fn node_verdict(results: &[PostBeefResult], txid: &str) -> Option<NodeVerdict> {
    if results.iter().any(PostBeefResult::is_success) {
        return None;
    }
    let ours = || {
        results.iter().flat_map(|r| {
            r.txid_results
                .iter()
                .filter(|t| t.txid.eq_ignore_ascii_case(txid))
                .map(move |t| (r, t))
        })
    };
    let competitors: Vec<TxId> = ours()
        .filter(|(_, t)| t.double_spend)
        .flat_map(|(_, t)| t.competing_txs.clone().unwrap_or_default())
        .map(|c| c.to_ascii_lowercase())
        .collect();
    if !competitors.is_empty() {
        return Some(NodeVerdict::Conflicted { competitors });
    }
    ours()
        .find(|(_, t)| {
            !t.is_success()
                && !t.service_error
                && !t.orphan_mempool
                && !says_not_final(t.data.as_deref().unwrap_or_default())
        })
        .map(|(r, t)| NodeVerdict::Rejected {
            reason: format!(
                "{}: {}",
                r.name,
                t.data.clone().unwrap_or_else(|| {
                    if t.double_spend {
                        "double spend".to_string()
                    } else {
                        t.status.clone()
                    }
                })
            ),
        })
}

/// The BEEF of one of the wallet's own transactions, from its stored
/// request (or its transaction row): its input BEEF with the transaction
/// merged as bytes, no path of its own.
async fn stored_beef(storage: &StorageSqlx, txid: &str) -> Result<Option<Vec<u8>>> {
    let row: Option<(Vec<u8>, Option<Vec<u8>>)> = sqlx::query_as(
        "SELECT raw_tx, input_beef FROM proven_tx_reqs WHERE lower(txid) = ? AND raw_tx IS NOT NULL \
         UNION ALL \
         SELECT raw_tx, input_beef FROM transactions WHERE lower(txid) = ? AND raw_tx IS NOT NULL \
         LIMIT 1",
    )
    .bind(txid.to_ascii_lowercase())
    .bind(txid.to_ascii_lowercase())
    .fetch_optional(storage.pool())
    .await?;
    let Some((raw_tx, input_beef)) = row else {
        return Ok(None);
    };
    let mut beef = match input_beef.filter(|b| !b.is_empty()) {
        Some(bytes) => Beef::from_binary(&bytes)
            .map_err(|e| anyhow::anyhow!("the stored input BEEF does not parse: {e}"))?,
        None => Beef::new(),
    };
    beef.merge_raw_tx(raw_tx, None);
    Ok(Some(beef.to_binary()))
}

/// Proven creators adopted per pass at most (`adopt_proven`).
pub const MAX_ADOPT_PER_PASS: usize = 100;

/// Track the creators the toolbox proved before the tracker held a row for
/// them (its monitor's backstop, `check_for_proofs`, moves a transaction to
/// `completed` with `proven_tx_id` set): every `completed` transaction with
/// a stored proof that creates a coin we hold and has no tracker row. The
/// stored path is replayed as the row's input against the headers of the
/// moment, as the guard's [`read_creators`] replays it: a root that checks
/// gives the row the word `mined` at its height with that path as its
/// evidence; a root that does not meet the header at its height gives a row
/// with the pending re-ask `proofFailed`, which the next pass runs (the
/// path dropped, a fresh one asked for). A header the service could not
/// give, or a stored path that does not parse, adopts nothing this pass.
/// The toolbox's own rows are never written. The stack's wallet charter,
/// section 1: about a transaction the wallet believes only the tracker's
/// word, and each action hosts one tracker state.
///
/// Load: one store query per pass, and one header read per height not
/// already read this pass, for at most [`MAX_ADOPT_PER_PASS`] creators.
async fn adopt_proven<V: WalletServices + ?Sized>(
    storage: &StorageSqlx,
    services: &V,
    snapshot: &mut HeaderSnapshot,
    params: &Params,
    now: Timestamp,
    report: &mut TickReport,
) -> Result<()> {
    let found = sqlx::query(
        "SELECT lower(t.txid) AS txid, MAX(t.is_outgoing) AS made, p.merkle_path \
         FROM transactions t \
         JOIN proven_txs p ON p.proven_tx_id = t.proven_tx_id \
         WHERE t.txid IS NOT NULL AND t.status = 'completed' \
           AND NOT EXISTS (SELECT 1 FROM tracker_states s WHERE s.txid = lower(t.txid)) \
           AND EXISTS (SELECT 1 FROM outputs o \
                       WHERE o.transaction_id = t.transaction_id AND o.spent_by IS NULL) \
         GROUP BY lower(t.txid) ORDER BY MIN(t.transaction_id) LIMIT ?",
    )
    .bind(MAX_ADOPT_PER_PASS as i64)
    .fetch_all(storage.pool())
    .await?;
    let mut rows = Vec::with_capacity(found.len());
    for r in &found {
        let txid: String = r.get("txid");
        let path: Option<Vec<u8>> = r.get("merkle_path");
        let Some(path) = path
            .as_deref()
            .and_then(|bytes| MerklePath::from_binary(bytes).ok())
        else {
            report.faults.push(format!(
                "{txid}: the stored proof does not parse; not adopted"
            ));
            continue;
        };
        rows.push(TrackedRow {
            built: r.get::<i64, _>("made") != 0,
            bump: Some(path.to_hex()),
            ..TrackedRow::new(&txid)
        });
    }
    snapshot
        .extend(services, rows.iter().filter_map(TrackedRow::proof_height))
        .await;
    for row in rows {
        let (state, fault) = row.replay(params, &*snapshot);
        match fault {
            None if matches!(state.word(), Word::Mined(_)) => {
                report.adopted_mined.push(row.txid.clone());
            }
            Some(CheckError::RootMismatch(_)) => {
                tracing::warn!(
                    marker = "tracker_adopted_proof_failed",
                    txid = %row.txid,
                    reask = ?state.reask(),
                    "a proven creator's stored merkle path does not meet the active header; tracked with its re-ask"
                );
                report.adopted_proof_failed.push(row.txid.clone());
            }
            // Could not look: nothing is adopted, the next pass looks again.
            Some(fault) => {
                report
                    .faults
                    .push(format!("{}: not adopted: {fault}", row.txid));
                continue;
            }
            None => {
                report.faults.push(format!(
                    "{}: not adopted: the stored proof gave the word {}",
                    row.txid,
                    word_label(state.word())
                ));
                continue;
            }
        }
        store_row(storage, &row, &state, now).await?;
    }
    Ok(())
}

/// Record in the wallet what a pass's re-announces got accepted (E4): the
/// wallet's storage is the verdict for its own actions, so a claim the
/// wallet made is internalized from the BEEF it already holds, and nothing
/// scans the chain for it. Each outcome is added to `report.stored`.
pub async fn record_accepted<W: WalletInterface>(wallet: &W, report: &mut TickReport) {
    for (txid, args) in std::mem::take(&mut report.on_accept) {
        let line = match serde_json::from_str::<InternalizeActionArgs>(&args) {
            Ok(args) => match wallet.internalize_action(args, "bsv-wallet-cli").await {
                Ok(result) if result.accepted => format!("{txid}: recorded in the wallet"),
                Ok(_) => format!("{txid}: the wallet did not accept its own transaction"),
                Err(e) => format!(
                    "{txid}: not recorded in the wallet ({e}); `bsv-wallet receive {txid}` records it once it is mined"
                ),
            },
            Err(e) => format!("{txid}: not recorded in the wallet (stored arguments: {e})"),
        };
        if !line.ends_with("recorded in the wallet") {
            tracing::warn!(marker = "own_transaction_not_recorded", "{line}");
        }
        report.stored.push(line);
    }
}

// ---------------------------------------------------------------------------
// The spend guard
// ---------------------------------------------------------------------------

/// The wallet spend guard on one replayed state. A word marked suspect is
/// refused as it stands and nothing is fed: the guard reads the mark and
/// never clears it (bsv-wallet-cli #164; the keeper's reading there of
/// 2026-10-24; bsv-stack-lean `docs/charters/wallet.md` lines 75 and 102,
/// `docs/charters/tracker.md` lines 86-87, 115, 121 and 132;
/// `lean/Wallet.lean` `offerableUnder`, which reads `!a.tracker.suspect`
/// and writes nothing). A fork or a split at or below the word's height
/// marks it with `Reask::Fork` (bsv-tracker 0.8.0 `src/state.rs`, as
/// in 0.7.0, `on_chain`), and while the fork is in progress our header at the height
/// still carries the root on both branches, so the crate's `recheck`
/// (`src/state.rs` 424-450), which clears the mark whenever the root holds,
/// proves nothing at the door; and `SpendAttempt` would write `Reask::Spend`
/// over the fork's re-ask (`on_host`, 348). The mark clears only by the
/// tracker's own steps: a tip's or a resolved split's recheck through
/// [`deliver_chain_event`], a reorg's recheck, or a new proof.
///
/// No `Recheck` is fed to a suspect word under any re-ask. A row is
/// replayed from its inputs, never restored from compact bytes, so the
/// crate's restore rule (`State::from_compact`, a restored `mined` suspect
/// with `Reask::Recheck` until rechecked) never reaches the guard: the
/// stored proof is checked again against this snapshot, and every mark the
/// replayed state carries was set by a stored chain event replayed against
/// this snapshot's new branch. The one suspect word under `Reask::Recheck`
/// a replay gives is a reorg's open recheck (the header service still
/// answers, at the word's height, the header the reorg deactivated, which
/// the new branch reads as no answer); a recheck here, against the plain
/// snapshot, would read that deactivated header, find the root, and clear
/// a mark the reorg's own recheck had just held.
///
/// A word not suspect runs the crate's guard as before: `SpendAttempt`,
/// then the scheduled `Recheck` (the stack's `Tracker.spend_guard`), and an
/// unavailable header is an error that prevents the spend. Its proof was
/// checked against the same headers in the replay, so the recheck moves no
/// word; it executes the `Reask::Spend` just scheduled, and the row stored
/// back is byte for byte what it was.
fn guard_state<H: Headers>(
    state: &mut State,
    params: &Params,
    headers: &H,
) -> std::result::Result<bool, CheckError> {
    if state.suspect() {
        return Ok(false);
    }
    state.step(params, headers, Input::Host(HostAction::SpendAttempt))?;
    state.step(params, headers, Input::Evidence(Evidence::Recheck))?;
    Ok(matches!(state.word(), Word::Mined(_)) && !state.suspect())
}

/// Run the spend guard for a coin of the tracked transaction `txid`, as
/// the host: the row is loaded (its stored path checked against a fresh
/// header), the guard runs on it ([`guard_state`]), and the row is stored
/// again. A suspect row is stored with its mark's re-ask (`fork@h` for a
/// fork or a split) and its `chain` column as loaded, which replays the
/// mark on the next load. `Ok(true)` only for a checked, non-suspect `mined`; `Err` when the
/// header service could not answer. A transaction the host does not track
/// is not mined.
pub async fn spend_guard<V: WalletServices + ?Sized, C: Clock>(
    storage: &StorageSqlx,
    services: &V,
    clock: &C,
    opts: &TickOptions,
    txid: &str,
) -> Result<std::result::Result<bool, CheckError>> {
    ensure_tables(storage).await?;
    let Some(row) = load_row(storage, txid).await? else {
        return Ok(Ok(false));
    };
    let params = opts.params();
    let snapshot = HeaderSnapshot::take(services, row.proof_height()).await;
    let (mut state, fault) = row.replay(&params, &snapshot);
    match fault {
        // Could not look (no tip, no header at the height, a path that
        // does not parse): an error, no spend, and the row is left as it
        // was. "Could not look" changes no word.
        Some(fault) if !matches!(fault, CheckError::RootMismatch(_)) => return Ok(Err(fault)),
        _ => {}
    }
    let split = under_split(state.word(), open_split_floor(storage).await?);
    let verdict = guard_state(&mut state, &params, &snapshot).and_then(|offered| {
        Ok(offered && !split && deep_enough(&state, &snapshot, opts.spend_depth)?)
    });
    store_row(storage, &row, &state, clock.now()).await?;
    Ok(verdict)
}

/// The guard's reading of one coin's creator, for the coin gate
/// ([`crate::spend_gate`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CreatorReading {
    /// The tracker's word after the guard, as its label.
    pub word: &'static str,
    /// How deep the creator's block is under the tip (the tip is 1); 0 for
    /// a word with no block.
    pub depth: u64,
    /// A fork in progress, or a split not yet resolved, at or below its
    /// height marks it suspect.
    pub suspect: bool,
    /// The guard's door with the mark set aside: a checked `mined` word
    /// whose block is deep enough under the spend depth (floor 1), suspect
    /// or not. False for every other word, whose door is the rule's, read
    /// by [`crate::spend_gate::hold_reason`] from the origin and the
    /// options. The stack's `Wallet.doorSpendable` reads no mark
    /// (`offerableUnder` checks `!a.tracker.suspect` apart from the door);
    /// the gate's answer is [`CreatorReading::offered`].
    pub door: bool,
    /// Could not look (no tip, no header at the height, a stored path that
    /// does not parse): the fault. Nothing is offered on it. For the
    /// header service's own fault (`CheckError::Headers`) this is the
    /// service's error in its own words, not the variant's name.
    pub fault: Option<String>,
    /// The fault is the header service's: its tip or a header the pass
    /// needed could not be read (a transport fault, an outage), so a later
    /// read may answer where this one did not. Never set for a snapshot that
    /// simply holds no header at the height (`Unavailable`), a path that
    /// does not parse, or a root that does not match (bsv-wallet-cli #59).
    pub headers_fault: bool,
}

impl CreatorReading {
    /// The gate's answer: the door with the mark read. `mined`, checked
    /// against the headers of the moment, not suspect (no fork or open
    /// split at or below its height, no open recheck), at the spend depth
    /// (floor 1): the guard offers it with no rule. A reading with a fault
    /// has its door closed, so it is never offered.
    #[must_use]
    pub fn offered(&self) -> bool {
        self.door && !self.suspect
    }
}

/// A check fault as a reading carries it: the header service's own error for
/// a `Headers` fault (the toolbox's words, e.g. "Service error:
/// get_chain_tip_header: no header service answered (...)"), the variant
/// otherwise.
fn fault_text(fault: &CheckError) -> String {
    match fault {
        CheckError::Headers(e) => e.to_string(),
        other => other.to_string(),
    }
}

/// Run the spend guard over many creators against one read of the headers:
/// the tip once, and one header per height a stored path names. Each
/// creator is the tracked row when the tracker holds one, else the
/// toolbox's stored merkle path (`proven_txs`, given here as its bytes)
/// replayed as a row's input: a stored proof is checked again against the
/// headers of the moment, never trusted by age. A creator with neither
/// reads `unknown`. Nothing is written: the tracker's rows are not stored
/// back and no row is adopted.
pub async fn read_creators<V: WalletServices + ?Sized>(
    storage: &StorageSqlx,
    services: &V,
    opts: &TickOptions,
    creators: &[(String, Option<Vec<u8>>)],
) -> Result<HashMap<String, CreatorReading>> {
    ensure_tables(storage).await?;
    let params = opts.params();
    let mut rows = Vec::with_capacity(creators.len());
    for (txid, stored_path) in creators {
        let mut row = match load_row(storage, txid).await? {
            Some(row) => row,
            None => TrackedRow {
                built: false,
                ..TrackedRow::new(txid)
            },
        };
        if row.bump.is_none() {
            row.bump = stored_path
                .as_deref()
                .and_then(|bytes| MerklePath::from_binary(bytes).ok())
                .map(|path| path.to_hex());
        }
        rows.push(row);
    }
    let snapshot =
        HeaderSnapshot::take(services, rows.iter().filter_map(TrackedRow::proof_height)).await;
    let split_floor = open_split_floor(storage).await?;
    let mut readings = HashMap::with_capacity(rows.len());
    for row in rows {
        let (mut state, fault) = row.replay(&params, &snapshot);
        let fault = match fault {
            Some(fault) if !matches!(fault, CheckError::RootMismatch(_)) => Some(fault),
            _ => None,
        };
        // The door: a `mined` word deep enough, the mark set aside. A tip
        // the depth could not read is the guard's fault for a word it
        // offers, as before; for a suspect word, which the guard refuses
        // before the depth, it closes the door and is no fault.
        let verdict = match fault {
            Some(_) => Ok(false),
            None => guard_state(&mut state, &params, &snapshot).and_then(|offered| {
                if !matches!(state.word(), Word::Mined(_)) {
                    return Ok(false);
                }
                match deep_enough(&state, &snapshot, opts.spend_depth) {
                    Err(e) if offered => Err(e),
                    deep => Ok(deep.unwrap_or(false)),
                }
            }),
        };
        let depth = match (state.word().height(), snapshot.tip()) {
            (Some(height), Some((tip, _))) => {
                (u64::from(tip) + 1).saturating_sub(u64::from(height))
            }
            _ => 0,
        };
        let (door, fault) = match verdict {
            Ok(door) => (door, fault),
            Err(e) => (false, fault.or(Some(e))),
        };
        let headers_fault = matches!(fault, Some(CheckError::Headers(_)));
        let split = under_split(state.word(), split_floor);
        readings.insert(
            row.txid.clone(),
            CreatorReading {
                word: word_label(state.word()),
                depth,
                suspect: state.suspect() || split,
                door,
                fault: fault.as_ref().map(fault_text),
                headers_fault,
            },
        );
    }
    Ok(readings)
}

/// Is the block holding a `mined` word's transaction at least
/// `spend_depth` deep under `headers`' tip (the tip is depth 1; the stack's
/// `Tracker.depth hd h = hd.tip + 1 - h`)? The depth reads as 1 at least
/// (R3's floor). An unreadable tip is an error, as for the guard.
fn deep_enough<H: Headers>(
    state: &State,
    headers: &H,
    spend_depth: Height,
) -> std::result::Result<bool, CheckError> {
    let Some(height) = state.word().height() else {
        return Ok(false);
    };
    let tip = headers.tip_height().map_err(CheckError::Headers)?;
    let depth = (u64::from(tip) + 1).saturating_sub(u64::from(height));
    Ok(depth >= u64::from(spend_depth.max(DEFAULT_SPEND_DEPTH)))
}

// ---------------------------------------------------------------------------
// Chain events from the header service's feed
// ---------------------------------------------------------------------------

/// The splits the feed has delivered and not yet seen resolved: each
/// split's key and its `forkHeight` (`tracker_meta`, key `open_splits`),
/// written in the delivery's transaction.
async fn read_open_splits<'e, E: sqlx::Executor<'e, Database = sqlx::Sqlite>>(
    executor: E,
) -> Result<BTreeMap<String, Height>> {
    let value: Option<String> =
        sqlx::query_scalar("SELECT value FROM tracker_meta WHERE key = 'open_splits'")
            .fetch_optional(executor)
            .await?;
    Ok(match value {
        Some(v) => serde_json::from_str(&v)?,
        None => BTreeMap::new(),
    })
}

/// The lowest `forkHeight` of the splits still open, if any. The guard
/// holds every coin whose creator is mined at or above it until its
/// `splitResolved`: the header service answers one branch, and which
/// branch stands is open (depth is the host's; the ruling of 2026-10-16,
/// the scenario `tracker-split-observation-writes-no-word.md` column 4).
pub async fn open_split_floor(storage: &StorageSqlx) -> Result<Option<Height>> {
    ensure_tables(storage).await?;
    Ok(read_open_splits(storage.pool()).await?.into_values().min())
}

/// Is a `mined` word at `height` above an open split?
fn under_split(word: &Word, floor: Option<Height>) -> bool {
    matches!((word, floor), (Word::Mined(m), Some(floor)) if floor <= m.height())
}

/// The feed's stored cursor: the last envelope delivered, `0` at birth.
/// One row of `tracker_meta`, beside the tracker's rows.
pub async fn feed_cursor(storage: &StorageSqlx) -> Result<u64> {
    ensure_tables(storage).await?;
    let value: Option<String> =
        sqlx::query_scalar("SELECT value FROM tracker_meta WHERE key = 'feed_cursor'")
            .fetch_optional(storage.pool())
            .await?;
    Ok(match value {
        Some(v) => v
            .parse()
            .map_err(|e| anyhow::anyhow!("the stored feed cursor {v:?}: {e}"))?,
        None => 0,
    })
}

/// The frozen record: an outpoint the feed's `frozen` event named, with
/// the envelope's cursor and when it was delivered. The spend gate reads it
/// and never offers such a coin (R6: nothing here clears a row).
async fn ensure_frozen(storage: &StorageSqlx) -> Result<()> {
    if crate::store::has_column(storage.pool(), "frozen_outpoints", "frozen_at").await? {
        return Ok(());
    }
    sqlx::query(
        "CREATE TABLE IF NOT EXISTS frozen_outpoints (\
            txid TEXT NOT NULL, \
            vout INTEGER NOT NULL, \
            cursor INTEGER NOT NULL, \
            frozen_at INTEGER NOT NULL, \
            PRIMARY KEY (txid, vout))",
    )
    .execute(&mut *crate::store::writer(storage).await?)
    .await?;
    Ok(())
}

/// Every frozen outpoint: (lowercase txid, vout).
pub async fn frozen_outpoints(
    storage: &StorageSqlx,
) -> Result<std::collections::HashSet<(String, i64)>> {
    ensure_frozen(storage).await?;
    let rows = sqlx::query("SELECT txid, vout FROM frozen_outpoints")
        .fetch_all(storage.pool())
        .await?;
    Ok(rows
        .iter()
        .map(|r| (r.get::<String, _>("txid"), r.get::<i64, _>("vout")))
        .collect())
}

/// How many outpoints the frozen record holds.
pub async fn frozen_count(storage: &StorageSqlx) -> Result<u64> {
    ensure_frozen(storage).await?;
    let n: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM frozen_outpoints")
        .fetch_one(storage.pool())
        .await?;
    Ok(n.max(0) as u64)
}

/// What one delivered envelope did to one tracked transaction.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ChainMove {
    /// The transaction the event named, by index.
    pub txid: String,
    /// The tracker's word after the event.
    pub word: String,
    /// Its pending re-ask, if any.
    pub reask: Option<String>,
    /// Marked by a fork or a split, awaiting a recheck.
    pub suspect: bool,
    /// The tracker's fault for this transaction, reported, if any.
    pub fault: Option<String>,
}

/// One delivered envelope.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ChainDelivery {
    /// The envelope's cursor, now stored.
    pub cursor: u64,
    /// The event's kind.
    pub kind: &'static str,
    /// The transactions it named.
    pub moved: Vec<ChainMove>,
}

/// The event's wire kind.
pub fn event_kind(event: &ChainEvent) -> &'static str {
    match event {
        ChainEvent::Tip { .. } => "tip",
        ChainEvent::Fork { .. } => "fork",
        ChainEvent::Reorg { .. } => "reorg",
        ChainEvent::Invalidated { .. } => "invalidated",
        ChainEvent::Frozen { .. } => "frozen",
        ChainEvent::TipAge { .. } => "tipAge",
        ChainEvent::Split { .. } => "split",
        ChainEvent::SplitResolved { .. } => "splitResolved",
    }
}

/// The headers a reorg replaced, by height: the event's deactivated
/// headers, lowercase.
fn deactivated_of(event: &ChainEvent, row: Option<&TrackedRow>) -> BTreeMap<Height, Header> {
    if let ChainEvent::Invalidated { block_hash } = event {
        return row
            .and_then(TrackedRow::proof)
            .and_then(Result::ok)
            .and_then(|proof| {
                proof.root().ok().map(|merkle_root| {
                    BTreeMap::from([(
                        proof.height(),
                        Header {
                            hash: block_hash.to_ascii_lowercase(),
                            merkle_root,
                        },
                    )])
                })
            })
            .unwrap_or_default();
    }
    let ChainEvent::Reorg {
        deactivated_headers,
        ..
    } = event
    else {
        return BTreeMap::new();
    };
    deactivated_headers
        .iter()
        .map(|h| {
            (
                h.height,
                Header {
                    hash: h.hash.to_ascii_lowercase(),
                    merkle_root: h.merkle_root.to_ascii_lowercase(),
                },
            )
        })
        .collect()
}

/// The branch a reorg took away, over the host's headers: a stored proof
/// is replayed against the header it was checked against (a deactivated
/// header at its height; the tip no lower than the highest of them), so
/// the tracker holds the `mined` word the reorg names. Only proofs are
/// stepped through this view; it writes no `mined` word on the new branch.
struct OldBranch<'a, H: Headers + ?Sized> {
    live: &'a H,
    deactivated: &'a BTreeMap<Height, Header>,
}

impl<H: Headers + ?Sized> Headers for OldBranch<'_, H> {
    fn header_at(&self, height: Height) -> std::result::Result<Option<Header>, HeaderError> {
        match self.deactivated.get(&height) {
            Some(header) => Ok(Some(header.clone())),
            None => self.live.header_at(height),
        }
    }

    fn tip_height(&self) -> std::result::Result<Height, HeaderError> {
        let live = self.live.tip_height()?;
        Ok(self
            .deactivated
            .keys()
            .next_back()
            .map_or(live, |top| live.max(*top)))
    }
}

/// The new branch, as the crate's recheck must see it (ruling (5) of
/// 2026-10-20 19:4xZ, bsv-stack-lean `docs/RULINGS-2026-10-12.md`; the
/// bsv-tracker 0.7.0 migration note): the host's headers, except that a
/// height where the header service still answers a header the reorg
/// deactivated has no answer. A service that has not moved yet leaves the
/// word suspect with `Reask::Recheck` until the next tip, never `mined` on
/// the old branch's header.
struct NewBranch<'a, H: Headers + ?Sized> {
    live: &'a H,
    deactivated: &'a BTreeMap<Height, Header>,
}

impl<H: Headers + ?Sized> Headers for NewBranch<'_, H> {
    fn header_at(&self, height: Height) -> std::result::Result<Option<Header>, HeaderError> {
        let header = self.live.header_at(height)?;
        Ok(header.filter(|h| {
            self.deactivated
                .get(&height)
                .is_none_or(|gone| gone.hash != h.hash)
        }))
    }

    fn tip_height(&self) -> std::result::Result<Height, HeaderError> {
        self.live.tip_height()
    }
}

/// Deliver one chain envelope (already decoded, at `cursor`) to the
/// tracked transactions it names, through `Tracker::on_chain` with the new
/// branch's headers (`NewBranch`: a height where the service still
/// answers a deactivated header has no answer), and store the moved rows
/// and the cursor in one transaction. The crate rechecks each named word
/// by its stored root (bsv-tracker 0.7.0): the same root, it stands; a
/// moved root, `stale` with `Reask::Reorg`; no header, suspect with
/// `Reask::Recheck` until the next tip. A cursor at or below the stored one was delivered before
/// (delivery is at least once) and changes nothing.
///
/// The rows are named by index, never by a scan of the chain: a fork, a
/// reorg or a split names the mined rows at or above its height; a tip,
/// the rows an earlier event marked; an invalidated block, the rows on its
/// hash; a freeze or a tip age names none here (they are the spend and
/// freshness guards' inputs). A freeze writes its outpoint to the frozen
/// record ([`frozen_outpoints`]) in the same transaction as the cursor. The header service is read only when a row
/// is named: the tip and the heights of the named rows' proofs.
pub async fn deliver_chain_event<V: WalletServices + ?Sized, C: Clock>(
    storage: &StorageSqlx,
    services: &V,
    clock: &C,
    opts: &TickOptions,
    cursor: u64,
    event: ChainEvent,
) -> Result<ChainDelivery> {
    let kind = event_kind(&event);
    if cursor <= feed_cursor(storage).await? {
        return Ok(ChainDelivery {
            cursor,
            kind,
            moved: vec![],
        });
    }
    let named = format!("SELECT {ROW_COLUMNS} FROM tracker_states");
    let query = match &event {
        ChainEvent::Fork { height: from, .. }
        | ChainEvent::Reorg {
            fork_height: from, ..
        }
        | ChainEvent::Split {
            fork_height: from, ..
        }
        | ChainEvent::SplitResolved {
            fork_height: from, ..
        } => Some(
            sqlx::query(&format!(
                "{named} WHERE word = 'mined' AND height >= ? ORDER BY txid"
            ))
            .bind(i64::from(*from))
            .fetch_all(storage.pool())
            .await?,
        ),
        ChainEvent::Tip { .. } => Some(
            sqlx::query(&format!(
                "{named} WHERE word = 'mined' AND chain <> '[]' ORDER BY txid"
            ))
            .fetch_all(storage.pool())
            .await?,
        ),
        ChainEvent::Invalidated { block_hash } => Some(
            sqlx::query(&format!("{named} WHERE header_hash = ? ORDER BY txid"))
                .bind(block_hash.to_ascii_lowercase())
                .fetch_all(storage.pool())
                .await?,
        ),
        ChainEvent::Frozen { .. } | ChainEvent::TipAge { .. } => None,
    };
    let mut rows: Vec<TrackedRow> = query.iter().flatten().map(row_of).collect();

    let now = clock.now();
    let mut moved = Vec::new();
    if matches!(event, ChainEvent::Frozen { .. }) {
        ensure_frozen(storage).await?;
    }
    // The header service is read before the write lock: no write
    // transaction is held across a service await.
    let snapshot = if rows.is_empty() {
        None
    } else {
        Some(HeaderSnapshot::take(services, rows.iter().filter_map(TrackedRow::proof_height)).await)
    };
    let mut tx = storage.write_tx().await?;
    if let ChainEvent::Frozen { outpoint } = &event {
        sqlx::query(
            "INSERT INTO frozen_outpoints (txid, vout, cursor, frozen_at) VALUES (?, ?, ?, ?) \
             ON CONFLICT(txid, vout) DO NOTHING",
        )
        .bind(outpoint.txid.to_ascii_lowercase())
        .bind(i64::from(outpoint.vout))
        .bind(cursor as i64)
        .bind(now as i64)
        .execute(&mut *tx)
        .await?;
    }
    if let Some(snapshot) = snapshot {
        // The stored proofs replay against the branch the event (and any
        // reorg a row keeps) took away; the event's recheck reads the new
        // branch, with no answer at a height the service has not moved.
        let deactivated = if matches!(event, ChainEvent::Invalidated { .. }) {
            rows.iter()
                .flat_map(|row| deactivated_of(&event, Some(row)))
                .collect()
        } else {
            deactivated_of(&event, None)
        };
        let mut gone = deactivated.clone();
        for row in &rows {
            gone.extend(row.deactivated());
        }
        gone.extend(deactivated.iter().map(|(h, header)| (*h, header.clone())));
        let new_branch = NewBranch {
            live: &snapshot,
            deactivated: &gone,
        };
        let mut tracker = Tracker::new(opts.params());
        for row in &rows {
            row.replay_into(&mut tracker, &snapshot, &deactivated);
        }
        for update in tracker.on_chain(&new_branch, event.clone()) {
            let (Some(state), Some(row)) = (
                tracker.get(&update.txid),
                rows.iter_mut().find(|r| r.txid == update.txid),
            ) else {
                continue;
            };
            row.keep_chain(state, &event)?;
            if matches!(update.reask, Some(Reask::Reorg { .. })) {
                // Asked now, not after the pause an earlier re-ask earned.
                row.asks = 0;
                row.next_ask_at = None;
            }
            store_row_in(&mut *tx, row, state, now).await?;
            let fault = update.result.err().map(|e| e.to_string());
            if let Some(fault) = &fault {
                tracing::warn!(txid = %update.txid, kind, cursor, %fault, "a chain event's recheck could not look");
            }
            moved.push(ChainMove {
                txid: update.txid,
                word: word_label(state.word()).to_string(),
                reask: state.reask().map(reask_label),
                suspect: state.suspect(),
                fault,
            });
        }
        // A named row the event did not select (a tip names only suspect
        // words) whose replay moved it: the stored reorg it keeps was
        // rechecked against the new branch in the replay, the crate's own
        // step. Its `mined` word on a moved root is `stale` now.
        for row in rows.iter_mut() {
            if moved.iter().any(|m| m.txid == row.txid) {
                continue;
            }
            let Some(state) = tracker.get(&row.txid) else {
                continue;
            };
            if matches!(state.word(), Word::Mined(_))
                && !(matches!(event, ChainEvent::Tip { .. })
                    && row
                        .chain_events()
                        .iter()
                        .any(|kept| matches!(kept, ChainEvent::Invalidated { .. })))
            {
                continue;
            }
            row.keep_chain(state, &event)?;
            if matches!(state.reask(), Some(Reask::Reorg { .. })) {
                row.asks = 0;
                row.next_ask_at = None;
            }
            store_row_in(&mut *tx, row, state, now).await?;
            moved.push(ChainMove {
                txid: row.txid.clone(),
                word: word_label(state.word()).to_string(),
                reask: state.reask().map(reask_label),
                suspect: state.suspect(),
                fault: None,
            });
        }
    }
    if let ChainEvent::Split {
        key, fork_height, ..
    }
    | ChainEvent::SplitResolved {
        key, fork_height, ..
    } = &event
    {
        let mut open = read_open_splits(&mut *tx).await?;
        if matches!(event, ChainEvent::Split { .. }) {
            open.insert(key.clone(), *fork_height);
        } else {
            open.remove(key);
        }
        sqlx::query(
            "INSERT INTO tracker_meta (key, value) VALUES ('open_splits', ?) \
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        )
        .bind(serde_json::to_string(&open)?)
        .execute(&mut *tx)
        .await?;
    }
    sqlx::query(
        "INSERT INTO tracker_meta (key, value) VALUES ('feed_cursor', ?) \
         ON CONFLICT(key) DO UPDATE SET value = excluded.value",
    )
    .bind(cursor.to_string())
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(ChainDelivery {
        cursor,
        kind,
        moved,
    })
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::broadcast_reconcile::DEFAULT_ABSENCE_MINUTES;
    use bsv_wallet_toolbox::services::mock::{MockResponse, MockWalletServices};
    use bsv_wallet_toolbox::services::{BlockHeader, GetMerklePathResult};
    use bsv_wallet_toolbox::{
        WalletStorageWriter, BROADCAST_STATUS_ACCEPTED, BROADCAST_STATUS_MINED,
    };
    use chrono::Utc;

    /// A clock the test sets.
    pub(crate) struct FixedClock(pub Timestamp);
    impl Clock for FixedClock {
        fn now(&self) -> Timestamp {
            self.0
        }
    }

    pub(crate) fn opts() -> TickOptions {
        TickOptions {
            age_threshold: 600,
            max_asks: 20,
            spend_depth: DEFAULT_SPEND_DEPTH,
            feed_held: false,
            monitor_runs: false,
        }
    }

    /// A migrated in-memory wallet with one user.
    pub(crate) async fn wallet_storage() -> (StorageSqlx, i64) {
        let storage = StorageSqlx::in_memory().await.unwrap();
        storage
            .migrate("tracker-tests", &("02".to_string() + &"ab".repeat(32)))
            .await
            .unwrap();
        storage.make_available().await.unwrap();
        let (user, _) = storage
            .find_or_insert_user(&("02".to_string() + &"cd".repeat(32)))
            .await
            .unwrap();
        (storage, user.user_id)
    }

    pub(crate) async fn insert_tx(storage: &StorageSqlx, user_id: i64, txid: &str, status: &str) {
        sqlx::query(
            "INSERT INTO transactions (user_id, status, reference, is_outgoing, satoshis, version, lock_time, description, txid, raw_tx, created_at, updated_at) \
             VALUES (?, ?, ?, 1, 0, 1, 0, 'd', ?, X'01000000', ?, ?)",
        )
        .bind(user_id)
        .bind(status)
        .bind(&txid[..8])
        .bind(txid)
        .bind(Utc::now())
        .bind(Utc::now())
        .execute(storage.writer())
        .await
        .unwrap();
    }

    /// A coin of ours whose creator `txid` the toolbox's monitor proved
    /// before the tracker saw it: the transaction `completed` with
    /// `proven_tx_id` set to a stored proof at `height` (its root the
    /// txid), one unspent change output of `satoshis` in the default
    /// basket. A received coin (`is_outgoing` 0), as `fund` makes one.
    pub(crate) async fn proven_coin(
        storage: &StorageSqlx,
        user_id: i64,
        txid: &str,
        height: u32,
        satoshis: i64,
    ) {
        let basket = storage
            .find_or_create_default_basket(user_id)
            .await
            .unwrap()
            .basket_id;
        let proven = sqlx::query(
            "INSERT INTO proven_txs (txid, height, idx, block_hash, merkle_root, merkle_path, raw_tx, created_at, updated_at) \
             VALUES (?, ?, 0, ?, ?, ?, X'01000000', ?, ?)",
        )
        .bind(txid)
        .bind(height as i64)
        .bind("bb".repeat(32))
        .bind(txid)
        .bind(MerklePath::from_coinbase_txid(txid, height).to_binary())
        .bind(Utc::now())
        .bind(Utc::now())
        .execute(storage.writer())
        .await
        .unwrap()
        .last_insert_rowid();
        let tx = sqlx::query(
            "INSERT INTO transactions (user_id, status, reference, is_outgoing, satoshis, version, lock_time, description, txid, raw_tx, proven_tx_id, created_at, updated_at) \
             VALUES (?, 'completed', ?, 0, ?, 1, 0, 'Internalize external funding', ?, X'01000000', ?, ?, ?)",
        )
        .bind(user_id)
        .bind(&txid[..8])
        .bind(satoshis)
        .bind(txid)
        .bind(proven)
        .bind(Utc::now())
        .bind(Utc::now())
        .execute(storage.writer())
        .await
        .unwrap()
        .last_insert_rowid();
        sqlx::query(
            "INSERT INTO outputs (user_id, transaction_id, basket_id, vout, satoshis, locking_script, txid, type, spendable, change, provided_by, purpose, output_description, created_at, updated_at) \
             VALUES (?, ?, ?, 0, ?, X'76a914dbc0a7c84983c5bf199b7b2d41b3acf0408ee5aa88ac', ?, 'P2PKH', 1, 1, 'storage', 'change', 'c', ?, ?)",
        )
        .bind(user_id)
        .bind(tx)
        .bind(basket)
        .bind(satoshis)
        .bind(txid)
        .bind(Utc::now())
        .bind(Utc::now())
        .execute(storage.writer())
        .await
        .unwrap();
    }

    /// A header at `height` whose merkle root is `root`.
    pub(crate) fn header(height: u32, root: &str, nonce: u32) -> BlockHeader {
        BlockHeader {
            version: 1,
            previous_hash: "00".repeat(32),
            merkle_root: root.to_string(),
            time: 1_700_000_000,
            bits: 0x1d00_ffff,
            nonce,
            hash: String::new(),
            height,
        }
    }

    /// A path proving `txid` alone in a block at `height`: its root is the
    /// txid.
    pub(crate) fn path_of(txid: &str, height: u32) -> String {
        MerklePath::from_coinbase_txid(txid, height).to_hex()
    }

    pub(crate) fn proof_answer(path: Option<String>) -> MockResponse<GetMerklePathResult> {
        MockResponse::Success(GetMerklePathResult {
            name: Some("Services".to_string()),
            merkle_path: path,
            header: None,
            error: None,
            notes: vec![],
        })
    }

    /// E3 (the rulings of 2026-10-09). The wallet's own unproven
    /// transaction is tracked, a broadcaster's acceptance announces it, and
    /// its age asks for a proof: one request, for that transaction. The
    /// proof checks against the header service's header and the word is
    /// `mined`. The row is in the wallet's database, and the next pass
    /// reads no mined row and asks nothing.
    #[tokio::test]
    async fn a_pass_tracks_announces_and_proves_one_named_transaction() {
        let (storage, user) = wallet_storage().await;
        let txid = "a1".repeat(32);
        insert_tx(&storage, user, &txid, "unproven").await;
        storage
            .record_broadcast_status(&txid, "arcade", BROADCAST_STATUS_ACCEPTED)
            .await
            .unwrap();
        let services = MockWalletServices::builder()
            .get_merkle_path_response(proof_answer(Some(path_of(&txid, 900))))
            .build();
        services.set_header_for_height(header(900, &txid, 0));
        let now = Utc::now().timestamp() as u64;

        // Young: announced on the hint, nothing asked.
        let report = tick(&storage, &services, &FixedClock(now), &opts())
            .await
            .unwrap();
        assert_eq!((report.adopted, report.hints, report.asked), (1, 1, 0));
        assert_eq!(
            stored_word(&storage, &txid).await.unwrap(),
            Some(("announced".to_string(), None, None))
        );
        assert_eq!(services.call_count("get_merkle_path"), 0);

        // Past the age threshold: the re-ask runs, the proof checks.
        let report = tick(&storage, &services, &FixedClock(now + 601), &opts())
            .await
            .unwrap();
        assert_eq!(report.asked, 1);
        assert_eq!(report.mined, vec![txid.clone()]);
        assert_eq!(
            stored_word(&storage, &txid).await.unwrap(),
            Some(("mined".to_string(), Some(900), None))
        );
        assert_eq!(services.call_count("get_merkle_path"), 1);

        // A mined row is not read again while no header moved.
        let report = tick(&storage, &services, &FixedClock(now + 1300), &opts())
            .await
            .unwrap();
        assert_eq!((report.tracked, report.asked), (0, 0));
        assert_eq!(services.call_count("get_merkle_path"), 1);
    }

    /// The evidence rule in the host: a broadcaster's "mined" is a hint. It
    /// asks for the proof at once; with no proof the word does not move,
    /// and the same transaction is not asked again until its pause is over.
    #[tokio::test]
    async fn a_broadcasters_mined_is_a_hint_that_asks_for_the_proof() {
        let (storage, user) = wallet_storage().await;
        let txid = "b2".repeat(32);
        insert_tx(&storage, user, &txid, "unproven").await;
        storage
            .record_broadcast_status(&txid, "arcade", BROADCAST_STATUS_MINED)
            .await
            .unwrap();
        let services = MockWalletServices::builder()
            .get_merkle_path_response(proof_answer(None))
            .build();
        let now = Utc::now().timestamp() as u64;

        let report = tick(&storage, &services, &FixedClock(now), &opts())
            .await
            .unwrap();
        assert_eq!(report.asked, 1);
        assert_eq!(report.no_proof, vec![txid.clone()]);
        assert!(report.mined.is_empty());
        let (word, height, reask) = stored_word(&storage, &txid).await.unwrap().unwrap();
        assert_eq!((word.as_str(), height), ("built", None));
        assert_eq!(reask.as_deref(), Some("hint:arcade|mined"));

        // One minute later: the pause is not over, nothing is asked.
        let report = tick(&storage, &services, &FixedClock(now + 60), &opts())
            .await
            .unwrap();
        assert_eq!(report.asked, 0);
        // After the pause it is asked again, and the pause doubles.
        let report = tick(&storage, &services, &FixedClock(now + 600), &opts())
            .await
            .unwrap();
        assert_eq!(report.asked, 1);
        let report = tick(&storage, &services, &FixedClock(now + 1200), &opts())
            .await
            .unwrap();
        assert_eq!(report.asked, 0);
        assert_eq!(services.call_count("get_merkle_path"), 2);
    }

    /// A header we stood on moved: the mined rows at or above it are read
    /// again and asked again, and a mined row below it is not touched.
    #[tokio::test]
    async fn a_moved_header_reasks_the_rows_at_or_above_it_and_no_other() {
        let (storage, user) = wallet_storage().await;
        let low = "c3".repeat(32);
        let high = "d4".repeat(32);
        let services = MockWalletServices::builder()
            .get_merkle_path_response(MockResponse::Sequence(vec![
                proof_answer(Some(path_of(&low, 100))),
                proof_answer(Some(path_of(&high, 200))),
                proof_answer(Some(path_of(&high, 201))),
            ]))
            .build();
        services.set_header_for_height(header(100, &low, 0));
        services.set_header_for_height(header(200, &high, 0));
        services.set_tip_header(header_with_hash(200, &high, 0));
        let now = Utc::now().timestamp() as u64;
        for txid in [&low, &high] {
            insert_tx(&storage, user, txid, "unproven").await;
            storage
                .record_broadcast_status(txid, "arcade", BROADCAST_STATUS_MINED)
                .await
                .unwrap();
        }
        let report = tick(&storage, &services, &FixedClock(now), &opts())
            .await
            .unwrap();
        assert_eq!(report.mined.len(), 2, "{report:?}");

        // Height 200 is replaced by a block without `high`; it is mined
        // again at 201.
        services.set_header_for_height(header(200, &"ee".repeat(32), 7));
        services.set_header_for_height(header(201, &high, 0));
        services.set_tip_header(header_with_hash(201, &high, 0));
        let report = tick(&storage, &services, &FixedClock(now + 60), &opts())
            .await
            .unwrap();
        assert_eq!(report.moved_from, Some(200));
        assert_eq!(
            report.tracked, 1,
            "only the row at or above the moved header"
        );
        assert_eq!(report.moved, vec![high.clone()]);
        assert_eq!(report.mined, vec![high.clone()]);
        assert_eq!(
            stored_word(&storage, &high).await.unwrap().unwrap().1,
            Some(201)
        );
        assert_eq!(
            stored_word(&storage, &low).await.unwrap().unwrap().1,
            Some(100)
        );
    }

    /// E2 (the rulings of 2026-10-09). A transaction a broadcaster called
    /// not final is held with its BEEF: nothing is asked or announced
    /// before its lock time; at the lock time it is announced again; "not
    /// final" again (the node's clock is behind ours) waits one pause; an
    /// acceptance announces it, and from there its age asks for the proof.
    #[tokio::test]
    async fn a_not_final_transaction_is_announced_again_at_its_lock_time() {
        use bsv_wallet_toolbox::services::mock::{
            error_post_beef_result, success_post_beef_result,
        };
        let (storage, _user) = wallet_storage().await;
        let txid = "c7".repeat(32);
        let lock = 2_000_000_000u64;
        let services = MockWalletServices::builder()
            .post_beef_response(MockResponse::Sequence(vec![
                MockResponse::Success(vec![error_post_beef_result("Arcade", "476 non-final")]),
                MockResponse::Success(vec![success_post_beef_result("Arcade", &[&txid])]),
            ]))
            .get_merkle_path_response(proof_answer(None))
            .build();

        let hint = Hint::new(
            "Arcade|rejected",
            HintStatus::Rejected {
                reason: "476 non-final".to_string(),
            },
            lock - 3600,
        );
        let again = Reannounce {
            beef: vec![1, 2, 3],
            at: lock,
            on_accept: None,
        };
        let word = heard(
            &storage,
            &FixedClock(lock - 3600),
            &opts(),
            &txid,
            hint,
            Some(lock),
            Some(again),
        )
        .await
        .unwrap();
        assert_eq!(
            word,
            (
                "built".to_string(),
                Some("hint:Arcade|rejected".to_string())
            )
        );

        // Before the lock time: nothing announced, no proof asked.
        let report = tick(&storage, &services, &FixedClock(lock - 1), &opts())
            .await
            .unwrap();
        assert_eq!((report.tracked, report.asked), (1, 0), "{report:?}");
        assert_eq!(services.call_count("post_beef"), 0);

        // At the lock time: announced again; the node still says not final.
        let report = tick(&storage, &services, &FixedClock(lock), &opts())
            .await
            .unwrap();
        assert_eq!(report.not_final, vec![txid.clone()]);
        assert_eq!(services.call_count("post_beef"), 1);
        let report = tick(&storage, &services, &FixedClock(lock + 60), &opts())
            .await
            .unwrap();
        assert!(report.not_final.is_empty() && report.reannounced.is_empty());
        assert_eq!(services.call_count("post_beef"), 1, "one pause between two");

        // One pause on: accepted. The word is the tracker's `announced`.
        let report = tick(&storage, &services, &FixedClock(lock + 600), &opts())
            .await
            .unwrap();
        assert_eq!(report.reannounced, vec![txid.clone()]);
        assert_eq!(services.call_count("post_beef"), 2);
        assert_eq!(
            stored_word(&storage, &txid).await.unwrap().unwrap().0,
            "announced"
        );
        assert_eq!(services.call_count("get_merkle_path"), 0);

        // It is not announced a third time; its age asks for the proof.
        let report = tick(&storage, &services, &FixedClock(lock + 1300), &opts())
            .await
            .unwrap();
        assert_eq!(report.asked, 1);
        assert_eq!(services.call_count("post_beef"), 2);
        assert_eq!(services.call_count("get_merkle_path"), 1);
    }

    /// A courier's mempool presence (tracker 0.5.0) survives the stored row,
    /// its courier with it; a presence row without one reads as `unknown`.
    #[test]
    fn a_mempool_presence_hint_round_trips_through_the_stored_row() {
        let hint = Hint::new(
            "arcade".to_string(),
            HintStatus::MempoolPresent {
                source: "courier-a".to_string(),
            },
            1_700_000_000,
        );
        let stored = StoredHint::of(&hint);
        let json = serde_json::to_string(&stored).unwrap();
        let back: StoredHint = serde_json::from_str(&json).unwrap();
        assert_eq!(back.hint(), hint);

        let bare = StoredHint {
            courier: None,
            ..stored
        };
        assert_eq!(bare.hint().status, HintStatus::Unknown);
    }

    /// Ruling (2) of 2026-10-20 19:4xZ (bsv-stack-lean
    /// `docs/RULINGS-2026-10-12.md`; bsv-tracker 0.7.0): a courier's mempool
    /// presence on a `built` transaction reads `announced`, as a
    /// broadcaster's acceptance does; until 0.7.0 it left `built`. A stored
    /// row replays to the new word, and the tier it serves moves with it.
    #[test]
    fn a_stored_presence_hint_on_a_built_transaction_replays_to_announced() {
        let mut row = TrackedRow::new(&"e7".repeat(32));
        row.hints.push(StoredHint::of(&Hint::new(
            "courier-a|mempool_present".to_string(),
            HintStatus::MempoolPresent {
                source: "courier-a".to_string(),
            },
            1_700_000_000,
        )));
        let (state, fault) = row.replay(&opts().params(), &NoHeaders);
        assert_eq!(fault, None);
        assert_eq!(word_label(state.word()), "announced");
        assert_eq!(row.tier(), "announced");
    }

    /// The words that mean "not final", and one that does not.
    #[test]
    fn not_final_is_read_from_the_refusal() {
        for said in [
            "476 non-final",
            "bad-txns-nonfinal",
            "tx is not final",
            "code 476",
        ] {
            assert!(says_not_final(said), "{said}");
        }
        for said in [
            "fee too low",
            "DOUBLE_SPEND_ATTEMPTED",
            "txid 4476aa",
            "HTTP 500",
        ] {
            assert!(!says_not_final(said), "{said}");
        }
    }

    /// A tip header carrying the hash its bytes have.
    pub(crate) fn header_with_hash(height: u32, root: &str, nonce: u32) -> BlockHeader {
        let mut h = header(height, root, nonce);
        h.hash = header_of(&h.to_binary()).unwrap().hash;
        h
    }

    /// R3 (b): the spend depth is the host's, named in its config, 1 the
    /// floor. A coin whose block is the tip (one block deep) is offered at
    /// depth 1 and held at depth 3; two blocks later it is offered at 3; a
    /// depth of 0 reads as 1.
    #[tokio::test]
    async fn the_spend_depth_holds_a_coin_until_its_block_is_deep_enough() {
        let (storage, user) = wallet_storage().await;
        let txid = "e7".repeat(32);
        insert_tx(&storage, user, &txid, "unproven").await;
        storage
            .record_broadcast_status(&txid, "arcade", BROADCAST_STATUS_MINED)
            .await
            .unwrap();
        let services = MockWalletServices::builder()
            .get_merkle_path_response(proof_answer(Some(path_of(&txid, 900))))
            .build();
        services.set_header_for_height(header(900, &txid, 0));
        services.set_tip_header(header_with_hash(900, &txid, 0));
        let clock = FixedClock(Utc::now().timestamp() as u64);
        tick(&storage, &services, &clock, &opts()).await.unwrap();
        let at = |spend_depth| TickOptions {
            spend_depth,
            ..opts()
        };

        let one = spend_guard(&storage, &services, &clock, &at(1), &txid)
            .await
            .unwrap();
        let three = spend_guard(&storage, &services, &clock, &at(3), &txid)
            .await
            .unwrap();
        let zero = spend_guard(&storage, &services, &clock, &at(0), &txid)
            .await
            .unwrap();
        println!("one block deep: depth 1 {one:?}, depth 3 {three:?}, depth 0 {zero:?}");
        assert_eq!(one, Ok(true));
        assert_eq!(three, Ok(false));
        assert_eq!(zero, Ok(true), "0 reads as the floor, 1");

        services.set_tip_header(header_with_hash(902, &"0c".repeat(32), 0));
        let three = spend_guard(&storage, &services, &clock, &at(3), &txid)
            .await
            .unwrap();
        println!("three blocks deep: depth 3 {three:?}");
        assert_eq!(three, Ok(true));
        assert_eq!(
            TickOptions {
                spend_depth: 0,
                ..opts()
            }
            .floored()
            .spend_depth,
            1
        );
    }

    /// The wallet spend guard ([`guard_state`]), run by the host over a
    /// stored row. Three runs: the
    /// header is the one the proof was checked against; the header service
    /// cannot answer; the header moved.
    #[tokio::test]
    async fn the_spend_guard_runs_as_the_host() {
        let (storage, user) = wallet_storage().await;
        let txid = "e5".repeat(32);
        insert_tx(&storage, user, &txid, "unproven").await;
        storage
            .record_broadcast_status(&txid, "arcade", BROADCAST_STATUS_MINED)
            .await
            .unwrap();
        let services = MockWalletServices::builder()
            .get_merkle_path_response(proof_answer(Some(path_of(&txid, 900))))
            .build();
        services.set_header_for_height(header(900, &txid, 0));
        let clock = FixedClock(Utc::now().timestamp() as u64);
        tick(&storage, &services, &clock, &opts()).await.unwrap();

        // 1. Mined, checked, not suspect: the coin may be spent.
        let guard = spend_guard(&storage, &services, &clock, &opts(), &txid)
            .await
            .unwrap();
        println!(
            "spend guard, header in place:   {guard:?}, stored {:?}",
            stored_word(&storage, &txid).await.unwrap().unwrap()
        );
        assert_eq!(guard, Ok(true));

        // 2. No tip from the header service: an error, and no spend.
        services.set_tip_unavailable();
        let guard = spend_guard(&storage, &services, &clock, &opts(), &txid)
            .await
            .unwrap();
        println!(
            "spend guard, no header service: {guard:?}, stored {:?}",
            stored_word(&storage, &txid).await.unwrap().unwrap()
        );
        assert!(matches!(guard, Err(CheckError::Headers(_))), "{guard:?}");
        assert_eq!(
            stored_word(&storage, &txid).await.unwrap().unwrap().0,
            "mined",
            "could not look changes no word"
        );

        // 3. The header at the height moved: not mined, a re-ask pending.
        services.set_tip_header(header_with_hash(901, &"00".repeat(32), 1));
        services.set_header_for_height(header(900, &"ee".repeat(32), 7));
        let guard = spend_guard(&storage, &services, &clock, &opts(), &txid)
            .await
            .unwrap();
        println!(
            "spend guard, header moved:      {guard:?}, stored {:?}",
            stored_word(&storage, &txid).await.unwrap().unwrap()
        );
        assert_eq!(guard, Ok(false));

        // A transaction the host does not track is not mined.
        let guard = spend_guard(&storage, &services, &clock, &opts(), &"f6".repeat(32))
            .await
            .unwrap();
        assert_eq!(guard, Ok(false));

        // The guard's own error path, on a state held in memory: mined,
        // then the header service loses the height.
        let params = opts().params();
        services.set_tip_header(header_with_hash(900, &txid, 0));
        services.set_header_for_height(header(900, &txid, 0));
        let held = HeaderSnapshot::take(&services, [900]).await;
        let row = load_row(&storage, &txid).await.unwrap().unwrap();
        let (mut state, fault) = row.replay(&params, &held);
        assert_eq!(fault, None);
        let lost = HeaderSnapshot::take(&services, []).await;
        let guard = guard_state(&mut state, &params, &lost);
        println!("spend guard, height unanswered: {guard:?}");
        assert_eq!(guard, Err(CheckError::Unavailable(900)));
    }

    /// A creator mined at 900, the tip at 905, the header service's header
    /// at 900 the one its proof checks against.
    pub(crate) async fn mined_at_900(txid: &str) -> (StorageSqlx, MockWalletServices, FixedClock) {
        let (storage, user) = wallet_storage().await;
        insert_tx(&storage, user, txid, "unproven").await;
        storage
            .record_broadcast_status(txid, "arcade", BROADCAST_STATUS_MINED)
            .await
            .unwrap();
        let services = MockWalletServices::builder()
            .get_merkle_path_response(proof_answer(Some(path_of(txid, 900))))
            .build();
        services.set_header_for_height(header(900, txid, 0));
        services.set_tip_header(header_with_hash(905, &"0a".repeat(32), 0));
        let clock = FixedClock(Utc::now().timestamp().cast_unsigned());
        tick(&storage, &services, &clock, &opts()).await.unwrap();
        assert_eq!(
            stored_word(&storage, txid).await.unwrap().unwrap().0,
            "mined"
        );
        (storage, services, clock)
    }

    /// An event header, as the feed carries one.
    fn event_header(height: u32, hash: &str, root: &str) -> serde_json::Value {
        serde_json::json!({
            "version": 1, "previousHash": "00".repeat(32), "merkleRoot": root,
            "time": 1_700_000_000u32, "bits": 0x1d00_ffffu32, "nonce": 0, "height": height,
            "hash": hash, "chainWork": format!("{}10", "00".repeat(31)),
        })
    }

    /// A creator whose proof a reorg took, as the host stores it: the
    /// tracker row with the path at `height` (its root the txid) and the
    /// stored reorg that deactivated the header the path was checked in.
    /// The caller's header service answers another root at `height`, so the
    /// row replays `stale` (the recheck on the new branch).
    pub(crate) async fn stale_row(storage: &StorageSqlx, txid: &str, height: u32) {
        ensure_tables(storage).await.unwrap();
        let gone = header_with_hash(height, txid, 0).hash;
        let reorg = serde_json::json!({
            "v": 1, "kind": "reorg", "forkHeight": height, "depth": 1,
            "deactivatedHeaders": [event_header(height, &gone, txid)],
            "newTip": event_header(height + 1, &"b3".repeat(32), &"ee".repeat(32)),
        });
        sqlx::query(
            "INSERT INTO tracker_states (txid, bump, word, chain) VALUES (?, ?, 'stale', ?)",
        )
        .bind(txid)
        .bind(path_of(txid, height))
        .bind(serde_json::json!([reorg.to_string()]).to_string())
        .execute(storage.writer())
        .await
        .unwrap();
    }

    async fn deliver(
        storage: &StorageSqlx,
        services: &MockWalletServices,
        clock: &FixedClock,
        cursor: u64,
        envelope: serde_json::Value,
    ) {
        let event = decode_envelope(envelope.to_string().as_bytes()).unwrap();
        deliver_chain_event(storage, services, clock, &opts(), cursor, event)
            .await
            .unwrap();
    }

    fn fork_at(height: u32, txid: &str) -> serde_json::Value {
        serde_json::json!({
            "v": 1, "kind": "fork", "height": height, "depth": 1,
            "competingTips": [
                event_header(height, &"d1".repeat(32), txid),
                event_header(height, &"d2".repeat(32), &"ee".repeat(32)),
            ],
        })
    }

    async fn guard_of(
        storage: &StorageSqlx,
        services: &MockWalletServices,
        clock: &FixedClock,
        txid: &str,
    ) -> std::result::Result<bool, CheckError> {
        spend_guard(storage, services, clock, &opts(), txid)
            .await
            .unwrap()
    }

    async fn word(storage: &StorageSqlx, txid: &str) -> (String, Option<Height>, Option<String>) {
        stored_word(storage, txid).await.unwrap().unwrap()
    }

    async fn reading(
        storage: &StorageSqlx,
        services: &MockWalletServices,
        txid: &str,
    ) -> CreatorReading {
        read_creators(storage, services, &opts(), &[(txid.to_string(), None)])
            .await
            .unwrap()
            .remove(txid)
            .unwrap()
    }

    /// #164: a fork in progress at the creator's height holds its coin
    /// until the chain moves. The guard refuses the coin and clears
    /// nothing: the stored row keeps `fork@900` and its `chain`, the mark
    /// survives a reload, and `read_creators` reads it suspect, not
    /// offered, while our header at 900 still carries the root. The next
    /// tip rechecks the word through the feed, the tracker's own step, and
    /// the coin is offered again.
    #[tokio::test]
    async fn a_fork_at_the_creators_height_holds_its_coin_until_a_tip_rechecks_it() {
        let txid = "e8".repeat(32);
        let (storage, services, clock) = mined_at_900(&txid).await;
        deliver(&storage, &services, &clock, 1, fork_at(900, &txid)).await;
        let fork = ("mined".to_string(), Some(900), Some("fork@900".to_string()));
        assert_eq!(word(&storage, &txid).await, fork);

        let guard = guard_of(&storage, &services, &clock, &txid).await;
        let stored = word(&storage, &txid).await;
        println!("spend guard under a fork at 900: {guard:?}, stored {stored:?}");
        assert_eq!(guard, Ok(false));
        assert_eq!(
            stored, fork,
            "the refused spend clears no mark, writes no re-ask"
        );

        let row = load_row(&storage, &txid).await.unwrap().unwrap();
        assert_eq!(row.chain.len(), 1, "the fork stays on the row");
        let snapshot = HeaderSnapshot::take(&services, row.proof_height()).await;
        let (state, fault) = row.replay(&opts().params(), &snapshot);
        assert_eq!(fault, None);
        assert!(state.suspect(), "the mark survives a reload");
        assert_eq!(state.reask(), Some(&Reask::Fork { height: 900 }));

        let held = reading(&storage, &services, &txid).await;
        println!("read_creators under the fork: {held:?}");
        assert_eq!(
            (held.word, held.suspect, held.offered(), held.fault),
            ("mined", true, false, None)
        );
        let again = guard_of(&storage, &services, &clock, &txid).await;
        assert_eq!(again, Ok(false), "a second attempt is refused the same");

        let tip = serde_json::json!({
            "v": 1, "kind": "tip", "height": 906, "hash": "d3".repeat(32),
            "time": 1_700_000_000u32,
            "header": event_header(906, &"d3".repeat(32), &"ef".repeat(32)),
        });
        deliver(&storage, &services, &clock, 2, tip).await;
        assert_eq!(
            word(&storage, &txid).await,
            ("mined".to_string(), Some(900), None)
        );
        let offered = reading(&storage, &services, &txid).await;
        println!("read_creators after the tip: {offered:?}");
        assert_eq!((offered.suspect, offered.offered()), (false, true));
        let guard = guard_of(&storage, &services, &clock, &txid).await;
        assert_eq!(guard, Ok(true));
    }

    /// The reading's door sets the mark aside: a suspect `mined` creator 6
    /// blocks deep has its door open and is not offered (the stack's
    /// `offerableUnder`, `!a.tracker.suspect && doorSpendable ...`); at a
    /// spend depth of 7 the same creator is too shallow and both are false.
    #[tokio::test]
    async fn a_suspect_creators_door_is_its_depth_and_its_answer_is_held() {
        let txid = "eb".repeat(32);
        let (storage, services, clock) = mined_at_900(&txid).await;
        deliver(&storage, &services, &clock, 1, fork_at(900, &txid)).await;
        let deep = reading(&storage, &services, &txid).await;
        println!("read_creators under the fork, spend depth 1: {deep:?}");
        assert_eq!(
            (deep.depth, deep.suspect, deep.door, deep.offered()),
            (6, true, true, false)
        );
        let shallow_opts = TickOptions {
            spend_depth: 7,
            ..opts()
        };
        let shallow = read_creators(&storage, &services, &shallow_opts, &[(txid.clone(), None)])
            .await
            .unwrap()
            .remove(&txid)
            .unwrap();
        println!("read_creators under the fork, spend depth 7: {shallow:?}");
        assert_eq!(
            (shallow.suspect, shallow.door, shallow.offered()),
            (true, false, false)
        );
    }

    /// A fork above the creator's block marks nothing: the coin is offered
    /// and the guard answers as before.
    #[tokio::test]
    async fn a_fork_above_the_creators_height_leaves_its_coin_offered() {
        let txid = "e9".repeat(32);
        let (storage, services, clock) = mined_at_900(&txid).await;
        deliver(&storage, &services, &clock, 1, fork_at(901, &txid)).await;
        let offered = reading(&storage, &services, &txid).await;
        println!("read_creators under a fork at 901: {offered:?}");
        assert_eq!(
            (offered.word, offered.suspect, offered.offered()),
            ("mined", false, true)
        );
        let guard = guard_of(&storage, &services, &clock, &txid).await;
        assert_eq!(guard, Ok(true));
        assert_eq!(
            word(&storage, &txid).await,
            ("mined".to_string(), Some(900), None)
        );
    }

    /// A reorg's open recheck: the reorg at 900 deactivated the header the
    /// proof was checked against, and the header service still answers it,
    /// so the new branch has no answer at 900 and the word stands suspect
    /// with `Reask::Recheck`. The guard reads the plain snapshot, where that
    /// deactivated header still carries the root; it refuses the coin and
    /// leaves the mark for the next tip.
    #[tokio::test]
    async fn a_reorgs_open_recheck_is_not_cleared_by_the_guard() {
        let txid = "ea".repeat(32);
        let (storage, services, clock) = mined_at_900(&txid).await;
        let gone = header_of(&header(900, &txid, 0).to_binary()).unwrap().hash;
        let reorg = serde_json::json!({
            "v": 1, "kind": "reorg", "forkHeight": 900, "depth": 1,
            "deactivatedHeaders": [event_header(900, &gone, &txid)],
            "newTip": event_header(901, &"b3".repeat(32), &"ee".repeat(32)),
        });
        deliver(&storage, &services, &clock, 1, reorg).await;
        let open = ("mined".to_string(), Some(900), Some("recheck".to_string()));
        assert_eq!(word(&storage, &txid).await, open);

        let guard = guard_of(&storage, &services, &clock, &txid).await;
        let stored = word(&storage, &txid).await;
        println!("spend guard on the reorg's open recheck: {guard:?}, stored {stored:?}");
        assert_eq!(guard, Ok(false));
        assert_eq!(stored, open);
        let held = reading(&storage, &services, &txid).await;
        assert_eq!((held.suspect, held.offered()), (true, false));
    }

    async fn invalidated_at_900(txid: &str) -> (StorageSqlx, MockWalletServices, FixedClock) {
        let (storage, services, clock) = mined_at_900(txid).await;
        let gone = header_of(&header(900, txid, 0).to_binary()).unwrap().hash;
        deliver(
            &storage,
            &services,
            &clock,
            1,
            serde_json::json!({ "v": 1, "kind": "invalidated", "blockHash": gone }),
        )
        .await;
        (storage, services, clock)
    }

    async fn tip_after_invalidation(
        storage: &StorageSqlx,
        services: &MockWalletServices,
        clock: &FixedClock,
        root: &str,
    ) {
        services.set_header_for_height(header(900, root, 1));
        let tip = header_with_hash(906, &"0b".repeat(32), 1);
        services.set_tip_header(tip.clone());
        deliver(
            storage,
            services,
            clock,
            2,
            serde_json::json!({
                "v": 1, "kind": "tip", "height": 906, "hash": tip.hash,
                "time": tip.time, "header": event_header(906, &tip.hash, &tip.merkle_root),
            }),
        )
        .await;
    }

    async fn coin_of_tracked_creator(storage: &StorageSqlx, txid: &str) -> i64 {
        let user: i64 = sqlx::query_scalar("SELECT user_id FROM transactions WHERE txid = ?")
            .bind(txid)
            .fetch_one(storage.pool())
            .await
            .unwrap();
        let basket = storage
            .find_or_create_default_basket(user)
            .await
            .unwrap()
            .basket_id;
        sqlx::query(
            "INSERT INTO outputs (user_id, transaction_id, basket_id, vout, satoshis, locking_script, \
             txid, type, spendable, change, provided_by, purpose, output_description, created_at, updated_at) \
             SELECT user_id, transaction_id, ?, 0, 1000, X'51', txid, 'P2PKH', 1, 1, 'storage', \
             'change', 'invalidation test', created_at, updated_at FROM transactions WHERE txid = ?",
        )
        .bind(basket)
        .bind(txid)
        .execute(storage.writer())
        .await
        .unwrap();
        user
    }

    async fn assess_tracked_coin(
        storage: &StorageSqlx,
        services: &MockWalletServices,
        user: i64,
    ) -> crate::spend_gate::Assessment {
        let gate = crate::spend_gate::GateOptions {
            spend_depth: 1,
            own_change: true,
            allow_pending: crate::spend_gate::AllowPending::Off,
            tracker: opts(),
        };
        crate::spend_gate::assess(
            crate::spend_gate::UserCoins::new(storage, user),
            services,
            &gate,
            None,
        )
        .await
        .unwrap()
    }

    /// #206: the alert cannot re-prove a word with the header it invalidated.
    /// Its mark survives storage, the guard and the gate's assessment; a new
    /// header carrying the same root lets the next tip clear it.
    #[tokio::test]
    async fn an_invalidated_block_holds_its_coin_until_a_tip_rechecks_it() {
        let txid = "f1".repeat(32);
        let (storage, services, clock) = invalidated_at_900(&txid).await;
        let open = ("mined".to_string(), Some(900), Some("recheck".to_string()));
        println!(
            "invalidated with its header still served: {:?}",
            word(&storage, &txid).await
        );
        assert_eq!(word(&storage, &txid).await, open);

        let row = load_row(&storage, &txid).await.unwrap().unwrap();
        assert_eq!(row.chain.len(), 1);
        let held = reading(&storage, &services, &txid).await;
        println!("read_creators after invalidation: {held:?}");
        assert!(
            held.suspect && !held.offered(),
            "the mark survives a reload"
        );
        assert_eq!(held.word, "mined");
        assert!(held.fault.is_none());
        assert_eq!(
            guard_of(&storage, &services, &clock, &txid).await,
            Ok(false)
        );
        assert_eq!(word(&storage, &txid).await, open);

        let user = coin_of_tracked_creator(&storage, &txid).await;
        let held = assess_tracked_coin(&storage, &services, user).await;
        println!("assess after invalidation: {held:?}");
        assert!(held.offered.is_empty());
        assert_eq!(held.held.len(), 1);
        assert!(held.held[0].reason.starts_with("suspect: "));
        assert_eq!(word(&storage, &txid).await, open);

        tip_after_invalidation(&storage, &services, &clock, &txid).await;
        assert_eq!(
            word(&storage, &txid).await,
            ("mined".to_string(), Some(900), None)
        );
        let row = load_row(&storage, &txid).await.unwrap().unwrap();
        assert!(row.chain.is_empty());
        let recovered = reading(&storage, &services, &txid).await;
        assert!(!recovered.suspect && recovered.offered());
        let offered = assess_tracked_coin(&storage, &services, user).await;
        println!("assess after the new header's tip: {offered:?}");
        assert!(offered.held.is_empty());
        assert_eq!(offered.offered_satoshis(), 1000);
    }

    #[tokio::test]
    async fn an_invalidated_blocks_moved_root_reads_stale_after_the_next_tip() {
        let txid = "f2".repeat(32);
        let (storage, services, clock) = invalidated_at_900(&txid).await;
        tip_after_invalidation(&storage, &services, &clock, &"ef".repeat(32)).await;
        let stale = ("stale".to_string(), None, Some("reorg@900".to_string()));
        println!(
            "invalidated, then a moved root: {:?}",
            word(&storage, &txid).await
        );
        assert_eq!(word(&storage, &txid).await, stale);
        let read = reading(&storage, &services, &txid).await;
        assert_eq!(
            (read.word, read.suspect, read.offered(), read.fault),
            ("stale", false, false, None)
        );
        assert_eq!(word(&storage, &txid).await, stale);
    }

    #[tokio::test]
    async fn an_invalidated_hash_no_row_was_checked_against_changes_nothing() {
        let txid = "f3".repeat(32);
        let (storage, services, clock) = mined_at_900(&txid).await;
        let before = load_row(&storage, &txid).await.unwrap().unwrap();
        let before_word = word(&storage, &txid).await;
        let event = ChainEvent::Invalidated {
            block_hash: "ff".repeat(32),
        };
        let delivered = deliver_chain_event(&storage, &services, &clock, &opts(), 1, event)
            .await
            .unwrap();
        println!("an unrelated invalidation: {delivered:?}");
        assert!(delivered.moved.is_empty());
        assert_eq!(feed_cursor(&storage).await.unwrap(), 1);
        assert_eq!(load_row(&storage, &txid).await.unwrap().unwrap(), before);
        assert_eq!(word(&storage, &txid).await, before_word);
        let offered = reading(&storage, &services, &txid).await;
        assert_eq!((offered.suspect, offered.offered()), (false, true));
    }

    /// Ruling 7a at the reading: a creator whose proof a reorg took reads
    /// `stale` with no fault (its stored path checks against the header the
    /// reorg deactivated, and the recheck on the new branch is the word, not
    /// a fault; never the header service's), not suspect, its door closed
    /// as for every word but a deep checked `mined`. The door is the rule's:
    /// `hold_reason` offers our own change under `SPEND_OWN_CHANGE=1`.
    #[tokio::test]
    async fn a_stale_creators_reading_has_no_fault_and_its_door_is_the_rules() {
        let txid = "ee".repeat(32);
        let (storage, _user) = wallet_storage().await;
        stale_row(&storage, &txid, 900).await;
        let services = MockWalletServices::builder().build();
        services.set_header_for_height(header(900, &"0c".repeat(32), 0));
        services.set_tip_header(header_with_hash(905, &"0a".repeat(32), 0));
        let stale = reading(&storage, &services, &txid).await;
        println!("a stale creator's reading: {stale:?}");
        assert_eq!(
            stale,
            CreatorReading {
                word: "stale",
                depth: 0,
                suspect: false,
                door: false,
                fault: None,
                headers_fault: false,
            }
        );
        let gate = |own_change| crate::spend_gate::GateOptions {
            spend_depth: 1,
            own_change,
            allow_pending: crate::spend_gate::AllowPending::Off,
            tracker: opts(),
        };
        let hold = |own_change| {
            crate::spend_gate::hold_reason(
                &stale,
                crate::spend_gate::Origin::Made,
                (&txid, 0),
                &gate(own_change),
            )
        };
        assert_eq!(hold(true), None);
        assert_eq!(
            hold(false).as_deref(),
            Some("stale: its block left the chain; own change, SPEND_OWN_CHANGE=0")
        );
    }

    /// `CreatorReading::offered` is the answer `read_creators` stored as a
    /// field before (`door && !state.suspect() && !split`) on every kind of
    /// reading it produces: a plain deep creator is offered; a shallow one,
    /// a suspect one, one under an open split with no mark on its row, and
    /// one whose tip could not be read are not. The fault's reading has its
    /// door closed (its replay faults, the word stays `built`, unmarked): a
    /// closed door is the guard's answer when it cannot look.
    #[tokio::test]
    async fn the_offered_method_is_the_stored_answer_on_every_kind_of_reading() {
        let txid = "ec".repeat(32);
        let (storage, services, _clock) = mined_at_900(&txid).await;
        let plain = reading(&storage, &services, &txid).await;
        let deep_opts = TickOptions {
            spend_depth: 7,
            ..opts()
        };
        let shallow = read_creators(&storage, &services, &deep_opts, &[(txid.clone(), None)])
            .await
            .unwrap()
            .remove(&txid)
            .unwrap();

        let marked = "ed".repeat(32);
        let (m_storage, m_services, m_clock) = mined_at_900(&marked).await;
        deliver(&m_storage, &m_services, &m_clock, 1, fork_at(900, &marked)).await;
        let suspect = reading(&m_storage, &m_services, &marked).await;

        sqlx::query("INSERT INTO tracker_meta (key, value) VALUES ('open_splits', ?)")
            .bind(serde_json::json!({ format!("split:{}", "5a".repeat(32)): 900 }).to_string())
            .execute(&mut *crate::store::writer(&storage).await.unwrap())
            .await
            .unwrap();
        let split = reading(&storage, &services, &txid).await;
        assert_eq!(
            word(&storage, &txid).await,
            ("mined".to_string(), Some(900), None),
            "the split marks no row: the reading's suspect is the split's"
        );

        services.set_tip_unavailable();
        let fault = reading(&storage, &services, &txid).await;

        for (kind, r, door, suspect, offered) in [
            ("plain", &plain, true, false, true),
            ("shallow", &shallow, false, false, false),
            ("suspect", &suspect, true, true, false),
            ("open split", &split, true, true, false),
            ("fault", &fault, false, false, false),
        ] {
            println!("{kind}: {r:?}, offered() {}", r.offered());
            assert_eq!(
                (r.door, r.suspect, r.offered()),
                (door, suspect, offered),
                "{kind}"
            );
        }
        assert!(plain.fault.is_none() && fault.fault.is_some(), "{fault:?}");
    }

    /// F1 of the stack's coin read (bsv-stack-lean #164): the daemon's
    /// served pass tracks the wallet's creators. An `unproven` creator is
    /// adopted as before; a `completed` creator the toolbox's monitor proved
    /// first is adopted by replaying its stored path against the headers of
    /// the moment: `mined` with its evidence when the header at its height
    /// carries the root, the re-ask `proofFailed` and no word when it does
    /// not. A creator tracked while unproven keeps its row when the toolbox
    /// moves it to `completed`. The toolbox's rows are not written, and the
    /// next pass adopts nothing.
    #[tokio::test]
    async fn the_served_pass_adopts_an_unproven_creator_and_a_proven_completed_one() {
        let (storage, user) = wallet_storage().await;
        let unproven = "d1".repeat(32);
        let earlier = "d2".repeat(32);
        insert_tx(&storage, user, &unproven, "unproven").await;
        insert_tx(&storage, user, &earlier, "unproven").await;
        let services = MockWalletServices::builder().build();
        services.set_tip_header(header_with_hash(960, &"0a".repeat(32), 0));
        let now = Utc::now().timestamp() as u64;
        let pass = crate::broadcast_reconcile::run_served_pass(
            &storage,
            &services,
            &FixedClock(now),
            &opts(),
            None,
            DEFAULT_ABSENCE_MINUTES,
        )
        .await
        .unwrap();
        println!("{}", pass.summary());
        assert_eq!(pass.tracker.adopted, 2);
        assert_eq!(
            stored_word(&storage, &earlier).await.unwrap(),
            Some(("built".to_string(), None, None))
        );

        // The toolbox's monitor proves `earlier` (completed), and two coins
        // arrive already proven: one whose root the header service's
        // header at 900 carries, one whose header at 950 carries another.
        sqlx::query("UPDATE transactions SET status = 'completed' WHERE txid = ?")
            .bind(&earlier)
            .execute(storage.writer())
            .await
            .unwrap();
        let checks = "d3".repeat(32);
        let moved = "d4".repeat(32);
        proven_coin(&storage, user, &checks, 900, 3_000).await;
        proven_coin(&storage, user, &moved, 950, 2_000).await;
        services.set_header_for_height(header(900, &checks, 0));
        services.set_header_for_height(header(950, &"ee".repeat(32), 0));
        let toolbox_before: Vec<(String, String, Option<i64>)> =
            sqlx::query_as("SELECT txid, status, proven_tx_id FROM transactions ORDER BY txid")
                .fetch_all(storage.pool())
                .await
                .unwrap();

        let pass = crate::broadcast_reconcile::run_served_pass(
            &storage,
            &services,
            &FixedClock(now + 60),
            &opts(),
            None,
            DEFAULT_ABSENCE_MINUTES,
        )
        .await
        .unwrap();
        println!("{}", pass.summary());
        assert_eq!(pass.tracker.adopted_mined, vec![checks.clone()]);
        assert_eq!(pass.tracker.adopted_proof_failed, vec![moved.clone()]);
        assert_eq!(
            stored_word(&storage, &checks).await.unwrap(),
            Some(("mined".to_string(), Some(900), None))
        );
        let row = load_row(&storage, &checks).await.unwrap().unwrap();
        assert_eq!(
            row.bump,
            Some(path_of(&checks, 900)),
            "the evidence is the stored path"
        );
        assert!(!row.built, "a received coin's creator was not built here");
        assert_eq!(
            stored_word(&storage, &moved).await.unwrap(),
            Some((
                "unknown".to_string(),
                None,
                Some("proof_failed@950".to_string())
            ))
        );
        assert_eq!(
            stored_word(&storage, &earlier).await.unwrap(),
            Some(("built".to_string(), None, None)),
            "a creator tracked while unproven keeps its row"
        );
        let toolbox_after: Vec<(String, String, Option<i64>)> =
            sqlx::query_as("SELECT txid, status, proven_tx_id FROM transactions ORDER BY txid")
                .fetch_all(storage.pool())
                .await
                .unwrap();
        assert_eq!(
            toolbox_after, toolbox_before,
            "no write to the toolbox's rows"
        );
        assert_eq!(services.call_count("get_merkle_path"), 0);

        let pass = crate::broadcast_reconcile::run_served_pass(
            &storage,
            &services,
            &FixedClock(now + 120),
            &opts(),
            None,
            DEFAULT_ABSENCE_MINUTES,
        )
        .await
        .unwrap();
        println!("{}", pass.summary());
        assert!(pass.tracker.adopted_mined.is_empty());
        assert!(pass.tracker.adopted_proof_failed.is_empty());
    }
}
