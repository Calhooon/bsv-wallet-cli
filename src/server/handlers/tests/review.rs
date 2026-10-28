use super::*;

/// Toolbox #264 preserves this carrier; words alone cannot name its class.
#[tokio::test]
async fn review_actions_is_a_typed_conflict() {
    let review =
        bsv_sdk::wallet::ReviewActionsRefusal::new(vec![bsv_sdk::wallet::ReviewActionResult {
            txid: [0; 32],
            status: bsv_sdk::wallet::ReviewActionResultStatus::DoubleSpend,
            competing_txs: Some(vec![[7; 32]]),
            competing_beef: None,
        }]);
    let words = review.message.clone();
    for error in [
        AppError::from(bsv_sdk::Error::WalletReviewActions(Box::new(
            review.clone(),
        ))),
        AppError::from(bsv_wallet_toolbox::Error::ReviewActions(Box::new(review))),
    ] {
        served_as(error, (StatusCode::CONFLICT, "CONFLICT"), &words).await;
    }
    served_as(
        AppError::from(bsv_sdk::Error::WalletError(words.clone())),
        (StatusCode::BAD_REQUEST, "ERROR"),
        &format!("wallet error: {words}"),
    )
    .await;
}
