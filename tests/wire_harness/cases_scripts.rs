//! The door's script answers at `POST /internalizeAction` (the port of the
//! stable line's 0.18.9, #634), in the `funded` world: the toolbox runs the
//! unlocking script of every input of every transaction of a received
//! AtomicBEEF that the BEEF does not prove, and refuses one that does not
//! pass, and a spend of a coin the BEEF proves before Genesis is not
//! judged. A failing script in the unproven subject is 103's answer, a
//! passing payment 102's and 104's. Here: a failing script in an unproven
//! ancestor, a coin proven before Genesis, a held transaction handed again
//! unproven (refused) and proven (taken as before), the passing shape
//! through two unmined parents, and the basket after them (nothing stored
//! by a refusal). Recorded last, so no file before them moves.

use bsv_sdk::primitives::PrivateKey;
use serde_json::json;

use super::payments::{held_repeat, stranger_payment, Payment};
use super::recorder::{post, Recorder};
use crate::wire_support::ROOT_KEY;

const WORLD: &str = "funded";
/// The basket the rows insert into.
const BASKET: &str = "wire scripts";

fn insertion(atomic: &[u8], description: &str) -> serde_json::Value {
    json!({"tx": atomic, "outputs": [{"outputIndex": 0, "protocol": "basket insertion",
        "insertionRemittance": {"basket": BASKET}}],
        "description": description})
}

pub(crate) async fn record(r: &mut Recorder) {
    let key = PrivateKey::from_hex(ROOT_KEY).unwrap();
    let (_, atomic) = stranger_payment(Payment::AncestorBadUnlock, &key).await;
    r.case(
        "internalizeAction.scripts.ancestorBadUnlock",
        WORLD,
        post(
            "/internalizeAction",
            insertion(&atomic, "wire ancestor bad unlock"),
        ),
    )
    .await;
    let (_, atomic) = stranger_payment(Payment::PreGenesis, &key).await;
    r.case(
        "internalizeAction.scripts.preGenesisCoin",
        WORLD,
        post("/internalizeAction", insertion(&atomic, "wire pre-Genesis")),
    )
    .await;
    let (proven, _) = held_repeat(&key).await;
    r.case(
        "internalizeAction.scripts.heldProven",
        WORLD,
        post("/internalizeAction", insertion(&proven, "wire held")),
    )
    .await;
    let (_, unproven) = stranger_payment(Payment::HeldBadUnlock, &key).await;
    r.case(
        "internalizeAction.scripts.heldRepeatUnproven",
        WORLD,
        post("/internalizeAction", insertion(&unproven, "wire held")),
    )
    .await;
    r.case(
        "internalizeAction.scripts.heldRepeatProven",
        WORLD,
        post("/internalizeAction", insertion(&proven, "wire held")),
    )
    .await;
    let (_, atomic) = stranger_payment(Payment::AncestorsPass, &key).await;
    r.case(
        "internalizeAction.scripts.ancestorsPass",
        WORLD,
        post(
            "/internalizeAction",
            insertion(&atomic, "wire two unmined parents"),
        ),
    )
    .await;
    r.case(
        "listOutputs.scripts.basket",
        WORLD,
        post("/listOutputs", json!({"basket": BASKET})),
    )
    .await;
}

pub(crate) fn notes() -> Vec<(String, String)> {
    [
        ("284-internalizeAction.scripts.ancestorBadUnlock.json", "#634 (the port of the stable line's 0.18.9): a payment whose unproven ancestor's input does not pass (the placeholder 00 against the stranger's P2PKH), its subject a valid spend of that ancestor: 400 VALIDATION_ERROR, spend refused at its offset, input 0 of the ancestor, OP_EQUALVERIFY; nothing stored (290). The subject's own failing input is 103. New; recorded last, so no file before it moves."),
        ("285-internalizeAction.scripts.preGenesisCoin.json", "#634: a valid spend of a coin its BEEF proves at 600,000 (the header stub answers its root): 400 VALIDATION_ERROR, input 0 not judged: coin height 600000 is before Genesis height 620538; nothing stored (290)."),
        ("286-internalizeAction.scripts.heldProven.json", "#634: a spend whose input is the placeholder 00, carrying its own BUMP: a mined transaction is taken without its scripts run, 200, stored (290)."),
        ("287-internalizeAction.scripts.heldRepeatUnproven.json", "#634: 286's transaction handed again unproven, beside its mined parent: checked the same way, 400 VALIDATION_ERROR, OP_EQUALVERIFY, input 0 of the held transaction; the row as it was (290)."),
        ("288-internalizeAction.scripts.heldRepeatProven.json", "#634: 286's proven BEEF again: answered as before, 200."),
        ("289-internalizeAction.scripts.ancestorsPass.json", "#634: a payment through two unmined parents, every input a valid signature, the oldest spending a parent mined after Genesis: 200, stored (290)."),
        ("290-listOutputs.scripts.basket.json", "#634: the basket after 284 to 289: 286's output and 289's alone; the refusals stored nothing."),
    ]
    .into_iter()
    .map(|(f, n)| (f.to_string(), n.to_string()))
    .collect()
}
