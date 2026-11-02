//! The answer's rule (#44): the store busy is 503 `STORE_BUSY` with
//! `Retry-After: 1` by its type only; every other fault keeps its word and status.
use super::*;
use crate::test_support::response_answer;
use axum::http::StatusCode;
use axum::response::IntoResponse;
use bsv_sdk::primitives::PublicKey;

mod faults;
mod mutants;
mod review;

async fn answer(e: AppError) -> (StatusCode, Option<String>, serde_json::Value) {
    response_answer(e.into_response()).await
}

async fn served_as(served: AppError, expected: (StatusCode, &str), display: &str) {
    let (status, _, body) = answer(served).await;
    assert_eq!((status, body["code"].as_str().unwrap()), expected);
    assert_eq!(body["message"], display);
}

/// Every word the old classifier read, the old belt's busy words among
/// them: none answers 503 by its words (a `WalletError`'s text, the same
/// text doubly wrapped, an untyped `anyhow`), and the word STORE_BUSY is in
/// no 4xx body.
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
        for e in [
            AppError::from(bsv_sdk::Error::WalletError(msg.to_string())),
            AppError::from(bsv_sdk::Error::WalletError(format!("wallet error: {msg}"))),
            // The toolbox's faults are typed since beta.4 (#342): a storage
            // fault's words are not the store busy either.
            AppError::from(bsv_sdk::Error::WalletFault(bsv_sdk::WalletFault::new(
                bsv_sdk::WalletFaultFamily::Storage,
                msg,
            ))),
            AppError::from(anyhow::anyhow!("{msg}")),
        ] {
            let (status, retry, body) = answer(e).await;
            assert_ne!(status, StatusCode::SERVICE_UNAVAILABLE, "{msg}");
            assert_ne!(body["code"], "STORE_BUSY", "{msg}");
            assert!(retry.is_none(), "{msg}");
            if status.is_client_error() {
                assert!(!body.to_string().contains("STORE_BUSY"), "{msg}: {body}");
            }
        }
    }
}

/// Every published WERR class is mapped independently of its message.
#[tokio::test]
async fn every_reference_class_has_a_host_answer() {
    use bsv_sdk::WalletRefusalClass as C;
    let rows = [
        (C::UnsupportedAction, 501, "NOT_IMPLEMENTED"),
        (C::InvalidHmac, 400, "VALIDATION_ERROR"),
        (C::InvalidSignature, 400, "VALIDATION_ERROR"),
        (C::ReviewActions, 400, "5"),
        (C::InvalidParameter, 400, "VALIDATION_ERROR"),
        (C::InsufficientFunds, 402, "INSUFFICIENT_FUNDS"),
        (C::AbortRefused, 409, "CONFLICT"),
        (C::NotImplemented, 501, "NOT_IMPLEMENTED"),
        (C::Internal, 500, "INTERNAL_ERROR"),
        (C::InvalidOperation, 409, "CONFLICT"),
        (C::UtxoReviewInconclusive, 409, "CONFLICT"),
        (C::ActionBatchState, 409, "CONFLICT"),
        (C::BroadcastUnavailable, 502, "SERVICE_ERROR"),
        (C::InvalidMerkleRoot, 400, "VALIDATION_ERROR"),
        (C::MissingParameter, 400, "VALIDATION_ERROR"),
        (C::BadRequest, 400, "VALIDATION_ERROR"),
        (C::NetworkChain, 502, "SERVICE_ERROR"),
        (C::Unauthorized, 403, "ACCESS_DENIED"),
        (C::NotActive, 409, "CONFLICT"),
        (C::InvalidPublicKey, 400, "VALIDATION_ERROR"),
    ];
    assert_eq!(rows.len(), C::ALL.len());
    for (class, status, code) in rows {
        for message in [
            "unrelated words",
            "Store busy: Insufficient funds: Invalid BEEF",
        ] {
            let error = bsv_sdk::Error::WalletRefusal {
                class,
                message: message.into(),
            };
            for mapped in [
                AppError::from(error.clone()),
                AppError::from(anyhow::Error::new(error)),
            ] {
                let (actual, retry, body) = answer(mapped).await;
                assert_eq!(actual.as_u16(), status, "{class:?}");
                assert_eq!(body, json!({"code": code, "message": message}));
                assert!(retry.is_none());
            }
        }
    }
}

