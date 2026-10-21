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
//!    is the sender's broadcast. The action carries the word as a label,
//!    [`RECEIVED_PENDING_LABEL`], appended to its labels before the door, so
//!    the toolbox writes it inside the action's one transaction (a
//!    deferred-signing template keeps it when `signAction` signs it); shown
//!    by `actions` and `/listActions` as `underRule`. A 0.15 store's
//!    records (`spend_gate_actions`, by txid or by reference) keep
//!    answering; nothing writes them from 0.16.
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
//! # The shape: the offer
//!
//! The gate names the coins it offers and the toolbox allocates from those
//! only: one call, `Wallet::create_action_offering(args, originator,
//! offered)` (bsv-wallet-toolbox-rs 0.11.0; bsv-wallet-cli #44 beside
//! bsv-low #828). The toolbox's pre-check picks within the offer and its one
//! `BEGIN IMMEDIATE` transaction locks within it, so a coin outside it is
//! never allocated, under concurrency too, and the gate writes nothing
//! around the door: no hold before it, no restore after it (0.15 marked
//! every coin it did not offer `change = 0` for the call and restored it
//! after, a write after the broadcast; LOW's finding). The one leftover is
//! [`heal_holds`] for the holds a 0.15 process left behind: run once at
//! `daemon` and `serve` start (retried on the next served request when it
//! failed), and before the gate in `send`, `drain`, `split` and `gift-send`
//! when a hold is there ([`heal_if_held`]).
//!
//! The coins are read in the toolbox's own preference (completed first, then
//! the largest), a batch of creators at a time, until the coins offered
//! cover the outputs with a margin; the coins not read are not offered. Only
//! when the toolbox then finds no funds (or draws `Unspendable coin:`, a
//! coin offered it cannot carry) is every coin read, and the call run once
//! more; if that fails for funds, the refusal lists every coin held
//! back ([`Refusal`]). Nothing is fetched but the header service's tip and
//! headers: no chain scan, no explorer.

use std::collections::HashMap;

use anyhow::Result;
use bsv_sdk::wallet::{CreateActionArgs, CreateActionResult};
use bsv_tracker::Height;
use bsv_wallet_toolbox::{StorageSqlx, Wallet, WalletServices};
use serde::Serialize;
use sqlx::{Row, SqlitePool};

use crate::tracker_host::{read_creators, CreatorReading, TickOptions};

/// `SPEND_OWN_CHANGE` when unset: this host chains payments, so its own
/// change is offered before it is mined (R1 (b)).
pub const DEFAULT_SPEND_OWN_CHANGE: bool = true;
/// Creators read per batch while the offered coins do not yet cover.
const BATCH: usize = 32;
/// Satoshis read beyond the outputs before the lazy read stops (fees).
const FEE_MARGIN: u64 = 10_000;
/// The most coins one offer names. The toolbox binds every offered output
/// id, with the coins other calls in flight hold and the coins it skips, as
/// variables of one statement (`create_action.rs` `AND o.output_id IN
/// (...)`), and SQLite refuses a statement past its bound-variable limit
/// (`SQLITE_MAX_VARIABLE_NUMBER`, 32766 since 3.32.0). Half of it leaves the
/// other half for the reservations and the skips. Past the cap the largest
/// coins by value are offered and the rest are left unread: a transaction
/// this many inputs wide is past any fee or size a host would pay, so a
/// shortfall the largest 16384 coins cannot cover is the funds' refusal.
pub const OFFER_CAP: usize = 16_384;
/// R1 (d)'s rule, by the ruling's name (`lean/Wallet.lean`
/// `receivedPending`), as recorded on an action that named it.
pub const RECEIVED_PENDING: &str = "received-pending";
/// R1 (d)'s rule as the action's label: the toolbox writes it with the
/// action, in its one transaction (bsv-wallet-cli #44). The host writes
/// nothing about the rule after the door.
pub const RECEIVED_PENDING_LABEL: &str = "spend-gate:received-pending";
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
    /// Offerable coins past the offer's cap, not offered (D3 of the #44
    /// delta review): named in the refusal's text, never silently absent.
    #[serde(skip)]
    pub capped: Option<Capped>,
}

/// Offerable coins left out of an offer by its cap ([`OFFER_CAP`]): how
/// many, their satoshis, and the cap they were past.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Capped {
    pub coins: usize,
    pub satoshis: u64,
    pub cap: usize,
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
                "Insufficient funds: no offerable coin covers the action ({}; {} sats offered, {} coin(s) held back",
                self.toolbox,
                self.offered_satoshis,
                self.held.len()
            )?;
            if let Some(c) = &self.capped {
                write!(
                    f,
                    "; {} coins ({} sats) beyond the offer's cap of {}, the largest offered",
                    c.coins, c.satoshis, c.cap
                )?;
            }
            write!(f, ")")?;
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
/// refusal does, and so is the store busy, as [`busy_json`] (#44);
/// anything else is returned as it was.
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
        Err(e) if json && e.is_store_busy() => {
            println!("{}", busy_json(&e.to_string()));
            std::process::exit(1);
        }
        Err(e) => Err(e.into()),
    }
}

