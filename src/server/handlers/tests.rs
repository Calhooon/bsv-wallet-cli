//! The answer's rule (#44): the store busy is 503 `STORE_BUSY` with
//! `Retry-After: 1` by its type only; every other fault keeps its own word
//! and status.

use super::*;
use bsv_wallet_toolbox::WalletStorageWriter;

async fn answer(e: AppError) -> (StatusCode, Option<String>, serde_json::Value) {
    let response = e.into_response();
    let status = response.status();
    let retry = response
        .headers()
        .get(axum::http::header::RETRY_AFTER)
        .map(|v| v.to_str().unwrap().to_string());
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    (status, retry, serde_json::from_slice(&bytes).unwrap())
}

/// Every input the classifier reads, the old belt's busy words among them:
/// none answers 503 by its words, and the word STORE_BUSY is in no 4xx
/// body.
#[tokio::test]
async fn no_message_is_store_busy_by_its_words_and_no_4xx_says_it() {
    let inputs = [
        // The belt's words of e0ff758: by words alone they are not busy.
        "Store busy: sqlite code 5",
        "wallet error: Store busy: sqlite code 517",
        "Storage error: spend guard: Store busy: sqlite code 6",
        "wallet error: SQLx error: error returned from database: (code: 517) database is locked",
        "database is locked (code 517)",
        "Database error: database table is locked: outputs (code 6)",
        // Every prefix the classifier maps.
        "Insufficient funds: need 30005, have 0",
        "insufficient funds: need 1",
        "Authentication required",
        "Invalid identity key: 02",
        "Access denied: an input the spend guard does not offer",
        "Entity not found: transaction",
        "not found: output",
        "Duplicate entity: label",
        "Invalid operation: the trait's default cannot enforce an offer",
        "Unspendable coin: aa.0: its stored ancestry is incomplete",
        "Invalid BEEF at byte 9: TrailingBytes",
        "Validation error: inputBEEF: invalid merkle root",
        "Invalid argument: description",
        "Origin header required",
        "Storage error: spend gate: no such table",
        "Database error: no such table: outputs",
        "Internal error: x",
        "Service error: the header service was not read for the merkle root r at height 9 before the write lock, twice; nothing written",
        "Network error: refused",
        "Broadcast failed: x",
        "SQLx error: error returned from database: (code: 19) UNIQUE constraint failed",
        "anything else",
    ];
    for msg in inputs {
        for wrapped in [msg.to_string(), format!("wallet error: {msg}")] {
            let (status, retry, body) = answer(AppError::from_wallet_error(wrapped.clone())).await;
            assert_ne!(status, StatusCode::SERVICE_UNAVAILABLE, "{wrapped}");
            assert_ne!(body["code"], "STORE_BUSY", "{wrapped}");
            assert!(retry.is_none(), "{wrapped}");
            if status.is_client_error() {
                assert!(
                    !body.to_string().contains("STORE_BUSY"),
                    "{wrapped}: {body}"
                );
            }
        }
    }
    // The same through the SDK's own error, untyped.
    for msg in inputs {
        let (status, _, body) = answer(AppError::from_wallet_error(bsv_sdk::Error::WalletError(
            msg.to_string(),
        )))
        .await;
        assert_ne!(status, StatusCode::SERVICE_UNAVAILABLE, "{msg}");
        if status.is_client_error() {
            assert!(!body.to_string().contains("STORE_BUSY"), "{msg}: {body}");
        }
    }
}

/// The toolbox's typed busy from the door: 503, `Retry-After: 1`, the body
/// exactly `{code, message}` with the error's Display.
#[tokio::test]
async fn the_typed_wallet_busy_from_the_door_is_503_retry_after_one() {
    let busy = bsv_sdk::Error::WalletBusy {
        detail: "Store busy: sqlite code 517".to_string(),
        retry_after_ms: None,
    };
    let display = busy.to_string();
    for e in [
        AppError::from_gate_error(crate::spend_gate::GateError::Wallet(busy.clone())),
        AppError::from_wallet_error(busy),
    ] {
        let (status, retry, body) = answer(e).await;
        assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(retry.as_deref(), Some("1"));
        assert_eq!(body, json!({"code": "STORE_BUSY", "message": display}));
    }
}

