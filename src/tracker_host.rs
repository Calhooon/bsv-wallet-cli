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
//! requests for the same transaction. A mined row is read again only when
//! the header at or below its height moved (the last twelve tips are kept
//! to see that) or when a spend names it ([`spend_guard`]).

use std::collections::{BTreeMap, HashMap, VecDeque};
use std::convert::Infallible;

use anyhow::Result;
use bsv_sdk::transaction::MerklePath;
use bsv_tracker::{
    CheckError, Clock, Evidence, Header, HeaderError, Headers, Height, Hint, HintSource,
    HintStatus, HostAction, Input, Params, Proof, ProofFetcher, Reask, State, Timestamp, TxId,
    Word,
};
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
/// Recent tips kept to see a moved header.
const TIP_RING: usize = 12;

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
    observed: Timestamp,
}

impl StoredHint {
    fn of(hint: &Hint) -> Self {
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
        };
        Self {
            source: hint.source.clone(),
            status: status.to_string(),
            height,
            reason,
            competitors,
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
}

impl TrackedRow {
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
    /// `headers` like any other proof; a path that no longer checks leaves
    /// the hint-tier word and a re-ask, and is returned as the fault.
    fn replay<H: Headers>(&self, params: &Params, headers: &H) -> (State, Option<CheckError>) {
        let mut state = State::new(self.txid.clone());
        if self.built {
            let _ = state.step(params, headers, Input::Host(HostAction::Build));
        }
        for hint in &self.hints {
            let _ = state.step(params, headers, Input::Hint(hint.hint()));
        }
        let fault = match self.proof() {
            Some(Ok(proof)) => state
                .step(params, headers, Input::Evidence(Evidence::Proof(proof)))
                .err(),
            Some(Err(fault)) => Some(fault),
            None => None,
        };
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
            updated_at INTEGER NOT NULL DEFAULT 0)",
        "CREATE INDEX IF NOT EXISTS tracker_states_word ON tracker_states (word)",
        "CREATE INDEX IF NOT EXISTS tracker_states_height ON tracker_states (height)",
        "CREATE TABLE IF NOT EXISTS tracker_meta (key TEXT PRIMARY KEY, value TEXT NOT NULL)",
    ] {
        sqlx::query(sql).execute(storage.pool()).await?;
    }
    Ok(())
}

const ROW_COLUMNS: &str = "txid, built, hints, bump, asks, next_ask_at";

