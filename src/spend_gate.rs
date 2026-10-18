//! The spend guard on the binary's real coin selection (bsv-wallet-cli #28,
//! 0.10.0): which of the wallet's coins the toolbox's `createAction` may
//! allocate.
//!
//! The toolbox chooses the funding coins itself, inside its own store
//! transaction (`allocate_change_input`: a change output of the default
//! basket, unspent, its transaction `completed` or `unproven`). Like the
//! reference it allocates unproven change (`[SRC]` ts-stack `edf6e03`
//! `wallet-toolbox/src/storage/StorageKnex.ts:2222-2231`: `completed`,
//! `unproven`, and `sending` unless excluded), whoever made the coin, and
//! reads no depth. The stack's wallet charter (bsv-stack-lean
//! `docs/charters/wallet.md` section 2, rulings of 2026-10-12;
//! `lean/Wallet.lean` `offerable`, `ruleFor`, `guardDepth`) departs from it,
//! and this gate applies the departure. A coin is offered when, in this
//! order:
//!
//! 1. its creator's tracker word is `mined(h)`, our header at `h` carries
//!    the checked root now, the creator is not marked suspect, and the block
//!    is `SPEND_DEPTH` deep (the tip is 1). **R3 (b)**: the depth is the
//!    host's parameter, 1 the floor ([`crate::tracker_host::read_creators`],
//!    the 0.9.0 `spend_guard` run over many creators at once); or
//! 2. its creator's word claims nothing about the chain (`pending`:
//!    `unknown`, `built`, `announced`, `seen`), the creator is an action of
//!    ours (`made`: change of a transaction this wallet created and signed),
//!    and the config names the rule, `SPEND_OWN_CHANGE` (default `1` for
//!    this host; `0` turns it off). **R1 (b)**: the one rule a host may name
//!    is for its own `made` change (`Rules .made`); a received coin
//!    (`internalizeAction`, any counterparty's payment) with no proof is
//!    never offered by a host setting (`ruleFor` reads no `received` arm
//!    host-wide); or
//! 3. its creator's word is `pending`, the coin was received, and this one
//!    action names the rule at the door: `allowPending: "received"` on the
//!    served `/createAction`, `--allow-pending received` on `send`, `drain`,
//!    `split` and `gift-send` ([`GateOptions::allow_received`]). **R1 (d)**
//!    (the owner's ruling of 2026-10-18 00:1xZ; `lean/Wallet.lean`
//!    `receivedPending`, `ruleFor`): the rule under the word is the
//!    ruling's own, any creator word that claims nothing about the chain,
//!    never a host setting, and there is no environment variable for it.
//!    The body of every such coin was checked at internalize: the toolbox
//!    refuses an incomplete ancestry and checks every merkle root the BEEF
//!    carries against the header service in Strict mode
//!    (bsv-wallet-toolbox-rs 0.8.0 `storage/sqlx/internalize_action.rs`
//!    96-119), and `fund` refuses an invalid BEEF; so the one trusted word
//!    is the sender's broadcast. The action records the word
//!    (`spend_gate_actions`, [`RECEIVED_PENDING`]) by its txid, or a
//!    deferred-signing template by its reference until `signAction` names
//!    its txid; shown by `actions` and `/listActions` as `underRule`.
//!
//! A coin whose creator is `stale`, `rejected`, `conflicted` or `abandoned`
//! is never offered; a coin an action of ours holds is the toolbox's lock
//! (`spent_by`) and is never a candidate. A frozen coin (the feed's
//! `frozen` event, recorded in `frozen_outpoints` by
//! [`crate::tracker_host::deliver_chain_event`]) is never offered, rule or
//! no rule (R6: never cleared here).
//!
//! Coins a caller names (`createAction`'s `inputs`, `respend`'s exact
//! inputs) pass the same per-coin judgment ([`assess_named`]); a named coin
//! the gate would hold refuses the action with the same `held[]` listing
//! (the charter's `onCreate`: every input offerable).
//!
//! # The shape: held for the call, restored after
//!
//! The toolbox's allocation cannot be filtered before it runs: it is one SQL
//! query inside `create_action`. So the gate marks every coin it does not
//! offer as not change (`outputs.change = 0`) for the duration of the call,
//! records each in `spend_gate_holds`, and restores exactly those after.
//! `change`, not `spendable`: with `acceptDelayedBroadcast` the toolbox
//! allocates without reading `spendable` (`require_spendable = !is_delayed`),
//! but it always reads `change = 1`. A hold a crashed call left behind is
//! restored by the next call (any hold older than [`STALE_HOLD_SECS`]).
//!
//! The coins are read in the toolbox's own preference (completed first, then
//! the largest), a batch of creators at a time, until the coins offered
//! cover the outputs with a margin; the coins not read are held too. Only
//! when the toolbox then finds no funds is every coin read, and the call run
//! once more; if that fails for funds, the refusal lists every coin held
//! back ([`Refusal`]). Nothing is fetched but the header service's tip and
//! headers: no chain scan, no explorer.

