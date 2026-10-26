//! #237 through the served handler: the caller's signer is read at the gate.

use super::{at_the_door, mined_coin, served_response, storage_for, store_state, stored_rows};
use crate::test_support::{mock_services, serve_router};
use axum::{body::Body, http::Request, Router};
use bsv_sdk::{
    primitives::PrivateKey,
    script::LockingScript,
    transaction::{Beef, MerklePath, Transaction, TransactionInput, TransactionOutput},
};
use bsv_wallet_toolbox::{Services, StorageSqlx, Wallet};
use serde_json::{json, Value};
use std::sync::{Arc, Mutex};
use tower::ServiceExt;

struct CarriedWorld {
    wallet: Arc<Wallet<StorageSqlx, Services>>,
    calls: Arc<Mutex<Vec<String>>>,
    mock: tokio::task::JoinHandle<()>,
    body: Value,
    own: String,
}

impl Drop for CarriedWorld {
    fn drop(&mut self) {
        self.mock.abort();
    }
}

/// The wire harness's one-byte puzzle, with a mined funding coin beside it.
async fn world() -> CarriedWorld {
    let mut puzzle = Transaction::new();
    puzzle
        .add_input(TransactionInput::new("e4".repeat(32), 0))
        .unwrap();
    puzzle
        .add_output(TransactionOutput::new(
            5_000,
            LockingScript::from_hex("5187").unwrap(),
        ))
        .unwrap();
    let mut beef = Beef::new();
    beef.merge_bump(MerklePath::from_coinbase_txid(&puzzle.id(), 801));
    beef.merge_transaction(puzzle.clone());
    let key = PrivateKey::random();
    let (storage, user, basket) = storage_for(&key.public_key().to_hex()).await;
    let (own, _) = mined_coin(&storage, (user, basket), 0xd1, 50_000, 800).await;
    let roots = [(800, own.clone()), (801, puzzle.id())]
        .into_iter()
        .collect();
    let calls = Arc::new(Mutex::new(Vec::new()));
    let (url, mock) = headers(roots, calls.clone()).await;
    let services = mock_services(&url);
    let wallet = Arc::new(Wallet::new(Some(key), storage, services).await.unwrap());
    let body = json!({"description": "carried signer", "inputBEEF": beef.to_binary(),
        "inputs": [{"outpoint": format!("{}.0", puzzle.id()), "inputDescription": "the puzzle"}],
        "outputs": [{"lockingScript": "76a914dbc0a7c84983c5bf199b7b2d41b3acf0408ee5aa88ac",
            "satoshis": 4991, "outputDescription": "an output"}],
        "options": {"signAndProcess": false, "randomizeOutputs": false}});
    CarriedWorld {
        wallet,
        calls,
        mock,
        body,
        own,
    }
}

/// A local header service with a call log; no request needs a remote service.
async fn headers(
    roots: std::collections::HashMap<u32, String>,
    calls: Arc<Mutex<Vec<String>>>,
) -> (String, tokio::task::JoinHandle<()>) {
    let app = Router::new().fallback(move |uri: axum::http::Uri| {
        let calls = calls.clone();
        let roots = roots.clone();
        async move {
            calls.lock().unwrap().push(uri.to_string());
            let height = uri
                .query()
                .and_then(|q| q.strip_prefix("height="))
                .and_then(|h| h.parse().ok())
                .unwrap_or(900);
            axum::Json(json!({"status": "success", "value": {
                "version": 536_870_912, "previousHash": "00".repeat(32),
                "merkleRoot": roots.get(&height).cloned().unwrap_or_else(|| "0c".repeat(32)),
                "time": 1_700_000_000, "bits": 402_917_821_u32, "nonce": 7,
                "height": height, "hash": format!("{height:064x}")
            }}))
        }
    });
    serve_router(app).await
}

async fn serve(w: &CarriedWorld, body: Value) -> (u16, Value, bool) {
    let app = crate::server::make_router(w.wallet.clone(), crate::server::ServerConfig::default());
    let request = Request::post("/createAction")
        .header("content-type", "application/json")
        .header("origin", "http://gate.local")
        .body(Body::from(body.to_string()))
        .unwrap();
    let (response, asked) = at_the_door(app.oneshot(request)).await;
    let (status, body) = served_response(response.unwrap()).await;
    (status, body, asked)
}

fn request(w: &CarriedWorld, fields: &Value) -> Value {
    let mut body = w.body.clone();
    body["inputs"][0]
        .as_object_mut()
        .unwrap()
        .extend(fields.as_object().unwrap().clone());
    body
}

/// Also put an owned named coin before the bad carried input: no creator read.
async fn refused(fields: Value, why: &str) {
    refused_at_door(fields, why, true).await;
}

