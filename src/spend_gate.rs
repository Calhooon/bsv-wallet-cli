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
//!    the checked root now, the creator is not marked suspect (a fork or a
//!    split at or below `h`, read as the tracker left it: the guard never
//!    clears the mark, #164), and the block is `SPEND_DEPTH` deep (the tip
//!    is 1). **R3 (b)**: the depth is the host's parameter, 1 the floor
//!    ([`crate::tracker_host::read_creators`], the 0.9.0 `spend_guard` run
//!    over many creators at once); or
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
//! A coin whose creator is `stale` (its proof's block left the chain, the
//! transaction confirming again) ranks as `seen` at the door, offered under
//! rule 2 or 3 as a `pending` creator's coin is and never with no rule (the
//! owner's ruling 7a of 2026-10-20, `lean/Wallet.lean` `doorWord`,
//! `stale_ranks_as_seen`). A coin whose creator is `rejected`, `conflicted`
//! or `abandoned` is never offered. Nor is our own coin whose creator's
//! lineage is dead, by any rule or word and whatever the creator's own word,
//! mined included: an input of the creator, or of an ancestor of ours on the
//! walk, was created by an action of this wallet, made or received, whose
//! word is dead, a held proof that checks reading mined for the walk as for
//! the door (the owner's lineage ruling of 2026-10-22, `lean/Wallet.lean`
//! `deadLineage`; held `dead lineage: ancestor <txid> <word>` after the
//! door, in the Lean's clause order; [`lineage_candidates`], one read of
//! the store per call, a dead word beside a proof read with the creators in
//! the gate's one reading and its one retry, then [`Lineage::dead`]; the
//! CLI's walk until the toolbox's lands, toolbox #42). A coin an action of
//! ours holds is the toolbox's lock
//! (`spent_by`) and is never a candidate. A frozen coin (the feed's
//! `frozen` event, recorded in `frozen_outpoints` by
//! [`crate::tracker_host::deliver_chain_event`]) is never offered, rule or
//! no rule (R6: never cleared here). A coin the store marks not spendable
//! with no action of ours spending it (`spendable = 0`, `spent_by` empty:
//! the relinquished record `reconcile-outputs` writes, the charter's
//! `lost(c)`) is held `lost` before its creator is read, as the frozen coin
//! is (#165): the toolbox's allocation would refuse it, and the gate's word
//! names why.
//!
//! Coins a caller names (`createAction`'s `inputs`, `respend`'s exact
//! inputs) pass the same per-coin judgment ([`assess_named`]); a named coin
//! the gate would hold refuses the action with the same `held[]` listing
//! (the charter's `onCreate`: every input offerable). A named outpoint that
//! is not a coin of this wallet is refused at the gate too, the caller's
//! 400 ([`GateError::Scope`], [`NOT_HELD`]), before anything is stored or
//! the toolbox asked (#166; the charter: a coin is an output of ours, and
//! the door decides before the store is asked). The one outpoint left to
//! the toolbox is the caller's own input whose transaction its `inputBEEF`
//! carries whole (a covenant, a puzzle): not a coin of ours, and the
//! toolbox checks the BEEF.
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
//!
//! # The word's scope (#63, 0.18.0)
//!
//! The word may name the coins it reaches: `allowPending: {"received":
//! ["txid:vout", ...]}` on the served `/createAction`, `--allow-pending
//! received --coin txid:vout` (repeatable) on the commands
//! ([`AllowPending::ReceivedOnly`]). Only the named received coins are
//! offered under it; every other received unproven coin is held as without
//! the word, its reason naming the scope ([`NOT_NAMED_HINT`]). A named coin
//! that is not a received coin this wallet could spend (not ours, own
//! change, consumed by another action, a nosend) refuses the action before
//! the door, naming it ([`GateError::Scope`]). The scope narrows what the
//! word reaches and never widens it; the string form `"received"` stays
//! every received coin.
//!
//! # The header service's fault (#59, 0.18.0)
//!
//! A coin whose creator the gate could not check because the header service
//! did not answer (its tip or a header the check needs: a transport fault,
//! an outage) is held "headers unavailable: <the service's error>", and the
//! hold is marked as the service's ([`HeldCoin::headers_fault`]). Such a
//! hold is no verdict about the wallet: the gate reads those creators once
//! more after [`HEADER_RETRY_PAUSE`], once per gated call, and when the
//! coins still held on the fault could have covered the action (with what
//! was offered, the outputs' sum at least), the answer is "could not
//! verify now" ([`Refusal::headers_fault`], code `HEADERS_UNAVAILABLE`, the
//! route's 503 with `Retry-After`), never `INSUFFICIENT_FUNDS`. A shortfall
//! the faulted coins could not have covered is the funds' refusal as
//! before. A named input held on the fault alone is the same answer.

use std::collections::HashMap;
use std::time::Duration;

use anyhow::Result;
use bsv_sdk::wallet::{CreateActionArgs, CreateActionResult};
use bsv_tracker::Height;
use bsv_wallet_toolbox::{StorageSqlx, Wallet, WalletServices};
use serde::Serialize;
use sqlx::{Row, SqlitePool};

use crate::server::error::Kind;
use crate::toolbox_words;
use crate::tracker_host::{read_creators, CreatorReading, TickOptions};

