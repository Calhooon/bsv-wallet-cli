//! The lineage rule at the door (bsv-wallet-cli #163; the owner's ruling of
//! 2026-10-22 15:4xZ, bsv-stack-lean `docs/charters/wallet.md` "The lineage
//! rule" and `docs/DECISIONS-2026-10-19.md` section 11 (a);
//! `lean/Wallet.lean` `State.skel`, `lineageDead`, `deadLineage`,
//! `offerableUnder`): an action of ours is dead for the door when an input
//! of it was created by an action whose word is dead, or by one whose own
//! lineage is dead. A coin of a dead lineage is never offered, by any rule
//! or word and whatever its creator's own word (mined included), and a
//! createAction naming one is refused (`dead_lineage_never_offered`,
//! `dead_lineage_create_refused`). The gate reads it after the creator's
//! door, in the Lean's clause order (`WalletGate.clause`): a coin the door
//! already holds keeps the door's reason.
//!
//! **The store's edges.** The toolbox's `outputs.spent_by` names the
//! transaction of ours that consumed an output, so the inputs of ours of an
//! action are the outputs whose `spent_by` is its row, and each such input's
//! own transaction is an ancestor of ours (the Lean's `skel`). A received
//! creator spends no output of ours, so the walk stops there (the Lean's
//! `sk i.txid = none`); a received coin's lineage is never read at all (the
//! toolbox checked its BEEF at internalize).
//!
//! **The dead words.** The Lean's three (`dead`: `abandoned`, `rejected`,
//! `conflicted`) are read where this host keeps them: the tracker's stored
//! word (`tracker_states.word`) `conflicted` or `rejected`, which the
//! tracker writes only on evidence (a mined competitor's checked proof, the
//! evaluator's verdict); and the toolbox's `failed` status for the abandon,
//! since the CLI's tracker row stores no abandon (an action the host gives
//! up is the toolbox's `failed`; `tests/gate_vectors.rs` `ABANDONED`). A
//! stored `abandoned` word is read as dead too, though no store writes one
//! today. A `failed` transaction with a proof in `proven_txs` is not dead:
//! the tracker reads that proof as `mined`, and the toolbox's own BEEF walk
//! lets the proof outrank the status (`get_tx_with_proof`). The stored word
//! is read as stored, never replayed against the headers: no chain event
//! returns a dead word to live (`Tracker.dead_onChain`), and a reorg moves
//! the lineage only with its creators' words (`reorg_lifts_dead_lineage`).
//!
//! **The walk.** One query for the whole wallet, forward from the dead
//! actions to their descendants: the actions spending an output of a dead
//! action, then those spending an output of one of them, and so on. That set
//! is exactly the actions whose `lineageDead` holds, read from the dead end,
//! so a wallet with no dead action pays one indexed read and no walk. Each
//! (descendant, dead ancestor) pair is visited once (SQL's `UNION`), so a
//! cycle planted in `spent_by` ends the walk instead of hanging the door,
//! and adds nothing a dead word does not reach. The rule is a view: nothing
//! is written, no lock moves (`dead_lineage_releases_only_on_evidence`).

use std::collections::HashMap;

use anyhow::Result;
use bsv_wallet_toolbox::{StorageSqlx, WalletServices};

use crate::tracker_host::{read_creators, TickOptions};

/// The reason a dead lineage's hold opens with.
pub const DEAD_LINEAGE: &str = "dead lineage";

/// The dead action a creator's lineage reaches: an ancestor of ours whose
/// word is dead.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeadAncestor {
    /// The ancestor's txid (its reference when it was never signed).
    pub txid: String,
    /// Its dead word as the store holds it: `conflicted`, `rejected`,
    /// `abandoned` (the tracker's), or `failed` (the toolbox's abandon).
    pub word: String,
}

impl DeadAncestor {
    /// The hold's reason: `dead lineage: ancestor <txid> <word>`.
    #[must_use]
    pub fn reason(&self) -> String {
        format!("{DEAD_LINEAGE}: ancestor {} {}", self.txid, self.word)
    }
}

/// Every action whose lineage is dead, by lowercase txid, with the dead
/// ancestor its reason names: of several, the latest created (the nearest
/// in the creation order), a tracker word before a `failed` status. Read
/// in one pass ([`crate::store::dead_lineage_rows`]); a candidate that
/// holds a proof is read as the door reads it ([`read_creators`], the same
/// (txid, stored path) pair and `opts`) and counts only when that reading's
/// word is not `mined`. The header service is read only when such a
/// candidate exists; its fault there leaves the candidate dead as stored,
/// never a fault of the gate's. Nothing is written.
///
/// # Errors
///
/// The store's error when a read fails.
pub async fn dead_lineages<V: WalletServices + ?Sized>(
    storage: &StorageSqlx,
    services: &V,
    opts: &TickOptions,
) -> Result<HashMap<String, DeadAncestor>> {
    let rows = crate::store::dead_lineage_rows(storage.pool()).await?;
    let mut proofs: Vec<(String, Option<Vec<u8>>)> = Vec::new();
    for proof in rows.iter().filter_map(|r| r.proof.as_ref()) {
        if !proofs.iter().any(|(t, _)| t == &proof.0) {
            proofs.push(proof.clone());
        }
    }
    let readings = if proofs.is_empty() {
        HashMap::new()
    } else {
        read_creators(storage, services, opts, &proofs).await?
    };
    let mined = |txid: &str| readings.get(txid).is_some_and(|r| r.word == "mined");
    let mut out = HashMap::new();
    for row in rows {
        if row.proof.as_ref().is_some_and(|(txid, _)| mined(txid)) {
            continue;
        }
        out.entry(row.heir.to_ascii_lowercase())
            .or_insert(DeadAncestor {
                txid: row.ancestor,
                word: row.word,
            });
    }
    Ok(out)
}