use std::collections::HashMap;

use anyhow::Result;
use bsv_sdk::wallet::{CreateActionArgs, CreateActionResult, WalletInterface};
use bsv_tracker::Height;
use bsv_wallet_toolbox::{StorageSqlx, Wallet, WalletServices};
use serde::Serialize;
use sqlx::{Row, SqlitePool};

use crate::tracker_host::{read_creators, CreatorReading, TickOptions};

/// `SPEND_OWN_CHANGE` when unset: this host chains payments, so its own
/// change is offered before it is mined (R1 (b)).
pub const DEFAULT_SPEND_OWN_CHANGE: bool = true;
/// A hold older than this was left by a call that did not finish; the next
/// call restores it.
pub const STALE_HOLD_SECS: i64 = 600;
/// Creators read per batch while the offered coins do not yet cover.
const BATCH: usize = 32;
/// Satoshis read beyond the outputs before the lazy read stops (fees).
const FEE_MARGIN: u64 = 10_000;
/// R1 (d)'s rule, by the ruling's name (`lean/Wallet.lean`
/// `receivedPending`), as recorded on an action that named it.
pub const RECEIVED_PENDING: &str = "received-pending";
/// The door's one value for `allowPending`.
pub const ALLOW_PENDING_RECEIVED: &str = "received";
/// Appended to a held received `pending` coin's reason: what would offer it.
pub const ALLOW_PENDING_HINT: &str = "offered under allowPending: received";
/// The refusal's toolbox line when a coin a caller named is held.
pub const NAMED_INPUT_REFUSED: &str = "an input the spend guard does not offer";

/// The gate's two settings.
#[derive(Debug, Clone)]
pub struct GateOptions {
    /// `SPEND_DEPTH`, floor 1 (R3 (b)).
    pub spend_depth: Height,
    /// `SPEND_OWN_CHANGE` (R1 (b)): offer our own `made` change whose
    /// creator's word is `pending`.
    pub own_change: bool,
    /// R1 (d): this one action named `allowPending: received` at the door.
    /// Never read from the environment; off unless the caller says so.
    pub allow_received: bool,
}

impl Default for GateOptions {
    fn default() -> Self {
        Self {
            spend_depth: crate::tracker_host::DEFAULT_SPEND_DEPTH,
            own_change: DEFAULT_SPEND_OWN_CHANGE,
            allow_received: false,
        }
    }
}

/// The door's word: `allowPending` is on only for the one value
/// `"received"`; absent or anything else is off.
pub fn allow_pending(word: Option<&str>) -> bool {
    word == Some(ALLOW_PENDING_RECEIVED)
}

impl GateOptions {
    /// `SPEND_DEPTH` and `SPEND_OWN_CHANGE` (`0`, `false`, `no`, `off` turn
    /// it off; anything else, or unset, leaves it on).
    pub fn from_env() -> Self {
        let own_change = std::env::var("SPEND_OWN_CHANGE")
            .map(|v| {
                !matches!(
                    v.trim().to_ascii_lowercase().as_str(),
                    "0" | "false" | "no" | "off"
                )
            })
            .unwrap_or(DEFAULT_SPEND_OWN_CHANGE);
        Self {
            spend_depth: TickOptions::from_env().spend_depth,
            own_change,
            allow_received: false,
        }
    }

    /// These settings, with the action's word for R1 (d).
    pub fn with_allow_received(mut self, allow: bool) -> Self {
        self.allow_received = allow;
        self
    }

    fn tick(&self) -> TickOptions {
        TickOptions {
            spend_depth: self.spend_depth,
            ..TickOptions::from_env()
        }
        .floored()
    }
}

/// Who made a coin.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Origin {
    /// Change of an action this wallet created and signed.
    Made,
    /// A payment `internalizeAction` took in.
    Received,
}

/// One coin the gate did not offer, and why.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct HeldCoin {
    /// `txid.vout`.
    pub outpoint: String,
    pub satoshis: u64,
    /// The creator's tracker word (`mined`, `unknown`, `built`, ...), or
    /// `locked` for a coin an action of ours holds.
    pub word: String,
    pub origin: Origin,
    /// Minutes since the wallet learned of the coin.
    pub age_minutes: i64,
    /// The setting or rule that holds it: `depth 1: 0 blocks`, `received,
    /// no proof; offered under allowPending: received`, `own change,
    /// SPEND_OWN_CHANGE=0`, `locked by <txid>`, `frozen: never offered`.
    pub reason: String,
    #[serde(skip)]
    output_id: i64,
}

/// The refusal: no offered coin covers the action. Every coin held back is
/// listed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Refusal {
    /// The toolbox's own words for the shortfall.
    pub toolbox: String,
    /// Satoshis the gate offered.
    pub offered_satoshis: u64,
    pub held: Vec<HeldCoin>,
}