/// An erased fault cannot acquire a class from a familiar prefix.
#[tokio::test]
async fn untyped_wallet_faults_stay_unclassified() {
    for message in [
        "Insufficient funds: need 5",
        "Service error: no header",
        "Unspendable coin: aa.0",
        "Entity not found: output",
        "Validation error: bad",
    ] {
        let error = bsv_sdk::Error::WalletError(message.into());
        let display = error.to_string();
        let (status, _, body) = answer(AppError::from(error)).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(body, json!({"code": "ERROR", "message": display}));
    }
}

/// The tracker and response-wait timeout retain their typed transport fault.
#[tokio::test]
async fn a_transport_fault_is_a_service_error_without_a_busy_promise() {
    let error = bsv_sdk::Error::TransportFault {
        fault: bsv_sdk::TransportFault::MayHaveBeenReceived,
        status: None,
        detail: "Request timed out".into(),
    };
    let display = error.to_string();
    for mapped in [
        AppError::from(error.clone()),
        AppError::from(bsv_wallet_toolbox::Error::SdkError(error)),
    ] {
        let (status, retry, body) = answer(mapped).await;
        assert_eq!(status, StatusCode::BAD_GATEWAY);
        assert_eq!(body["code"], "SERVICE_ERROR");
        assert!(body["message"].as_str().unwrap().ends_with(&display));
        assert!(retry.is_none());
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
        AppError::from(busy),
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

/// A managed coin with incomplete ancestry carries InvalidOperation.
/// The reference has no separate unspendable code or typed coin member.
#[tokio::test]
async fn incomplete_ancestry_is_a_conflict_by_class() {
    let error = bsv_wallet_toolbox::Error::UnspendableCoin {
        outpoint: format!("{}.1", "ab".repeat(32)),
        missing: vec!["cd".repeat(32)],
    };
    let sdk = bsv_sdk::Error::from(error);
    assert!(matches!(
        sdk,
        bsv_sdk::Error::WalletRefusal {
            class: bsv_sdk::WalletRefusalClass::InvalidOperation,
            ..
        }
    ));
    let (status, _, body) = answer(AppError::from(sdk)).await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(body["code"], "CONFLICT");
    assert!(body.get("coin").is_none());
}

/// CORS: a browser caller may read `Retry-After` (the router's layer).
#[tokio::test]
async fn cors_exposes_retry_after() {
    use tower::ServiceExt;
    let storage = StorageSqlx::in_memory().await.unwrap();
    let key = bsv_sdk::primitives::PrivateKey::random();
    let wallet = crate::test_support::test_wallet(storage, "cors", key).await;
    let app = crate::server::make_router(
        wallet,
        crate::server::ServerConfig::admitting(["https://app.example"]),
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

/// #411: a body's originator is judged as the door reads the body, after
/// the lone-surrogate rewrite, so a body serde could not read before it is
/// no way around the check; the same body naming the Origin's host passes.
#[tokio::test]
async fn a_body_originator_is_judged_after_the_surrogate_rewrite() {
    use tower::ServiceExt;
    let storage = StorageSqlx::in_memory().await.unwrap();
    let key = bsv_sdk::primitives::PrivateKey::random();
    let wallet = crate::test_support::test_wallet(storage, "body-originator", key).await;
    let app = crate::server::make_router(
        wallet,
        crate::server::ServerConfig::admitting(["http://app.example"]),
    );
    let call = |originator: &str| {
        let body = format!(
            r#"{{"originator":"{originator}","protocolID":[2,"gate test"],"keyID":"\ud83d","counterparty":"self","plaintext":[1]}}"#
        );
        axum::http::Request::post("/encrypt")
            .header("Origin", "http://app.example")
            .header("Content-Type", "application/json")
            .body(axum::body::Body::from(body))
            .unwrap()
    };
    let (status, _, body) = crate::test_support::response_answer(
        app.clone().oneshot(call("evil.example")).await.unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert!(
        body["message"].as_str().unwrap().contains("disagrees"),
        "{body}"
    );
    let (status, _, body) =
        crate::test_support::response_answer(app.oneshot(call("app.example")).await.unwrap()).await;
    assert_eq!(status, StatusCode::OK, "{body}");
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
#[tokio::test]
async fn create_action_args_refuses_a_bad_word_hex_or_outpoint() {
    let req: McCreateActionReq = serde_json::from_value(serde_json::json!({
        "description": "bad scope", "allowPending": { "received": ["nope"] }
    }))
    .unwrap();
    let (_, _, body) = answer(create_action_args(req).unwrap_err()).await;
    let err = body["message"].as_str().unwrap();
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
    let (_, _, body) = answer(create_action_args(req).unwrap_err()).await;
    let err = body["message"].as_str().unwrap();
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
        headers_fault: Some(
            "Service error: get_chain_tip_header: no header service answered (chaintracks: timeout)"
                .to_string(),
        ),
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

/// `privileged: true` (#145) is 501 `PRIVILEGED_NOT_SUPPORTED` by its
/// kind, `{code, message}` and no `Retry-After`; absent or `false` passes.
#[tokio::test]
async fn privileged_true_is_refused_by_its_kind_and_false_passes() {
    let reason = Some("a reason for it");
    assert!(refuse_privileged("encrypt", None, None).is_ok());
    assert!(refuse_privileged("encrypt", Some(false), None).is_ok());
    assert!(refuse_privileged("encrypt", Some(false), reason).is_ok());
    for reason in [None, reason] {
        let e = refuse_privileged("encrypt", Some(true), reason).unwrap_err();
        let (status, retry, body) = answer(e).await;
        assert_eq!(status, StatusCode::NOT_IMPLEMENTED);
        assert!(retry.is_none());
        assert_eq!(body["code"], "PRIVILEGED_NOT_SUPPORTED");
        assert_eq!(body.as_object().unwrap().len(), 2, "{body}");
        assert!(body["message"].as_str().unwrap().starts_with("encrypt: "));
    }
}

/// The nine request structs read `privileged` and `privilegedReason`
/// (#145) beside their own fields; absent and `null` are `None`.
#[test]
fn the_nine_request_structs_read_privileged_and_its_reason() {
    type Asked = (Option<bool>, Option<String>);
    fn read<T: serde::de::DeserializeOwned>(base: &serde_json::Value, get: fn(T) -> Asked) {
        let parse = |extra: &serde_json::Value| {
            let mut body = base.clone();
            body.as_object_mut()
                .unwrap()
                .extend(extra.as_object().cloned().unwrap());
            get(serde_json::from_value(body).unwrap())
        };
        assert_eq!(parse(&json!({})), (None, None));
        assert_eq!(parse(&json!({"privileged": null})), (None, None));
        assert_eq!(parse(&json!({"privileged": false})), (Some(false), None));
        let reason = Some("a reason for it".to_string());
        let both = json!({"privileged": true, "privilegedReason": "a reason for it"});
        assert_eq!(parse(&both), (Some(true), reason));
    }
    let body = json!({"protocolID": [2, "t"], "keyID": "1", "counterparty": "self",
        "verifier": "self", "data": [1], "signature": [1], "plaintext": [1],
        "ciphertext": [1], "hmac": [1]});
    read::<McGetPublicKeyReq>(&body, |r| (r.privileged, r.privileged_reason));
    read::<McCreateSignatureReq>(&body, |r| (r.privileged, r.privileged_reason));
    read::<McVerifySignatureReq>(&body, |r| (r.privileged, r.privileged_reason));
    read::<McEncryptReq>(&body, |r| (r.privileged, r.privileged_reason));
    read::<McDecryptReq>(&body, |r| (r.privileged, r.privileged_reason));
    read::<McCreateHmacReq>(&body, |r| (r.privileged, r.privileged_reason));
    read::<McVerifyHmacReq>(&body, |r| (r.privileged, r.privileged_reason));
    read::<McRevealCounterpartyKeyLinkageReq>(&body, |r| (r.privileged, r.privileged_reason));
    read::<McRevealSpecificKeyLinkageReq>(&body, |r| (r.privileged, r.privileged_reason));
}

/// abortAction's pre-check under a busy store (#168): a real `SQLITE_BUSY`
/// from `abort_refused_for`'s own read (a rollback-journal file whose
/// lock another connection holds, no busy timeout) is 503 `STORE_BUSY`
/// with `Retry-After: 1`, as at every door; any other fault of the
/// pre-check keeps its 400 `ERROR`.
#[tokio::test]
async fn the_abort_pre_check_under_a_busy_store_is_503_store_busy() {
    use crate::test_support::{busy_holder, no_wait_file};
    use sqlx::sqlite::SqliteJournalMode;
    use sqlx::Connection;
    let dir = tempfile::TempDir::new().unwrap();
    let options = no_wait_file(&dir.path().join("busy.db")).journal_mode(SqliteJournalMode::Delete);
    let table = "CREATE TABLE transactions (txid TEXT, reference TEXT)";
    let mut holder = busy_holder(&options, table).await;
    let reader = sqlx::SqlitePool::connect_with(options).await.unwrap();
    sqlx::query("BEGIN EXCLUSIVE")
        .execute(&mut holder)
        .await
        .unwrap();
    let busy = crate::retire_guard::abort_refused_for(&reader, "cmVm")
        .await
        .expect_err("the store's lock is held");
    assert!(crate::store::is_store_busy(&busy), "{busy}");
    let message = busy.to_string();
    let (status, retry, body) = answer(abort_pre_check_fault(busy)).await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE, "{body}");
    assert_eq!(retry.as_deref(), Some("1"));
    assert_eq!(body, json!({"code": "STORE_BUSY", "message": message}));
    let _ = holder.close().await;

    let fault = anyhow::Error::new(sqlx::Error::RowNotFound);
    let message = fault.to_string();
    let (status, retry, body) = answer(abort_pre_check_fault(fault)).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert!(retry.is_none());
    assert_eq!(body, json!({"code": "ERROR", "message": message}));
}

/// Both host routes share the abort's real-file busy fixture. Dotfile
/// locking keeps readers out even though the toolbox requests WAL.
async fn host_route_store_faults(path: &str, code: &str) {
    use sqlx::Connection;
    use tower::ServiceExt;

    let (_dir, wallet, holder) = crate::test_support::busy_read_wallet().await;
    let storage = wallet.storage();
    let app = crate::server::make_router(
        wallet.clone(),
        crate::server::ServerConfig::admitting(["http://host-route.test"]),
    );
    let request = || {
        axum::http::Request::get(path)
            .header("Origin", "http://host-route.test")
            .body(axum::body::Body::empty())
            .unwrap()
    };
    let read = || {
        sqlx::query_scalar::<_, String>("SELECT name FROM sqlite_master")
            .fetch_optional(storage.pool())
    };
    let busy = read().await.expect_err("another connection holds the lock");
    assert!(crate::store::sqlx_is_busy(&busy), "{busy}");
    assert_eq!(
        response_answer(app.clone().oneshot(request()).await.unwrap()).await,
        (
            StatusCode::SERVICE_UNAVAILABLE,
            Some("1".to_string()),
            json!({"code": "STORE_BUSY", "message": busy.to_string()}),
        )
    );
    holder.close().await.unwrap();

    storage.pool().close().await;
    let plain = read().await.expect_err("the reader pool is closed");
    assert!(!crate::store::sqlx_is_busy(&plain));
    // Words alone are no signal, including the words of a real busy.
    let untyped = anyhow::anyhow!(busy.to_string());
    for (response, message) in [
        (app.oneshot(request()).await.unwrap(), plain.to_string()),
        (
            crate::server::host_route_fault(&untyped, code),
            untyped.to_string(),
        ),
    ] {
        assert_eq!(
            response_answer(response).await,
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                None,
                json!({"code": code, "message": message})
            )
        );
    }
}

#[tokio::test]
async fn chain_feed_under_a_busy_store_is_503_store_busy() {
    host_route_store_faults("/chain-feed", "ERR_CHAIN_FEED").await;
}

#[tokio::test]
async fn tx_word_under_a_busy_store_is_503_store_busy() {
    host_route_store_faults(&format!("/tx/{}", "11".repeat(32)), "ERR_TX_WORD").await;
}
mod custom_instructions;
mod page_bounds;

/// A caller's key argument is read by the reference wallet's argument rule
/// (`validatePublicKey`, ts-stack@edf6e03
/// `sdk/src/wallet/validationHelpers.ts:185-191`) through bsv-rs 0.9.0-beta.6's
/// `validate_public_key` (#579; #377's host reader retired at the beta.6
/// pin, #386): the lenient forms bsv-rs's `from_hex` reads (#498, cases of
/// its `tests/vectors/public_key_forms_498.json`) and an uncompressed `04`
/// key (which #377's reader read) are refused at the doors as a typed
/// `WalletRefusal` of class InvalidParameter in the reference's words, served
/// 400 VALIDATION_ERROR; surrounding whitespace is trimmed as the reference
/// trims it; the canonical form reads the same key.
#[test]
fn a_lenient_key_form_is_refused_at_the_doors() {
    const G: &str = "0279be667ef9dcbbac55a06295ce870b07029bfcdb2dce28d959f2815b16f81798";
    const GY: &str = "483ada7726a3c4655da4fbfc0e1108a8fd17b448a68554199c47d08ffb10d4b8";
    let uncompressed = format!("04{}{GY}", &G[2..]);
    let refused = [
        // odd_length_compressed_even
        (G[1..].to_string(), "even length, not 65."),
        // odd_length_uncompressed
        (format!("4{}{GY}", &G[2..]), "even length, not 129."),
        // hybrid_even
        (format!("06{}{GY}", &G[2..]), "no more than 66 length."),
        // x_above_p_compressed_even
        (
            "02fffffffffffffffffffffffffffffffffffffffffffffffffffffffefffffc30".to_string(),
            "a valid compressed secp256k1 public key",
        ),
        // uncompressed: #377's reader read it, the reference refuses it
        (uncompressed.clone(), "no more than 66 length."),
        // fixture 026's key
        ("02zz".to_string(), "hexadecimal string."),
    ];
    for (form, ending) in &refused {
        let e = validate_public_key(form, "counterparty").unwrap_err();
        assert!(
            matches!(
                e,
                bsv_sdk::Error::WalletRefusal {
                    class: bsv_sdk::WalletRefusalClass::InvalidParameter,
                    ..
                }
            ),
            "{form}: {e:?}"
        );
        assert_eq!(
            e.to_string(),
            format!("The counterparty parameter must be {ending}"),
            "{form}"
        );
        let served = AppError::from(parse_counterparty(form).unwrap_err());
        assert_eq!(
            served.into_response().status(),
            StatusCode::BAD_REQUEST,
            "{form}"
        );
    }
    let canonical = PublicKey::from_hex(G).unwrap();
    assert!(PublicKey::from_hex(&uncompressed).is_ok());
    assert_eq!(validate_public_key(G, "verifier").unwrap(), canonical);
    for padded in [format!(" {G}"), format!("{G}\n"), format!("\t{G} ")] {
        assert_eq!(validate_public_key(&padded, "verifier").unwrap(), canonical);
    }
    assert!(matches!(parse_counterparty(G).unwrap(), Counterparty::Other(k) if k == canonical));
}
