//! The stack's wallet-gate vectors replayed against the spend gate (#130,
//! bsv-stack-lean #184): every row of `tests/vectors/wallet_gate_vectors.json`
//! is the Lean evaluator's answer (`lean/Wallet.lean` `offerableUnder` and
//! the definitions it calls) for one coin at one step of a witness trace,
//! and this test asks the CLI's gate the same question about the same state.
//!
//! The copy is pinned by `tests/vectors/wallet_gate_vectors.pin` (the stack's
//! commit and the file's sha256), checked before any row is read. For each
//! row a fresh in-memory store is built in the store's own records: the
//! creator's action (`is_outgoing` for `made`), the coin (its satoshis, its
//! lock as `spent_by` a live or a completed action of ours, `lost` as the
//! relinquished record `spendable = 0`), the creator's evidence as the host
//! keeps it (the toolbox's `proven_txs` path for `mined`; the tracker's row
//! for `built`, the hints `heard` stores for `announced` and `seen`, a stored
//! reorg for `stale`, a stored fork for the suspect mark, a stored verdict
//! for `rejected` and `conflicted`), for a row whose creator's lineage is
//! dead a conflicted parent of ours whose change the creator spent
//! (`gate_vectors_store::dead_parent`), and the mock header service at the row's tip with the
//! row's root at the creator's proof height. The gate is then asked
//! through its public doors, nothing restated here:
//!
//! - `read_creators`: the creator's reading. Its word must be the row's: a
//!   difference is this test's construction failing, never a finding. Its
//!   suspect mark is read after the guard (which never clears it, #164) and
//!   is compared.
//! - `lineage_candidates` and `Lineage::dead`: the creator's lineage, against
//!   `dead_lineage` (#163), a candidate's proof read as the gate reads it.
//! - `hold_reason` on that reading: the creator's door (`door_spendable`;
//!   `guard_depth` where the reason names the depth).
//! - `assess_named` (a named input) and `assess` (the allocation's offer):
//!   `offerable`, and `offerable_clause` from the hold's reason. A row with
//!   `allow_received` true is asked under the plain word and under the
//!   scoped word naming its coin (#63); both must answer the row.
//!
//! A difference between the gate and the row is a finding: it is listed,
//! row and field, in `tests/vectors/wallet_gate_divergences.json` with the
//! finding it belongs to, and the list is exact (a new difference fails, a
//! listed one that no longer differs fails). Rows the CLI cannot hold are
//! in [`UNEXPRESSED`], exact too. docs/GATE-VECTORS.md is the page.

mod gate_vectors_store;

use std::collections::{BTreeMap, BTreeSet};

use bsv_sdk::primitives::hash::sha256;
use bsv_wallet_cli::spend_gate::{
    assess, assess_named, hold_reason, lineage_candidates, AllowPending, GateError, GateOptions,
    Origin, UserCoins,
};
use bsv_wallet_cli::tracker_host::{read_creators, TickOptions};
use gate_vectors_store::{b, build, height, key_of, n, s, Built};
use serde_json::Value;

const VECTORS: &[u8] = include_bytes!("vectors/wallet_gate_vectors.json");
const PIN: &str = include_str!("vectors/wallet_gate_vectors.pin");
const DIVERGENCES: &str = include_str!("vectors/wallet_gate_divergences.json");
const ROWS: usize = 490;

/// The rows the CLI cannot hold, by key, with why. Exact: a row that leaves
/// this class, or a listed key no row has, fails the test.
const UNEXPRESSED: &[(&str, &str)] = &[
    (
        "WalletGate.contendedSteps | step 5 | T1.0 | allow_received false",
        ABANDONED,
    ),
    (
        "WalletGate.contendedSteps | step 5 | T1.0 | allow_received true",
        ABANDONED,
    ),
    (
        "Wallet.Check.depth_floor | step 1 | F.0 | allow_received false",
        ABOVE_TIP,
    ),
    (
        "Wallet.Check.depth_floor | step 1 | F.0 | allow_received true",
        ABOVE_TIP,
    ),
];

const ABANDONED: &str = "the creator's word is abandoned: the CLI's tracker row stores no \
     abandon (HostAction::Abandon is never kept or replayed by tracker_host), so no store \
     reads `abandoned`; an action the host gives up is the toolbox's `failed` status, which \
     the allocation never reads as a candidate";