impl std::fmt::Display for Refusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.toolbox == NAMED_INPUT_REFUSED {
            write!(
                f,
                "Access denied: {} ({} coin(s) held back)",
                self.toolbox,
                self.held.len()
            )?;
        } else {
            write!(
                f,
                "Insufficient funds: no offerable coin covers the action ({}; {} sats offered, {} coin(s) held back)",
                self.toolbox,
                self.offered_satoshis,
                self.held.len()
            )?;
        }
        for c in &self.held {
            write!(
                f,
                "\n  {} {} sats: {}, {}, {} min old: {}",
                c.outpoint,
                c.satoshis,
                c.word,
                match c.origin {
                    Origin::Made => "made",
                    Origin::Received => "received",
                },
                c.age_minutes,
                c.reason
            )?;
        }
        Ok(())
    }
}

impl std::error::Error for Refusal {}

impl Refusal {
    /// A coin the caller named is held: the action is refused before the
    /// toolbox runs.
    pub fn is_named_input(&self) -> bool {
        self.toolbox == NAMED_INPUT_REFUSED
    }

    /// The refusal's code: `INPUT_NOT_OFFERED` for a named coin held,
    /// else `INSUFFICIENT_FUNDS`.
    pub fn code(&self) -> &'static str {
        if self.is_named_input() {
            "INPUT_NOT_OFFERED"
        } else {
            "INSUFFICIENT_FUNDS"
        }
    }

    /// The held coins, as the route and `--json` give them.
    pub fn held_json(&self) -> serde_json::Value {
        serde_json::Value::Array(
            self.held
                .iter()
                .map(|c| {
                    serde_json::json!({
                        "outpoint": c.outpoint,
                        "satoshis": c.satoshis,
                        "word": c.word,
                        "origin": c.origin,
                        "ageMinutes": c.age_minutes,
                        "reason": c.reason,
                    })
                })
                .collect(),
        )
    }

    /// The refusal as one JSON object, the route's body shape:
    /// `{code, message, offeredSatoshis, held}`.
    pub fn to_json(&self) -> serde_json::Value {
        serde_json::json!({
            "code": self.code(),
            "message": self.to_string(),
            "offeredSatoshis": self.offered_satoshis,
            "held": self.held_json(),
        })
    }
}

/// A command's gated `createAction`: under `--json` a refusal is printed as
/// [`Refusal::to_json`] on stdout and the process exits 1, as the text
/// refusal does; anything else is returned as it was.
pub fn refused_as_json<T>(
    result: std::result::Result<T, GateError>,
    json: bool,
) -> anyhow::Result<T> {
    match result {
        Ok(value) => Ok(value),
        Err(GateError::Refused(refusal)) if json => {
            println!("{}", refusal.to_json());
            std::process::exit(1);
        }
        Err(e) => Err(e.into()),
    }
}

/// What a gated `createAction` failed on.
#[derive(Debug)]
pub enum GateError {
    /// No offerable coin covers it.
    Refused(Refusal),
    /// The toolbox's own error, as it gave it.
    Wallet(bsv_sdk::Error),
    /// The store or the header service failed under the gate.
    Gate(anyhow::Error),
}

impl std::fmt::Display for GateError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Refused(r) => r.fmt(f),
            Self::Wallet(e) => e.fmt(f),
            Self::Gate(e) => write!(f, "Storage error: spend gate: {e}"),
        }
    }
}

impl std::error::Error for GateError {}

/// A candidate coin, as the toolbox would see it.
#[derive(Debug, Clone)]
struct Candidate {
    output_id: i64,
    txid: String,
    vout: i64,
    satoshis: u64,
    origin: Origin,
    age_minutes: i64,
    proof: Option<Vec<u8>>,
    frozen: bool,
}

/// The gate's verdict on the coins it read.
#[derive(Debug, Default, Clone)]
pub struct Assessment {
    /// Coins offered: (output id, satoshis).
    pub offered: Vec<(i64, u64)>,
    /// Coins held back, with why.
    pub held: Vec<HeldCoin>,
    /// Coins not read (enough was offered before them); held too.
    pub unread: Vec<i64>,
}

impl Assessment {
    pub fn offered_satoshis(&self) -> u64 {
        self.offered.iter().map(|(_, s)| s).sum()
    }
}