/// A pre-door gate fault: SQLite's busy or the writer's acquire ceiling is
/// 503 by its type; the toolbox's `Store busy:` words in a plain error are
/// not, nor a bare pool timeout (the reader's), and any other fault is the
/// gate's 500.
#[tokio::test]
async fn a_pre_door_fault_is_503_only_when_it_is_the_store_busy() {
    use crate::spend_gate::GateError;
    let typed = [
        anyhow::Error::new(bsv_wallet_toolbox::Error::StoreBusy {
            reason: bsv_wallet_toolbox::StoreBusyReason::WriterNotAcquired,
        }),
        anyhow::Error::new(bsv_wallet_toolbox::Error::StoreBusy {
            reason: bsv_wallet_toolbox::StoreBusyReason::SqliteBusy { code: 5 },
        }),
    ];
    for e in typed {
        let (status, retry, body) = answer(AppError::from_gate_error(GateError::Gate(e))).await;
        assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(retry.as_deref(), Some("1"));
        assert_eq!(body["code"], "STORE_BUSY");
        assert!(
            body["message"]
                .as_str()
                .unwrap()
                .starts_with("Storage error: spend gate: "),
            "{body}"
        );
    }
    for e in [
        anyhow::anyhow!("Store busy: sqlite code 5"),
        anyhow::Error::new(sqlx::Error::RowNotFound),
        // A bare pool timeout (the reader's 30 s acquire): a storage fault.
        anyhow::Error::new(sqlx::Error::PoolTimedOut),
    ] {
        let (status, retry, body) = answer(AppError::from_gate_error(GateError::Gate(e))).await;
        assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR, "{body}");
        assert_eq!(body["code"], "INTERNAL_ERROR");
        assert!(retry.is_none());
    }
    let (status, _, body) = answer(AppError::from_pre_door(
        anyhow::Error::new(bsv_wallet_toolbox::Error::StoreBusy {
            reason: bsv_wallet_toolbox::StoreBusyReason::WriterNotAcquired,
        }),
        "spend guard",
    ))
    .await;
    assert_eq!(
        (status, &body["code"]),
        (StatusCode::SERVICE_UNAVAILABLE, &json!("STORE_BUSY"))
    );
}

/// `Unspendable coin:` is the wallet's own coin, 409, the coin named;
/// `Service error:` stays 502, the shortfall 402.
#[tokio::test]
async fn the_unspendable_coin_is_409_and_names_the_coin() {
    let coin = format!("{}.1", "ab".repeat(32));
    let msg = format!(
        "wallet error: Unspendable coin: {coin}: its stored ancestry is incomplete, ancestor(s) not stored: [{}]; createAction fetches none, and the wallet's other coins do not cover the action",
        "cd".repeat(32)
    );
    let (status, retry, body) = answer(AppError::from_wallet_error(msg.clone())).await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert!(retry.is_none());
    assert_eq!(body["code"], "UNSPENDABLE_COIN");
    assert_eq!(body["coin"], coin);
    assert_eq!(body["message"], msg);

    let (status, _, body) = answer(AppError::from_wallet_error(
        "wallet error: Service error: the header service was not read",
    ))
    .await;
    assert_eq!(
        (status, &body["code"]),
        (StatusCode::BAD_GATEWAY, &json!("SERVICE_ERROR"))
    );
    let (status, _, body) = answer(AppError::from_wallet_error("Insufficient funds: need 5")).await;
    assert_eq!(
        (status, &body["code"]),
        (StatusCode::PAYMENT_REQUIRED, &json!("INSUFFICIENT_FUNDS"))
    );
}

