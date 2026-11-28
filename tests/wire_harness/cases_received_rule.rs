//! The toolbox's received-coin rule at the served doors (the port of the
//! stable line's 0.18.8, #627), read through `POST /internalizeAction`,
//! `POST /createAction` and `POST /listActions` in the `received-rule`
//! world: two received payments taken in by the toolbox's own
//! `internalizeAction` (purpose `receive`, unproven), a caller's own
//! `underReceivedRule` stripped at `internalizeAction`, a caller's own
//! label at `createAction` that does not lift the gate's hold with no
//! word (275's answer), the gate's hold with no word, the scoped word
//! over the two (the named coin spent, the other unspent), the plain word,
//! and the listing of the labels with `underRule`; then the strip at
//! `createAction` where it can be seen, in the `internalize` world: a
//! spend with no word over a coin whose transaction is completed (offered
//! with no word), its labels carrying a label of the caller's own beside
//! two spellings of the rule, and its listing (the caller's label alone).
//! Recorded last, so no file before them moves. The answers that need the tracker's own pass (a
//! received coin the gate reads as mined at its depth while the toolbox has
//! not completed its transaction, and the same once it has) are reached by
//! no served request, so they are tests of `src/spend_gate/tests/
//! received_rule.rs`.
use serde_json::{json, Value};

use super::cases_internalize_door::payment;
use super::cases_internalize_rules::beef;
use super::recorder::{post, Recorded, Recorder};
use super::worlds::Payment;

const WORLD: &str = "received-rule";
/// A payment to a key of neither party.
const LOCK: &str = "76a914dbc0a7c84983c5bf199b7b2d41b3acf0408ee5aa88ac";
/// A received coin's 9,000 less the fee of one input and one output: an
/// exact spend, so the action makes no change output.
const EXACT: u64 = 8_980;

/// The subject txid of an `AtomicBEEF`.
fn subject(atomic: &[u8]) -> String {
    bsv_sdk::transaction::Transaction::from_atomic_beef(atomic)
        .expect("an AtomicBEEF")
        .id()
}

/// `POST /createAction` of one output of `satoshis`, with `extra` merged
/// into the body.
async fn create(r: &mut Recorder, name: &str, satoshis: u64, extra: Value) -> Recorded {
    let case = format!("createAction.receivedRule.{name}");
    create_in(r, WORLD, &case, satoshis, extra).await
}

/// [`create`], in `world` under the case name `case`.
async fn create_in(
    r: &mut Recorder,
    world: &str,
    case: &str,
    satoshis: u64,
    extra: Value,
) -> Recorded {
    let mut body = json!({"description": "wire received rule",
        "outputs": [{"lockingScript": LOCK, "outputDescription": "an output", "satoshis": satoshis}],
        "options": {"randomizeOutputs": false}});
    for (k, v) in extra.as_object().expect("an object") {
        body[k] = v.clone();
    }
    r.case(case, world, post("/createAction", body)).await
}

/// The actions labelled `labels` (every action for none), their labels
/// included.
async fn listed(r: &mut Recorder, name: &str, labels: Value) -> Recorded {
    r.case(
        &format!("listActions.receivedRule.{name}"),
        WORLD,
        post(
            "/listActions",
            json!({"labels": labels, "includeLabels": true}),
        ),
    )
    .await
}

/// The `received-rule` world's rows: no coin of its own (the basket
/// wanting none, so an exact spend makes no change) and the roots of the
/// stranger's payments' parents; its coins are the payments [`record`]
/// takes in.
pub(crate) async fn world(
    storage: &bsv_wallet_toolbox::StorageSqlx,
    identity: &str,
    key: &bsv_sdk::primitives::PrivateKey,
) -> std::collections::HashMap<u32, String> {
    super::worlds::Rows::open(storage, identity)
        .await
        .wanting_no_change(key)
        .await
}