async fn ensure_schema(pool: &SqlitePool) -> Result<()> {
    sqlx::query(
        "CREATE TABLE IF NOT EXISTS spend_gate_holds (\
            output_id INTEGER PRIMARY KEY, \
            holder TEXT NOT NULL, \
            held_at INTEGER NOT NULL)",
    )
    .execute(pool)
    .await?;
    // The rule an action was created under (R1 (d)): the toolbox's
    // `transactions` row is the reference's shape and not ours to widen.
    // An action is named by its txid, or, a deferred-signing template with
    // no txid yet, by the toolbox's `reference` until `signAction` gives it one.
    sqlx::query(
        "CREATE TABLE IF NOT EXISTS spend_gate_actions (\
            txid TEXT PRIMARY KEY, \
            rule TEXT NOT NULL, \
            recorded_at INTEGER NOT NULL, \
            reference TEXT)",
    )
    .execute(pool)
    .await?;
    let has_reference: Option<String> = sqlx::query_scalar(
        "SELECT name FROM pragma_table_info('spend_gate_actions') WHERE name = 'reference'",
    )
    .fetch_optional(pool)
    .await?;
    if has_reference.is_none() {
        sqlx::query("ALTER TABLE spend_gate_actions ADD COLUMN reference TEXT")
            .execute(pool)
            .await?;
    }
    sqlx::query(
        "CREATE UNIQUE INDEX IF NOT EXISTS spend_gate_actions_reference \
         ON spend_gate_actions (reference)",
    )
    .execute(pool)
    .await?;
    Ok(())
}

/// Record that the action `txid` was created under `rule`.
pub async fn record_rule(pool: &SqlitePool, txid: &str, rule: &str) -> Result<()> {
    ensure_schema(pool).await?;
    sqlx::query(
        "INSERT INTO spend_gate_actions (txid, rule, recorded_at) VALUES (?, ?, ?) \
         ON CONFLICT(txid) DO NOTHING",
    )
    .bind(txid.to_ascii_lowercase())
    .bind(rule)
    .bind(chrono::Utc::now().timestamp())
    .execute(pool)
    .await?;
    Ok(())
}

/// Record that the deferred-signing template `reference` (no txid yet) was
/// created under `rule`; [`fill_txid`] names it by its txid once signed.
pub async fn record_rule_by_reference(
    pool: &SqlitePool,
    reference: &str,
    rule: &str,
) -> Result<()> {
    ensure_schema(pool).await?;
    sqlx::query(
        "INSERT INTO spend_gate_actions (txid, rule, recorded_at, reference) VALUES (NULL, ?, ?, ?) \
         ON CONFLICT(reference) DO NOTHING",
    )
    .bind(rule)
    .bind(chrono::Utc::now().timestamp())
    .bind(reference)
    .execute(pool)
    .await?;
    Ok(())
}

/// The template `reference` was signed as `txid`: its row, where one was
/// recorded, gains the txid and keeps the reference.
pub async fn fill_txid(pool: &SqlitePool, reference: &str, txid: &str) -> Result<()> {
    ensure_schema(pool).await?;
    sqlx::query("UPDATE spend_gate_actions SET txid = ? WHERE reference = ? AND txid IS NULL")
        .bind(txid.to_ascii_lowercase())
        .bind(reference)
        .execute(pool)
        .await?;
    Ok(())
}

/// The toolbox's `reference` string of a signable transaction, as
/// `signAction` / `abortAction` take it back.
pub fn reference_string(reference: &[u8]) -> String {
    String::from_utf8(reference.to_vec()).unwrap_or_else(|_| hex::encode(reference))
}

/// The rule each action was created under, where one was recorded, by
/// txid; a template not yet signed has none (see [`ActionRules`]).
pub async fn rules_of(pool: &SqlitePool) -> Result<HashMap<String, String>> {
    ensure_schema(pool).await?;
    let rows = sqlx::query("SELECT txid, rule FROM spend_gate_actions WHERE txid IS NOT NULL")
        .fetch_all(pool)
        .await?;
    Ok(rows
        .iter()
        .map(|r| (r.get::<String, _>("txid"), r.get::<String, _>("rule")))
        .collect())
}

/// The rules a listing shows as `underRule`: by txid, and for a template
/// not yet signed (listed with an all-zero txid) by its shape in the
/// listing, read off the toolbox row its reference names.
#[derive(Debug, Default, Clone)]
pub struct ActionRules {
    pub by_txid: HashMap<String, String>,
    /// [`template_key`] → rule, only where every unsigned toolbox row of
    /// that shape carries the same rule: a shape shared with a template
    /// under no rule, or another rule, names none.
    pub by_template: HashMap<String, String>,
}

/// A template's shape as a listing shows it: status, description, satoshis.
pub fn template_key(status: &str, description: &str, satoshis: i64) -> String {
    format!("{status}\u{1f}{description}\u{1f}{satoshis}")
}

