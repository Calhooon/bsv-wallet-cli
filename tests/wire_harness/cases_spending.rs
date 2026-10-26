//! The cases of the spend gate's refusals, the store's faults and the funded
//! world's spends, payments and listings.

use bsv_sdk::primitives::PrivateKey;

use super::cases::exact;
use super::recorder::{get, post, Recorded, Recorder};
use super::worlds::{
    funded_lock, parent_tx, puzzle_parent, stranger_key, stranger_payment, Payment,
    FEE_PUZZLE_1_OUT, FROZEN, FUNDED_COINS, MINED, P2PKH, PAYMENT_PREFIX, PAYMENT_SUFFIX, PENDING,
    PUZZLE, PUZZLE_UNLOCK, RECEIVED,
};
use crate::wire_support::{BOUND, ROOT_KEY};

/// #157: record the internalize-only byte bound after every existing case.
pub(crate) async fn record_internalize_bound(r: &mut Recorder) {
    record_internalize_instructions(
        r,
        "internalizeAction.customInstructionsBound",
        format!("{}a", "🦀".repeat(250)),
    )
    .await;
}

async fn record_internalize_instructions(
    r: &mut Recorder,
    name: &str,
    instructions: String,
) -> Recorded {
    use serde_json::json;
    let key = PrivateKey::from_hex(ROOT_KEY).unwrap();
    let (_, atomic) = stranger_payment(Payment::Insertion, &key).await;
    r.case(
        name,
        "funded",
        post(
            "/internalizeAction",
            json!({"tx": atomic, "outputs": [{"outputIndex": 0, "protocol": "basket insertion",
                "insertionRemittance": {"basket": "instructions-bound",
                    "customInstructions": instructions}}],
                "description": "wire internalize instructions bound"}),
        ),
    )
    .await
}

/// The stack's internalize-bound scenario, steps 3 and 4: accept at the
/// 1,000-byte bound, then read the whole stored string through `listOutputs`.
pub(crate) async fn record_internalize_at_bound(r: &mut Recorder) {
    let instructions = "🦀".repeat(250);
    assert_eq!(instructions.len(), 1000);
    let accepted = record_internalize_instructions(
        r,
        "internalizeAction.customInstructionsAtBound",
        instructions.clone(),
    )
    .await;
    assert_eq!(accepted.value("/accepted"), true);
    let list = post(
        "/listOutputs",
        serde_json::json!({"basket": "instructions-bound", "includeCustomInstructions": true}),
    );
    let (response, body) = r.world("funded").await.serve_raw(&list).await;
    assert_eq!(response.status, 200);
    let body = body.unwrap();
    assert_eq!(body.pointer("/totalOutputs").unwrap().compact(), "1");
    assert_eq!(
        body.pointer("/outputs/0/customInstructions")
            .unwrap()
            .as_str(),
        Some(instructions.as_str()),
    );
    println!("internalizeAction.customInstructionsAtBound: HTTP 200, stored 1000 UTF-8 bytes");
}

/// The spend gate's refusals.
pub(crate) async fn record_gate_refusals(r: &mut Recorder) {
    use serde_json::json;
    r.case(
        "createAction.inputNotOffered",
        "tracked",
        post(
            "/createAction",
            json!({"description": "wire frozen coin", "allowPending": "received",
                "inputs": [{"outpoint": format!("{FROZEN}.0"), "unlockingScriptLength": 107, "inputDescription": "named"}],
                "outputs": [{"lockingScript": P2PKH, "satoshis": 1000, "outputDescription": "an output"}]}),
        ),
    )
    .await;
    r.case(
        "createAction.headersUnavailable",
        "headers-down",
        post(
            "/createAction",
            json!({"description": "wire headers down", "outputs": [
                {"lockingScript": P2PKH, "satoshis": 1000, "outputDescription": "an output"}]}),
        ),
    )
    .await;
    r.case("getHeight.headersDown", "headers-down", get("/getHeight"))
        .await;
    r.case(
        "createAction.unspendableCoin",
        "received",
        post(
            "/createAction",
            json!({"description": "wire incomplete ancestry", "outputs": [
                {"lockingScript": P2PKH, "satoshis": 1000, "outputDescription": "an output"}]}),
        ),
    )
    .await;
    // The received coin's creator is completed (a broadcaster took it): an
    // abort by its reference is refused.
    r.case(
        "abortAction.broadcasterTook",
        "received",
        post(
            "/abortAction",
            json!({"reference": format!("ref-{}", &RECEIVED[..8])}),
        ),
    )
    .await;
}

