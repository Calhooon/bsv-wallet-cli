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
//! for `rejected` and `conflicted`), and the mock header service at the
//! row's tip with the row's root at the creator's proof height. The gate is
//! then asked through its public doors, nothing restated here:
//!
//! - `read_creators`: the creator's reading. Its word must be the row's: a
//!   difference is this test's construction failing, never a finding. Its
//!   suspect mark is read after the guard's own recheck and is compared.
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

use std::collections::{BTreeMap, BTreeSet};

use bsv_sdk::primitives::hash::{sha256, sha256d};
use bsv_sdk::transaction::MerklePath;
use bsv_tracker::{ChainEnvelope, ChainEvent, EventHeader, Hint, HintStatus};
use bsv_wallet_cli::spend_gate::{
    assess, assess_named, hold_reason, AllowPending, GateOptions, Origin,
};
use bsv_wallet_cli::tracker_host::{heard, read_creators, SystemClock, TickOptions};
use bsv_wallet_toolbox::services::mock::MockWalletServices;
use bsv_wallet_toolbox::services::BlockHeader;
use bsv_wallet_toolbox::{StorageSqlx, WalletStorageWriter};
use serde_json::Value;

const VECTORS: &[u8] = include_bytes!("vectors/wallet_gate_vectors.json");
const PIN: &str = include_str!("vectors/wallet_gate_vectors.pin");
const DIVERGENCES: &str = include_str!("vectors/wallet_gate_divergences.json");
const ROWS: usize = 490;
const LOCK: &str = "76a914dbc0a7c84983c5bf199b7b2d41b3acf0408ee5aa88ac";

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
    ("coin_word, coin_word_data", "the gate answers offered or held with a reason; HeldCoin.word is the creator's tracker word (or locked, frozen), not the coin's word"),
    ("dead_lineage", "the gate reads the creator only, never its ancestors (the finding no-lineage)"),
    ("host_answer, host_answer_data", "a row names its door (createAction, internalizeAction, ...) but carries none of its arguments (no BEEF, no outputs), so the host's answer at that door cannot be replayed from the row"),
];

fn s<'a>(row: &'a Value, key: &str) -> Option<&'a str> {
    row.get(key).and_then(Value::as_str)
}

fn b(row: &Value, key: &str) -> Option<bool> {
    row.get(key).and_then(Value::as_bool)
}

fn n(row: &Value, key: &str) -> Option<u64> {
    row.get(key).and_then(Value::as_u64)
}

fn key_of(row: &Value) -> String {
    format!(
        "{} | step {} | {} | allow_received {}",
        s(row, "case").unwrap_or("?"),
        n(row, "step").unwrap_or(0),
        s(row, "coin").unwrap_or("?"),
        b(row, "allow_received").unwrap_or(false)
    )
}

/// A Lean name (`F`, `T1`, `rB1`) as a 32-byte display-order hash.
fn hash_of(name: &str) -> String {
    hex::encode(sha256(format!("wallet-gate-vectors:{name}").as_bytes()))
}

fn display(raw: &[u8]) -> String {
    let mut reversed = raw.to_vec();
    reversed.reverse();
    hex::encode(reversed)
}

/// A header at `height` committing to `root`, its hash computed.
fn header(height: u32, root: &str) -> BlockHeader {
    let mut h = BlockHeader {
        version: 1,
        previous_hash: "00".repeat(32),
        merkle_root: root.to_string(),
        time: 1_700_000_000,
        bits: 0x1d00_ffff,
        nonce: height,
        hash: String::new(),
        height,
    };
    h.hash = display(&sha256d(&h.to_binary()));
    h
}

fn event_header(h: &BlockHeader) -> EventHeader {
    EventHeader {
        version: h.version,
        previous_hash: h.previous_hash.clone(),
        merkle_root: h.merkle_root.clone(),
        time: h.time,
        bits: h.bits,
        nonce: h.nonce,
        height: h.height,
        hash: h.hash.clone(),
        // A hash-shaped work, as the decoder asks.
        chain_work: format!("{}01", "00".repeat(31)),
    }
}