mod lineage;
pub use lineage::{join_pairs, lineage_candidates, DeadAncestor, Lineage, Pair, DEAD_LINEAGE};

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
/// Appended to a held received `pending` coin's reason when the action's
/// word is scoped and does not name it (#63).
pub const NOT_NAMED_HINT: &str = "not named under allowPending: received";
/// The pause before the one re-read of the creators the header service did
/// not answer for (#59): the blink seen on the 0.17.1 gate and on LOW's
/// vultr funder lasted about two seconds.
pub const HEADER_RETRY_PAUSE: Duration = Duration::from_secs(2);
/// The route's `Retry-After` on `HEADERS_UNAVAILABLE`, in seconds.
pub const HEADERS_RETRY_AFTER_SECS: u64 = 2;
/// The refusal's code when the header service's fault held the coins that
/// could have covered the action (#59).
pub const HEADERS_UNAVAILABLE: &str = Kind::HeadersUnavailable.code();
/// The reason prefix of a coin held on the header service's fault.
const HEADERS_UNAVAILABLE_PREFIX: &str = "headers unavailable: ";
/// The refusal's toolbox line when a coin a caller named is held.
pub const NAMED_INPUT_REFUSED: &str = "an input the spend guard does not offer";

/// The gate's two settings, and the tracker's parameters its reads run
/// under.
#[derive(Debug, Clone)]
pub struct GateOptions {
    /// `SPEND_DEPTH`, floor 1 (R3 (b)).
    pub spend_depth: Height,
    /// `SPEND_OWN_CHANGE` (R1 (b)): offer our own `made` change whose
    /// creator's word is `pending`.
    pub own_change: bool,
    /// R1 (d): this one action's word at the door ([`AllowPending`]).
    /// Never read from the configuration; off unless the caller says so.
    pub allow_pending: AllowPending,
    /// The tracker's parameters (`TRACKER_AGE_SECS`, `TRACKER_MAX_ASKS`)
    /// the gate's reads of the creators run under.
    pub tracker: TickOptions,
}

impl Default for GateOptions {
    fn default() -> Self {
        Self {
            spend_depth: crate::tracker_host::DEFAULT_SPEND_DEPTH,
            own_change: DEFAULT_SPEND_OWN_CHANGE,
            allow_pending: AllowPending::Off,
            tracker: TickOptions::default(),
        }
    }
}

/// The door's word: `allowPending` is on only for the one value
/// `"received"`; absent or anything else is off.
pub fn allow_pending(word: Option<&str>) -> bool {
    word == Some(ALLOW_PENDING_RECEIVED)
}

/// R1 (d)'s word at the door, as one action names it: off, every received
/// unproven coin, or only the named received coins (#63).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum AllowPending {
    /// No received unproven coin is offered.
    #[default]
    Off,
    /// `allowPending: "received"`, `--allow-pending received`: every
    /// received unproven coin of the wallet is offered.
    Received,
    /// `allowPending: {"received": ["txid:vout", ...]}`, `--allow-pending
    /// received --coin txid:vout` (repeatable): only the named received
    /// coins are offered under the word; every other received unproven coin
    /// is held as without it. Never empty (an empty list is [`Self::Off`]);
    /// sorted, no repeats; txids lowercased.
    ReceivedOnly(Vec<(String, u32)>),
}

impl AllowPending {
    /// The word is on for this action: the action carries its label.
    pub fn is_on(&self) -> bool {
        !matches!(self, Self::Off)
    }

    /// The word reaches the received coin `txid:vout`.
    pub fn reaches(&self, txid: &str, vout: u32) -> bool {
        match self {
            Self::Off => false,
            Self::Received => true,
            Self::ReceivedOnly(coins) => coins.iter().any(|(t, v)| t == txid && *v == vout),
        }
    }

    /// The coins a scoped word names; none for the other two.
    pub fn named(&self) -> &[(String, u32)] {
        match self {
            Self::ReceivedOnly(coins) => coins,
            _ => &[],
        }
    }

    /// The served `/createAction`'s `allowPending`: absent is off; the
    /// string `"received"` is every received coin and any other string is
    /// off (as 0.15 read it); an object `{"received": ["txid:vout", ...]}`
    /// is the scope (an empty list is off); an object of another shape, or
    /// an outpoint that does not parse, is the caller's error. Any other
    /// JSON value is off.
    pub fn from_json(value: Option<&serde_json::Value>) -> std::result::Result<Self, String> {
        const SHAPE: &str = "allowPending: an object is {\"received\": [\"txid:vout\", ...]}";
        match value {
            None => Ok(Self::Off),
            Some(serde_json::Value::String(word)) => Ok(if allow_pending(Some(word)) {
                Self::Received
            } else {
                Self::Off
            }),
            Some(serde_json::Value::Object(map)) => {
                if map.len() != 1 {
                    return Err(SHAPE.to_string());
                }
                let Some(items) = map.get(ALLOW_PENDING_RECEIVED).and_then(|v| v.as_array()) else {
                    return Err(SHAPE.to_string());
                };
                let mut coins = Vec::with_capacity(items.len());
                for item in items {
                    let Some(text) = item.as_str() else {
                        return Err(SHAPE.to_string());
                    };
                    coins.push(parse_outpoint(text)?);
                }
                Ok(Self::scoped(coins))
            }
            Some(_) => Ok(Self::Off),
        }
    }