/// A command's spend-guard run before the gate: under `--json` the store
/// busy is printed as [`busy_json`] and the process exits 1; anything else
/// is returned as it was.
pub fn busy_as_json<T>(result: anyhow::Result<T>, json: bool) -> anyhow::Result<T> {
    match result {
        Err(e) if json && crate::store::is_store_busy(&e) => {
            println!("{}", busy_json(&format!("Storage error: spend guard: {e}")));
            std::process::exit(1);
        }
        other => other,
    }
}

/// The store busy as one JSON object, the route's 503 body: `{code:
/// "STORE_BUSY", message}`. Nothing was written, built, locked or posted;
/// the same command may be repeated.
pub fn busy_json(message: &str) -> serde_json::Value {
    serde_json::json!({"code": "STORE_BUSY", "message": message})
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

impl GateError {
    /// The store busy, by its type (#44): the toolbox's `WalletBusy` from
    /// the door, or a pre-door read or write that hit SQLite's busy or the
    /// writer's ceiling. Never by its words.
    pub fn is_store_busy(&self) -> bool {
        match self {
            Self::Wallet(bsv_sdk::Error::WalletBusy { .. }) => true,
            Self::Gate(e) => crate::store::is_store_busy(e),
            _ => false,
        }
    }
}

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
    /// Coins not read (enough was offered before them), and the coins past
    /// the offer's cap; not offered.
    pub unread: Vec<i64>,
    /// The offerable coins the cap left out of the offer (also `unread`).
    pub capped: Option<Capped>,
}

impl Assessment {
    pub fn offered_satoshis(&self) -> u64 {
        self.offered.iter().map(|(_, s)| s).sum()
    }

    /// The offer: the output ids of the coins offered, as the toolbox's
    /// `create_action_offering` takes them.
    pub fn offer(&self) -> Vec<i64> {
        self.offered.iter().map(|(id, _)| *id).collect()
    }
}

/// The toolbox's `reference` string of a signable transaction, as
/// `signAction` / `abortAction` take it back.
pub fn reference_string(reference: &[u8]) -> String {
    String::from_utf8(reference.to_vec()).unwrap_or_else(|_| hex::encode(reference))
}

/// The rule each action was created under, by txid; a template not yet
/// signed has none (see [`ActionRules`]).
pub async fn rules_of(storage: &StorageSqlx) -> Result<HashMap<String, String>> {
    Ok(action_rules(storage).await?.by_txid)
}

/// Label the action `transaction_id` with `rule`'s label inside the
/// caller's open transaction: for an act that writes its own action row
/// (`respend`), as the toolbox does for one through the door.
pub async fn label_in(
    conn: &mut sqlx::SqliteConnection,
    user_id: i64,
    transaction_id: i64,
    rule: &str,
) -> Result<()> {
    if rule != RECEIVED_PENDING {
        anyhow::bail!("no label for the rule {rule:?}");
    }
    let label_id: i64 = sqlx::query_scalar(
        "INSERT INTO tx_labels (user_id, label) VALUES (?, ?) \
         ON CONFLICT(label, user_id) DO UPDATE SET is_deleted = 0 RETURNING tx_label_id",
    )
    .bind(user_id)
    .bind(RECEIVED_PENDING_LABEL)
    .fetch_one(&mut *conn)
    .await?;
    sqlx::query(
        "INSERT INTO tx_labels_map (tx_label_id, transaction_id) VALUES (?, ?) \
         ON CONFLICT(tx_label_id, transaction_id) DO UPDATE SET is_deleted = 0",
    )
    .bind(label_id)
    .bind(transaction_id)
    .execute(&mut *conn)
    .await?;
    Ok(())
}

/// Strip R1 (d)'s label from a caller's labels, in any case or padding.
/// The label is the gate's record of the door's word: a caller who could
/// write it would read as `underRule` and grant a respend the received
/// rule without ever naming `allowPending`. Every door a caller's labels
/// pass (the gate's `createAction`, the served `internalizeAction`) strips
/// it; `signAction` takes no labels.
pub fn strip_rule_label(labels: &mut Option<Vec<String>>) {
    if let Some(labels) = labels.as_mut() {
        labels.retain(|l| !l.trim().eq_ignore_ascii_case(RECEIVED_PENDING_LABEL));
    }
}