fn envelope(event: ChainEvent) -> String {
    serde_json::to_string(&ChainEnvelope::new(event)).unwrap()
}

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
    } else if reason.starts_with("spent by") {
        "held by a lock"
    } else if reason.starts_with("suspect:") {
        "creator suspect (fork in progress)"
    } else if reason.ends_with(": never offered") {
        "door: a chain word against the creator"
    } else if reason.contains("; received, no proof;")
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

/// The guard's depth a reason names (`depth N: ...`).
fn depth_of(reason: Option<&str>) -> Option<u64> {
    reason?
        .strip_prefix("depth ")?
        .split(':')
        .next()?
        .parse()
        .ok()
}

/// A row's number as a height.
fn height(row: &Value, key: &str) -> Option<u32> {
    n(row, key).map(|v| u32::try_from(v).unwrap())
}

/// One row's store and header service, built from its inputs.
struct Built {
    storage: StorageSqlx,
    services: MockWalletServices,
    user: i64,
    basket: i64,
    txid: String,
    vout: u32,
    proof: Option<Vec<u8>>,
    /// The header at the creator's proof height, as the service answers.
    live_at_proof: Option<BlockHeader>,
    output_id: Option<i64>,
    origin: Option<Origin>,
}

/// A migrated in-memory store with the tracker's tables, and its user and
/// default basket.
async fn store() -> (StorageSqlx, i64, i64) {
    let identity = format!("02{}", "ab".repeat(32));
    let storage = StorageSqlx::in_memory().await.unwrap();
    storage.migrate("gate-vectors", &identity).await.unwrap();
    storage.make_available().await.unwrap();
    let user = storage.find_or_insert_user(&identity).await.unwrap().0;
    let basket = storage.find_or_create_default_basket(user.user_id);
    let basket = basket.await.unwrap().basket_id;
    // The tracker's tables, as every read of them makes them.
    let none = MockWalletServices::builder().build();
    read_creators(&storage, &none, &TickOptions::default(), &[])
        .await
        .unwrap();
    (storage, user.user_id, basket)
}

/// The header service of the row: the tip at `headers_tip`, and at the
/// creator's proof height a header carrying the row's root there. The
/// proof is coinbase-style (its root is the creator's txid), so the header
/// carries the txid when the row's two root names agree.
fn headers(row: &Value, txid: &str) -> (MockWalletServices, Option<BlockHeader>) {
    let services = MockWalletServices::builder().build();
    let tip = height(row, "headers_tip").unwrap();
    let proof_root = s(row, "creator_proof_root");
    let live = height(row, "creator_proof_height").map(|h| {
        let root = match s(row, "header_root_at_proof_height") {
            Some(name) if Some(name) == proof_root => txid.to_string(),
            Some(name) => hash_of(name),
            None => hash_of("no header"),
        };
        header(h, &root)
    });
    if let Some(h) = &live {
        services.set_header_for_height(h.clone());
    }
    match &live {
        Some(h) if h.height == tip => services.set_tip_header(h.clone()),
        _ => services.set_tip_header(header(tip, &hash_of(&format!("tip {tip}")))),
    }
    (services, live)
}

async fn build(row: &Value) -> Built {
    let (storage, user, basket) = store().await;
    let (creator, vout) = s(row, "coin").unwrap().rsplit_once('.').unwrap();
    let txid = hash_of(creator);
    let (services, live_at_proof) = headers(row, &txid);
    let mut built = Built {
        storage,
        services,
        user,
        basket,
        txid,
        vout: vout.parse().unwrap(),
        proof: None,
        live_at_proof,
        output_id: None,
        origin: None,
    };
    if b(row, "coin_held") == Some(true) {
        let made = s(row, "creator_origin") == Some("made");
        let mined = s(row, "creator_word") == Some("mined");
        let tx_row = insert_action(
            &built,
            creator,
            &built.txid,
            made,
            if mined { "completed" } else { "unproven" },
        )
        .await;
        evidence(row, &mut built, made).await;
        built.output_id = Some(insert_coin(row, &built, tx_row).await);
        built.origin = Some(if made { Origin::Made } else { Origin::Received });
    }
    built
}