pub(crate) async fn record(r: &mut Recorder) {
    let named = beef(Payment::Wallet).await;
    let other = beef(Payment::StrayInsertion).await;
    let (named_txid, other_txid) = (subject(&named), subject(&other));
    // A caller's own copy of the toolbox's label, in spellings the toolbox
    // reads as it, beside a label of its own: the copies are stripped.
    r.case(
        "internalizeAction.receivedRule.callersLabel",
        WORLD,
        post(
            "/internalizeAction",
            json!({"tx": named, "outputs": [payment()], "description": "wire received rule",
                "labels": ["wire received", "underReceivedRule", " UNDERRECEIVEDRULE\t"]}),
        ),
    )
    .await;
    r.case(
        "internalizeAction.receivedRule.second",
        WORLD,
        post(
            "/internalizeAction",
            json!({"tx": other, "outputs": [payment()], "description": "wire received rule"}),
        ),
    )
    .await;
    let received = listed(r, "received", json!([])).await;
    assert_eq!(received.value("/totalActions"), 2);
    // No word: both coins held, received with no proof.
    create(r, "noWord", 1_000, json!({})).await;
    // A caller's own label with no word does not lift the hold: 275's answer.
    create(
        r,
        "callersLabel",
        1_000,
        json!({"labels": ["underReceivedRule"]}),
    )
    .await;
    let scope = json!({"allowPending": {"received": [format!("{named_txid}:0")]}});
    // The scoped word past the named coin: the other held, not named.
    create(r, "scopedPast", 9_500, scope.clone()).await;
    let mut scoped = scope;
    scoped["labels"] = json!(["wire mine"]);
    create(r, "scoped", EXACT, scoped).await;
    let left = r
        .case(
            "listOutputs.receivedRule.afterScoped",
            WORLD,
            post("/listOutputs", json!({"basket": "default"})),
        )
        .await;
    assert_eq!(left.value("/totalOutputs"), 1, "the other coin alone");
    assert_eq!(
        left.value("/outputs/0/outpoint"),
        json!(format!("{other_txid}.0"))
    );
    create(
        r,
        "word",
        EXACT,
        json!({"allowPending": "received", "labels": ["wire mine"]}),
    )
    .await;
    listed(r, "labelled", json!(["underReceivedRule"])).await;
    record_strip_at_create(r).await;
}

/// The label a caller's own spend carries beside its copies of the rule.
const OWN: &str = "wire strip own";
/// The `internalize` world's 40,000-sat completed coin less the fee of one
/// input and one output: an exact spend, so the action makes no change.
const COMPLETED: u64 = 39_980;

/// The strip at `createAction`, seen in a spend that succeeds: in the
/// `internalize` world (its 40,000-sat mined coin, the transaction
/// completed, offered with no word, unspent at 270), an exact spend with
/// no word whose labels carry the caller's own beside `underReceivedRule`
/// in two spellings the toolbox reads as the rule (as 272 does at
/// `internalizeAction`); then the stored labels.
async fn record_strip_at_create(r: &mut Recorder) {
    let spent = r
        .case(
            "createAction.receivedRule.stripAtCreate",
            "internalize",
            post(
                "/createAction",
                json!({"description": "wire strip at create",
                    "outputs": [{"lockingScript": LOCK, "outputDescription": "an output", "satoshis": COMPLETED}],
                    "labels": [OWN, "underReceivedRule", " UNDERRECEIVEDRULE\t"],
                    "options": {"randomizeOutputs": false}}),
            ),
        )
        .await;
    assert!(spent.value("/txid").is_string(), "the spend signs");
    let listed = r
        .case(
            "listActions.receivedRule.stripAtCreate",
            "internalize",
            post(
                "/listActions",
                json!({"labels": [OWN], "includeLabels": true}),
            ),
        )
        .await;
    assert_eq!(listed.value("/totalActions"), 1);
    assert_eq!(
        listed.value("/actions/0/labels"),
        json!([OWN]),
        "the caller's own label alone: no underreceivedrule, no underRule"
    );
}

