//! Beta.6 package facts: a refused BEEF graph at the create door (toolbox
//! #546, bsv-wallet-cli#386).
use super::recorder::{post, Recorder};
use super::worlds::{parent_tx, FEE_PUZZLE_1_OUT, P2PKH, PUZZLE};
use serde_json::json;

pub(crate) async fn record(r: &mut Recorder) {
    // An unproven transaction whose parent the BEEF does not carry and the
    // store does not hold: the toolbox's assembly refuses the graph as
    // `ValidationError` (WalletRefusal InvalidParameter), served 400
    // VALIDATION_ERROR by its class; since toolbox 0.14.0-beta.6 the text
    // ends at the byte count, no dump path (#546).
    let absent = parent_tx(0xe7, PUZZLE.2 + 100, &hex::decode("5187").unwrap());
    let mut child = bsv_sdk::transaction::Transaction::new();
    child
        .add_input(bsv_sdk::transaction::TransactionInput::new(absent.id(), 0))
        .unwrap();
    child
        .add_output(bsv_sdk::transaction::TransactionOutput::new(
            PUZZLE.2,
            bsv_sdk::script::LockingScript::from_binary(&hex::decode("5187").unwrap()).unwrap(),
        ))
        .unwrap();
    let mut beef = bsv_sdk::transaction::Beef::new();
    beef.merge_transaction(child.clone());
    let refused = r
        .case(
            "createAction.refusedGraph",
            "funded",
            post(
                "/createAction",
                json!({"description": "wire refused graph", "inputBEEF": beef.to_binary(),
                    "inputs": [{"outpoint": format!("{}.0", child.id()), "unlockingScriptLength": 1,
                        "inputDescription": "an orphan"}],
                    "outputs": [{"lockingScript": P2PKH, "satoshis": PUZZLE.2 - FEE_PUZZLE_1_OUT,
                        "outputDescription": "an output"}],
                    "options": {"signAndProcess": false, "randomizeOutputs": false}}),
            ),
        )
        .await;
    assert_eq!(refused.value("/code"), "VALIDATION_ERROR");
}