/// CORS: a browser caller may read `Retry-After` (the router's layer).
#[tokio::test]
async fn cors_exposes_retry_after() {
    use tower::ServiceExt;
    let storage = StorageSqlx::in_memory().await.unwrap();
    let key = bsv_sdk::primitives::PrivateKey::random();
    storage
        .migrate("cors", &key.public_key().to_hex())
        .await
        .unwrap();
    storage.make_available().await.unwrap();
    let services = bsv_wallet_toolbox::Services::with_options(
        bsv_wallet_toolbox::Chain::Main,
        bsv_wallet_toolbox::ServicesOptions::mainnet(),
    )
    .unwrap();
    let wallet = Wallet::new(Some(key), storage, services).await.unwrap();
    let app = crate::server::make_router(
        crate::server::make_wallet_state(wallet),
        crate::server::ServerConfig::default(),
    );
    let response = app
        .oneshot(
            axum::http::Request::get("/isAuthenticated")
                .header("Origin", "https://app.example")
                .body(axum::body::Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let exposed = response
        .headers()
        .get(axum::http::header::ACCESS_CONTROL_EXPOSE_HEADERS)
        .map(|v| v.to_str().unwrap().to_ascii_lowercase());
    assert_eq!(exposed.as_deref(), Some("retry-after"));
}

/// F4 of the #44 CLI review: the served `internalizeAction` strips a
/// caller's copy of the gate's rule label; its other labels pass.
#[test]
fn internalize_strips_the_gate_rule_label() {
    let req: McInternalizeActionReq = serde_json::from_value(serde_json::json!({
        "tx": [1, 2, 3],
        "outputs": [],
        "description": "a payment",
        "labels": ["spend-gate:received-pending", "invoice", " Spend-Gate:received-pending"]
    }))
    .unwrap();
    let args = internalize_args(req);
    assert_eq!(args.labels, Some(vec!["invoice".to_string()]));
    let req: McInternalizeActionReq = serde_json::from_value(serde_json::json!({
        "tx": [1], "outputs": [], "description": "no labels"
    }))
    .unwrap();
    assert_eq!(internalize_args(req).labels, None);
}

/// The served `createAction` body every field of which
/// `create_action_args` maps (the one mapping the handler and the fuzz
/// target `served_json` both call), and the txid of its named input.
fn served_create_body() -> (McCreateActionReq, String) {
    let txid = "ab".repeat(32);
    let req: McCreateActionReq = serde_json::from_value(serde_json::json!({
        "description": "a send",
        "outputs": [{
            "lockingScript": "76a914", "satoshis": 1500, "outputDescription": "to them",
            "basket": "b", "tags": ["t"]
        }],
        "inputs": [{ "outpoint": format!("{txid}.3"), "unlockingScript": "00", "sequenceNumber": 7 }],
        "inputBEEF": [1, 2],
        "lockTime": 500,
        "labels": ["invoice"],
        "options": { "noSend": true, "trustSelf": "known", "signAndProcess": false },
        "allowPending": { "received": [format!("{txid}:3")] }
    }))
    .unwrap();
    (req, txid)
}

/// The served body's own fields and its outputs, the scripts decoded from
/// hex.
#[test]
fn create_action_args_maps_the_served_body() {
    let (req, _) = served_create_body();
    let (args, _) = create_action_args(req).unwrap();
    assert_eq!(args.description, "a send");
    assert_eq!(args.input_beef, Some(vec![1, 2]));
    assert_eq!(args.lock_time, Some(500));
    assert_eq!(args.version, None);
    assert_eq!(args.labels, Some(vec!["invoice".to_string()]));
    let outputs = args.outputs.unwrap();
    assert_eq!(outputs.len(), 1);
    assert_eq!(outputs[0].locking_script, vec![0x76, 0xa9, 0x14]);
    assert_eq!(outputs[0].satoshis, 1500);
    assert_eq!(outputs[0].output_description, "to them");
    assert_eq!(outputs[0].basket.as_deref(), Some("b"));
    assert_eq!(outputs[0].custom_instructions, None);
    assert_eq!(outputs[0].tags, Some(vec!["t".to_string()]));
}

/// The served body's named inputs, mapped.
#[test]
fn create_action_args_maps_the_served_inputs() {
    let (req, txid) = served_create_body();
    let (args, _) = create_action_args(req).unwrap();
    let inputs = args.inputs.unwrap();
    assert_eq!(inputs.len(), 1);
    assert_eq!(inputs[0].outpoint.to_string(), format!("{txid}.3"));
    assert_eq!(inputs[0].input_description, "input");
    assert_eq!(inputs[0].unlocking_script, Some(vec![0]));
    assert_eq!(inputs[0].unlocking_script_length, None);
    assert_eq!(inputs[0].sequence_number, Some(7));
}

/// The served body's options and `trustSelf: "known"` carried, the door's
/// `allowPending` word read.
#[test]
fn create_action_args_maps_the_served_options_and_word() {
    let (req, _) = served_create_body();
    let (args, allow_pending) = create_action_args(req).unwrap();
    let options = args.options.unwrap();
    assert_eq!(options.no_send, Some(true));
    assert_eq!(options.sign_and_process, Some(false));
    assert_eq!(options.accept_delayed_broadcast, None);
    assert_eq!(options.randomize_outputs, None);
    assert!(matches!(options.trust_self, Some(TrustSelf::Known)));
    assert!(matches!(
        allow_pending,
        crate::spend_gate::AllowPending::ReceivedOnly(_)
    ));
}

/// The bare body: no inputs is the wallet's own funding
/// (`Some(vec![])`), no options, no word.
#[test]
fn create_action_args_maps_the_bare_body() {
    let req: McCreateActionReq =
        serde_json::from_value(serde_json::json!({ "description": "bare" })).unwrap();
    let (args, allow_pending) = create_action_args(req).unwrap();
    assert_eq!(args.inputs.as_ref().map(Vec::len), Some(0));
    assert!(args.outputs.is_none());
    assert!(args.options.is_none());
    assert_eq!(allow_pending, crate::spend_gate::AllowPending::Off);
}

/// The door's refusals, in the handler's words: a scope naming an outpoint
/// that does not parse, a locking script that is not hex, an input
/// outpoint that does not parse.
#[test]
fn create_action_args_refuses_a_bad_word_hex_or_outpoint() {
    let req: McCreateActionReq = serde_json::from_value(serde_json::json!({
        "description": "bad scope", "allowPending": { "received": ["nope"] }
    }))
    .unwrap();
    let err = create_action_args(req).unwrap_err().to_string();
    assert!(err.starts_with("Invalid argument: "), "{err}");
    let req: McCreateActionReq = serde_json::from_value(serde_json::json!({
        "description": "bad hex",
        "outputs": [{ "lockingScript": "zz", "satoshis": 1, "outputDescription": "x" }]
    }))
    .unwrap();
    assert!(create_action_args(req).is_err());
    let req: McCreateActionReq = serde_json::from_value(serde_json::json!({
        "description": "bad outpoint", "inputs": [{ "outpoint": "nope" }]
    }))
    .unwrap();
    let err = create_action_args(req).unwrap_err().to_string();
    assert!(err.starts_with("invalid input outpoint 'nope'"), "{err}");
}

/// "Could not verify now" (#59): the gate's refusal on the header service's
/// fault is 503 `HEADERS_UNAVAILABLE` with `Retry-After`, its body the
/// refusal's (`offeredSatoshis`, `held`), never 402.
#[tokio::test]
async fn the_header_services_fault_is_503_headers_unavailable_with_retry_after() {
    use crate::spend_gate::{GateError, HeldCoin, Origin, Refusal};
    let held = HeldCoin {
        outpoint: format!("{}.0", "9c".repeat(32)),
        satoshis: 50_000,
        word: "mined".to_string(),
        origin: Origin::Received,
        age_minutes: 9,
        reason: "headers unavailable: Service error: get_chain_tip_header: no header service answered (chaintracks: timeout)".to_string(),
        headers_fault: true,
        output_id: 0,
    };
    let refusal = Refusal {
        toolbox: "Insufficient funds: need 1005, have 0".to_string(),
        offered_satoshis: 0,
        held: vec![held],
        capped: None,
        headers_fault: Some(
            "Service error: get_chain_tip_header: no header service answered (chaintracks: timeout)"
                .to_string(),
        ),
    };
    let message = refusal.to_string();
    let (status, retry, body) =
        answer(AppError::from_gate_error(GateError::Refused(refusal))).await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(
        retry.as_deref(),
        Some(
            crate::spend_gate::HEADERS_RETRY_AFTER_SECS
                .to_string()
                .as_str()
        )
    );
    assert_eq!(body["code"], "HEADERS_UNAVAILABLE");
    assert_eq!(body["message"], message);
    assert!(
        message.starts_with("Could not verify now: the header service did not answer ("),
        "{message}"
    );
    assert_eq!(body["offeredSatoshis"], 0);
    assert!(body["held"][0]["reason"]
        .as_str()
        .unwrap()
        .starts_with("headers unavailable: "));
    assert!(
        !message.contains("Insufficient funds: no offerable coin"),
        "{message}"
    );
}

/// A scoped word naming a coin the wallet could not spend under it (#63)
/// is the caller's 400, with no `Retry-After` and no listing.
#[tokio::test]
async fn a_scope_naming_a_wrong_coin_is_the_callers_400() {
    let message = "Invalid argument: allowPending: received names ab:0, which is not a received coin this wallet could spend under it: not a coin of this wallet".to_string();
    let (status, retry, body) = answer(AppError::from_gate_error(
        crate::spend_gate::GateError::Scope(message.clone()),
    ))
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(retry.is_none());
    assert_eq!(
        body,
        json!({"code": "VALIDATION_ERROR", "message": message})
    );
}