/// Every recorded rule, as a listing shows it.
pub async fn action_rules(pool: &SqlitePool) -> Result<ActionRules> {
    let by_txid = rules_of(pool).await?;
    let rows = sqlx::query(
        "SELECT t.status, t.description, t.satoshis, g.rule FROM transactions t \
         LEFT JOIN spend_gate_actions g ON g.reference = t.reference AND g.txid IS NULL \
         WHERE t.txid IS NULL",
    )
    .fetch_all(pool)
    .await?;
    let mut shapes: HashMap<String, Option<String>> = HashMap::new();
    let mut ambiguous = std::collections::HashSet::new();
    for r in &rows {
        let key = template_key(
            &r.get::<String, _>("status"),
            &r.get::<String, _>("description"),
            r.get::<i64, _>("satoshis"),
        );
        let rule: Option<String> = r.get("rule");
        match shapes.get(&key) {
            Some(seen) if *seen != rule => {
                ambiguous.insert(key);
            }
            Some(_) => {}
            None => {
                shapes.insert(key, rule);
            }
        }
    }
    let by_template = shapes
        .into_iter()
        .filter(|(k, _)| !ambiguous.contains(k))
        .filter_map(|(k, rule)| rule.map(|r| (k, r)))
        .collect();
    Ok(ActionRules {
        by_txid,
        by_template,
    })
}

fn blocks(n: u64) -> String {
    if n == 1 {
        "1 block".to_string()
    } else {
        format!("{n} blocks")
    }
}

/// Why `reading` holds a coin of `origin`, or `None` when it is offered.
pub fn hold_reason(reading: &CreatorReading, origin: Origin, opts: &GateOptions) -> Option<String> {
    if let Some(fault) = &reading.fault {
        return Some(format!("headers unavailable: {fault}"));
    }
    if reading.offered {
        return None;
    }
    let depth = format!(
        "depth {}: {}",
        opts.spend_depth
            .max(crate::tracker_host::DEFAULT_SPEND_DEPTH),
        blocks(reading.depth)
    );
    match reading.word {
        "mined" if reading.suspect => {
            Some("suspect: a fork or an unresolved split at or below its height".to_string())
        }
        "mined" => Some(depth),
        "stale" | "rejected" | "conflicted" | "abandoned" => {
            Some(format!("{}: never offered", reading.word))
        }
        // `pending`: the word claims nothing about the chain; only R1's
        // rule for our own change (b), or the action's word for a received
        // coin (d), offers it.
        _ => match origin {
            Origin::Received if opts.allow_received => None,
            Origin::Received => Some(format!("{depth}; received, no proof; {ALLOW_PENDING_HINT}")),
            Origin::Made if opts.own_change => None,
            Origin::Made => Some(format!("{depth}; own change, SPEND_OWN_CHANGE=0")),
        },
    }
}

/// Every coin the toolbox could allocate: a change output of a default
/// basket, unspent, its transaction `completed` or `unproven`. In the
/// toolbox's preference: completed first, then the largest.
async fn candidates(pool: &SqlitePool) -> Result<Vec<Candidate>> {
    let frozen = crate::tracker_host::frozen_outpoints(pool).await?;
    let rows = sqlx::query(
        "SELECT o.output_id, t.txid, o.vout, o.satoshis, t.is_outgoing, \
                CAST((julianday('now') - julianday(o.created_at)) * 1440 AS INTEGER) AS age, \
                p.merkle_path \
         FROM outputs o \
         JOIN transactions t ON o.transaction_id = t.transaction_id \
         JOIN output_baskets b ON o.basket_id = b.basket_id AND b.name = 'default' \
         LEFT JOIN proven_txs p ON p.txid = t.txid \
         WHERE o.change = 1 AND o.spent_by IS NULL AND t.txid IS NOT NULL \
           AND t.status IN ('completed', 'unproven') \
         ORDER BY CASE WHEN t.status = 'completed' THEN 0 ELSE 1 END, o.satoshis DESC",
    )
    .fetch_all(pool)
    .await?;
    Ok(rows
        .iter()
        .map(|r| Candidate {
            output_id: r.get("output_id"),
            txid: r.get::<String, _>("txid").to_ascii_lowercase(),
            vout: r.get("vout"),
            satoshis: r.get::<i64, _>("satoshis").max(0) as u64,
            origin: if r.get::<i64, _>("is_outgoing") != 0 {
                Origin::Made
            } else {
                Origin::Received
            },
            age_minutes: r.get::<Option<i64>, _>("age").unwrap_or(0).max(0),
            proof: r.get("merkle_path"),
            frozen: frozen.contains(&(
                r.get::<String, _>("txid").to_ascii_lowercase(),
                r.get::<i64, _>("vout"),
            )),
        })
        .collect())
}

/// The frozen record's hold (R6: never cleared here).
const FROZEN: &str = "frozen: never offered";