/// A store fault, a store busy, and the host routes' faults.
pub(crate) async fn record_store_faults(r: &mut Recorder) {
    use serde_json::json;
    r.case(
        "createAction.storeFault",
        "broken",
        post(
            "/createAction",
            json!({"description": "wire store fault", "outputs": [
                {"lockingScript": P2PKH, "satoshis": 1000, "outputDescription": "an output"}]}),
        ),
    )
    .await;
    r.case(
        "listOutputs.storeFault",
        "broken",
        post("/listOutputs", json!({"basket": "default"})),
    )
    .await;
    r.case(
        "createAction.storeBusy",
        "busy",
        post(
            "/createAction",
            json!({"description": "wire busy", "outputs": [
                {"lockingScript": P2PKH, "satoshis": 1000, "outputDescription": "an output"}]}),
        ),
    )
    .await;
    r.case("chain-feed.hostFault", "broken-host", get("/chain-feed"))
        .await;
    r.case("tx.hostFault", "broken-host", get(&format!("/tx/{MINED}")))
        .await;
}

/// abortAction under a store no reader can open (#168): its pre-check's
/// read is the store busy, 503 `STORE_BUSY`. Recorded last, so no file
/// before it moves.
pub(crate) async fn record_abort_store_busy(r: &mut Recorder) {
    use serde_json::json;
    r.case(
        "abortAction.storeBusy",
        "busy-read",
        post(
            "/abortAction",
            json!({"reference": "bm8tc3VjaC1yZWZlcmVuY2U="}),
        ),
    )
    .await;
}

/// Our own change passes the named gate under its rule, then the toolbox
/// refuses the caller's allocation of managed change (#234). Recorded
/// last so the existing corpus stays byte-identical.
pub(crate) async fn record_named_change(r: &mut Recorder) {
    use serde_json::json;
    r.case(
        "createAction.namedChange",
        "named-change",
        post(
            "/createAction",
            json!({"description": "wire named change",
                "inputs": [{"outpoint": format!("{PENDING}.0"), "unlockingScriptLength": 107,
                    "inputDescription": "our change"}],
                "outputs": [{"lockingScript": P2PKH, "satoshis": 1000,
                    "outputDescription": "an output"}]}),
        ),
    )
    .await;
}

/// #237: both carried-input signer refusals, recorded after the existing set.
pub(crate) async fn record_carried_signer_refusals(r: &mut Recorder) {
    use serde_json::json;
    for (name, signer) in [
        ("createAction.callerInputNoSigner", json!({})),
        (
            "createAction.callerInputLengthMismatch",
            json!({"unlockingScript": PUZZLE_UNLOCK, "unlockingScriptLength": 2}),
        ),
    ] {
        record_carried_signer_case(r, name, signer).await;
    }
}

/// #265: empty and non-hex scripts, appended after the pinned corpus.
pub(crate) async fn record_empty_signer_refusals(r: &mut Recorder) {
    for (name, script) in [
        ("createAction.callerInputEmptyScript", ""),
        ("createAction.callerInputNonHexScript", "zz"),
    ] {
        record_carried_signer_case(r, name, serde_json::json!({"unlockingScript": script})).await;
    }
}