const ABOVE_TIP: &str = "the creator is mined at a height above the headers' tip: the CLI \
     checks the stored proof against the headers on every read and the tracker refuses a proof \
     above the tip, so no store reads `mined` there (the reading is `unknown`)";

/// The answer columns the CLI's gate gives no value for, with why. The rows'
/// inputs they read still drive the replay.
const COLUMNS_NOT_ANSWERED: &[(&str, &str)] = &[
    ("rule_for, rule_for_allows", "the gate names no rule per creator: the host's rule is SPEND_OWN_CHANGE (the made arm) and the action's word (allowPending), read inside hold_reason"),
    ("received_pending, door_word", "the gate's door reads the tracker's word itself; it computes no door word and no received-pending verdict apart from the hold"),
    ("coin_word, coin_word_data", "the gate answers offered or held with a reason; HeldCoin.word is the creator's tracker word, a named input's failed transaction status, or locked, frozen, lost, not the coin's word"),
    ("host_answer, host_answer_data", "a row names its door (createAction, internalizeAction, ...) but carries none of its arguments (no BEEF, no outputs), so the host's answer at that door cannot be replayed from the row"),
];

/// Why the CLI cannot hold this row, or `None`.
fn unexpressed(row: &Value) -> Option<&'static str> {
    if s(row, "creator_word") == Some("abandoned") {
        return Some(ABANDONED);
    }
    if s(row, "creator_word") == Some("mined")
        && n(row, "creator_proof_height") > n(row, "headers_tip")
    {
        return Some(ABOVE_TIP);
    }
    None
}

/// The Lean clause a hold's reason names.
fn clause_of(reason: Option<&str>) -> String {
    let Some(reason) = reason else {
        return "offered".to_string();
    };
    let clause = if reason.starts_with("frozen") {
        "frozen"
    } else if reason.starts_with("spent by")
        || reason.starts_with("lost:")
        || (reason.starts_with("its transaction is ")
            && reason.ends_with(": not spendable until it is sent"))
    {
        "held by a lock"
    } else if reason.starts_with("suspect:") {
        "creator suspect (fork in progress)"
    } else if reason.starts_with("dead lineage:") {
        "dead lineage"
    } else if reason.ends_with(": never offered") {
        // Both `<word>` and the named door's `its transaction is failed`.
        "door: a chain word against the creator"
    } else if reason.contains("; received, no proof;")
        || reason.contains("; received, confirming again;")
        || reason.ends_with("; own change, SPEND_OWN_CHANGE=0")
    {
        "door: no rule for the creator's origin"
    } else if reason.starts_with("depth ") {
        "door: mined, shallower than the guard's depth"
    } else {
        return format!("(unmapped) {reason}");
    };
    clause.to_string()
}

/// A free received coin whose creator is unknown in the pinned vectors.
async fn free_received_coin() -> (Built, u32) {
    let rows: Value = serde_json::from_slice(VECTORS).unwrap();
    let row = rows["rows"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| {
            b(row, "coin_held") == Some(true)
                && s(row, "coin_lock") == Some("free")
                && b(row, "coin_frozen") == Some(false)
                && s(row, "creator_origin") == Some("received")
                && s(row, "creator_word") == Some("unknown")
        })
        .expect("a free received coin with an unknown creator");
    (build(row).await, height(row, "spend_depth").unwrap())
}

