//! The lineage rule at the door (bsv-wallet-cli #163; the owner's ruling of
//! 2026-10-22 15:4xZ, bsv-stack-lean `docs/charters/wallet.md` "The lineage
//! rule" and `docs/DECISIONS-2026-10-19.md` section 11 (a);
//! `lean/Wallet.lean` `State.skel`, `lineageDead`, `deadLineage`,
//! `offerableUnder`): an action of ours is dead for the door when an input
//! of it was created by an action of this wallet, made or received, whose
//! word is dead, or by one whose own lineage is dead. A coin of a dead lineage is never offered, by any rule
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
//! tracker writes only on evidence (a node's verdict, stored beside the
//! word so a replay reads it back; this host feeds no competitor proof);
//! and the toolbox's `failed` status for the abandon,
//! since the CLI's tracker row stores no abandon (an action the host gives
//! up is the toolbox's `failed`; `tests/gate_vectors.rs` `ABANDONED`). A
//! stored `abandoned` word is read too, though no store writes one today.
//! Each is a candidate, whether the action was made or received (the
//! Lean's `w.actions` holds received actions, and `skel` reads a parent's
//! word whatever its origin).
//!
//! **One word per creator.** The Lean keeps one word per action, and the
//! lineage rule reads the word the door reads (`lean/Wallet.lean`
//! `State.skel` reads `dead a.tracker.word`, as `doorCloses` and
//! `offerableUnder` do); a proof whose root checks outranks every other
//! word (`lean/Tracker.lean` `onProof_of_rootChecks`, the charter's row).
//! So a candidate that holds a proof (its tracker row's bump, or the
//! toolbox's `proven_txs` row) is read by the door itself, in the gate's
//! own readings: [`lineage_candidates`] reads the store alone and hands the
//! gate the (txid, stored path) pairs of candidates whose heirs include an
//! eligible creator for this call ([`Lineage::proofs`], #217). Allocation
//! uses the whole eligible creator set, including later lazy batches, so
//! their ancestors keep the same initial reading and retry; the named
//! door uses its eligible named creators. A candidate with no eligible
//! heir adds no proof read or retry pause. The gate reads the selected
//! pairs in one [`crate::tracker_host::read_creators`] call with the
//! initial creators (one read of the headers, so a candidate that is also
//! a judged coin's creator is read once). The allocation's door and `createAction`'s
//! named inputs re-read once what faulted in that call (#59), candidates
//! and creators together (`read_batch`); `respend`'s exact inputs read once
//! (`retry = false`). [`Lineage::dead`] derives the dead set after each
//! door's reading. A proof that checks makes it `mined` for the walk as for
//! the door, and it is not dead; a proof the headers could not check (the
//! header service silent) or whose root does not match leaves the stored
//! word, dead for both, the door holding that creator closed. The lineage
//! adds a header read only when a selected candidate holds a proof and no
//! judged creator brings the gate to the headers already, and its fault there is
//! the stored word, never the gate's fault. A candidate with no proof is
//! dead as stored: a replay reads nothing else from a stored verdict, and
//! a stored `abandoned` stays dead as stored. No chain event returns a dead
//! word to live (`Tracker.dead_onChain`; a proof is not a chain event), and
//! a reorg moves the lineage only with its creators' words
//! (`reorg_lifts_dead_lineage`).
//!
//! **The walk.** One query for the whole wallet, forward from the
//! candidates to their descendants: the actions spending an output of a
//! candidate, then those spending an output of one of them, and so on. A
//! wallet with no candidate pays one indexed read, no walk and no header
//! read. The query returns each (descendant, candidate) pair once (SQL's
//! `UNION`), not the nearest candidate per descendant, since which
//! candidates stay dead is known only once their proofs are read: the pairs
//! of a candidate the reading clears are dropped, and each descendant keeps
//! the earliest pair left in the order [`Lineage::dead`] names. What
//! remains is exactly the actions whose `lineageDead` holds: a descendant
//! reached only through a cleared candidate is reached by no dead one. The
//! rows are bounded by the (descendant, candidate) pairs, so a cycle
//! planted in `spent_by` ends the walk instead of hanging the door, and
//! adds nothing a dead word does not reach. The rule is a view: nothing is
//! written, no lock moves (`dead_lineage_releases_only_on_evidence`).