fn row_of(row: &sqlx::sqlite::SqliteRow) -> TrackedRow {
    let hints: String = row.get("hints");
    TrackedRow {
        txid: row.get("txid"),
        built: row.get::<i64, _>("built") != 0,
        hints: serde_json::from_str(&hints).unwrap_or_default(),
        bump: row.get("bump"),
        asks: row.get::<i64, _>("asks").max(0) as u32,
        next_ask_at: row
            .get::<Option<i64>, _>("next_ask_at")
            .map(|t| t.max(0) as Timestamp),
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
    let (height, header_hash) = match state.word() {
        Word::Mined(mined) => (
            Some(mined.height() as i64),
            Some(mined.checked().header().hash.clone()),
        ),
        _ => (None, None),
    };
    sqlx::query(
        "INSERT INTO tracker_states \
            (txid, built, hints, bump, word, height, header_hash, reask, asks, next_ask_at, updated_at) \
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?) \
         ON CONFLICT(txid) DO UPDATE SET built = excluded.built, hints = excluded.hints, \
            bump = excluded.bump, word = excluded.word, height = excluded.height, \
            header_hash = excluded.header_hash, reask = excluded.reask, asks = excluded.asks, \
            next_ask_at = excluded.next_ask_at, updated_at = excluded.updated_at",
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
    .bind(now as i64)
    .execute(storage.pool())
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
}

impl TickOptions {
    /// `TRACKER_AGE_SECS` and `TRACKER_MAX_ASKS`, with their defaults.
    pub fn from_env() -> Self {
        fn env<T: std::str::FromStr>(key: &str, default: T) -> T {
            std::env::var(key)
                .ok()
                .and_then(|v| v.trim().parse().ok())
                .unwrap_or(default)
        }
        Self {
            age_threshold: env("TRACKER_AGE_SECS", DEFAULT_AGE_SECS).max(1),
            max_asks: env("TRACKER_MAX_ASKS", DEFAULT_MAX_ASKS),
        }
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
    /// Faults the host reports: `txid: what failed`.
    pub faults: Vec<String>,
    /// What the wallet's storage did with each checked proof.
    pub stored: Vec<String>,
}

impl TickReport {
    /// Nothing moved and nothing was asked.
    pub fn is_quiet(&self) -> bool {
        self.adopted == 0
            && self.hints == 0
            && self.asked == 0
            && self.moved.is_empty()
            && self.faults.is_empty()
    }

    /// One line.
    pub fn summary(&self) -> String {
        format!(
            "tracker tick: {} tracked ({} new), {} hint(s), {} re-ask(s): {} mined, {} without a proof, {} moved, {} fault(s)",
            self.tracked,
            self.adopted,
            self.hints,
            self.asked,
            self.mined.len(),
            self.no_proof.len(),
            self.moved.len(),
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
    .execute(storage.pool())
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
    .execute(storage.pool())
    .await?;
    report.adopted = adopted.rows_affected() as usize;

    // Did a header we stood on move? Then the mined rows at or above it
    // are read again, and no other mined row is.
    let mut snapshot = HeaderSnapshot::take(services, []).await;
    let (moved_from, ring) = moved_since(services, &mut snapshot, read_ring(storage).await).await;
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
    for state in states.values_mut() {
        let _ = state.step(&params, &snapshot, Input::Host(HostAction::Tick(now)));
    }

    // Read again: a proof for each re-ask that is due, and its header.
    let due: Vec<TxId> = rows
        .iter()
        .filter(|row| {
            states[&row.txid].reask().is_some()
                && row.asks < MAX_ASKS_PER_TX
                && row.next_ask_at.is_none_or(|at| at <= now)
        })
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
    write_ring(storage, &ring).await?;
    Ok(report)
}

// ---------------------------------------------------------------------------
// The spend guard
// ---------------------------------------------------------------------------

/// The wallet spend guard, as the tracker's README gives it (bsv-tracker
/// `README.md`, "Wallet: spend guard"), unchanged: the scheduled recheck
/// runs before the coin is used, and an unavailable header is an error that
/// prevents the spend.
fn readme_spend_guard<H: Headers>(
    state: &mut State,
    params: &Params,
    headers: &H,
) -> std::result::Result<bool, CheckError> {
    state.step(params, headers, Input::Host(HostAction::SpendAttempt))?;
    state.step(params, headers, Input::Evidence(Evidence::Recheck))?;
    Ok(matches!(state.word(), Word::Mined(_)) && !state.suspect())
}

/// Run the spend guard for a coin of the tracked transaction `txid`, as
/// the host: the row is loaded (its stored path checked against a fresh
/// header), the README's guard runs on it, and the row is stored again.
/// `Ok(true)` only for a checked, non-suspect `mined`; `Err` when the
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
    let verdict = readme_spend_guard(&mut state, &params, &snapshot);
    store_row(storage, &row, &state, clock.now()).await?;
    Ok(verdict)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
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
        .execute(storage.pool())
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

    /// A tip header carrying the hash its bytes have.
    fn header_with_hash(height: u32, root: &str, nonce: u32) -> BlockHeader {
        let mut h = header(height, root, nonce);
        h.hash = header_of(&h.to_binary()).unwrap().hash;
        h
    }

    /// The README's wallet spend guard, run by the host over a stored row
    /// (bsv-tracker `README.md`, "Wallet: spend guard"). Three runs: the
    /// header is the one the proof was checked against; the header service
    /// cannot answer; the header moved.
    #[tokio::test]
    async fn the_readme_spend_guard_runs_as_the_host() {
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
        let guard = readme_spend_guard(&mut state, &params, &lost);
        println!("spend guard, height unanswered: {guard:?}");
        assert_eq!(guard, Err(CheckError::Unavailable(900)));
    }
}
