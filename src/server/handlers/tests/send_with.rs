//! `createAction`'s `sendWith` and `noSendChange` at the door
//! (#147): a non-empty list in either is refused by name before anything
//! is asked or built; absent, `null` and `[]` map as before.

use super::super::{refuse_send_with, McCreateActionReq};
use super::answer;
use axum::http::StatusCode;
use serde_json::json;

/// A non-empty `sendWith` or `noSendChange` (#147) is 501
/// `SEND_WITH_NOT_SUPPORTED` from the request's mapping, before anything
/// is asked or built, naming the option; absent, `null` and `[]` map as
/// before.
#[tokio::test]
async fn a_named_send_with_or_no_send_change_is_refused_and_an_empty_one_passes() {
    let request = |options: serde_json::Value| -> McCreateActionReq {
        serde_json::from_value(json!({"description": "a payment", "options": options})).unwrap()
    };
    for passes in [
        json!({}),
        json!({"sendWith": null, "noSendChange": null}),
        json!({"sendWith": [], "noSendChange": []}),
        json!({"noSend": true, "sendWith": []}),
    ] {
        assert!(
            refuse_send_with(request(passes.clone()).options.as_ref()).is_ok(),
            "{passes}"
        );
    }
    let txid = "ab".repeat(32);
    for (options, named) in [
        (json!({"sendWith": [txid]}), "sendWith"),
        (json!({"sendWith": [txid], "noSendChange": []}), "sendWith"),
        (
            json!({"noSendChange": [format!("{txid}.1")]}),
            "noSendChange",
        ),
        (
            json!({"sendWith": [txid], "noSendChange": [format!("{txid}.1")]}),
            "sendWith",
        ),
        (json!({"sendWith": "not a list"}), "sendWith"),
    ] {
        let Err(e) = refuse_send_with(request(options.clone()).options.as_ref()) else {
            panic!("{options} passed");
        };
        let (status, retry, body) = answer(e).await;
        assert_eq!(status, StatusCode::NOT_IMPLEMENTED, "{options}");
        assert!(retry.is_none());
        assert_eq!(body["code"], "SEND_WITH_NOT_SUPPORTED");
        assert_eq!(body.as_object().unwrap().len(), 2, "{body}");
        let message = body["message"].as_str().unwrap();
        assert!(
            message.starts_with(&format!("createAction: {named} is not supported: ")),
            "{message}"
        );
    }
}

/// The served wallet of the handler's test: an empty in-memory store whose
/// services are the toolbox's mock (every post of the store goes through
/// them and is counted), behind this host's router. The wallet's own
/// `Services` is the configured type the served state requires; no request
/// of the test below reaches a line that reads it: each is answered by the
/// refusal or by the read of `allowPending` after it, both ahead of the
/// handler's use of the wallet.
async fn served_over_mock_services() -> (
    crate::server::handlers::WalletState,
    axum::Router,
    std::sync::Arc<bsv_wallet_toolbox::services::mock::MockWalletServices>,
) {
    use bsv_wallet_toolbox::services::mock::MockWalletServices;
    use bsv_wallet_toolbox::{
        StorageSqlx, Wallet, WalletServices, WalletStorageProvider, WalletStorageWriter,
    };
    let storage = StorageSqlx::in_memory().await.unwrap();
    let key = bsv_sdk::primitives::PrivateKey::random();
    storage
        .migrate("send-with-door", &key.public_key().to_hex())
        .await
        .unwrap();
    storage.make_available().await.unwrap();
    let services = bsv_wallet_toolbox::Services::with_options(
        bsv_wallet_toolbox::Chain::Main,
        bsv_wallet_toolbox::ServicesOptions::mainnet(),
    )
    .unwrap();
    let wallet =
        crate::server::make_wallet_state(Wallet::new(Some(key), storage, services).await.unwrap());
    let mock = std::sync::Arc::new(MockWalletServices::new());
    WalletStorageProvider::set_services(
        wallet.storage(),
        mock.clone() as std::sync::Arc<dyn WalletServices>,
    );
    let router = crate::server::make_router(wallet.clone(), crate::server::ServerConfig::default());
    (wallet, router, mock)
}

/// `POST /createAction` of `app` with `body`: the status and the JSON answer.
async fn post_create_action(
    app: axum::Router,
    body: serde_json::Value,
) -> (StatusCode, serde_json::Value) {
    use tower::ServiceExt;
    let response = app
        .oneshot(
            axum::http::Request::post("/createAction")
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

/// The rows of `transactions` and of `outputs`.
async fn rows(wallet: &crate::server::handlers::WalletState) -> (i64, i64) {
    sqlx::query_as("SELECT (SELECT COUNT(*) FROM transactions), (SELECT COUNT(*) FROM outputs)")
        .fetch_one(wallet.storage().pool())
        .await
        .unwrap()
}

/// The door itself (#147): `POST /createAction` through this host's router
/// with a non-empty `sendWith`, and with a non-empty `noSendChange`, is 501
/// `SEND_WITH_NOT_SUPPORTED` with the two-key body naming the option; no
/// row of `transactions` or `outputs` is written and the store's mock
/// services are asked for no post. Every body also carries an
/// `allowPending` of no known shape, which the handler reads directly
/// after the refusal: the refused calls show the refusal is ahead of that
/// read, and the control (the same call with neither option, or with an
/// empty list in each) is not refused by this check and meets that read's
/// 400 instead, so no request here goes on to the wallet.
#[tokio::test]
async fn the_served_create_action_refuses_a_named_send_with_or_no_send_change() {
    let (wallet, app, mock) = served_over_mock_services().await;
    let before = rows(&wallet).await;
    let txid = "ab".repeat(32);
    let call = |options: serde_json::Value| {
        json!({
            "description": "a payment",
            "options": options,
            "allowPending": {"received": "no list"}
        })
    };
    for (options, named) in [
        (json!({"sendWith": [txid]}), "sendWith"),
        (
            json!({"noSend": true, "noSendChange": [format!("{txid}.1")]}),
            "noSendChange",
        ),
    ] {
        let (status, body) = post_create_action(app.clone(), call(options.clone())).await;
        assert_eq!(status, StatusCode::NOT_IMPLEMENTED, "{options}: {body}");
        assert_eq!(body["code"], "SEND_WITH_NOT_SUPPORTED", "{body}");
        assert_eq!(body.as_object().unwrap().len(), 2, "{body}");
        let message = body["message"].as_str().unwrap();
        assert!(
            message.starts_with(&format!("createAction: {named} is not supported: ")),
            "{message}"
        );
        assert_eq!(rows(&wallet).await, before, "{options}: nothing stored");
        assert_eq!(mock.call_count("post_beef"), 0, "{options}: nothing posted");
    }

    for control in [json!({}), json!({"sendWith": [], "noSendChange": []})] {
        let (status, body) = post_create_action(app.clone(), call(control.clone())).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{control}: {body}");
        assert_ne!(body["code"], "SEND_WITH_NOT_SUPPORTED", "{body}");
        let message = body["message"].as_str().unwrap();
        assert!(message.contains("allowPending"), "{message}");
    }
    assert_eq!(rows(&wallet).await, before, "the controls store nothing");
    assert_eq!(mock.call_count("post_beef"), 0, "the controls post nothing");
}
