//! The toolbox's received-coin rule at the served doors (the port of the
//! stable line's 0.18.8, #627), read through `POST /internalizeAction`,
//! `POST /createAction` and `POST /listActions` in the `received-rule`
//! world: two received payments taken in by the toolbox's own
//! `internalizeAction` (purpose `receive`, unproven), a caller's own
//! `underReceivedRule` stripped at `internalizeAction` and at
//! `createAction`, the gate's hold with no word, the scoped word over the
//! two (the named coin spent, the other unspent), the plain word, and the
//! listing of the labels with `underRule`. Recorded last, so no file
//! before them moves. The answers that need the tracker's own pass (a
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
    let mut body = json!({"description": "wire received rule",
        "outputs": [{"lockingScript": LOCK, "outputDescription": "an output", "satoshis": satoshis}],
        "options": {"randomizeOutputs": false}});
    for (k, v) in extra.as_object().expect("an object") {
        body[k] = v.clone();
    }
    r.case(
        &format!("createAction.receivedRule.{name}"),
        WORLD,
        post("/createAction", body),
    )
    .await
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
    // A caller's own label with no word: stripped, the same hold.
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
}

pub(crate) fn notes() -> Vec<(String, String)> {
    [
        ("272-internalizeAction.receivedRule.callersLabel.json", "#627 (the stable line's 0.18.8): a received payment taken in through the toolbox's own internalizeAction (purpose receive, unproven), its labels carrying the caller's own underReceivedRule in two spellings the toolbox reads as it beside a label of its own, 200. New; recorded last, so no file before it moves."),
        ("273-internalizeAction.receivedRule.second.json", "#627: a second received payment, 200: two received unproven coins for the scoped word."),
        ("274-listActions.receivedRule.received.json", "#627: the two payments, unproven; 272's labels are the caller's own alone: its copies of underReceivedRule were stripped at the door."),
        ("275-createAction.receivedRule.noWord.json", "#627: with no allowPending the gate holds both received coins (received, no proof; offered under allowPending: received), 402 INSUFFICIENT_FUNDS with the coins under held, nothing offered."),
        ("276-createAction.receivedRule.callersLabel.json", "#627: a caller's own underReceivedRule with no word is stripped before the door: 275's answer."),
        ("277-createAction.receivedRule.scopedPast.json", "#627: the scoped word naming 272's coin, for more than it holds: 9000 offered, the other coin held not named under allowPending: received, 402 INSUFFICIENT_FUNDS."),
        ("278-createAction.receivedRule.scoped.json", "#627: the scoped word within the named coin: the toolbox selects it (the gate passes underReceivedRule beside its own label) and the spend signs and posts, 200; an exact spend, no change."),
        ("279-listOutputs.receivedRule.afterScoped.json", "#627: after 278 the default basket holds 273's coin alone, unspent."),
        ("280-createAction.receivedRule.word.json", "#627: the plain word: the other received coin is selected and the spend signs and posts, 200."),
        ("281-listActions.receivedRule.labelled.json", "#627: the actions labelled underReceivedRule, with includeLabels: 278 and 280, each with the caller's label, the gate's spend-gate:received-pending and the toolbox's label as the store keeps it (underreceivedrule), underRule received-pending."),
    ]
    .into_iter()
    .map(|(f, n)| (f.to_string(), n.to_string()))
    .collect()
}
