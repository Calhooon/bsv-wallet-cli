//! Beta.6 package facts: a refused BEEF graph at the create door (toolbox
//! #546, bsv-wallet-cli#386).
use super::cases_spending::caller_input_template;
use super::recorder::{post, Recorder};
use super::worlds::{parent_tx, PUZZLE};

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
                caller_input_template("wire refused graph", &beef.to_binary(), &child, "an orphan"),
            ),
        )
        .await;
    assert_eq!(refused.value("/code"), "VALIDATION_ERROR");
}