/// The rule a label names, if it is one of the gate's.
pub fn rule_of_label(label: &str) -> Option<&'static str> {
    (label == RECEIVED_PENDING_LABEL).then_some(RECEIVED_PENDING)
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

/// Every recorded rule, as a listing shows it: the action's label first
/// ([`RECEIVED_PENDING_LABEL`], written by the toolbox with the action),
/// then a 0.15 store's `spend_gate_actions` rows (by txid, or by the
/// reference of a template, which names its txid once signed). Reads only.
pub async fn action_rules(storage: &StorageSqlx) -> Result<ActionRules> {
    let pool = storage.pool();
    // (txid or NULL, reference, label): one row per toolbox action that
    // carries the gate's label.
    let labelled = sqlx::query(
        "SELECT lower(t.txid) AS txid, t.reference, l.label FROM transactions t \
         JOIN tx_labels_map m ON m.transaction_id = t.transaction_id AND m.is_deleted = 0 \
         JOIN tx_labels l ON l.tx_label_id = m.tx_label_id AND l.is_deleted = 0 \
         WHERE l.label = ?",
    )
    .bind(RECEIVED_PENDING_LABEL)
    .fetch_all(pool)
    .await?;
    let mut by_txid: HashMap<String, String> = HashMap::new();
    let mut by_reference: HashMap<String, String> = HashMap::new();
    for r in &labelled {
        let Some(rule) = rule_of_label(&r.get::<String, _>("label")) else {
            continue;
        };
        match r.get::<Option<String>, _>("txid") {
            Some(txid) => {
                by_txid.insert(txid, rule.to_string());
            }
            None => {
                if let Some(reference) = r.get::<Option<String>, _>("reference") {
                    by_reference.insert(reference, rule.to_string());
                }
            }
        }
    }
    // A 0.15 store's records, read where the label says nothing.
    if crate::store::has_column(pool, "spend_gate_actions", "reference").await? {
        let legacy = sqlx::query(
            "SELECT lower(COALESCE(g.txid, t.txid)) AS txid, g.reference, g.rule \
             FROM spend_gate_actions g LEFT JOIN transactions t ON t.reference = g.reference",
        )
        .fetch_all(pool)
        .await?;
        for r in &legacy {
            let rule: String = r.get("rule");
            match r.get::<Option<String>, _>("txid") {
                Some(txid) => {
                    by_txid.entry(txid).or_insert(rule);
                }
                None => {
                    if let Some(reference) = r.get::<Option<String>, _>("reference") {
                        by_reference.entry(reference).or_insert(rule);
                    }
                }
            }
        }
    }
    let rows = sqlx::query(
        "SELECT t.status, t.description, t.satoshis, t.reference FROM transactions t \
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
        let rule: Option<String> = r
            .get::<Option<String>, _>("reference")
            .and_then(|reference| by_reference.get(&reference).cloned());
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
async fn candidates(storage: &StorageSqlx) -> Result<Vec<Candidate>> {
    let frozen = crate::tracker_host::frozen_outpoints(storage).await?;
    let pool = storage.pool();
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
    let all = candidates(storage).await?;
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
    cap_offer(&mut out, OFFER_CAP);
    Ok(out)
}

/// At most `cap` ([`OFFER_CAP`]) coins offered: the largest by value stay
/// offered (in the order read), the rest go unread and are counted in
/// `capped`, so a refusal names them.
fn cap_offer(out: &mut Assessment, cap: usize) {
    if out.offered.len() <= cap {
        return;
    }
    let mut by_value: Vec<usize> = (0..out.offered.len()).collect();
    by_value.sort_by(|a, b| out.offered[*b].1.cmp(&out.offered[*a].1).then(a.cmp(b)));
    let mut keep = vec![false; out.offered.len()];
    for i in &by_value[..cap] {
        keep[*i] = true;
    }
    let offered = std::mem::take(&mut out.offered);
    let mut capped = Capped {
        coins: 0,
        satoshis: 0,
        cap,
    };
    for (coin, kept) in offered.into_iter().zip(keep) {
        if kept {
            out.offered.push(coin);
        } else {
            out.unread.push(coin.0);
            capped.coins += 1;
            capped.satoshis += coin.1;
        }
    }
    out.capped = Some(capped);
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
    let frozen = crate::tracker_host::frozen_outpoints(storage).await?;
    let pool = storage.pool();
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

/// Heal the holds a 0.15 process left: every coin listed in
/// `spend_gate_holds` that still reads `change = 0` is change again, and the
/// list is emptied, in one transaction. 0.15 held the coins it did not offer
/// for the call and restored them after; a call that did not finish left its
/// holds behind. From 0.16 the gate offers instead and writes no hold, so
/// the table is read only here (kept readable for this; nothing writes a
/// row). Run once at `daemon` and `serve` start (and on the next served
/// request when that failed), and in a spending command's gated path
/// ([`heal_before_spend`]); a store without the table is left untouched.
/// Returns the coins made change again.
pub async fn heal_holds(storage: &StorageSqlx) -> Result<u64> {
    if !crate::store::has_column(storage.pool(), "spend_gate_holds", "output_id").await? {
        return Ok(0);
    }
    let mut tx = storage.write_tx().await?;
    let healed = sqlx::query(
        "UPDATE outputs SET change = 1 WHERE change = 0 \
         AND output_id IN (SELECT output_id FROM spend_gate_holds)",
    )
    .execute(&mut *tx)
    .await?
    .rows_affected();
    sqlx::query("DELETE FROM spend_gate_holds")
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(healed)
}

/// [`heal_holds`] only when there is a hold to heal: a read on the reader
/// first (the table, then `EXISTS` a row), so a store with nothing to heal
/// never queues for the writer. Returns the coins made change again.
pub async fn heal_if_held(storage: &StorageSqlx) -> Result<u64> {
    if !crate::store::has_column(storage.pool(), "spend_gate_holds", "output_id").await? {
        return Ok(0);
    }
    let held: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM spend_gate_holds)")
        .fetch_one(storage.pool())
        .await?;
    if !held {
        return Ok(0);
    }
    heal_holds(storage).await
}

/// The heal in a spending command's gated path (`send`, `drain`, `split`,
/// `gift-send`): a user who never runs `daemon` or `serve` still gets back
/// the coins a 0.15 process held. Logged either way, never the command's
/// error: a coin left held is only not offered, and the gate's refusal
/// stays true.
pub async fn heal_before_spend(storage: &StorageSqlx) {
    match heal_if_held(storage).await {
        Ok(0) => {}
        Ok(n) => tracing::info!(
            coins = n,
            "spend gate: healed the holds a 0.15 process left"
        ),
        Err(e) => tracing::warn!(
            error = %e,
            "spend gate: the holds a 0.15 process left were not healed"
        ),
    }
}

fn is_insufficient(e: &bsv_sdk::Error) -> bool {
    e.to_string()
        .to_ascii_lowercase()
        .contains("insufficient funds")
}

/// The toolbox's `Unspendable coin:` refusal (409): a coin it was offered
/// cannot be carried, and the other coins offered do not cover the action.
fn is_unspendable(e: &bsv_sdk::Error) -> bool {
    e.to_string().contains("Unspendable coin:")
}

/// One toolbox call, offered the coins the assessment offers: the door.
async fn run_offered<V: WalletServices + Send + Sync + 'static>(
    wallet: &Wallet<StorageSqlx, V>,
    args: CreateActionArgs,
    originator: &str,
    assessment: &Assessment,
) -> bsv_sdk::Result<CreateActionResult> {
    let answer = wallet
        .create_action_offering(args, originator, &assessment.offer())
        .await;
    #[cfg(test)]
    let _ = tests::AT_THE_DOOR.try_with(|at| at());
    answer
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
                capped: None,
            }));
        }
    }
    // The action carries the word it was created under (R1 (d)) as its
    // label: the toolbox writes it inside the action's one transaction, and
    // nothing is written after the door.
    // A caller's own copy of the label is stripped first (F4 of the #44 CLI
    // review): only the door's word writes it.
    let mut args = args;
    strip_rule_label(&mut args.labels);
    if opts.allow_received {
        args.labels
            .get_or_insert_with(Vec::new)
            .push(RECEIVED_PENDING_LABEL.to_string());
    }
    run_gated(wallet, args, originator, opts).await
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
    match run_offered(wallet, args.clone(), originator, &first).await {
        Ok(result) => return Ok(result),
        // An unspendable coin in the lazy offer: the coins not read may
        // cover without it (F7 of the #44 CLI review).
        Err(e) if lazy && is_unspendable(&e) => {}
        Err(e) if !is_insufficient(&e) => return Err(GateError::Wallet(e)),
        Err(e) if !lazy => return Err(refusal(storage, &first, &e).await),
        Err(_) => {}
    }
    // The lazy read left coins unread: read every coin and try once more.
    let full = assess(storage, wallet.services(), opts, None)
        .await
        .map_err(GateError::Gate)?;
    match run_offered(wallet, args, originator, &full).await {
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
        capped: assessment.capped,
    })
}

#[cfg(test)]
mod tests;