async fn refused_at_door(fields: Value, why: &str, at_gate: bool) {
    let w = world().await;
    let before = store_state(w.wallet.storage()).await;
    let rows = stored_rows(w.wallet.storage()).await;
    for own_before in [false, true] {
        let mut body = request(&w, &fields);
        let outpoint = body["inputs"][0]["outpoint"].as_str().unwrap().to_string();
        if own_before {
            body["inputs"].as_array_mut().unwrap().insert(
                0,
                json!({
                "outpoint": format!("{}.0", w.own), "unlockingScriptLength": 107}),
            );
        }
        let (status, answer, asked) = serve(&w, body).await;
        println!("carried refusal: HTTP {status} {answer}; toolbox called: {asked}");
        assert_eq!(status, 400);
        let message = if at_gate {
            format!("Invalid argument: inputs names {outpoint}, which is not a valid caller input: {why}")
        } else {
            why.to_string()
        };
        assert_eq!(
            answer,
            json!({"code": "VALIDATION_ERROR", "message": message})
        );
        assert!(!asked, "the toolbox was called");
        assert!(w.calls.lock().unwrap().is_empty(), "a creator was read");
        assert_eq!(
            stored_rows(w.wallet.storage()).await,
            rows,
            "nothing stored"
        );
        assert_eq!(
            store_state(w.wallet.storage()).await,
            before,
            "no allocation or sequence change"
        );
    }
}

#[tokio::test]
async fn a_carried_input_with_no_signer_is_refused_by_the_served_gate() {
    refused(json!({}), "names no signer: The unlockingScript, unlockingScriptLength parameter must be at least one valid value.").await;
}

#[tokio::test]
async fn a_carried_input_with_a_mismatched_length_is_refused_by_the_served_gate() {
    refused(
        json!({"unlockingScript": "51", "unlockingScriptLength": 2}),
        "The unlockingScriptLength parameter must be length unlockingScript if both valid.",
    )
    .await;
}

async fn reaches_toolbox(fields: Value) {
    let w = world().await;
    let (status, answer, asked) = serve(&w, request(&w, &fields)).await;
    assert_eq!(status, 200, "{answer}");
    assert!(asked, "the toolbox was not called");
    assert!(answer["signableTransaction"]["tx"].is_array(), "{answer}");
    assert!(
        !w.calls.lock().unwrap().is_empty(),
        "the toolbox checks the BEEF"
    );
}

#[tokio::test]
async fn a_carried_input_with_only_a_length_reaches_the_served_toolbox() {
    reaches_toolbox(json!({"unlockingScriptLength": 1})).await;
}

#[tokio::test]
async fn a_carried_input_with_a_script_reaches_the_served_toolbox() {
    reaches_toolbox(json!({"unlockingScript": "51"})).await;
    reaches_toolbox(json!({"unlockingScript": "51", "unlockingScriptLength": 1})).await;
}

#[tokio::test]
async fn a_carried_input_with_an_empty_script_is_refused_by_the_served_gate() {
    refused(
        json!({"unlockingScript": ""}),
        "The unlockingScript parameter must be hexadecimal string.",
    )
    .await;
}

#[tokio::test]
async fn a_carried_input_with_an_empty_script_and_a_length_is_refused_by_the_served_gate() {
    for length in [0, 1] {
        refused(
            json!({"unlockingScript": "", "unlockingScriptLength": length}),
            "The unlockingScript parameter must be hexadecimal string.",
        )
        .await;
    }
}

#[tokio::test]
async fn a_carried_input_with_zero_length_and_no_script_is_refused_by_the_served_gate() {
    for fields in [
        json!({"unlockingScriptLength": 0}),
        json!({"unlockingScript": null, "unlockingScriptLength": 0}),
    ] {
        refused(
            fields,
            "The unlockingScriptLength parameter must be a positive integer.",
        )
        .await;
    }
}

#[tokio::test]
async fn a_carried_input_treats_null_signer_fields_as_absent() {
    refused(json!({"unlockingScript": null, "unlockingScriptLength": null}),
        "names no signer: The unlockingScript, unlockingScriptLength parameter must be at least one valid value.").await;
    reaches_toolbox(json!({"unlockingScript": null, "unlockingScriptLength": 1})).await;
    reaches_toolbox(json!({"unlockingScript": "51", "unlockingScriptLength": null})).await;
}

#[tokio::test]
async fn a_carried_input_with_a_non_hex_script_is_refused_by_the_served_handler() {
    for script in ["zz", "5g"] {
        refused_at_door(
            json!({"unlockingScript": script}),
            "The unlockingScript parameter must be hexadecimal string.",
            false,
        )
        .await;
    }
}

#[tokio::test]
async fn a_carried_input_with_an_odd_length_script_is_refused_by_the_served_handler() {
    for (script, length) in [("5", 1), (" 5 ", 1), ("51g", 3)] {
        refused_at_door(
            json!({"unlockingScript": script}),
            &format!("The unlockingScript parameter must be even length, not {length}."),
            false,
        )
        .await;
    }
}

#[tokio::test]
async fn a_carried_input_with_a_padded_script_reaches_the_served_toolbox() {
    reaches_toolbox(json!({"unlockingScript": " 51 "})).await;
    reaches_toolbox(json!({"unlockingScript": " 51 ", "unlockingScriptLength": 1})).await;
}

#[tokio::test]
async fn a_carried_input_with_a_whitespace_script_is_refused_by_the_served_gate() {
    refused(
        json!({"unlockingScript": "  "}),
        "The unlockingScript parameter must be hexadecimal string.",
    )
    .await;
}
