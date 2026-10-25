//! #201: all gate reads belong to the user whose action is at the door.

use super::*;
use axum::response::IntoResponse;

struct TwoUsers {
    wallet: Wallet<StorageSqlx, MockWalletServices>,
    owner: Wallet<StorageSqlx, MockWalletServices>,
    txid: String,
    output: i64,
}

async fn two_users() -> TwoUsers {
    let root = PrivateKey::random();
    let (storage, _, _) = storage_for(&root.public_key().to_hex()).await;
    let other_root = PrivateKey::random();
    let other = other_root.public_key().to_hex();
    let (owner, _) = storage.find_or_insert_user(&other).await.unwrap();
    let basket = storage
        .find_or_create_default_basket(owner.user_id)
        .await
        .unwrap();
    let txid = "ab".repeat(32);
    let output = coin(
        &storage,
        (owner.user_id, basket.basket_id),
        &txid,
        false,
        None,
        8_000,
        5,
    )
    .await;
    let services = headers_with(&"0c".repeat(32), 900, 900);
    let owner = Wallet::new(
        Some(other_root),
        storage.clone(),
        headers_with(&"0c".repeat(32), 900, 900),
    )
    .await
    .unwrap();
    let wallet = Wallet::new(Some(root), storage, services).await.unwrap();
    TwoUsers {
        wallet,
        owner,
        txid,
        output,
    }
}

async fn refusal_response(w: &TwoUsers, txid: &str, scoped: bool) -> serde_json::Value {
    let mut args = pay(1_000);
    let gate = if scoped {
        opts(1, true).with_allow_pending(AllowPending::ReceivedOnly(vec![(txid.into(), 0)]))
    } else {
        args.inputs = Some(vec![named_input(txid)]);
        opts(1, true)
    };
    let (result, asked) = at_the_door(create_action(&w.wallet, args, "gate.local", &gate)).await;
    let err = result.unwrap_err();
    assert!(matches!(err, GateError::Scope(_)), "{err:?}");
    assert!(!asked, "the toolbox was asked");
    let response = crate::server::error::AppError::from_gate_error(err).into_response();
    assert_eq!(response.status(), axum::http::StatusCode::BAD_REQUEST);
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    serde_json::from_slice(&body).unwrap()
}

async fn not_held(scoped: bool) {
    let w = two_users().await;
    let before = stored_rows(w.wallet.storage()).await;
    let absent = "cd".repeat(32);
    let foreign = refusal_response(&w, &w.txid, scoped).await;
    let missing = refusal_response(&w, &absent, scoped).await;
    assert_eq!(foreign["code"], "VALIDATION_ERROR");
    let message = foreign["message"].as_str().unwrap();
    assert!(message.ends_with(NOT_HELD), "{foreign}");
    assert_eq!(message.replace(&w.txid, &absent), missing["message"]);
    assert_eq!(foreign.as_object().unwrap().len(), 2, "no coin details");
    assert_eq!(w.wallet.services().total_calls(), 0, "no creator read");
    assert_eq!(
        stored_rows(w.wallet.storage()).await,
        before,
        "nothing stored"
    );
}

#[tokio::test]
async fn another_users_named_input_is_not_held() {
    not_held(false).await;
}

#[tokio::test]
async fn another_users_scoped_coin_is_not_held() {
    not_held(true).await;
}

#[tokio::test]
async fn only_the_owner_holds_and_is_offered_the_coin() {
    let w = two_users().await;
    let owner = UserCoins::from_wallet(&w.owner).await.unwrap();
    let caller = UserCoins::from_wallet(&w.wallet).await.unwrap();
    let named = [(w.txid.clone(), 0)];
    let plain = opts(1, true);
    let held = assess_named(owner, w.wallet.services(), &plain, &named, &[], None, false)
        .await
        .unwrap();
    assert_eq!(
        serde_json::to_value(&held).unwrap(),
        serde_json::json!([{
            "outpoint": format!("{}.0", w.txid), "satoshis": 8_000,
            "word": "unknown", "origin": "received", "age_minutes": 5,
            "reason": "depth 1: 0 blocks; received, no proof; offered under allowPending: received"
        }])
    );
    let owned = assess(owner, w.wallet.services(), &plain, None)
        .await
        .unwrap();
    assert_eq!(owned.held, held, "the same held listing on both doors");
    let scoped = plain.with_allow_pending(AllowPending::ReceivedOnly(named.to_vec()));
    assert_eq!(check_scope(owner, &named).await.unwrap(), None);
    assert!(assess_named(
        owner,
        w.wallet.services(),
        &scoped,
        &named,
        &[],
        None,
        false
    )
    .await
    .unwrap()
    .is_empty());
    for gate in [opts(1, true).with_allow_received(true), scoped] {
        let owned = assess(owner, w.wallet.services(), &gate, None)
            .await
            .unwrap();
        assert_eq!(owned.offered, vec![(w.output, 8_000)]);
        let foreign = assess(caller, w.wallet.services(), &gate, None)
            .await
            .unwrap();
        assert!(foreign.offered.is_empty() && foreign.held.is_empty() && foreign.unread.is_empty());
    }
}

#[tokio::test]
async fn another_users_lost_and_spent_coins_never_enter_a_refusal() {
    let w = two_users().await;
    let storage = w.wallet.storage();
    let caller = UserCoins::from_wallet(&w.wallet).await.unwrap();
    let owner = UserCoins::from_wallet(&w.owner).await.unwrap();
    let gate = opts(1, true).with_allow_received(true);
    lose(storage, w.output).await;
    assert_eq!(
        assess(owner, w.wallet.services(), &gate, None)
            .await
            .unwrap()
            .held[0]
            .word,
        "lost"
    );
    assert!(assess(caller, w.wallet.services(), &gate, None)
        .await
        .unwrap()
        .held
        .is_empty());
    sqlx::query("UPDATE outputs SET spent_by = transaction_id WHERE output_id = ?")
        .bind(w.output)
        .execute(storage.writer())
        .await
        .unwrap();
    assert_eq!(locked(owner).await.unwrap().len(), 1);
    assert!(locked(caller).await.unwrap().is_empty());
    let error = bsv_sdk::Error::WalletError("Insufficient funds".into());
    let GateError::Refused(refused) = refusal(caller, &Assessment::default(), &error, 1_000).await
    else {
        panic!("expected a funds refusal");
    };
    assert!(refused.held.is_empty(), "{refused:?}");
}
