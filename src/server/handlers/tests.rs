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
/// not, and any other fault is the gate's 500.
#[tokio::test]
async fn a_pre_door_fault_is_503_only_when_it_is_the_store_busy() {
    use crate::spend_gate::GateError;
    let typed = [
        anyhow::Error::new(sqlx::Error::PoolTimedOut),
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
    ] {
        let (status, retry, body) = answer(AppError::from_gate_error(GateError::Gate(e))).await;
        assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR, "{body}");
        assert_eq!(body["code"], "INTERNAL_ERROR");
        assert!(retry.is_none());
    }
    let (status, _, body) = answer(AppError::from_pre_door(
        anyhow::Error::new(sqlx::Error::PoolTimedOut),
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
