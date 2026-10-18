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
//!    never offered, by no setting (`ruleFor` reads no `received` arm).
//!
//! A coin whose creator is `stale`, `rejected`, `conflicted` or `abandoned`
//! is never offered; a coin an action of ours holds is the toolbox's lock
//! (`spent_by`) and is never a candidate. A frozen coin has no record in
//! this store (the feed's `frozen` event moves no tracked row at 0.9.0), so
//! there is nothing to read for it here.
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

/// The gate's two settings.
#[derive(Debug, Clone)]
pub struct GateOptions {
    /// `SPEND_DEPTH`, floor 1 (R3 (b)).
    pub spend_depth: Height,
    /// `SPEND_OWN_CHANGE` (R1 (b)): offer our own `made` change whose
    /// creator's word is `pending`.
    pub own_change: bool,
}

impl Default for GateOptions {
    fn default() -> Self {
        Self {
            spend_depth: crate::tracker_host::DEFAULT_SPEND_DEPTH,
            own_change: DEFAULT_SPEND_OWN_CHANGE,
        }
    }
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
        }
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
    /// no proof`, `own change, SPEND_OWN_CHANGE=0`, `locked by <txid>`.
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
        write!(
            f,
            "Insufficient funds: no offerable coin covers the action ({}; {} sats offered, {} coin(s) held back)",
            self.toolbox,
            self.offered_satoshis,
            self.held.len()
        )?;
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
    Ok(())
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
        "mined" if reading.suspect => Some("suspect: a fork at or below its height".to_string()),
        "mined" => Some(depth),
        "stale" | "rejected" | "conflicted" | "abandoned" => {
            Some(format!("{}: never offered", reading.word))
        }
        // `pending`: the word claims nothing about the chain; only R1's
        // rule for our own change offers it.
        _ => match origin {
            Origin::Received => Some(format!("{depth}; received, no proof")),
            Origin::Made if opts.own_change => None,
            Origin::Made => Some(format!("{depth}; own change, SPEND_OWN_CHANGE=0")),
        },
    }
}

/// Every coin the toolbox could allocate: a change output of a default
/// basket, unspent, its transaction `completed` or `unproven`. In the
/// toolbox's preference: completed first, then the largest.
async fn candidates(pool: &SqlitePool) -> Result<Vec<Candidate>> {
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
        })
        .collect())
}

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