/// The coins an action of ours holds (`spent_by` a live action): listed in
/// a refusal, never offered.
async fn locked(pool: &SqlitePool) -> Result<Vec<HeldCoin>> {
    let rows = sqlx::query(
        "SELECT o.output_id, t.txid, o.vout, o.satoshis, t.is_outgoing, \
                CAST((julianday('now') - julianday(o.created_at)) * 1440 AS INTEGER) AS age, \
                COALESCE(s.txid, s.reference) AS holder \
         FROM outputs o \
         JOIN transactions t ON o.transaction_id = t.transaction_id \
         JOIN transactions s ON o.spent_by = s.transaction_id \
         JOIN output_baskets b ON o.basket_id = b.basket_id AND b.name = 'default' \
         WHERE o.change = 1 AND s.status NOT IN ('completed', 'failed') \
           AND t.status IN ('completed', 'unproven')",
    )
    .fetch_all(pool)
    .await?;
    Ok(rows
        .iter()
        .map(|r| HeldCoin {
            outpoint: format!("{}.{}", r.get::<String, _>("txid"), r.get::<i64, _>("vout")),
            satoshis: r.get::<i64, _>("satoshis").max(0) as u64,
            word: "locked".to_string(),
            origin: if r.get::<i64, _>("is_outgoing") != 0 {
                Origin::Made
            } else {
                Origin::Received
            },
            age_minutes: r.get::<Option<i64>, _>("age").unwrap_or(0).max(0),
            reason: format!(
                "locked by {}",
                r.get::<Option<String>, _>("holder").unwrap_or_default()
            ),
            output_id: r.get("output_id"),
        })
        .collect())
}

/// Read the coins and judge each. With `need`, stop reading once the coins
/// offered reach it (the rest are `unread`); with `None`, read every coin.
pub async fn assess<V: WalletServices + ?Sized>(
    storage: &StorageSqlx,
    services: &V,
    opts: &GateOptions,
    need: Option<u64>,
) -> Result<Assessment> {
    let all = candidates(storage.pool()).await?;
    let tick = opts.tick();
    let mut by_creator: Vec<(String, Option<Vec<u8>>)> = Vec::new();
    for c in &all {
        if !by_creator.iter().any(|(t, _)| t == &c.txid) {
            by_creator.push((c.txid.clone(), c.proof.clone()));
        }
    }
    let mut readings: HashMap<String, CreatorReading> = HashMap::new();
    let mut out = Assessment::default();
    let mut next = 0;
    for c in &all {
        let covered = need.is_some_and(|need| out.offered_satoshis() >= need);
        if c.frozen {
            out.held.push(HeldCoin {
                outpoint: format!("{}.{}", c.txid, c.vout),
                satoshis: c.satoshis,
                word: "frozen".to_string(),
                origin: c.origin,
                age_minutes: c.age_minutes,
                reason: FROZEN.to_string(),
                output_id: c.output_id,
            });
            continue;
        }
        if !readings.contains_key(&c.txid) {
            if covered {
                out.unread.push(c.output_id);
                continue;
            }
            let end = if need.is_some() {
                (next + BATCH).min(by_creator.len())
            } else {
                by_creator.len()
            };
            let batch: Vec<_> = by_creator[next..end]
                .iter()
                .filter(|(t, _)| !readings.contains_key(t))
                .cloned()
                .collect();
            next = end;
            readings.extend(read_creators(storage, services, &tick, &batch).await?);
        }
        let Some(reading) = readings.get(&c.txid) else {
            out.unread.push(c.output_id);
            continue;
        };
        match hold_reason(reading, c.origin, opts) {
            None if !covered => out.offered.push((c.output_id, c.satoshis)),
            None => out.unread.push(c.output_id),
            Some(reason) => out.held.push(HeldCoin {
                outpoint: format!("{}.{}", c.txid, c.vout),
                satoshis: c.satoshis,
                word: reading.word.to_string(),
                origin: c.origin,
                age_minutes: c.age_minutes,
                reason,
                output_id: c.output_id,
            }),
        }
    }
    Ok(out)
}