use std::collections::{HashMap, HashSet};

use anyhow::Result;
use bsv_wallet_toolbox::StorageSqlx;

use crate::store::LineageRow;
use crate::tracker_host::CreatorReading;

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

/// A (txid, stored path) pair as [`crate::tracker_host::read_creators`]
/// takes it.
pub type Pair = (String, Option<Vec<u8>>);

/// The store's half of the lineage rule: every (heir, dead candidate) pair
/// of the walk ([`crate::store::dead_lineage_rows`]). [`Self::proofs`] selects
/// the candidates with an eligible heir that hold a proof, each once.
/// No header is read here: the gate reads those candidates with its own
/// creators, in one reading per call, and [`Lineage::dead`] derives the dead set from it.
#[derive(Debug, Default)]
pub struct Lineage {
    rows: Vec<LineageRow>,
    proofs: Vec<Pair>,
}

/// Read the lineage's candidates of the user `user_id` in one pass on the
/// reader: no service, no header. Nothing is written.
///
/// # Errors
///
/// The store's error when the read fails.
pub async fn lineage_candidates(storage: &StorageSqlx, user_id: i64) -> Result<Lineage> {
    let rows = crate::store::dead_lineage_rows(storage.pool(), user_id).await?;
    let mut proofs = Vec::new();
    join_pairs(&mut proofs, rows.iter().filter_map(|r| r.proof.clone()));
    Ok(Lineage { rows, proofs })
}

impl Lineage {
    /// The distinct proof pairs of candidates with a made heir in
    /// `creators`. The gate reads these beside the creators it may judge
    /// in this call. A received heir selects none: no rule reads a received
    /// coin's lineage (`coin_hold`), so its candidates would be read for
    /// nothing (#221). Keep their original order even when an earlier heir
    /// is not judged.
    #[must_use]
    pub fn proofs(&self, creators: &[Pair]) -> Vec<Pair> {
        if self.proofs.is_empty() {
            return Vec::new();
        }
        let judged: HashSet<_> = creators.iter().map(|(txid, _)| txid.as_str()).collect();
        let selected: HashSet<_> = self
            .rows
            .iter()
            .filter(|r| r.made && judged.contains(r.heir.to_ascii_lowercase().as_str()))
            .filter_map(|r| r.proof.as_ref().map(|(txid, _)| txid.as_str()))
            .collect();
        self.proofs
            .iter()
            .filter(|(txid, _)| selected.contains(txid.as_str()))
            .cloned()
            .collect()
    }

    /// Every action whose lineage is dead, by lowercase txid, with the dead
    /// ancestor its reason names: of several, the latest created (the
    /// nearest in the creation order), a tracker word before a `failed`
    /// status. A candidate that holds a proof is cleared exactly when the
    /// gate's reading of it (`readings`, the door's own) says `mined`; one
    /// with no proof, or one `readings` does not hold, is dead as stored.
    #[must_use]
    pub fn dead(
        &self,
        readings: &HashMap<String, CreatorReading>,
    ) -> HashMap<String, DeadAncestor> {
        let mined = |txid: &str| readings.get(txid).is_some_and(|r| r.word == "mined");
        let mut out = HashMap::new();
        for row in &self.rows {
            if row.proof.as_ref().is_some_and(|(txid, _)| mined(txid)) {
                continue;
            }
            out.entry(row.heir.to_ascii_lowercase())
                .or_insert_with(|| DeadAncestor {
                    txid: row.ancestor.clone(),
                    word: row.word.clone(),
                });
        }
        out
    }
}

/// Add each of `more` to `pairs` unless a pair of its txid is there: one
/// reading per creator.
pub fn join_pairs(pairs: &mut Vec<Pair>, more: impl IntoIterator<Item = Pair>) {
    for pair in more {
        if !pairs.iter().any(|(t, _)| t == &pair.0) {
            pairs.push(pair);
        }
    }
}
