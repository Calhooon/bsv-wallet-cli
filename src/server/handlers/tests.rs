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

// #157: exercise the served door with valid, proven BEEF. A refused request
// must not store even the valid output preceding the oversized field.
async fn custom_instructions_wallet() -> (WalletState, axum::Router, Vec<u8>) {
    use crate::test_support::{p2pkh, raw_tx, txid_of, TestChainTracker};
    use bsv_sdk::transaction::{Beef, MerklePath};
    let height = 900_000;
    let raw = raw_tx(
        &[(&"11".repeat(32), 0)],
        &[(1000, p2pkh([0xdb; 20])), (2000, p2pkh([0xdc; 20]))],
    );
    let txid = txid_of(&raw);
    let mut beef = Beef::new();
    let bump = beef.merge_bump(MerklePath::from_coinbase_txid(&txid, height));
    beef.merge_raw_tx(raw, Some(bump));
    let bytes = beef.to_binary_atomic(&txid).unwrap();

    let storage = StorageSqlx::in_memory().await.unwrap();
    let key = bsv_sdk::primitives::PrivateKey::random();
    storage
        .migrate("custom-instructions", &key.public_key().to_hex())
        .await
        .unwrap();
    storage.make_available().await.unwrap();
    bsv_wallet_toolbox::MonitorStorage::set_max_acceptable_proof_height(&storage, u32::MAX)
        .await
        .unwrap();
    let mut tracker = TestChainTracker::new(height + 1);
    tracker.add_root(height, txid);
    storage.set_chain_tracker(Arc::new(tracker)).await;
    let services = Services::with_options(
        bsv_wallet_toolbox::Chain::Main,
        bsv_wallet_toolbox::ServicesOptions::mainnet(),
    )
    .unwrap();
    let wallet =
        crate::server::make_wallet_state(Wallet::new(Some(key), storage, services).await.unwrap());
    let router = crate::server::make_router(wallet.clone(), crate::server::ServerConfig::default());
    (wallet, router, bytes)
}

async fn custom_instructions_post(
    app: axum::Router,
    tx: Vec<u8>,
    instructions: &str,
    preceding_output: bool,
) -> (StatusCode, serde_json::Value) {
    use tower::ServiceExt;
    let output = |index, instructions: &str| {
        json!({
            "outputIndex": index,
            "protocol": "basket insertion",
            "insertionRemittance": { "basket": "instructions", "customInstructions": instructions }
        })
    };
    let outputs = if preceding_output {
        vec![output(0, "valid preceding output"), output(1, instructions)]
    } else {
        vec![output(0, instructions)]
    };
    let body = json!({ "tx": tx, "outputs": outputs, "description": "custom instructions byte bound", "labels": ["instructions-bound"] });
    let response = app
        .oneshot(
            axum::http::Request::post("/internalizeAction")
                .header("Origin", "http://test.local")
                .header("Content-Type", "application/json")
                .body(axum::body::Body::from(serde_json::to_vec(&body).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    (status, serde_json::from_slice(&bytes).unwrap())
}

async fn custom_instructions_stored_counts(wallet: &WalletState) -> (i64, i64, i64, i64, i64) {
    sqlx::query_as("SELECT (SELECT COUNT(*) FROM transactions), (SELECT COUNT(*) FROM outputs), (SELECT COUNT(*) FROM proven_txs), (SELECT COUNT(*) FROM proven_tx_reqs), (SELECT COUNT(*) FROM output_baskets)")
        .fetch_one(wallet.storage().pool()).await.unwrap()
}

#[tokio::test]
async fn internalize_custom_instructions_accepts_1000_bytes_and_reads_them_back() {
    let instructions = "a".repeat(1000);
    let (wallet, app, tx) = custom_instructions_wallet().await;
    let (status, body) = custom_instructions_post(app, tx, &instructions, false).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["accepted"], true);
    let stored: String =
        sqlx::query_scalar("SELECT custom_instructions FROM outputs WHERE vout = 0")
            .fetch_one(wallet.storage().pool())
            .await
            .unwrap();
    assert_eq!(stored, instructions);
}

#[tokio::test]
async fn internalize_custom_instructions_refuses_1001_bytes_before_any_output_is_stored() {
    let (wallet, app, tx) = custom_instructions_wallet().await;
    let before = custom_instructions_stored_counts(&wallet).await;
    let (status, body) = custom_instructions_post(app, tx, &"a".repeat(1001), true).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert_eq!(
        body,
        json!({
            "code": "VALIDATION_ERROR",
            "message": "Validation error: customInstructions: no more than 1000 length. (UTF-8 bytes: limit 1000, got 1001)"
        })
    );
    assert_eq!(custom_instructions_stored_counts(&wallet).await, before);
}

#[tokio::test]
async fn internalize_custom_instructions_counts_four_byte_characters_as_four_bytes() {
    for (instructions, accepted) in [
        ("🦀".repeat(250), true),
        (format!("{}a", "🦀".repeat(250)), false),
        ("🦀".repeat(251), false),
    ] {
        let (wallet, app, tx) = custom_instructions_wallet().await;
        let before = custom_instructions_stored_counts(&wallet).await;
        let (status, body) = custom_instructions_post(app, tx, &instructions, false).await;
        if accepted {
            assert_eq!(instructions.len(), 1000);
            assert_eq!(status, StatusCode::OK, "{body}");
            assert_eq!(body["accepted"], true);
            let stored: String =
                sqlx::query_scalar("SELECT custom_instructions FROM outputs WHERE vout = 0")
                    .fetch_one(wallet.storage().pool())
                    .await
                    .unwrap();
            assert_eq!(stored, instructions);
        } else {
            assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
            assert_eq!(
                body,
                json!({
                    "code": "VALIDATION_ERROR",
                    "message": format!("Validation error: customInstructions: no more than 1000 length. (UTF-8 bytes: limit 1000, got {})", instructions.len())
                })
            );
            assert_eq!(custom_instructions_stored_counts(&wallet).await, before);
        }
    }
}