/// The coins a caller named (`createAction`'s `inputs`, `respend`'s exact
/// inputs), judged coin by coin as the toolbox's own allocation is: frozen,
/// locked (`spent_by`, unless it is `reserved_to`, the phantom a respend
/// replaces), or a creator the gate would not offer. Returns every named
/// coin it holds. An outpoint that is not one of this wallet's outputs is
/// not a coin of ours and is left to the toolbox (a covenant or a foreign
/// input carried in `inputBEEF`).
pub async fn assess_named<V: WalletServices + ?Sized>(
    storage: &StorageSqlx,
    services: &V,
    opts: &GateOptions,
    named: &[(String, u32)],
    reserved_to: Option<i64>,
) -> Result<Vec<HeldCoin>> {
    let pool = storage.pool();
    let frozen = crate::tracker_host::frozen_outpoints(pool).await?;
    let mut coins = Vec::new();
    for (txid, vout) in named {
        let txid = txid.to_ascii_lowercase();
        let row = sqlx::query(
            "SELECT o.output_id, o.satoshis, o.spent_by, t.is_outgoing, \
                    CAST((julianday('now') - julianday(o.created_at)) * 1440 AS INTEGER) AS age, \
                    COALESCE(s.txid, s.reference) AS holder, p.merkle_path \
             FROM outputs o \
             JOIN transactions t ON o.transaction_id = t.transaction_id \
             LEFT JOIN transactions s ON o.spent_by = s.transaction_id \
             LEFT JOIN proven_txs p ON p.txid = t.txid \
             WHERE lower(t.txid) = ? AND o.vout = ?",
        )
        .bind(&txid)
        .bind(i64::from(*vout))
        .fetch_optional(pool)
        .await?;
        let Some(r) = row else { continue };
        coins.push((txid, *vout, r));
    }
    let is_frozen = |txid: &str, vout: u32| frozen.contains(&(txid.to_string(), i64::from(vout)));
    let is_locked = |r: &sqlx::sqlite::SqliteRow| {
        let spent_by: Option<i64> = r.get("spent_by");
        spent_by.is_some() && spent_by != reserved_to
    };
    // Only a coin neither frozen nor locked needs its creator read.
    let creators: Vec<(String, Option<Vec<u8>>)> = coins
        .iter()
        .filter(|(t, v, r)| !is_frozen(t, *v) && !is_locked(r))
        .map(|(t, _, r)| (t.clone(), r.get("merkle_path")))
        .collect();
    let readings = if creators.is_empty() {
        HashMap::new()
    } else {
        read_creators(storage, services, &opts.tick(), &creators).await?
    };
    let mut held = Vec::new();
    for (txid, vout, r) in coins {
        let origin = if r.get::<i64, _>("is_outgoing") != 0 {
            Origin::Made
        } else {
            Origin::Received
        };
        let reading = readings.get(&txid);
        let (word, reason) = if is_frozen(&txid, vout) {
            ("frozen".to_string(), Some(FROZEN.to_string()))
        } else if is_locked(&r) {
            (
                "locked".to_string(),
                Some(format!(
                    "locked by {}",
                    r.get::<Option<String>, _>("holder").unwrap_or_default()
                )),
            )
        } else {
            match reading {
                Some(reading) => (reading.word.to_string(), hold_reason(reading, origin, opts)),
                None => ("unknown".to_string(), Some("not read".to_string())),
            }
        };
        if let Some(reason) = reason {
            held.push(HeldCoin {
                outpoint: format!("{txid}.{vout}"),
                satoshis: r.get::<i64, _>("satoshis").max(0) as u64,
                word,
                origin,
                age_minutes: r.get::<Option<i64>, _>("age").unwrap_or(0).max(0),
                reason,
                output_id: r.get("output_id"),
            });
        }
    }
    Ok(held)
}

/// A unique name for this call's holds.
fn holder() -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    static N: AtomicU64 = AtomicU64::new(0);
    format!(
        "{}:{}",
        std::process::id(),
        N.fetch_add(1, Ordering::Relaxed)
    )
}

/// Mark `ids` not change for the call: the toolbox does not allocate them.
async fn hold(pool: &SqlitePool, holder: &str, ids: &[i64]) -> Result<()> {
    ensure_schema(pool).await?;
    let now = chrono::Utc::now().timestamp();
    let mut tx = pool.begin().await?;
    for id in ids {
        // Only a coin still change, and not held by another call, is held:
        // restoring sets back exactly what was set here.
        let held = sqlx::query(
            "UPDATE outputs SET change = 0 WHERE output_id = ? AND change = 1 \
             AND output_id NOT IN (SELECT output_id FROM spend_gate_holds)",
        )
        .bind(id)
        .execute(&mut *tx)
        .await?
        .rows_affected();
        if held == 1 {
            sqlx::query(
                "INSERT INTO spend_gate_holds (output_id, holder, held_at) VALUES (?, ?, ?)",
            )
            .bind(id)
            .bind(holder)
            .bind(now)
            .execute(&mut *tx)
            .await?;
        }
    }
    tx.commit().await?;
    Ok(())
}

/// Restore what `holder` held, and any hold a call that did not finish left
/// behind ([`STALE_HOLD_SECS`]).
pub async fn restore(pool: &SqlitePool, holder: Option<&str>) -> Result<u64> {
    ensure_schema(pool).await?;
    let stale = chrono::Utc::now().timestamp() - STALE_HOLD_SECS;
    let mut tx = pool.begin().await?;
    let which = "SELECT output_id FROM spend_gate_holds WHERE holder = ? OR held_at < ?";
    let restored = sqlx::query(&format!(
        "UPDATE outputs SET change = 1 WHERE change = 0 AND output_id IN ({which})"
    ))
    .bind(holder.unwrap_or(""))
    .bind(stale)
    .execute(&mut *tx)
    .await?
    .rows_affected();
    sqlx::query("DELETE FROM spend_gate_holds WHERE holder = ? OR held_at < ?")
        .bind(holder.unwrap_or(""))
        .bind(stale)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(restored)
}

