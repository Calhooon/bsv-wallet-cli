//! #201 and #221: all gate reads belong to the user whose action is at the
//! door: the coins (#201), and the lineage walk, the rule labels and the
//! frozen set (#221).

use super::*;
use axum::response::IntoResponse;

struct TwoUsers {
    wallet: Wallet<StorageSqlx, MockWalletServices>,
    owner: Wallet<StorageSqlx, MockWalletServices>,
    txid: String,
    output: i64,
}

impl TwoUsers {
    async fn user_ids(&self) -> (i64, i64) {
        let caller = UserCoins::from_wallet(&self.wallet).await.unwrap();
        let owner = UserCoins::from_wallet(&self.owner).await.unwrap();
        (caller.user_id(), owner.user_id())
    }
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

/// The harness with a coin of this user's at `txid`, made or received:
/// (the harness, this user's id, the other user's id).
async fn with_my_coin(txid: &str, made: bool) -> (TwoUsers, i64, i64) {
    let w = two_users().await;
    let (me, owner) = w.user_ids().await;
    let storage = w.wallet.storage();
    let basket = storage.find_or_create_default_basket(me).await.unwrap();
    coin(storage, (me, basket.basket_id), txid, made, None, 6_000, 3).await;
    (w, me, owner)
}

/// This user's allocation read under `gate`.
async fn my_assessment(w: &TwoUsers, me: i64, gate: &GateOptions) -> Assessment {
    let coins = UserCoins::new(w.wallet.storage(), me);
    assess(coins, w.wallet.services(), gate, None)
        .await
        .unwrap()
}

/// #221: another user's failed transaction whose output's `spent_by` names
/// this user's action (a pair planted by hand: one transaction with inputs
/// from two users of the store) is no dead candidate of this user's walk,
/// so this user's change is offered through both doors and names nothing.
#[tokio::test]
async fn another_users_failed_transaction_leaves_our_child_offered() {
    let child = "c1".repeat(32);
    let (w, me, _) = with_my_coin(&child, true).await;
    let storage = w.wallet.storage();
    fail(storage, &w.txid).await;
    sqlx::query(
        "UPDATE outputs SET spent_by = (SELECT transaction_id FROM transactions WHERE txid = ?) \
         WHERE output_id = ?",
    )
    .bind(&child)
    .bind(w.output)
    .execute(storage.writer())
    .await
    .unwrap();
    let gate = opts(1, true);
    let a = my_assessment(&w, me, &gate).await;
    assert_eq!(reason_of(&a, &child), None, "{:?}", a.held);
    assert_eq!(a.offered.len(), 1);
    let (coins, named) = (UserCoins::new(storage, me), [(child.clone(), 0)]);
    let named = assess_named(coins, w.wallet.services(), &gate, &named, &[], None, false).await;
    assert!(named.unwrap().is_empty());
    let lineage = lineage_candidates(storage, me).await.unwrap();
    assert!(lineage.dead(&HashMap::new()).is_empty());
}

/// #221: another user's action of the same txid carrying the received
/// rule's label never reaches this user's rules (`rules_of`, the read
/// respend decides the replacement's rule from), while the owner's read
/// still names it.
#[tokio::test]
async fn another_users_label_never_reaches_our_respend() {
    let shared = "c2".repeat(32);
    let (w, me, owner) = with_my_coin(&shared, true).await;
    let storage = w.wallet.storage();
    let theirs: i64 = sqlx::query_scalar(
        "INSERT INTO transactions (user_id, status, reference, is_outgoing, satoshis, version, lock_time, description, txid, created_at, updated_at) \
         VALUES (?, 'unproven', 'ref-theirs', 1, 0, 1, 0, 'gate', ?, ?, ?) RETURNING transaction_id",
    )
    .bind(owner)
    .bind(&shared)
    .bind(Utc::now())
    .bind(Utc::now())
    .fetch_one(storage.writer())
    .await
    .unwrap();
    let mut conn = storage.writer().acquire().await.unwrap();
    label_in(&mut conn, owner, theirs, RECEIVED_PENDING)
        .await
        .unwrap();
    drop(conn);
    let theirs = rules_of(storage, owner).await.unwrap();
    assert_eq!(
        theirs.get(&shared).map(String::as_str),
        Some(RECEIVED_PENDING)
    );
    let ours = rules_of(storage, me).await.unwrap();
    assert!(ours.is_empty(), "{ours:?}");
}

/// #221: another user's unlabelled unsigned template of the same shape
/// cannot make this user's labelled template ambiguous in their listing.
#[tokio::test]
async fn another_users_template_never_hides_our_rule() {
    let w = two_users().await;
    let (me, owner) = w.user_ids().await;
    let storage = w.wallet.storage();
    for (user, reference) in [(me, "our-template"), (owner, "their-template")] {
        let id: i64 = sqlx::query_scalar(
            "INSERT INTO transactions (user_id, status, reference, is_outgoing, satoshis, version, lock_time, description, created_at, updated_at) \
             VALUES (?, 'unsigned', ?, 1, -1000, 1, 0, 'pay someone', ?, ?) RETURNING transaction_id",
        )
        .bind(user)
        .bind(reference)
        .bind(Utc::now())
        .bind(Utc::now())
        .fetch_one(storage.writer())
        .await
        .unwrap();
        if user == me {
            let mut conn = storage.writer().acquire().await.unwrap();
            label_in(&mut conn, me, id, RECEIVED_PENDING).await.unwrap();
        }
    }
    let list = w
        .wallet
        .list_actions(list_args(), "gate.local")
        .await
        .unwrap();
    let rules = action_rules(storage, me).await.unwrap();
    assert_eq!(
        listed(serde_json::to_value(list).unwrap(), &rules),
        vec![("unsigned".to_string(), Some(RECEIVED_PENDING.to_string()))]
    );
}

/// #221: a 0.15 record naming another user's signed action, by txid or
/// reference, answers for that owner only and never enters our action rules.
#[tokio::test]
async fn another_users_015_records_never_reach_our_rules() {
    let w = two_users().await;
    let (me, owner) = w.user_ids().await;
    let storage = w.wallet.storage();
    let by_reference = "c4".repeat(32);
    let basket = storage.find_or_create_default_basket(owner).await.unwrap();
    coin(
        storage,
        (owner, basket.basket_id),
        &by_reference,
        true,
        None,
        1_000,
        1,
    )
    .await;
    a_015_rule_table(storage).await;
    for (txid, reference) in [
        (Some(w.txid.to_ascii_uppercase()), None),
        (None, Some(format!("ref-{}", &by_reference[..8]))),
    ] {
        sqlx::query(
            "INSERT INTO spend_gate_actions (txid, rule, recorded_at, reference) VALUES (?, ?, 0, ?)",
        )
        .bind(txid)
        .bind(RECEIVED_PENDING)
        .bind(reference)
        .execute(storage.writer())
        .await
        .unwrap();
    }
    let theirs = action_rules(storage, owner).await.unwrap();
    assert_eq!(
        theirs.by_txid,
        HashMap::from([
            (w.txid, RECEIVED_PENDING.to_string()),
            (by_reference, RECEIVED_PENDING.to_string()),
        ])
    );
    let ours = action_rules(storage, me).await.unwrap();
    assert!(ours.by_txid.is_empty(), "{:?}", ours.by_txid);
    assert!(ours.by_template.is_empty(), "{:?}", ours.by_template);
}

/// #221: another user's frozen coin is not in this user's frozen set, and
/// this user's own frozen coin is still held frozen; the record's count
/// stays every user's.
#[tokio::test]
async fn another_users_frozen_coin_is_not_in_our_frozen_set() {
    let mine = "c3".repeat(32);
    let (w, me, owner) = with_my_coin(&mine, false).await;
    let storage = w.wallet.storage();
    freeze(storage, &w.txid, 7).await;
    freeze(storage, &mine, 8).await;
    let frozen = |user| crate::tracker_host::frozen_outpoints(storage, user);
    let set = |txid: &str| std::collections::HashSet::from([(txid.to_string(), 0)]);
    assert_eq!(frozen(me).await.unwrap(), set(&mine));
    assert_eq!(frozen(owner).await.unwrap(), set(&w.txid));
    assert_eq!(crate::tracker_host::frozen_count(storage).await.unwrap(), 2);
    let gate = opts(1, true).with_allow_received(true);
    let a = my_assessment(&w, me, &gate).await;
    assert!(a.offered.is_empty());
    assert_eq!(
        (
            a.held.len(),
            a.held[0].word.as_str(),
            a.held[0].reason.as_str()
        ),
        (1, "frozen", "frozen: never offered")
    );
}
