//! The toolbox's `internalizeAction` door read whole against the reference
//! (0.14.0-beta.8.1), served: the port of the stable line's 0.18.6. Each
//! answer the stable line's `tests/served_internalize_door.rs` reads that
//! no row of this corpus served before, and that a caller of this host can
//! reach, read through `POST /internalizeAction` in the `internalize` world
//! after the rows of 0.18.4 and 0.18.5: a root the tracker refutes, an
//! empty `outputs`, one `outputIndex` named twice on a new transaction and
//! on one the wallet holds, a sender identity key that is an uncompressed
//! public key, and an empty one; then one `outputIndex` named by a payment
//! and an insertion, and by two insertions (267, 268). The listings before
//! and after show that
//! no refusal stored anything; the accepting side of each is row 238 (the
//! same payment made well: proven at a seeded root, one output named once,
//! its sender a compressed key) and, for the held transaction, row 265.
//! Recorded last, so no file before them moves. The answer with no chain
//! tracker has no world here (every world's services name the header
//! stub): `tests/chaintracks_off.rs` reads it through the served router.
use serde_json::{json, Value};

use super::cases_internalize_rules::beef;
use super::recorder::{post, Recorded, Recorder};
use super::worlds::{stranger_key, Payment, PAYMENT_PREFIX, PAYMENT_SUFFIX};

const WORLD: &str = "internalize";

/// A wallet payment of output 0 with the stranger's prefix and suffix and
/// `sender` as given.
fn payment_from(sender: &str) -> Value {
    json!({"outputIndex": 0, "protocol": "wallet payment",
        "paymentRemittance": {"derivationPrefix": PAYMENT_PREFIX, "derivationSuffix": PAYMENT_SUFFIX,
            "senderIdentityKey": sender}})
}

/// The stranger's remittance, its key compressed (row 238's).
pub(crate) fn payment() -> Value {
    payment_from(&stranger_key().public_key().to_hex())
}

/// `POST /internalizeAction` of `tx` with `outputs`.
async fn internalize(r: &mut Recorder, name: &str, tx: &[u8], outputs: Value) -> Recorded {
    r.case(
        &format!("internalizeAction.door.{name}"),
        WORLD,
        post(
            "/internalizeAction",
            json!({"tx": tx, "outputs": outputs, "description": "wire internalize door"}),
        ),
    )
    .await
}

/// The wallet's actions and its default basket, as listed now.
async fn listings(r: &mut Recorder, when: &str) -> (Recorded, Recorded) {
    let actions = r
        .case(
            &format!("listActions.internalizeDoor.{when}"),
            WORLD,
            post("/listActions", json!({"labels": []})),
        )
        .await;
    let outputs = r
        .case(
            &format!("listOutputs.internalizeDoor.{when}"),
            WORLD,
            post("/listOutputs", json!({"basket": "default"})),
        )
        .await;
    (actions, outputs)
}

/// Each listed action's txid, status and satoshis.
fn stored_actions(listed: &Recorded) -> Vec<Value> {
    let actions = listed.value("/actions");
    let actions = actions.as_array().expect("the actions");
    actions
        .iter()
        .map(|a| json!([a["txid"], a["status"], a["satoshis"]]))
        .collect()
}

pub(crate) async fn record(r: &mut Recorder) {
    let (actions, outputs) = listings(r, "before").await;
    internalize(
        r,
        "rootRefuted",
        &beef(Payment::UnknownRoot).await,
        json!([payment()]),
    )
    .await;
    // A transaction the wallet does not hold (row 224 refused it).
    let new = beef(Payment::OtherKey).await;
    internalize(r, "emptyOutputs", &new, json!([])).await;
    internalize(r, "indexTwiceNew", &new, json!([payment(), payment()])).await;
    // The received payment row 238 stored.
    let held = beef(Payment::Wallet).await;
    internalize(r, "indexTwiceHeld", &held, json!([payment(), payment()])).await;
    let uncompressed = hex::encode(stranger_key().public_key().to_uncompressed());
    internalize(
        r,
        "senderUncompressed",
        &new,
        json!([payment_from(&uncompressed)]),
    )
    .await;
    internalize(r, "senderEmpty", &new, json!([payment_from("")])).await;
    let (actions_after, outputs_after) = listings(r, "after").await;
    assert_eq!(
        actions_after.value(""),
        actions.value(""),
        "no action stored"
    );
    assert_eq!(
        outputs_after.value(""),
        outputs.value(""),
        "no output moved"
    );
    // The accepting side of `indexTwiceHeld`: the held payment named once.
    internalize(r, "heldNamedOnce", &held, json!([payment()])).await;
    let held_after = r
        .case(
            "listOutputs.internalizeDoor.afterHeld",
            WORLD,
            post("/listOutputs", json!({"basket": "default"})),
        )
        .await;
    assert_eq!(
        held_after.value(""),
        outputs.value(""),
        "the held row as it was"
    );
    pairs(r, &new, &actions, &outputs).await;
}

