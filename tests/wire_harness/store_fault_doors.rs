//! #122 item 4's witness: one store fault, one answer, at the spend guard
//! and at the wallet calls. Run by `cargo test --test wire`.

use std::collections::BTreeSet;

use super::worlds::World;
use crate::wire_support::on_runtime;

/// #122 item 4: one store fault is one class at every door. The `broken`
/// world's store has lost its `outputs` table; the spend guard reads it
/// before `createAction`'s gate (a pre-door fault), and the wallet calls
/// read it through the toolbox (a `WalletFault` of the storage family,
/// bsv-rs #439). Each answers 500 `INTERNAL_ERROR` with the store's own
/// words; none is a 400 `ERROR` nor the caller's.
#[test]
fn one_store_fault_is_one_answer_at_the_guard_and_at_a_wallet_call() {
    on_runtime(|| async {
        use super::recorder::post;
        let world = World::build("broken").await;
        let output = serde_json::json!({
            "lockingScript": super::worlds::P2PKH,
            "satoshis": 1000,
            "outputDescription": "an output"
        });
        let doors = [
            (
                "the spend guard",
                post(
                    "/createAction",
                    serde_json::json!({"description": "one store fault", "outputs": [output]}),
                ),
            ),
            (
                "listOutputs",
                post("/listOutputs", serde_json::json!({"basket": "default"})),
            ),
            (
                "relinquishOutput",
                post(
                    "/relinquishOutput",
                    serde_json::json!({"basket": "default", "output": format!("{}.0", "ab".repeat(32))}),
                ),
            ),
        ];
        let mut answers = BTreeSet::new();
        for (door, request) in doors {
            let got = world.serve_raw(&request).await.0;
            let body = serde_json::to_value(got.json.expect("a JSON refusal")).unwrap();
            let message = body["message"].as_str().unwrap_or_default().to_string();
            assert!(
                message.contains("no such table: outputs"),
                "{door}: the store's own words: {message}"
            );
            answers.insert((got.status, body["code"].as_str().unwrap().to_string()));
        }
        assert_eq!(
            answers,
            BTreeSet::from([(500, "INTERNAL_ERROR".to_string())]),
            "one store fault, one answer"
        );
    });
}