async fn record_carried_signer_case(r: &mut Recorder, name: &str, signer: serde_json::Value) {
    use serde_json::json;
    let (puzzle, input_beef) = puzzle_parent();
    let mut input = json!({"outpoint": format!("{}.0", puzzle.id()),
        "inputDescription": "the puzzle"});
    input
        .as_object_mut()
        .unwrap()
        .extend(signer.as_object().unwrap().clone());
    r.case(
        name,
        "funded",
        post(
            "/createAction",
            json!({"description": "wire caller input", "inputBEEF": input_beef,
                "inputs": [input],
                "outputs": [{"lockingScript": P2PKH, "satoshis": PUZZLE.2 - FEE_PUZZLE_1_OUT,
                    "outputDescription": "an output"}],
                "options": {"signAndProcess": false, "randomizeOutputs": false}}),
        ),
    )
    .await;
}

/// The funded world: a 200 body from each spending or state-changing
/// call. Each action asks for exactly its coin less the fee, so the
/// toolbox builds no change output (whose derivation it draws at
/// random): the built bytes, the txid and the signature are the same
/// on every run. The template's reference is random: the cases that
/// send it back bind it to the served body.
pub(crate) async fn record_funded_sign(r: &mut Recorder) {
    use serde_json::json;
    r.case(
        "createAction.noSend",
        "funded",
        post(
            "/createAction",
            json!({"description": "wire no send", "outputs": [
                {"lockingScript": P2PKH, "satoshis": exact(0), "outputDescription": "an output"}],
                "options": {"noSend": true, "randomizeOutputs": false}}),
        ),
    )
    .await;
    let template = r
        .case(
            "createAction.template",
            "funded",
            post(
                "/createAction",
                json!({"description": "wire template", "outputs": [
                    {"lockingScript": P2PKH, "satoshis": exact(1), "outputDescription": "an output"}],
                    "options": {"signAndProcess": false, "randomizeOutputs": false}}),
            ),
        )
        .await;
    r.case(
        "signAction.ok",
        "funded",
        post(
            "/signAction",
            json!({"spends": {}, "reference": BOUND, "options": {"noSend": true}}),
        )
        .bind("/reference", &template, "/signableTransaction/reference"),
    )
    .await;
}

/// A template aborted, and an action in the default mode (signed and
/// posted at once) on the coin the abort released.
pub(crate) async fn record_funded_abort_and_broadcast(r: &mut Recorder) {
    use serde_json::json;
    let to_abort = r
        .case(
            "createAction.templateToAbort",
            "funded",
            post(
                "/createAction",
                json!({"description": "wire template to abort", "outputs": [
                    {"lockingScript": P2PKH, "satoshis": exact(2), "outputDescription": "an output"}],
                    "options": {"signAndProcess": false, "randomizeOutputs": false}}),
            ),
        )
        .await;
    r.case(
        "abortAction.ok",
        "funded",
        post("/abortAction", json!({"reference": BOUND})).bind(
            "/reference",
            &to_abort,
            "/signableTransaction/reference",
        ),
    )
    .await;
    // The default mode, as x402 asks it: signed and posted at once (the
    // broadcaster takes it), on the coin the abort released.
    r.case(
        "createAction.broadcast",
        "funded",
        post(
            "/createAction",
            json!({"description": "wire broadcast", "outputs": [
                {"lockingScript": P2PKH, "satoshis": exact(2), "outputDescription": "an output"}],
                "options": {"acceptDelayedBroadcast": false, "randomizeOutputs": false}}),
        ),
    )
    .await;
}