    /// A command's `--allow-pending <WORD>` with its `--coin <TXID:VOUT>`s:
    /// the word alone is every received coin; with coins, only those; a
    /// coin without the word is the caller's error.
    pub fn from_cli(word: Option<&str>, coins: &[String]) -> std::result::Result<Self, String> {
        if !allow_pending(word) {
            if coins.is_empty() {
                return Ok(Self::Off);
            }
            return Err(
                "--coin names a coin under --allow-pending received, which is not given"
                    .to_string(),
            );
        }
        if coins.is_empty() {
            return Ok(Self::Received);
        }
        let coins = coins
            .iter()
            .map(|c| parse_outpoint(c))
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(Self::scoped(coins))
    }

    fn scoped(mut coins: Vec<(String, u32)>) -> Self {
        coins.sort();
        coins.dedup();
        if coins.is_empty() {
            Self::Off
        } else {
            Self::ReceivedOnly(coins)
        }
    }
}

/// An outpoint as a caller writes it, `txid:vout` (or `txid.vout`, as the
/// listings print one): the txid 64 hex characters, lowercased here.
pub fn parse_outpoint(text: &str) -> std::result::Result<(String, u32), String> {
    let text = text.trim();
    let Some((txid, vout)) = text.rsplit_once([':', '.']) else {
        return Err(format!("{text:?} is not an outpoint (txid:vout)"));
    };
    if txid.len() != 64 || !txid.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(format!(
            "{text:?} is not an outpoint: the txid is not 64 hex characters"
        ));
    }
    let vout: u32 = vout
        .parse()
        .map_err(|_| format!("{text:?} is not an outpoint: the vout is not a number"))?;
    Ok((txid.to_ascii_lowercase(), vout))
}

impl GateOptions {
    /// `SPEND_DEPTH` and `SPEND_OWN_CHANGE` (`0`, `false`, `no`, `off` turn
    /// it off; anything else, or unset, leaves it on), as the configuration
    /// read them at start: built once, by the server at its start or by a
    /// command, never per spend.
    #[must_use]
    pub fn from_config(config: &crate::config::Config) -> Self {
        let tracker = TickOptions::from_config(config);
        Self {
            spend_depth: tracker.spend_depth,
            own_change: config.host.spend_own_change,
            allow_pending: AllowPending::Off,
            tracker,
        }
    }

    /// These settings, with the action's word for R1 (d): every received
    /// coin, or none.
    pub fn with_allow_received(mut self, allow: bool) -> Self {
        self.allow_pending = if allow {
            AllowPending::Received
        } else {
            AllowPending::Off
        };
        self
    }

    /// These settings, with the action's word for R1 (d) as given (#63).
    pub fn with_allow_pending(mut self, word: AllowPending) -> Self {
        self.allow_pending = word;
        self
    }

