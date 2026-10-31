use super::*;
use bsv_sdk::wallet::{
    substrates::HttpWalletJson, CreateActionArgs, Outpoint, ReviewActionResult,
    ReviewActionResultStatus, ReviewActionsRefusal, SendWithResult, SendWithResultStatus,
};

fn review() -> ReviewActionsRefusal {
    use crate::test_support::{p2pkh, raw_tx, txid_of};
    // The action's transaction carries its input's source: bsv-rs's HTTP
    // JSON client holds a completed `tx` to the reference's evidence row
    // (every input with its source transaction, bsv-rs #461) since beta.4.
    let mut beef = bsv_sdk::transaction::Beef::new();
    let parent = raw_tx(&[(&"11".repeat(32), 0)], &[(2000, p2pkh([7; 20]))]);
    let raw = raw_tx(&[(&txid_of(&parent), 0)], &[(1000, p2pkh([7; 20]))]);
    let id = txid_of(&raw);
    beef.merge_raw_tx(parent, None);
    beef.merge_raw_tx(raw, None);
    let txid = hex::decode(&id).unwrap().try_into().unwrap();
    let mut competing = bsv_sdk::transaction::Beef::new();
    let raw = raw_tx(&[(&"11".repeat(32), 0)], &[(900, p2pkh([7; 20]))]);
    let competing_id = txid_of(&raw);
    competing.merge_raw_tx(raw, None);
    ReviewActionsRefusal::with_members(
        vec![ReviewActionResult {
            txid,
            status: ReviewActionResultStatus::DoubleSpend,
            competing_txs: Some(vec![hex::decode(&competing_id)
                .unwrap()
                .try_into()
                .unwrap()]),
            competing_beef: Some(competing.to_binary()),
        }],
        vec![SendWithResult {
            txid,
            status: SendWithResultStatus::Failed,
        }],
        Some(txid),
        Some(beef.to_binary_atomic(&id).unwrap()),
        Some(vec![Outpoint { txid, vout: 0 }]),
    )
}

/// Toolbox #264's carrier and SDK #339's five members survive every boundary.
#[tokio::test]
async fn review_actions_serves_code_five_and_preserves_all_members() {
    let review = review();
    for error in [
        AppError::from(bsv_sdk::Error::WalletReviewActions(Box::new(
            review.clone(),
        ))),
        AppError::from(bsv_wallet_toolbox::Error::ReviewActions(Box::new(
            review.clone(),
        ))),
        AppError::from(anyhow::Error::new(bsv_sdk::Error::WalletReviewActions(
            Box::new(review.clone()),
        ))),
        AppError::from(anyhow::Error::new(
            bsv_wallet_toolbox::Error::ReviewActions(Box::new(review.clone())),
        )),
    ] {
        let (status, _, body) = answer(error).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(body["isError"], true);
        assert_eq!(body["code"], 5);
        assert_eq!(body["message"], review.message);
        assert_eq!(body["tx"], json!(review.tx));
        assert_eq!(body["txid"], hex::encode(review.txid.unwrap()));
        assert_eq!(body["noSendChange"], json!(review.no_send_change));
        assert_eq!(body["sendWithResults"], json!(review.send_with_results));
        assert!(body["reviewActionResults"][0]["competingBeef"].is_object());
    }
    let minimal = ReviewActionsRefusal::new(vec![]);
    let (_, _, body) = answer(AppError::from(bsv_sdk::Error::WalletReviewActions(
        Box::new(minimal),
    )))
    .await;
    for absent in ["txid", "tx", "noSendChange"] {
        assert!(body.get(absent).is_none(), "{absent} must be omitted");
    }
    assert_eq!(body["sendWithResults"], json!([]));
    served_as(
        AppError::from(bsv_sdk::Error::WalletError(review.message.clone())),
        (StatusCode::BAD_REQUEST, "ERROR"),
        &format!("wallet error: {}", review.message),
    )
    .await;
}

/// Drive the pinned HTTP reader against the actual `IntoResponse` body.
#[tokio::test]
async fn http_wallet_json_reads_served_review_actions() {
    let expected = review();
    let served = expected.clone();
    let app = axum::Router::new().route(
        "/createAction",
        axum::routing::post(move || {
            let review = served.clone();
            async move { AppError::from(bsv_sdk::Error::WalletReviewActions(Box::new(review))) }
        }),
    );
    let (url, server) = crate::test_support::serve_router(app).await;
    let client = HttpWalletJson::new(Some("test.local".into()), Some(url)).unwrap();
    let result = client
        .create_action(
            serde_json::from_value::<CreateActionArgs>(json!({"description": "review reader"}))
                .unwrap(),
            Some("test.local"),
        )
        .await;
    server.abort();
    let _ = server.await;
    match result {
        Err(bsv_sdk::Error::WalletReviewActions(actual)) => assert_eq!(*actual, expected),
        other => panic!("expected typed review with the same five members, got {other:?}"),
    }
}