/// An unspendable not-yet-sent record maps to the lock clause; a failed
/// transaction maps to the creator's chain word. Removing the status/flag
/// hold offers these received coins under the word, so both mappings bite.
#[tokio::test]
async fn a_named_inputs_status_hold_maps_to_its_record_or_creator_clause() {
    use bsv_wallet_toolbox::WalletStorageWriter;
    let (built, spend_depth) = free_received_coin().await;
    let gate = GateOptions {
        spend_depth,
        own_change: true,
        allow_pending: AllowPending::Received,
        tracker: TickOptions::default(),
    };
    for status in ["nosend", "unsigned", "sending", "failed"] {
        built
            .storage
            .mark_internalized_tx_failed(&built.txid)
            .await
            .unwrap();
        sqlx::query("UPDATE transactions SET status = ? WHERE txid = ?")
            .bind(status)
            .bind(&built.txid)
            .execute(built.storage.writer())
            .await
            .unwrap();
        let held = assess_named(
            UserCoins::new(&built.storage, built.user),
            &built.services,
            &gate,
            &[(built.txid.clone(), built.vout)],
            &[],
            None,
            false,
        )
        .await
        .unwrap();
        assert_eq!(
            held.len(),
            1,
            "{status} was offered under allowPending: received"
        );
        assert_eq!(held[0].word, status);
        let (ending, clause) = if status == "failed" {
            ("never offered", "door: a chain word against the creator")
        } else {
            ("not spendable until it is sent", "held by a lock")
        };
        assert_eq!(
            held[0].reason,
            format!("its transaction is {status}: {ending}")
        );
        assert_eq!(clause_of(Some(&held[0].reason)), clause);
    }
}

/// The guard's depth a reason names (`depth N: ...`).
fn depth_of(reason: Option<&str>) -> Option<u64> {
    reason?
        .strip_prefix("depth ")?
        .split(':')
        .next()?
        .parse()
        .ok()
}

/// One difference between the gate and a row.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct Divergence {
    key: String,
    door: String,
    field: String,
    lean: String,
    cli: String,
}

/// The differences of one row.
struct Checks<'a> {
    row: &'a Value,
    key: String,
    out: &'a mut Vec<Divergence>,
}

impl Checks<'_> {
    /// The row's `field` against the gate's answer through `door`.
    fn field(&mut self, door: &str, field: &str, cli: &dyn std::fmt::Display) {
        let lean = match &self.row[field] {
            Value::String(text) => text.clone(),
            other => other.to_string(),
        };
        let cli = cli.to_string();
        if lean != cli {
            self.out.push(Divergence {
                key: self.key.clone(),
                door: door.to_string(),
                field: field.to_string(),
                lean,
                cli,
            });
        }
    }
}

/// The host's settings of the row, with the action's word: off, or for a
/// row with the word the plain word and the scoped word naming its coin.
fn words(row: &Value, built: &Built) -> Vec<(&'static str, GateOptions)> {
    let base = GateOptions {
        spend_depth: height(row, "spend_depth").unwrap(),
        own_change: s(row, "host_rule_made").is_some(),
        allow_pending: AllowPending::Off,
        tracker: TickOptions::default(),
    };
    if b(row, "allow_received") != Some(true) {
        return vec![("", base)];
    }
    let scope = AllowPending::ReceivedOnly(vec![(built.txid.clone(), built.vout)]);
    vec![
        ("", base.clone().with_allow_pending(AllowPending::Received)),
        (" (scoped)", base.with_allow_pending(scope)),
    ]
}

/// The creator's reading and its door.
async fn creator_checks(checks: &mut Checks<'_>, built: &Built, origin: Origin) {
    let words = words(checks.row, built);
    let tick = TickOptions {
        spend_depth: words[0].1.spend_depth,
        ..TickOptions::default()
    }
    .floored();
    let creators = [(built.txid.clone(), built.proof.clone())];
    let readings = read_creators(&built.storage, &built.services, &tick, &creators)
        .await
        .unwrap();
    let reading = &readings[&built.txid];
    assert_eq!(
        reading.word,
        s(checks.row, "creator_word").unwrap(),
        "{}: the store built for the row reads another creator word (the replay's \
         construction, not the gate)",
        checks.key
    );
    // The mark after the guard, which reads it and never clears it
    // (#164): the gate's answer, compared.
    checks.field("read_creators", "creator_suspect", &reading.suspect);
    // The door with the mark set aside, the guard's door and not the guard
    // recomputed: Wallet.lean `offerableUnder` checks the mark apart from
    // `doorSpendable`, and the gate's hold word says the mark first.
    let mut unmarked = reading.clone();
    unmarked.suspect = false;
    for (tag, opts) in &words {
        let door = hold_reason(&unmarked, origin, (&built.txid, built.vout), opts);
        let name = format!("hold_reason{tag}");
        checks.field(&name, "door_spendable", &door.is_none());
        if let Some(depth) = depth_of(door.as_deref()) {
            checks.field(&name, "guard_depth", &depth);
        }
    }
}