/// The door's word (#645): the host route's answer for 272's payment, a
/// received body with no proof whose post the world's broadcaster took.
/// Recorded last of all, so no file before it moves.
pub(crate) async fn record_door_word(r: &mut Recorder) {
    let named = subject(&beef(Payment::Wallet).await);
    let word = r
        .case(
            "tx.receivedRule.doorWord",
            WORLD,
            super::recorder::get(&format!("/tx/{named}")),
        )
        .await;
    assert_eq!(word.value("/word"), json!("announced"));
}

/// The host's settings of the world `name`: the defaults, or for the worlds
/// of the word's rule the hold (`SPEND_RECEIVED=hold`, #659).
pub(crate) fn settings(name: &str) -> std::sync::Arc<bsv_wallet_cli::config::Config> {
    if matches!(name, "received-rule" | "received-built") {
        crate::wire_support::hold_config()
    } else {
        crate::wire_support::quiet_config()
    }
}

/// The tracker's row of `txid` at `built`. In the named-change world, the
/// create door's word for the action that made the coin, as every road
/// this host signs by writes it: our own change is offered from it. In
/// received-built, a received payment no broadcaster took, as the pass
/// leaves one whose take left no record (#659).
pub(crate) async fn built(storage: &bsv_wallet_toolbox::StorageSqlx, txid: &str) {
    bsv_wallet_cli::tracker_host::stored_word(storage, txid)
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO tracker_states (txid, built, hints, word) VALUES (?, 1, '[]', 'built')",
    )
    .bind(txid)
    .execute(storage.writer())
    .await
    .unwrap();
}

/// The host's setting (#659), recorded last of all, so no file before it
/// moves. The `received-rule` world above is a host that sets
/// `SPEND_RECEIVED=hold`: its rows are the word's rule, as they were
/// recorded. Here, the default in the `received-taken` world (the same
/// payment, the broadcaster taking its post): 275's request with no word,
/// for the coin's exact value (no change output, so every recording signs
/// the same bytes), is answered 200. Then the `received-built` world, a hold host
/// holding one received payment no broadcaster took (seeded as the
/// tracker's pass leaves a payment whose take left no record): its word is
/// `built`, and the action that says the word is refused all the same.
pub(crate) async fn record_setting(r: &mut Recorder) {
    let named = beef(Payment::Wallet).await;
    let internalize = |tx: &[u8]| {
        post(
            "/internalizeAction",
            json!({"tx": tx, "outputs": [payment()], "description": "wire received rule"}),
        )
    };
    r.case(
        "internalizeAction.receivedTaken.payment",
        "received-taken",
        internalize(&named),
    )
    .await;
    let spent = create_in(
        r,
        "received-taken",
        "createAction.receivedTaken.noWord",
        EXACT,
        json!({}),
    )
    .await;
    assert!(spent.value("/txid").is_string(), "the spend signs");

    let word = r
        .case(
            "tx.receivedBuilt.doorWord",
            "received-built",
            super::recorder::get(&format!("/tx/{}", super::worlds::PENDING)),
        )
        .await;
    assert_eq!(word.value("/word"), json!("built"));
    let held = create_in(
        r,
        "received-built",
        "createAction.receivedBuilt.word",
        1_000,
        json!({"allowPending": "received"}),
    )
    .await;
    assert_eq!(held.value("/code"), json!("INSUFFICIENT_FUNDS"));
    assert_eq!(held.value("/held/0/word"), json!("built"));
}