fn is_insufficient(e: &bsv_sdk::Error) -> bool {
    e.to_string()
        .to_ascii_lowercase()
        .contains("insufficient funds")
}

/// One toolbox call with every coin the assessment did not offer held.
async fn run_held<V: WalletServices + Send + Sync + 'static>(
    wallet: &Wallet<StorageSqlx, V>,
    args: CreateActionArgs,
    originator: &str,
    assessment: &Assessment,
) -> std::result::Result<bsv_sdk::Result<CreateActionResult>, GateError> {
    let pool = wallet.storage().pool();
    let holder = holder();
    let ids: Vec<i64> = assessment
        .held
        .iter()
        .map(|c| c.output_id)
        .chain(assessment.unread.iter().copied())
        .collect();
    hold(pool, &holder, &ids).await.map_err(GateError::Gate)?;
    let outcome = wallet.create_action(args, originator).await;
    restore(pool, Some(&holder))
        .await
        .map_err(GateError::Gate)?;
    Ok(outcome)
}

/// `createAction` through the gate: the toolbox allocates only coins the
/// gate offers. The caller holds the spending lock.
pub async fn create_action<V: WalletServices + Send + Sync + 'static>(
    wallet: &Wallet<StorageSqlx, V>,
    args: CreateActionArgs,
    originator: &str,
    opts: &GateOptions,
) -> std::result::Result<CreateActionResult, GateError> {
    let storage = wallet.storage();
    restore(storage.pool(), None)
        .await
        .map_err(GateError::Gate)?;
    let named: Vec<(String, u32)> = args
        .inputs
        .iter()
        .flatten()
        .map(|i| {
            (
                bsv_sdk::primitives::to_hex(&i.outpoint.txid),
                i.outpoint.vout,
            )
        })
        .collect();
    if !named.is_empty() {
        let held = assess_named(storage, wallet.services(), opts, &named, None)
            .await
            .map_err(GateError::Gate)?;
        if !held.is_empty() {
            return Err(GateError::Refused(Refusal {
                toolbox: NAMED_INPUT_REFUSED.to_string(),
                offered_satoshis: 0,
                held,
            }));
        }
    }
    let result = run_gated(wallet, args, originator, opts).await?;
    // The action records the word it was created under (R1 (d)): by its
    // txid, or a template for deferred signing by its reference.
    if opts.allow_received {
        if let Some(txid) = result.txid.as_ref() {
            record_rule(
                storage.pool(),
                &bsv_sdk::primitives::to_hex(txid),
                RECEIVED_PENDING,
            )
            .await
            .map_err(GateError::Gate)?;
        } else if let Some(st) = result.signable_transaction.as_ref() {
            record_rule_by_reference(
                storage.pool(),
                &reference_string(&st.reference),
                RECEIVED_PENDING,
            )
            .await
            .map_err(GateError::Gate)?;
        }
    }
    Ok(result)
}

async fn run_gated<V: WalletServices + Send + Sync + 'static>(
    wallet: &Wallet<StorageSqlx, V>,
    args: CreateActionArgs,
    originator: &str,
    opts: &GateOptions,
) -> std::result::Result<CreateActionResult, GateError> {
    let storage = wallet.storage();
    let need = args
        .outputs
        .as_ref()
        .map(|o| o.iter().map(|x| x.satoshis).sum::<u64>())
        .unwrap_or(0)
        .saturating_add(FEE_MARGIN);
    let first = assess(storage, wallet.services(), opts, Some(need))
        .await
        .map_err(GateError::Gate)?;
    let lazy = !first.unread.is_empty();
    match run_held(wallet, args.clone(), originator, &first).await? {
        Ok(result) => return Ok(result),
        Err(e) if !is_insufficient(&e) => return Err(GateError::Wallet(e)),
        Err(e) if !lazy => return Err(refusal(storage, &first, &e).await),
        Err(_) => {}
    }
    // The lazy read left coins unread: read every coin and try once more.
    let full = assess(storage, wallet.services(), opts, None)
        .await
        .map_err(GateError::Gate)?;
    match run_held(wallet, args, originator, &full).await? {
        Ok(result) => Ok(result),
        Err(e) if is_insufficient(&e) => Err(refusal(storage, &full, &e).await),
        Err(e) => Err(GateError::Wallet(e)),
    }
}

async fn refusal(storage: &StorageSqlx, assessment: &Assessment, e: &bsv_sdk::Error) -> GateError {
    let mut held = assessment.held.clone();
    match locked(storage.pool()).await {
        Ok(mut l) => held.append(&mut l),
        Err(fault) => return GateError::Gate(fault),
    }
    let toolbox = e.to_string();
    GateError::Refused(Refusal {
        toolbox: toolbox
            .strip_prefix("wallet error: ")
            .unwrap_or(&toolbox)
            .to_string(),
        offered_satoshis: assessment.offered_satoshis(),
        held,
    })
}

#[cfg(test)]
mod tests;