/// The coin through the two doors: a named input and the allocation.
async fn coin_checks(checks: &mut Checks<'_>, built: &Built) {
    let coin = [(built.txid.clone(), built.vout)];
    for (tag, opts) in &words(checks.row, built) {
        // A coin the wallet does not hold is refused at the gate, the
        // caller's error (#166): the Lean's 'no coin', not offerable.
        let held = match assess_named(
            UserCoins::new(&built.storage, built.user),
            &built.services,
            opts,
            &coin,
            &[],
            None,
            false,
        )
        .await
        {
            Ok(held) => Some(held),
            Err(GateError::Scope(_)) if built.output_id.is_none() => None,
            Err(e) => panic!("{}: assess_named: {e}", checks.key),
        };
        let door = format!("assess_named{tag}");
        checks.field(
            &door,
            "offerable",
            &held.as_ref().is_some_and(Vec::is_empty),
        );
        let held = held.unwrap_or_default();
        if built.output_id.is_some() {
            let reason = held.first().map(|c| c.reason.as_str());
            checks.field(&door, "offerable_clause", &clause_of(reason));
        }

        let a = assess(
            UserCoins::new(&built.storage, built.user),
            &built.services,
            opts,
            None,
        )
        .await
        .unwrap();
        let door = format!("assess{tag}");
        let offered = built
            .output_id
            .is_some_and(|id| a.offered.iter().any(|(o, _)| *o == id));
        checks.field(&door, "offerable", &offered);
        let held = built
            .output_id
            .and_then(|id| a.held.iter().find(|c| c.output_id == id));
        // A coin neither offered nor held is no candidate (locked, lost or
        // absent): its clause is the store's, not a reason.
        if offered || held.is_some() {
            let clause = clause_of(held.map(|c| c.reason.as_str()));
            checks.field(&door, "offerable_clause", &clause);
        }
    }
}

/// Ask the gate about one row; push every difference.
async fn replay(row: &Value, out: &mut Vec<Divergence>) {
    let built = build(row).await;
    let mut checks = Checks {
        row,
        key: key_of(row),
        out,
    };
    if let Some(origin) = built.origin {
        creator_checks(&mut checks, &built, origin).await;
    }
    // The creator's lineage as the gate reads it (#163), a dead candidate's
    // proof read with the door's options.
    let tick = TickOptions {
        spend_depth: height(row, "spend_depth").unwrap(),
        ..TickOptions::default()
    }
    .floored();
    let lineage = lineage_candidates(&built.storage).await.unwrap();
    let readings = match lineage.proofs() {
        [] => std::collections::HashMap::new(),
        proofs => read_creators(&built.storage, &built.services, &tick, proofs)
            .await
            .unwrap(),
    };
    let dead = lineage.dead(&readings);
    checks.field(
        "Lineage::dead",
        "dead_lineage",
        &dead.contains_key(&built.txid),
    );
    coin_checks(&mut checks, &built).await;
}

fn pin_line<'a>(name: &str) -> &'a str {
    PIN.lines()
        .find_map(|l| l.strip_prefix(&format!("{name} ")))
        .unwrap_or_else(|| panic!("the pin names no {name}"))
        .trim()
}

/// The copy is the pinned file: its sha256 is the pin's.
#[test]
fn the_copy_is_the_pinned_file() {
    assert_eq!(pin_line("repo"), "bsv-stack-lean");
    assert_eq!(pin_line("path"), "conformance/wallet-gate-vectors.json");
    assert_eq!(pin_line("commit").len(), 40);
    assert_eq!(
        hex::encode(sha256(VECTORS)),
        pin_line("sha256"),
        "tests/vectors/wallet_gate_vectors.json is not the pinned file: move the pin with the \
         copy (docs/GATE-VECTORS.md)"
    );
    let doc: Value = serde_json::from_slice(VECTORS).unwrap();
    assert_eq!(doc["schema"], "wallet-gate-vectors/1");
    assert_eq!(doc["row_count"], ROWS);
    assert_eq!(doc["rows"].as_array().unwrap().len(), ROWS);
}

/// The recorded divergences, by (row, door, field): (lean, cli, finding).
type Recorded = BTreeMap<(String, String, String), (String, String, String)>;