/// One `outputIndex` named twice whatever the protocols, on the new
/// transaction `new`: a payment with an insertion, and two insertions, into
/// a basket nothing holds (267, 268); then the listings (269 to 271), the
/// actions and the default basket as `actions` and `outputs` listed them.
async fn pairs(r: &mut Recorder, new: &[u8], actions: &Recorded, outputs: &Recorded) {
    let into = json!({"outputIndex": 0, "protocol": "basket insertion",
        "insertionRemittance": {"basket": "door"}});
    let pair = json!([payment(), into.clone()]);
    internalize(r, "indexTwicePaymentInsertion", new, pair).await;
    let pair = json!([into.clone(), into]);
    internalize(r, "indexTwiceInsertions", new, pair).await;
    let (actions_pairs, outputs_pairs) = listings(r, "afterPairs").await;
    // 265's merge rewrote the held action's description; each action's
    // txid, status and satoshis are 255's.
    assert_eq!(
        stored_actions(&actions_pairs),
        stored_actions(actions),
        "no action stored"
    );
    assert_eq!(
        outputs_pairs.value(""),
        outputs.value(""),
        "no output moved"
    );
    let door = r
        .case(
            "listOutputs.internalizeDoor.doorBasket",
            WORLD,
            post("/listOutputs", json!({"basket": "door"})),
        )
        .await;
    assert_eq!(door.value("/totalOutputs"), json!(0), "nothing inserted");
}

/// The rows' notes in the manifest.
pub(crate) fn notes() -> Vec<(String, String)> {
    [
        ("255-listActions.internalizeDoor.before.json", "toolbox #831: the internalize world's actions before rows 257 to 262. New; recorded last, so no file before it moves."),
        ("256-listOutputs.internalizeDoor.before.json", "toolbox #831: the default basket before rows 257 to 262."),
        ("257-internalizeAction.door.rootRefuted.json", "toolbox #831: a wallet payment whose BEEF's root the tracker refutes (its parent proven at a height the header stub answers another root for): Invalid BEEF at its byte, RootNotCarried, 400 VALIDATION_ERROR; nothing stored (263, 264). Accepting side: 238."),
        ("258-internalizeAction.door.emptyOutputs.json", "toolbox #831: outputs: [] is refused, InvalidParameter, 400 VALIDATION_ERROR; nothing stored (263). Accepting side: 238."),
        ("259-internalizeAction.door.indexTwiceNew.json", "toolbox #831: one outputIndex named by two wallet payments on a new transaction, InvalidParameter, 400 VALIDATION_ERROR; nothing stored (263). Accepting side: 238, named once."),
        ("260-internalizeAction.door.indexTwiceHeld.json", "toolbox #831: one outputIndex named by two wallet payments on the transaction the wallet holds (238's), InvalidParameter, 400 VALIDATION_ERROR; the row as it was (264, 266). Accepting side: 265."),
        ("261-internalizeAction.door.senderUncompressed.json", "toolbox #831: a wallet payment whose senderIdentityKey is an uncompressed public key is refused by the argument validator, no more than 66 length, InvalidParameter, 400 VALIDATION_ERROR; nothing stored (263). Accepting side: 238, the key compressed."),
        ("262-internalizeAction.door.senderEmpty.json", "toolbox #831: an empty senderIdentityKey is refused, InvalidParameter, 400 VALIDATION_ERROR; nothing stored (263)."),
        ("263-listActions.internalizeDoor.after.json", "toolbox #831: after rows 257 to 262 the actions are 255's: no refusal stored a transaction."),
        ("264-listOutputs.internalizeDoor.after.json", "toolbox #831: after rows 257 to 262 the default basket is 256's."),
        ("265-internalizeAction.door.heldNamedOnce.json", "toolbox #831: the accepting side of 260, the held payment (238's) named once, 200."),
        ("266-listOutputs.internalizeDoor.afterHeld.json", "toolbox #831: after 265 the default basket is 256's: the held row unchanged."),
        ("267-internalizeAction.door.indexTwicePaymentInsertion.json", "toolbox #831: one outputIndex named by a wallet payment and a basket insertion on a new transaction, InvalidParameter, 400 VALIDATION_ERROR; nothing stored (269, 270, 271)."),
        ("268-internalizeAction.door.indexTwiceInsertions.json", "toolbox #831: one outputIndex named by two basket insertions on a new transaction, InvalidParameter, 400 VALIDATION_ERROR; nothing stored (269, 270, 271)."),
        ("269-listActions.internalizeDoor.afterPairs.json", "toolbox #831: after 267 and 268 the actions are 255's, each its txid, status and satoshis (265's merge rewrote the held action's description): no refusal stored a transaction."),
        ("270-listOutputs.internalizeDoor.afterPairs.json", "toolbox #831: after 267 and 268 the default basket is 256's."),
        ("271-listOutputs.internalizeDoor.doorBasket.json", "toolbox #831: after 267 and 268 basket door holds nothing."),
    ]
    .into_iter()
    .map(|(f, n)| (f.to_string(), n.to_string()))
    .collect()
}