/// An action of the toolbox's `transactions`: its row id.
async fn insert_action(built: &Built, name: &str, txid: &str, ours: bool, status: &str) -> i64 {
    sqlx::query(
        "INSERT INTO transactions (user_id, status, reference, is_outgoing, satoshis, version, lock_time, description, txid, raw_tx) \
         VALUES (?, ?, ?, ?, 0, 1, 0, 'gate vectors', ?, X'01000000')",
    )
    .bind(built.user)
    .bind(status)
    .bind(format!("ref-{name}"))
    .bind(i64::from(ours))
    .bind(txid)
    .execute(built.storage.writer())
    .await
    .unwrap()
    .last_insert_rowid()
}

/// The creator's evidence, as the host keeps it.
async fn evidence(row: &Value, built: &mut Built, made: bool) {
    let word = s(row, "creator_word").unwrap();
    let data = s(row, "creator_word_data").unwrap_or("");
    match word {
        "mined" => mined(row, built, made).await,
        "stale" => stale(row, built, made).await,
        "rejected" => {
            let verdict = serde_json::json!({"kind": "rejected", "reason": data});
            insert_tracked(built, made, None, &[], Some(verdict)).await;
        }
        "conflicted" => {
            let competitors: Vec<String> = data.split_whitespace().map(hash_of).collect();
            let verdict = serde_json::json!({"kind": "conflicted", "competitors": competitors});
            insert_tracked(built, made, None, &[], Some(verdict)).await;
        }
        "built" => insert_tracked(built, true, None, &[], None).await,
        "announced" => hint(built, HintStatus::Accepted).await,
        "seen" => hint(built, HintStatus::Seen).await,
        // No record of the creator but the action: the tracker reads it
        // `unknown`.
        "unknown" => {}
        other => panic!(
            "{}: a creator word this replay does not build: {other}",
            key_of(row)
        ),
    }
}

/// The creator's proof: coinbase-style at the row's proof height.
fn proof_path(row: &Value, built: &Built) -> MerklePath {
    let h = height(row, "creator_proof_height").expect("the creator carries its proof");
    MerklePath::from_coinbase_txid(&built.txid, h)
}

/// A broadcaster's hint, stored as `heard` stores it.
async fn hint(built: &Built, status: HintStatus) {
    let hint = Hint::new("arcade", status, 1_700_000_000);
    let opts = TickOptions::default();
    heard(
        &built.storage,
        &SystemClock,
        &opts,
        &built.txid,
        hint,
        None,
        None,
    )
    .await
    .unwrap();
}

/// A mined creator: the toolbox's proof record, and for the suspect mark
/// the fork stored from the feed.
async fn mined(row: &Value, built: &mut Built, made: bool) {
    let path = proof_path(row, built);
    built.proof = Some(path.to_binary());
    insert_proven(built, &path).await;
    if b(row, "creator_suspect") == Some(true) {
        let h = built.live_at_proof.clone().unwrap();
        let fork = ChainEvent::Fork {
            height: h.height,
            competing_tips: [
                event_header(&h),
                event_header(&header(h.height, &hash_of("fork"))),
            ],
            depth: 1,
        };
        let chain = [envelope(fork)];
        insert_tracked(built, made, Some(&path.to_hex()), &chain, None).await;
    }
}

/// A stale creator, mined in a block that left the active chain: the stored
/// path, and the stored reorg that took the block it was checked in.
async fn stale(row: &Value, built: &Built, made: bool) {
    let path = &proof_path(row, built);
    let tip = height(row, "headers_tip").unwrap();
    let reorg = ChainEvent::Reorg {
        fork_height: path.block_height,
        depth: 1,
        deactivated_headers: vec![event_header(&header(path.block_height, &built.txid))],
        new_tip: event_header(&header(tip, &hash_of(&format!("tip {tip}")))),
    };
    let chain = [envelope(reorg)];
    insert_tracked(built, made, Some(&path.to_hex()), &chain, None).await;
}