/// A coin relinquished, and a template over the caller's own input.
pub(crate) async fn record_funded_relinquish_and_caller_input(r: &mut Recorder) {
    use serde_json::json;
    let key = PrivateKey::from_hex(ROOT_KEY).unwrap();
    let (seed, _, satoshis, suffix) = FUNDED_COINS[3];
    let fourth = parent_tx(seed, satoshis, &funded_lock(&key, suffix)).id();
    r.case(
        "relinquishOutput.ok",
        "funded",
        post(
            "/relinquishOutput",
            json!({"basket": "default", "output": format!("{fourth}.0")}),
        ),
    )
    .await;
    // A template over the caller's own input (the puzzle, carried in
    // `inputBEEF`), its unlocking script supplied in `signAction`'s spends.
    // The keeper's reading (#166, 2026-10-24): a caller-carried input whose whole transaction `inputBEEF` carries, signed by the caller, is the toolbox's BEEF verification and not the gate's question, so this case stays a 200.
    let (puzzle, input_beef) = puzzle_parent();
    let puzzle_template = r
        .case(
            "createAction.callerInput",
            "funded",
            post(
                "/createAction",
                json!({"description": "wire caller input", "inputBEEF": input_beef,
                    "inputs": [{"outpoint": format!("{}.0", puzzle.id()), "unlockingScriptLength": 1,
                        "inputDescription": "the puzzle"}],
                    "outputs": [{"lockingScript": P2PKH, "satoshis": PUZZLE.2 - FEE_PUZZLE_1_OUT,
                        "outputDescription": "an output"}],
                    "options": {"signAndProcess": false, "randomizeOutputs": false}}),
            ),
        )
        .await;
    r.case(
        "signAction.spends",
        "funded",
        post(
            "/signAction",
            json!({"spends": {"0": {"unlockingScript": PUZZLE_UNLOCK}}, "reference": BOUND,
                "options": {"noSend": true}}),
        )
        .bind(
            "/reference",
            &puzzle_template,
            "/signableTransaction/reference",
        ),
    )
    .await;
    // The stranger's payments: a valid spend inserted into a basket, the
    // same with an unlocking script that fails (accepted: a known defect,
    // recorded as served, docs/WIRE.md), and a BRC-29 payment to the
    // wallet (x402's refund).
}

/// The stranger's payments, and what the funded wallet lists after.
pub(crate) async fn record_payments_and_listings(r: &mut Recorder) {
    use serde_json::json;
    let key = PrivateKey::from_hex(ROOT_KEY).unwrap();
    let (_, atomic) = stranger_payment(Payment::Insertion, &key).await;
    r.case(
        "internalizeAction.basketInsertion",
        "funded",
        post(
            "/internalizeAction",
            json!({"tx": atomic, "outputs": [{"outputIndex": 0, "protocol": "basket insertion",
                "insertionRemittance": {"basket": "wire"}}],
                "description": "wire stranger payment"}),
        ),
    )
    .await;
    let (_, atomic) = stranger_payment(Payment::BadUnlock, &key).await;
    r.case(
        "internalizeAction.badUnlock",
        "funded",
        post(
            "/internalizeAction",
            json!({"tx": atomic, "outputs": [{"outputIndex": 0, "protocol": "basket insertion",
                "insertionRemittance": {"basket": "wire"}}],
                "description": "wire bad unlock"}),
        ),
    )
    .await;
    let (_, atomic) = stranger_payment(Payment::Wallet, &key).await;
    r.case(
        "internalizeAction.walletPayment",
        "funded",
        post(
            "/internalizeAction",
            json!({"tx": atomic, "outputs": [{"outputIndex": 0, "protocol": "wallet payment",
                "paymentRemittance": {"derivationPrefix": PAYMENT_PREFIX, "derivationSuffix": PAYMENT_SUFFIX,
                    "senderIdentityKey": stranger_key().public_key().to_hex()}}],
                "description": "wire payment"}),
        ),
    )
    .await;
    r.case(
        "listOutputs.funded",
        "funded",
        post("/listOutputs", json!({"basket": "default"})),
    )
    .await;
    r.case(
        "listActions.funded",
        "funded",
        post("/listActions", json!({"labels": []})),
    )
    .await;
}