    fn tick(&self) -> TickOptions {
        TickOptions {
            spend_depth: self.spend_depth,
            ..self.tracker.clone()
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
    /// `locked` for a coin a live action of ours consumed as an input,
    /// `frozen` or `lost` for the coin's own record.
    pub word: String,
    pub origin: Origin,
    /// Minutes since the wallet learned of the coin.
    pub age_minutes: i64,
    /// The setting or rule that holds it: `depth 1: 0 blocks`, `received,
    /// no proof; offered under allowPending: received`, `own change,
    /// SPEND_OWN_CHANGE=0`, `spent by <txid> (unproven)`, `frozen: never
    /// offered`, `lost: the store marks it not spendable`. `spent by` is
    /// never a hold: the coin is an input a live action of ours consumed,
    /// spent for good; it leaves the listing when that spender is completed
    /// (its proof lands) or failed (#53).
    pub reason: String,
    /// The hold is the header service's fault (the reason `headers
    /// unavailable: <its error>`): its error, no verdict on the coin; a
    /// later read may offer it (#59).
    #[serde(skip)]
    pub headers_fault: Option<String>,
    /// The store's output id; not in the JSON.
    #[serde(skip)]
    pub output_id: i64,
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
    /// "Could not verify now" (#59): the header service's error, when its
    /// fault held coins that could have covered the action (or every named
    /// input held is held on it). The code is then `HEADERS_UNAVAILABLE`,
    /// never a verdict about the wallet.
    #[serde(skip)]
    pub headers_fault: Option<String>,
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
        if let Some(fault) = &self.headers_fault {
            let on_fault = self.held.iter().filter(|c| c.headers_fault.is_some());
            let (coins, sats) = on_fault.fold((0usize, 0u64), |(n, s), c| (n + 1, s + c.satoshis));
            if self.toolbox == NAMED_INPUT_REFUSED {
                write!(
                    f,
                    "Could not verify now: the header service did not answer ({fault}); a named input is held on it, no verdict ({coins} coin(s), {sats} sats)"
                )?;
            } else {
                write!(
                    f,
                    "Could not verify now: the header service did not answer ({fault}); {coins} coin(s), {sats} sats, held on it, no verdict ({}; {} sats offered, {} coin(s) held back)",
                    self.toolbox,
                    self.offered_satoshis,
                    self.held.len()
                )?;
            }
        } else if self.toolbox == NAMED_INPUT_REFUSED {
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

    /// The refusal's code: `HEADERS_UNAVAILABLE` when the header service's
    /// fault is the reason (#59), else `INPUT_NOT_OFFERED` for a named coin
    /// held, else `INSUFFICIENT_FUNDS`.
    pub fn code(&self) -> &'static str {
        if self.headers_fault.is_some() {
            Kind::HeadersUnavailable.code()
        } else if self.is_named_input() {
            Kind::InputNotOffered.code()
        } else {
            Kind::InsufficientFunds.code()
        }
    }

    /// "Could not verify now": the header service's fault, not a verdict.
    pub fn is_headers_fault(&self) -> bool {
        self.headers_fault.is_some()
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
        Err(GateError::Scope(message)) if json => {
            println!(
                "{}",
                serde_json::json!({"code": Kind::Validation.code(), "message": message})
            );
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
    serde_json::json!({"code": Kind::StoreBusy.code(), "message": message})
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
    /// The caller's scoped word names a coin that is not a received coin
    /// this wallet could spend (#63), or a named input is not a coin of
    /// this wallet (#166): the caller's error, before the door.
    Scope(String),
}

impl std::fmt::Display for GateError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Refused(r) => r.fmt(f),
            Self::Wallet(e) => e.fmt(f),
            Self::Gate(e) => write!(f, "Storage error: spend gate: {e}"),
            Self::Scope(message) => f.write_str(message),
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
    /// The store's `spendable = 0` with no spender: the coin is lost.
    lost: bool,
}

impl Candidate {
    /// The hold the coin's own record makes, its creator unread: frozen
    /// (the Lean's `coinWord` reads it first), then lost.
    fn record_hold(&self) -> Option<(&'static str, &'static str)> {
        if self.frozen {
            Some(("frozen", FROZEN))
        } else if self.lost {
            Some(("lost", LOST))
        } else {
            None
        }
    }
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
    /// The header service did not answer for some creators and they were
    /// read once more after [`HEADER_RETRY_PAUSE`] (#59).
    pub headers_retried: bool,
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

/// Why `reading` holds the coin `outpoint` of `origin`, or `None` when it
/// is offered.
pub fn hold_reason(
    reading: &CreatorReading,
    origin: Origin,
    outpoint: (&str, u32),
    opts: &GateOptions,
) -> Option<String> {
    if let Some(fault) = &reading.fault {
        return Some(format!("{HEADERS_UNAVAILABLE_PREFIX}{fault}"));
    }
    if reading.offered() {
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
        "rejected" | "conflicted" | "abandoned" => Some(format!("{}: never offered", reading.word)),
        // `stale` ranks as `seen` at the door (the owner's ruling 7a of
        // 2026-10-20, `lean/Wallet.lean` `doorWord`): its proof's block left
        // the chain and the transaction is confirming again, offered under
        // the rule a pending creator's coin is and never with no rule.
        "stale" => rule_hold(origin, outpoint, opts, STALE, "received, confirming again"),
        // `pending`: the word claims nothing about the chain.
        _ => rule_hold(origin, outpoint, opts, &depth, "received, no proof"),
    }
}

/// The word a held `stale` creator's reason opens with.
const STALE: &str = "stale: its block left the chain";

/// The door of a creator whose word claims nothing about the chain (or
/// ranks as such): only R1's rule for our own change (b), or the action's
/// word for a received coin (d), offers it. `word` opens the reason and
/// `received` says what the received creator is.
fn rule_hold(
    origin: Origin,
    outpoint: (&str, u32),
    opts: &GateOptions,
    word: &str,
    received: &str,
) -> Option<String> {
    match origin {
        Origin::Received if opts.allow_pending.reaches(outpoint.0, outpoint.1) => None,
        Origin::Received if opts.allow_pending.is_on() => {
            Some(format!("{word}; {received}; {NOT_NAMED_HINT}"))
        }
        Origin::Received => Some(format!("{word}; {received}; {ALLOW_PENDING_HINT}")),
        Origin::Made if opts.own_change => None,
        Origin::Made => Some(format!("{word}; own change, SPEND_OWN_CHANGE=0")),
    }
}

/// Why the gate holds the coin `outpoint` of `origin`: the creator's door
/// ([`hold_reason`]), then the lineage rule, in the Lean's clause order
/// (`WalletGate.clause`: the door, then `deadLineage`); a dead lineage
/// outranks a header fault, which only says the gate could not look. A
/// received coin's lineage is never read; `dead` is [`Lineage::dead`]'s
/// answer over the gate's own readings. The reason, and the header
/// service's error when the hold is its fault ([`HeldCoin::headers_fault`]).
fn coin_hold(
    reading: &CreatorReading,
    dead: &HashMap<String, DeadAncestor>,
    origin: Origin,
    outpoint: (&str, u32),
    opts: &GateOptions,
) -> Option<(String, Option<String>)> {
    let lineage = match origin {
        Origin::Made => dead.get(outpoint.0),
        Origin::Received => None,
    };
    match (hold_reason(reading, origin, outpoint, opts), lineage) {
        (Some(_), Some(ancestor)) if reading.fault.is_some() => Some((ancestor.reason(), None)),
        (None, Some(ancestor)) => Some((ancestor.reason(), None)),
        (reason, _) => reason.map(|reason| (reason, headers_error(reading))),
    }
}

/// The lineage's candidates, read only when some coin of ours is judged: a
/// gate over received coins alone reads none.
async fn lineage_of(storage: &StorageSqlx, any_made: bool) -> Result<Lineage> {
    if any_made {
        lineage_candidates(storage).await
    } else {
        Ok(Lineage::default())
    }
}

/// Read `batch` into `readings` against one read of the headers. When
/// `retry` and the assessment has not retried yet, the readings the header
/// service did not answer for are read once more after
/// [`HEADER_RETRY_PAUSE`] (#59), and the assessment says so.
async fn read_batch<V: WalletServices + ?Sized>(
    storage: &StorageSqlx,
    services: &V,
    tick: &TickOptions,
    (batch, retry): (&[Pair], bool),
    readings: &mut HashMap<String, CreatorReading>,
    out: &mut Assessment,
) -> Result<()> {
    readings.extend(read_creators(storage, services, tick, batch).await?);
    if !retry || out.headers_retried {
        return Ok(());
    }
    let faulted: Vec<_> = batch
        .iter()
        .filter(|(t, _)| readings.get(t).is_some_and(|r| r.headers_fault))
        .cloned()
        .collect();
    if faulted.is_empty() {
        return Ok(());
    }
    out.headers_retried = true;
    let fault = readings
        .get(&faulted[0].0)
        .and_then(|r| r.fault.clone())
        .unwrap_or_default();
    tracing::warn!(
        creators = faulted.len(),
        pause_ms = HEADER_RETRY_PAUSE.as_millis() as u64,
        fault = %fault,
        "spend gate: the header service did not answer; reading it once more"
    );
    tokio::time::sleep(HEADER_RETRY_PAUSE).await;
    let again = read_creators(storage, services, tick, &faulted).await?;
    let answered = again.values().filter(|r| !r.headers_fault).count();
    tracing::info!(
        creators = faulted.len(),
        answered,
        "spend gate: the header service read once more"
    );
    readings.extend(again);
    Ok(())
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
                p.merkle_path, o.spendable \
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
            lost: r.get::<i64, _>("spendable") == 0,
        })
        .collect())
}

/// The reason for a coin an action of ours consumed: `spent by <txid>
/// (unproven)` while the spender is live (any status but `completed` or
/// `failed`; `None` = known live), `spent by <txid>` once it is completed,
/// `spent by <txid> (failed)` for a failed spender not yet released.
pub(crate) fn spent_by_reason(spender: &str, spender_status: Option<&str>) -> String {
    match spender_status {
        Some("completed") => format!("spent by {spender}"),
        Some("failed") => format!("spent by {spender} (failed)"),
        _ => format!("spent by {spender} (unproven)"),
    }
}

/// The frozen record's hold (R6: never cleared here).
const FROZEN: &str = "frozen: never offered";
/// The lost record's hold: the store marks the coin not spendable
/// (`spendable = 0`) and no action of ours spends it (`spent_by` empty), as
/// `reconcile-outputs` and a relinquish write it (#165; the charter's
/// `lost(c)`, `Wallet.coinWord`).
const LOST: &str = "lost: the store marks it not spendable";

/// The coins a live action of ours consumed (`spent_by` an action not yet
/// `completed` or `failed`): listed in a refusal as `spent by <txid>
/// (unproven)`, never offered. Not a hold that clears: each is an input that
/// action spent for good, and it leaves the listing when the spender is
/// completed (its proof lands) or failed (#53).
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
            reason: spent_by_reason(
                &r.get::<Option<String>, _>("holder").unwrap_or_default(),
                None,
            ),
            headers_fault: None,
            output_id: r.get("output_id"),
        })
        .collect())
}

