//! #313: the actual allocator and signer spend a named receipt beside a proven coin.
use super::*;

async fn funded_receipt() -> (Wallet<StorageSqlx, MockWalletServices>, String, i64) {
    let wallet = wallet_with_signable_funding().await;
    let user = UserCoins::from_wallet(&wallet).await.unwrap();
    let row = user
        .query()
        .build()
        .fetch_one(wallet.storage().pool())
        .await
        .unwrap();
    let source: i64 = row.get("output_id");
    let basket: i64 = sqlx::query_scalar(
        "SELECT basket_id FROM output_baskets WHERE user_id = ? AND name = 'default'",
    )
    .bind(user.user_id())
    .fetch_one(wallet.storage().pool())
    .await
    .unwrap();
    sqlx::query("UPDATE output_baskets SET number_of_desired_utxos = 0 WHERE basket_id = ?")
        .bind(basket)
        .execute(wallet.storage().writer())
        .await
        .unwrap();
    let lock: Vec<u8> = row.get("locking_script");
    let (proven, proven_id) = mined_coin_with_lock(
        wallet.storage(),
        (user.user_id(), basket),
        0xbb,
        50_000,
        901,
        &hex::encode(&lock),
    )
    .await;
    wallet
        .services()
        .set_header_for_height(header(901, &proven, 0));
    for id in [proven_id] {
        sqlx::query(
            "UPDATE outputs SET derivation_prefix = ?, derivation_suffix = ? WHERE output_id = ?",
        )
        .bind(row.get::<String, _>("derivation_prefix"))
        .bind(row.get::<String, _>("derivation_suffix"))
        .bind(id)
        .execute(wallet.storage().writer())
        .await
        .unwrap();
    }
    let mut args = pay(4_000);
    args.outputs.as_mut().unwrap()[0].locking_script = lock;
    args.options.as_mut().unwrap().no_send = Some(true);
    args.options.as_mut().unwrap().randomize_outputs = Some(false);
    let receipt = wallet
        .create_action_offering(args, Some("from.test"), &[source], &[source])
        .await
        .unwrap();
    let txid = hex::encode(receipt.txid.unwrap());
    // A synthetic received payment, with real signed bytes and stored ancestry.
    sqlx::query("UPDATE transactions SET is_outgoing = 0, status = 'unproven' WHERE txid = ?")
        .bind(&txid)
        .execute(wallet.storage().writer())
        .await
        .unwrap();
    sqlx::query("UPDATE outputs SET type = 'P2PKH', change = 1, spendable = 1, provided_by = 'storage', purpose = 'receive', basket_id = ?, derivation_prefix = ?, derivation_suffix = ? WHERE txid = ? AND vout = 0")
        .bind(basket).bind(row.get::<String, _>("derivation_prefix")).bind(row.get::<String, _>("derivation_suffix"))
        .bind(&txid).execute(wallet.storage().writer()).await.unwrap();
    (wallet, txid, proven_id)
}

#[tokio::test]
async fn named_receipt_requires_the_rule_then_is_signed_with_proven_coin_untouched() {
    let (wallet, receipt, proven_id) = funded_receipt().await;
    let from = vec![(receipt.clone(), 0)];
    let mut args = pay(1_000);
    args.options.as_mut().unwrap().no_send = Some(true);
    args.options.as_mut().unwrap().randomize_outputs = Some(false);
    let plain = opts(1, true);
    let refused = create_action_from(&wallet, args.clone(), "from.test", &plain, &from)
        .await
        .unwrap_err();
    assert!(matches!(refused, GateError::Refused(_)));
    assert!(refused.to_string().contains(&receipt));
    let ruled = plain.with_allow_pending(AllowPending::ReceivedOnly(from.clone()));
    let answer = create_action_from(&wallet, args.clone(), "from.test", &ruled, &from)
        .await
        .unwrap();
    let txid = hex::encode(answer.txid.unwrap());
    let raw: Vec<u8> = sqlx::query_scalar("SELECT raw_tx FROM transactions WHERE txid = ?")
        .bind(&txid)
        .fetch_one(wallet.storage().pool())
        .await
        .unwrap();
    let tx = bsv_sdk::transaction::Transaction::from_binary(&raw).unwrap();
    assert_eq!(tx.inputs.len(), 1);
    assert_eq!(tx.inputs[0].source_txid.as_deref(), Some(receipt.as_str()));
    let spent: Option<i64> = sqlx::query_scalar("SELECT spent_by FROM outputs WHERE output_id = ?")
        .bind(proven_id)
        .fetch_one(wallet.storage().pool())
        .await
        .unwrap();
    assert!(spent.is_none(), "the proven coin must remain unspent");
    assert_eq!(
        rules_of(
            wallet.storage(),
            UserCoins::from_wallet(&wallet).await.unwrap().user_id()
        )
        .await
        .unwrap()[&txid],
        RECEIVED_PENDING
    );
    let repeated = create_action_from(&wallet, args, "from.test", &opts(1, true), &from)
        .await
        .unwrap_err();
    assert!(matches!(repeated, GateError::Scope(_)));
    assert!(repeated.to_string().contains("spent or reserved"));
}

#[tokio::test]
async fn named_coins_refuse_foreign_duplicate_and_unmanaged_without_writes() {
    let (wallet, receipt, _) = funded_receipt().await;
    let before = stored_rows(wallet.storage()).await;
    for from in [
        vec![("aa".repeat(32), 0)],
        vec![(receipt.clone(), 0), (receipt.clone(), 0)],
    ] {
        let error = create_action_from(&wallet, pay(1_000), "from.test", &opts(1, true), &from)
            .await
            .unwrap_err();
        assert!(matches!(error, GateError::Scope(_)));
        assert_eq!(stored_rows(wallet.storage()).await, before);
    }
    sqlx::query("UPDATE outputs SET change = 0 WHERE txid = ? AND vout = 0")
        .bind(&receipt)
        .execute(wallet.storage().writer())
        .await
        .unwrap();
    let error = from_satoshis(&wallet, &[(receipt, 0)]).await.unwrap_err();
    assert!(matches!(error, GateError::Scope(_)));
}