fn recorded() -> Recorded {
    let doc: Value = serde_json::from_str(DIVERGENCES).unwrap();
    assert_eq!(
        doc["vectors_sha256"].as_str(),
        Some(pin_line("sha256")),
        "tests/vectors/wallet_gate_divergences.json was recorded against another copy"
    );
    let catalogue = doc["findings"].as_object().unwrap();
    doc["divergences"]
        .as_array()
        .unwrap()
        .iter()
        .map(|d| {
            let f = |k: &str| d[k].as_str().unwrap().to_string();
            assert!(
                catalogue.contains_key(&f("finding")),
                "a divergence names the finding {:?}, which the file does not describe",
                f("finding")
            );
            (
                (f("row"), f("door"), f("field")),
                (f("lean"), f("cli"), f("finding")),
            )
        })
        .collect()
}

/// Every difference found is recorded, every recorded one found; the
/// counts by finding printed.
fn check_divergences(found: &[Divergence]) {
    let recorded = recorded();
    let mut rows_by_finding: BTreeMap<&str, (usize, BTreeSet<&str>, BTreeSet<&str>)> =
        BTreeMap::new();
    let mut unlisted = Vec::new();
    let mut seen = BTreeSet::new();
    for d in found {
        let k = (d.key.clone(), d.door.clone(), d.field.clone());
        match recorded.get(&k) {
            Some((lean, cli, finding)) if *lean == d.lean && *cli == d.cli => {
                let entry = rows_by_finding.entry(finding).or_default();
                entry.0 += 1;
                entry.1.insert(&d.key);
                if d.field == "offerable" {
                    entry.2.insert(&d.key);
                }
                seen.insert(k);
            }
            _ => unlisted.push(d),
        }
    }
    let gone: Vec<_> = recorded.keys().filter(|k| !seen.contains(*k)).collect();
    println!(
        "gate vectors: {} divergences, every one recorded: {}",
        found.len(),
        unlisted.is_empty() && gone.is_empty()
    );
    for (finding, (count, rows, offerable)) in &rows_by_finding {
        println!(
            "gate vectors: finding {finding}: {count} differences over {} rows, offerable \
             differs on {} rows",
            rows.len(),
            offerable.len()
        );
    }
    for d in &unlisted {
        let line = serde_json::json!({"row": d.key, "door": d.door, "field": d.field, "lean": d.lean, "cli": d.cli, "finding": "?"});
        println!("DIVERGENCE {line}");
    }
    for k in &gone {
        println!("NO LONGER DIVERGES {k:?}");
    }
    assert!(
        unlisted.is_empty() && gone.is_empty(),
        "{} divergence(s) not recorded in tests/vectors/wallet_gate_divergences.json and {} \
         recorded that no longer diverge (each printed above, with its row and field)",
        unlisted.len(),
        gone.len()
    );
}

/// Every row replayed against the gate.
#[tokio::test]
async fn every_row_is_the_gates_answer() {
    assert_eq!(hex::encode(sha256(VECTORS)), pin_line("sha256"), "the pin");
    let doc: Value = serde_json::from_slice(VECTORS).unwrap();
    let rows = doc["rows"].as_array().unwrap();

    let mut keys = BTreeSet::new();
    let mut unexpressed_now = BTreeMap::new();
    let mut found = Vec::new();
    for row in rows {
        let key = key_of(row);
        assert!(keys.insert(key.clone()), "two rows share the key {key}");
        if let Some(why) = unexpressed(row) {
            unexpressed_now.insert(key, why);
            continue;
        }
        replay(row, &mut found).await;
    }

    let listed: BTreeMap<String, &str> = UNEXPRESSED
        .iter()
        .map(|(k, why)| (k.to_string(), *why))
        .collect();
    println!(
        "gate vectors: {} rows, {} expressed, {} unexpressed",
        rows.len(),
        rows.len() - unexpressed_now.len(),
        unexpressed_now.len()
    );
    for (columns, why) in COLUMNS_NOT_ANSWERED {
        println!("gate vectors: not answered by the gate: {columns}: {why}");
    }
    assert_eq!(
        unexpressed_now, listed,
        "UNEXPRESSED is not exact: a listed row has become expressible, or a row the CLI \
         cannot hold is not listed"
    );
    check_divergences(&found);
}