/// The toolbox's record of a checked proof (`proven_txs`).
async fn insert_proven(built: &Built, path: &MerklePath) {
    sqlx::query(
        "INSERT INTO proven_txs (txid, height, idx, block_hash, merkle_root, merkle_path, raw_tx) \
         VALUES (?, ?, 0, ?, ?, ?, X'01000000')",
    )
    .bind(&built.txid)
    .bind(i64::from(path.block_height))
    .bind(built.live_at_proof.as_ref().map(|h| h.hash.clone()))
    .bind(&built.txid)
    .bind(path.to_binary())
    .execute(built.storage.writer())
    .await
    .unwrap();
}

/// The coin, a change output of the default basket, with its lock:
/// `locked` and `spent` are `spent_by` a live or a completed action of
/// ours, `lost` the relinquished record (`spendable = 0`).
async fn insert_coin(row: &Value, built: &Built, tx_row: i64) -> i64 {
    let lock = s(row, "coin_lock").unwrap();
    let spender = match lock {
        "locked" | "spent" => {
            let name = s(row, "coin_lock_txid").unwrap();
            let status = if lock == "spent" {
                "completed"
            } else {
                "unproven"
            };
            Some(
                insert_action(
                    built,
                    &format!("spender-{name}"),
                    &hash_of(name),
                    true,
                    status,
                )
                .await,
            )
        }
        _ => None,
    };
    let satoshis = i64::try_from(n(row, "coin_satoshis").unwrap()).unwrap();
    sqlx::query(
        "INSERT INTO outputs (user_id, transaction_id, basket_id, vout, satoshis, locking_script, txid, type, spendable, change, provided_by, purpose, output_description, spent_by) \
         VALUES (?, ?, ?, ?, ?, ?, ?, 'P2PKH', ?, 1, 'storage', 'change', 'c', ?)",
    )
    .bind(built.user)
    .bind(tx_row)
    .bind(built.basket)
    .bind(i64::from(built.vout))
    .bind(satoshis)
    .bind(hex::decode(LOCK).unwrap())
    .bind(&built.txid)
    .bind(i64::from(lock == "free"))
    .bind(spender)
    .execute(built.storage.writer())
    .await
    .unwrap()
    .last_insert_rowid()
}

/// A tracker row in the host's own record (`tracker_states`).
async fn insert_tracked(
    built: &Built,
    was_built: bool,
    bump: Option<&str>,
    chain: &[String],
    verdict: Option<Value>,
) {
    sqlx::query(
        "INSERT INTO tracker_states (txid, built, hints, bump, chain, verdict) VALUES (?, ?, '[]', ?, ?, ?)",
    )
    .bind(&built.txid)
    .bind(i64::from(was_built))
    .bind(bump)
    .bind(serde_json::to_string(chain).unwrap())
    .bind(verdict.map(|v| v.to_string()))
    .execute(built.storage.writer())
    .await
    .unwrap();
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
    // The mark after the guard's own recheck (`SpendAttempt`, `Recheck`):
    // the gate's answer, compared.
    checks.field("read_creators", "creator_suspect", &reading.suspect);
    for (tag, opts) in &words {
        let door = hold_reason(reading, origin, (&built.txid, built.vout), opts);
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
        let held = assess_named(&built.storage, &built.services, opts, &coin, None)
            .await
            .unwrap();
        let door = format!("assess_named{tag}");
        checks.field(&door, "offerable", &held.is_empty());
        if built.output_id.is_some() {
            let reason = held.first().map(|c| c.reason.as_str());
            checks.field(&door, "offerable_clause", &clause_of(reason));
        }

        let a = assess(&built.storage, &built.services, opts, None)
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