/// Read the coins and judge each. With `need`, stop reading once the coins
/// offered reach it (the rest are `unread`); with `None`, read every coin.
/// The header service is read once; [`assess_retrying`] is the door's
/// read.
pub async fn assess<V: WalletServices + ?Sized>(
    storage: &StorageSqlx,
    services: &V,
    opts: &GateOptions,
    need: Option<u64>,
) -> Result<Assessment> {
    assess_inner(storage, services, opts, need, false).await
}

/// [`assess`], and when the header service did not answer for a batch's
/// creators, those creators read once more after [`HEADER_RETRY_PAUSE`]
/// (#59): once per assessment, and only when `retry` (the door's first
/// read; its full read retries only if the first did not).
pub async fn assess_retrying<V: WalletServices + ?Sized>(
    storage: &StorageSqlx,
    services: &V,
    opts: &GateOptions,
    need: Option<u64>,
    retry: bool,
) -> Result<Assessment> {
    assess_inner(storage, services, opts, need, retry).await
}

async fn assess_inner<V: WalletServices + ?Sized>(
    storage: &StorageSqlx,
    services: &V,
    opts: &GateOptions,
    need: Option<u64>,
    retry: bool,
) -> Result<Assessment> {
    let all = candidates(storage).await?;
    let tick = opts.tick();
    let lineage = lineage_of(
        storage,
        all.iter()
            .any(|c| c.origin == Origin::Made && c.record_hold().is_none()),
    )
    .await?;
    // Only a coin its own record does not hold needs its creator read: a
    // creator whose coins are all frozen or lost is never fetched.
    let mut by_creator: Vec<Pair> = Vec::new();
    join_pairs(
        &mut by_creator,
        all.iter()
            .filter(|c| c.record_hold().is_none())
            .map(|c| (c.txid.clone(), c.proof.clone())),
    );
    let batch_end = |next: usize| match need {
        Some(_) => (next + BATCH).min(by_creator.len()),
        None => by_creator.len(),
    };
    let mut readings: HashMap<String, CreatorReading> = HashMap::new();
    let mut out = Assessment::default();
    let mut next = 0;
    // The candidates holding a proof are read with the first batch of
    // creators, one reading of each, retried with them: the walk and the
    // door read one word per creator.
    if !lineage.proofs().is_empty() {
        next = batch_end(0);
        let mut first = lineage.proofs().to_vec();
        join_pairs(&mut first, by_creator[..next].iter().cloned());
        read_batch(
            storage,
            services,
            &tick,
            (&first, retry),
            &mut readings,
            &mut out,
        )
        .await?;
    }
    let dead = lineage.dead(&readings);
    for c in &all {
        let covered = need.is_some_and(|need| out.offered_satoshis() >= need);
        if let Some((word, reason)) = c.record_hold() {
            out.held.push(HeldCoin {
                outpoint: format!("{}.{}", c.txid, c.vout),
                satoshis: c.satoshis,
                word: word.to_string(),
                origin: c.origin,
                age_minutes: c.age_minutes,
                reason: reason.to_string(),
                headers_fault: None,
                output_id: c.output_id,
            });
            continue;
        }
        if !readings.contains_key(&c.txid) {
            if covered {
                out.unread.push(c.output_id);
                continue;
            }
            let end = batch_end(next);
            let batch: Vec<_> = by_creator[next..end]
                .iter()
                .filter(|(t, _)| !readings.contains_key(t))
                .cloned()
                .collect();
            next = end;
            read_batch(
                storage,
                services,
                &tick,
                (&batch, retry),
                &mut readings,
                &mut out,
            )
            .await?;
        }
        let Some(reading) = readings.get(&c.txid) else {
            out.unread.push(c.output_id);
            continue;
        };
        match coin_hold(reading, &dead, c.origin, (&c.txid, c.vout as u32), opts) {
            None if !covered => out.offered.push((c.output_id, c.satoshis)),
            None => out.unread.push(c.output_id),
            Some((reason, headers_fault)) => out.held.push(HeldCoin {
                outpoint: format!("{}.{}", c.txid, c.vout),
                satoshis: c.satoshis,
                word: reading.word.to_string(),
                origin: c.origin,
                age_minutes: c.age_minutes,
                reason,
                headers_fault,
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
/// replaces), lost (`spendable = 0` with no spender), or a creator the gate
/// would not offer. Returns every named coin it holds. An outpoint that is
/// not one of this wallet's outputs is not a coin of ours: the caller's
/// error ([`GateError::Scope`], [`NOT_HELD`]), before a creator is read
/// (#166), unless `carried` (the transactions the caller's `inputBEEF`
/// carries whole, [`carried_txids`]) holds its transaction: the caller's own
/// input (a covenant, a puzzle), left to the toolbox. When `retry` (the
/// named door's read) and the header service did not answer for a reading
/// of the call (a creator's or a lineage candidate's), those readings are
/// read once more after [`HEADER_RETRY_PAUSE`] (#59), through
/// [`read_batch`] as the allocation's are, and the dead set is derived
/// after; without it the header service is read once.
pub async fn assess_named<V: WalletServices + ?Sized>(
    storage: &StorageSqlx,
    services: &V,
    opts: &GateOptions,
    named: &[(String, u32)],
    carried: &[String],
    reserved_to: Option<i64>,
    retry: bool,
) -> std::result::Result<Vec<HeldCoin>, GateError> {
    let frozen = crate::tracker_host::frozen_outpoints(storage)
        .await
        .map_err(GateError::Gate)?;
    let pool = storage.pool();
    let mut coins = Vec::new();
    for (txid, vout) in named {
        let txid = txid.to_ascii_lowercase();
        let row = sqlx::query(
            "SELECT o.output_id, o.satoshis, o.spent_by, o.spendable, t.is_outgoing, \
                    CAST((julianday('now') - julianday(o.created_at)) * 1440 AS INTEGER) AS age, \
                    COALESCE(s.txid, s.reference) AS holder, s.status AS spender_status, \
                    p.merkle_path \
             FROM outputs o \
             JOIN transactions t ON o.transaction_id = t.transaction_id \
             LEFT JOIN transactions s ON o.spent_by = s.transaction_id \
             LEFT JOIN proven_txs p ON p.txid = t.txid \
             WHERE lower(t.txid) = ? AND o.vout = ?",
        )
        .bind(&txid)
        .bind(i64::from(*vout))
        .fetch_optional(pool)
        .await
        .map_err(|e| GateError::Gate(e.into()))?;
        match row {
            Some(r) => coins.push((txid, *vout, r)),
            None if carried.contains(&txid) => {}
            None => {
                return Err(GateError::Scope(named_refusal(
                    "inputs",
                    &format!("{txid}.{vout}"),
                    "a coin this wallet could spend",
                    NOT_HELD,
                )))
            }
        }
    }
    let is_frozen = |txid: &str, vout: u32| frozen.contains(&(txid.to_string(), i64::from(vout)));
    let is_locked = |r: &sqlx::sqlite::SqliteRow| {
        let spent_by: Option<i64> = r.get("spent_by");
        spent_by.is_some() && spent_by != reserved_to
    };
    let is_lost = |r: &sqlx::sqlite::SqliteRow| {
        r.get::<i64, _>("spendable") == 0 && r.get::<Option<i64>, _>("spent_by").is_none()
    };
    // Only a coin neither frozen, locked nor lost needs its creator read.
    let creators: Vec<Pair> = coins
        .iter()
        .filter(|(t, v, r)| !is_frozen(t, *v) && !is_locked(r) && !is_lost(r))
        .map(|(t, _, r)| (t.clone(), r.get("merkle_path")))
        .collect();
    let any_made = coins.iter().any(|(t, _, r)| {
        r.get::<i64, _>("is_outgoing") != 0 && creators.iter().any(|(c, _)| c == t)
    });
    let lineage = lineage_of(storage, any_made)
        .await
        .map_err(GateError::Gate)?;
    // One reading of the creators and the lineage's candidates holding a
    // proof, against one read of the headers and retried together: one
    // word per creator.
    let mut reads = creators;
    join_pairs(&mut reads, lineage.proofs().iter().cloned());
    let mut readings = HashMap::new();
    if !reads.is_empty() {
        read_batch(
            storage,
            services,
            &opts.tick(),
            (&reads, retry),
            &mut readings,
            &mut Assessment::default(),
        )
        .await
        .map_err(GateError::Gate)?;
    }
    let dead = lineage.dead(&readings);
    let mut held = Vec::new();
    for (txid, vout, r) in coins {
        let origin = if r.get::<i64, _>("is_outgoing") != 0 {
            Origin::Made
        } else {
            Origin::Received
        };
        let reading = readings.get(&txid);
        let mut headers_fault = None;
        let (word, reason) = if is_frozen(&txid, vout) {
            ("frozen".to_string(), Some(FROZEN.to_string()))
        } else if is_locked(&r) {
            (
                "locked".to_string(),
                Some(spent_by_reason(
                    &r.get::<Option<String>, _>("holder").unwrap_or_default(),
                    r.get::<Option<String>, _>("spender_status").as_deref(),
                )),
            )
        } else if is_lost(&r) {
            ("lost".to_string(), Some(LOST.to_string()))
        } else {
            match reading {
                Some(reading) => {
                    let judged = coin_hold(reading, &dead, origin, (&txid, vout), opts);
                    let reason = judged.map(|(reason, fault)| {
                        headers_fault = fault;
                        reason
                    });
                    (reading.word.to_string(), reason)
                }
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
                headers_fault,
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
    // A scoped word names coins this wallet could spend under it, or it is
    // the caller's error (#63), before anything is read for the action.
    if let Some(error) = check_scope(storage, opts.allow_pending.named())
        .await
        .map_err(GateError::Gate)?
    {
        return Err(GateError::Scope(error));
    }
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
        let carried = carried_txids(args.input_beef.as_deref());
        // A reading the header service did not answer for, a named coin's
        // creator or a lineage candidate, is read once more (#59), as the
        // allocation's are.
        let held = assess_named(
            storage,
            wallet.services(),
            opts,
            &named,
            &carried,
            None,
            true,
        )
        .await?;
        if !held.is_empty() {
            // Every named input must be offerable: one held by a rule is
            // the refusal; held on the service's fault alone, it is "could
            // not verify now".
            let headers_fault = if held.iter().all(|c| c.headers_fault.is_some()) {
                held.iter().find_map(service_error)
            } else {
                None
            };
            return Err(GateError::Refused(Refusal {
                toolbox: NAMED_INPUT_REFUSED.to_string(),
                offered_satoshis: 0,
                held,
                capped: None,
                headers_fault,
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
    if opts.allow_pending.is_on() {
        args.labels
            .get_or_insert_with(Vec::new)
            .push(RECEIVED_PENDING_LABEL.to_string());
    }
    run_gated(wallet, args, originator, opts).await
}

/// The transactions a caller's `inputBEEF` carries whole (not a txid-only
/// entry): a named input over one of them that is not a coin of this wallet
/// is the caller's own, left to the toolbox, which checks the BEEF. A BEEF
/// that does not parse carries none.
fn carried_txids(input_beef: Option<&[u8]>) -> Vec<String> {
    input_beef
        .and_then(|b| bsv_sdk::transaction::Beef::from_binary(b).ok())
        .map(|beef| {
            beef.txs
                .iter()
                .filter(|t| t.tx().is_some())
                .map(bsv_sdk::transaction::BeefTx::txid)
                .collect()
        })
        .unwrap_or_default()
}

/// The header service's error a coin is held on.
fn service_error(coin: &HeldCoin) -> Option<String> {
    coin.headers_fault.clone()
}

/// The header service's error when it is the reading's fault: the words
/// [`hold_reason`] gives after `headers unavailable: `.
fn headers_error(reading: &CreatorReading) -> Option<String> {
    reading.fault.clone().filter(|_| reading.headers_fault)
}

/// Why a named outpoint is refused when it is not an output of this wallet:
/// the one word for "not held", the scoped word's ([`check_scope`]) and a
/// named input's ([`assess_named`]) alike (#166).
pub const NOT_HELD: &str = "not a coin of this wallet";

/// The caller's error for a coin it names (the 400 of [`GateError::Scope`]):
/// `word` names `outpoint`, which is not `what`, because `why`.
fn named_refusal(word: &str, outpoint: &str, what: &str, why: &str) -> String {
    format!("Invalid argument: {word} names {outpoint}, which is not {what}: {why}")
}

/// The coins a scoped word names (#63), checked before the door: each must
/// be a received coin this wallet could spend under the word (an output of
/// ours, change of the default basket, not consumed, not frozen or lost,
/// its transaction `completed` or `unproven`, not an action of ours). The
/// first that is not is the caller's error, naming it and why: the word
/// never reaches own change, a nosend, a coin another action consumed, a
/// coin its own record holds (frozen, or lost: the store's `spendable = 0`
/// with no spender, #165), or a coin that is not ours. A named coin already
/// mined is offered as any mined coin is: the word is moot for it, not
/// wrong.
async fn check_scope(storage: &StorageSqlx, named: &[(String, u32)]) -> Result<Option<String>> {
    if named.is_empty() {
        return Ok(None);
    }
    let pool = storage.pool();
    let frozen = crate::tracker_host::frozen_outpoints(storage).await?;
    for (txid, vout) in named {
        let row = sqlx::query(
            "SELECT o.change, o.spent_by, o.spendable, t.is_outgoing, t.status, \
                    b.name AS basket, \
                    COALESCE(s.txid, s.reference) AS holder \
             FROM outputs o \
             JOIN transactions t ON o.transaction_id = t.transaction_id \
             LEFT JOIN output_baskets b ON o.basket_id = b.basket_id \
             LEFT JOIN transactions s ON o.spent_by = s.transaction_id \
             WHERE lower(t.txid) = ? AND o.vout = ?",
        )
        .bind(txid)
        .bind(i64::from(*vout))
        .fetch_optional(pool)
        .await?;
        let why = match row {
            None => NOT_HELD.to_string(),
            Some(r) => {
                let status: String = r.get("status");
                if r.get::<i64, _>("is_outgoing") != 0 {
                    "own change of an action of this wallet, not a received coin".to_string()
                } else if r.get::<Option<i64>, _>("spent_by").is_some() {
                    format!(
                        "spent by {}",
                        r.get::<Option<String>, _>("holder")
                            .unwrap_or_else(|| "an action of this wallet".to_string())
                    )
                } else if frozen.contains(&(txid.to_ascii_lowercase(), i64::from(*vout))) {
                    FROZEN.to_string()
                } else if r.get::<i64, _>("spendable") == 0 {
                    LOST.to_string()
                } else if !matches!(status.as_str(), "completed" | "unproven") {
                    format!("its transaction is {status}")
                } else if r.get::<Option<String>, _>("basket").as_deref() != Some("default")
                    || r.get::<i64, _>("change") != 1
                {
                    "not a change coin of the default basket".to_string()
                } else {
                    continue;
                }
            }
        };
        return Ok(Some(named_refusal(
            "allowPending: received",
            &format!("{txid}:{vout}"),
            "a received coin this wallet could spend under it",
            &why,
        )));
    }
    Ok(None)
}

async fn run_gated<V: WalletServices + Send + Sync + 'static>(
    wallet: &Wallet<StorageSqlx, V>,
    args: CreateActionArgs,
    originator: &str,
    opts: &GateOptions,
) -> std::result::Result<CreateActionResult, GateError> {
    let storage = wallet.storage();
    let outputs = args
        .outputs
        .as_ref()
        .map(|o| o.iter().map(|x| x.satoshis).sum::<u64>())
        .unwrap_or(0);
    let need = outputs.saturating_add(FEE_MARGIN);
    let first = assess_inner(storage, wallet.services(), opts, Some(need), true)
        .await
        .map_err(GateError::Gate)?;
    let lazy = !first.unread.is_empty();
    match run_offered(wallet, args.clone(), originator, &first).await {
        Ok(result) => return Ok(result),
        // An unspendable coin in the lazy offer: the coins not read may
        // cover without it (F7 of the #44 CLI review).
        Err(e) if lazy && toolbox_words::is_unspendable(&e) => {}
        Err(e) if !toolbox_words::is_insufficient(&e) => return Err(GateError::Wallet(e)),
        Err(e) if !lazy => return Err(refusal(storage, &first, &e, outputs).await),
        Err(_) => {}
    }
    // The lazy read left coins unread: read every coin and try once more
    // (the header service read once more only if the first read did not).
    let full = assess_inner(
        storage,
        wallet.services(),
        opts,
        None,
        !first.headers_retried,
    )
    .await
    .map_err(GateError::Gate)?;
    match run_offered(wallet, args, originator, &full).await {
        Ok(result) => Ok(result),
        Err(e) if toolbox_words::is_insufficient(&e) => {
            Err(refusal(storage, &full, &e, outputs).await)
        }
        Err(e) => Err(GateError::Wallet(e)),
    }
}

/// The funds' refusal, or "could not verify now" (#59) when the coins held
/// on the header service's fault, with what was offered, reach `outputs`
/// (the outputs' sum, the floor of what the action needs: a shortfall they
/// could not have covered is the funds' refusal whatever the service said).
async fn refusal(
    storage: &StorageSqlx,
    assessment: &Assessment,
    e: &bsv_sdk::Error,
    outputs: u64,
) -> GateError {
    let mut held = assessment.held.clone();
    match locked(storage.pool()).await {
        Ok(mut l) => held.append(&mut l),
        Err(fault) => return GateError::Gate(fault),
    }
    let on_fault: u64 = held
        .iter()
        .filter(|c| c.headers_fault.is_some())
        .map(|c| c.satoshis)
        .sum();
    let headers_fault =
        if on_fault > 0 && assessment.offered_satoshis().saturating_add(on_fault) >= outputs {
            held.iter().find_map(service_error)
        } else {
            None
        };
    // The toolbox's own line: a `WalletError`'s text without bsv-rs's
    // `wallet error: ` Display, any other error as it displays.
    let toolbox = match e {
        bsv_sdk::Error::WalletError(text) => text.clone(),
        other => other.to_string(),
    };
    GateError::Refused(Refusal {
        toolbox,
        offered_satoshis: assessment.offered_satoshis(),
        held,
        capped: assessment.capped,
        headers_fault,
    })
}

#[cfg(test)]
mod tests;