pub(crate) fn notes() -> Vec<(String, String)> {
    [
        ("272-internalizeAction.receivedRule.callersLabel.json", "#627 (the stable line's 0.18.8): a received payment taken in through the toolbox's own internalizeAction (purpose receive, unproven), its labels carrying the caller's own underReceivedRule in two spellings the toolbox reads as it beside a label of its own, 200. New; recorded last, so no file before it moves."),
        ("273-internalizeAction.receivedRule.second.json", "#627: a second received payment, 200: two received unproven coins for the scoped word."),
        ("274-listActions.receivedRule.received.json", "#627: the two payments, unproven; 272's labels are the caller's own alone: its copies of underReceivedRule were stripped at the door."),
        ("275-createAction.receivedRule.noWord.json", "#627: with no allowPending the gate holds both received coins (received, no proof; offered under allowPending: received), 402 INSUFFICIENT_FUNDS with the coins under held, nothing offered. Since #659 this world's host sets SPEND_RECEIVED=hold, which keeps this answer; under the default the same request is answered 200 (row 296)."),
        ("276-createAction.receivedRule.callersLabel.json", "#627: a caller's own underReceivedRule with no word does not lift the gate's hold: 275's answer, byte for byte. It pins the hold, not the strip."),
        ("277-createAction.receivedRule.scopedPast.json", "#627: the scoped word naming 272's coin, for more than it holds: 9000 offered, the other coin held not named under allowPending: received, 402 INSUFFICIENT_FUNDS."),
        ("278-createAction.receivedRule.scoped.json", "#627: the scoped word within the named coin: the toolbox selects it (the gate passes underReceivedRule beside its own label) and the spend signs and posts, 200; an exact spend, no change."),
        ("279-listOutputs.receivedRule.afterScoped.json", "#627: after 278 the default basket holds 273's coin alone, unspent."),
        ("280-createAction.receivedRule.word.json", "#627: the plain word: the other received coin is selected and the spend signs and posts, 200."),
        ("281-listActions.receivedRule.labelled.json", "#627: the actions labelled underReceivedRule, with includeLabels: 278 and 280, each with the caller's label, the gate's spend-gate:received-pending and the toolbox's label as the store keeps it (underreceivedrule), underRule received-pending."),
        ("282-createAction.receivedRule.stripAtCreate.json", "#627: the strip at createAction where it can be seen: in the internalize world, an exact spend with no word over its 40,000-sat coin, whose transaction is completed (offered with no word), its labels carrying the caller's own beside underReceivedRule in two spellings the toolbox reads as the rule (as 272 at internalizeAction), 200, signed and posted."),
        ("283-listActions.receivedRule.stripAtCreate.json", "#627: 282's action by the caller's label, with includeLabels: the caller's own label alone, no underreceivedrule, no underRule: its copies of underReceivedRule were stripped at createAction."),
        ("294-tx.receivedRule.doorWord.json", "#645 (P1, the door's word and the take): GET /tx/{txid} for 272's payment, a received body with no proof whose post the world's broadcaster took: 200, the word announced (built at the door by the tracker's reducer, then the take fed as a broadcaster's acceptance). Before, 404: the host wrote no row at the door. With it the held coins of 275, 276 and 277 carry the word announced where they carried unknown; no offer moves. New; recorded last, so no file before it moves."),
        ("295-internalizeAction.receivedTaken.payment.json", "#659 (the offer rule under the cut's default): 272's payment taken in by a host at its defaults (the received-taken world, SPEND_RECEIVED unset), the broadcaster taking its post: 200. New; recorded last, so no file before it moves."),
        ("296-createAction.receivedTaken.noWord.json", "#659: 275's request with no allowPending, at the default, for the coin's exact value (no change output): the received coin a broadcaster took (announced) is offered with no word, and the spend signs and posts, 200. Under the word's rule (275, a host that sets SPEND_RECEIVED=hold) a spend with no word is 402 INSUFFICIENT_FUNDS."),
        ("297-tx.receivedBuilt.doorWord.json", "#659: GET /tx/{txid} in the received-built world, a host that sets SPEND_RECEIVED=hold holding one received payment of 50,000 sats that no broadcaster took (seeded as the tracker's pass leaves a payment whose take left no record): 200, the word built."),
        ("298-createAction.receivedBuilt.word.json", "#659: allowPending: received over 297's coin: 402 INSUFFICIENT_FUNDS, the coin held `built; received, not taken by a broadcaster: offered once a broadcaster takes it or once mined`. The word no longer offers a received coin no broadcaster took; before, under the word, it was offered at every unmined word."),
    ]
    .into_iter()
    .map(|(f, n)| (f.to_string(), n.to_string()))
    .collect()
}
