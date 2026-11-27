//! A coin the store marks not spendable with no spender, at the gate's own
//! `create_action` and at the served `POST /createAction` (#646, the hold
//! of #165): four answers that had no test of their own.
//!
//! 1. the signed form under `acceptDelayedBroadcast`: refused, the coin
//!    named `lost`, nothing allocated, and nothing stored for the
//!    monitor's send pass to post;
//! 2. the same action posted at once: refused, the coin named `lost`;
//! 3. a received coin the spend guard itself holds (its own write), under
//!    `allowPending: received`, posted at once and accepting delayed
//!    broadcast: refused both times, the guard's row untouched;
//! 4. the served road, `options.acceptDelayedBroadcast: true`: the status,
//!    the code and the `held[]` entry.
//!
//! Every coin here is one the wallet can sign for (a BRC-29 derivation of
//! its own root key), so a gate that offered it would sign a spend and
//! store it `sending`: the refusal is the gate's, never a signer's.
//!
//! 1 to 3 run over `MockWalletServices`: a mock header service and a mock
//! broadcaster that accepts, so a post would be counted. 4 runs over the
//! served router, whose state takes the concrete `Services` and no other:
//! built by `test_support::mock_services`, as this line's served tests
//! build it. Its header service and its ARC are one loopback stub of the
//! test's own, which records every request; it has no `BananaBlocks` and no
//! `GorillaPool`; and `seal_network` sends every request of the process
//! that is not to loopback to a closed loopback port, so nothing leaves
//! the machine.

use super::{
    create_action, lose, opts, pay, AllowPending, CreateActionArgs, GateError, GateOptions,
    StorageSqlx, Wallet,
};
use crate::test_support::coins::{mined_coin_with_lock, storage_for};
use crate::tracker_host::tests::{header, header_with_hash};
use bsv_sdk::primitives::PrivateKey;
use bsv_sdk::wallet::CreateActionOptions;
use bsv_wallet_toolbox::services::mock::MockWalletServices;
use bsv_wallet_toolbox::{WalletServices, WalletStorageProvider};
use chrono::Utc;
use std::sync::Arc;

/// The derivation of the signable coins.
const PREFIX: &str = "dGVzdC1wcmVmaXg=";
const SUFFIX: &str = "dGVzdC1zdWZmaXg=";
/// The block of the lost coin, and the header service's tip.
const MINED_AT: u32 = 800;
const TIP: u32 = 900;
const LOST: &str = "lost: the store marks it not spendable";

/// The P2PKH lock (hex) of the key `root` signs a coin of the derivation
/// [`PREFIX`], [`SUFFIX`] with (BRC-29, counterparty self).
fn signable_lock(root: &PrivateKey) -> String {
    use bsv_sdk::wallet::{Counterparty, KeyDeriver, Protocol, SecurityLevel};
    let key = KeyDeriver::new(Some(root.clone()))
        .derive_private_key(
            &Protocol::new(SecurityLevel::Counterparty, "3241645161d8"),
            &format!("{PREFIX} {SUFFIX}"),
            &Counterparty::Self_,
        )
        .unwrap();
    format!("76a914{}88ac", hex::encode(key.public_key().hash160()))
}

/// A store of `root` whose only coin is mined at [`MINED_AT`] (its stored
/// proof a one-leaf block, its root its txid), 50,000 satoshis, signable,
/// and marked lost: `spendable = 0`, no spender. (storage, txid, id.)
async fn a_store_with_one_lost_coin(root: &PrivateKey) -> (StorageSqlx, String, i64) {
    let (storage, user, basket) = storage_for(&root.public_key().to_hex()).await;
    let lock = signable_lock(root);
    let (txid, id) =
        mined_coin_with_lock(&storage, (user, basket), 0xd5, 50_000, MINED_AT, &lock).await;
    sqlx::query(
        "UPDATE outputs SET derivation_prefix = ?, derivation_suffix = ? WHERE output_id = ?",
    )
    .bind(PREFIX)
    .bind(SUFFIX)
    .bind(id)
    .execute(storage.writer())
    .await
    .unwrap();
    lose(&storage, id).await;
    (storage, txid, id)
}

/// The wallet over [`a_store_with_one_lost_coin`], its header service the
/// mock's (the coin's block and the tip) and its broadcaster a mock that
/// accepts.
async fn a_wallet_with_one_lost_coin() -> (Wallet<StorageSqlx, MockWalletServices>, String, i64) {
    let root = PrivateKey::random();
    let (storage, txid, id) = a_store_with_one_lost_coin(&root).await;
    let services = MockWalletServices::builder().post_beef_success().build();
    services.set_header_for_height(header(MINED_AT, &txid, 0));
    services.set_tip_header(header_with_hash(TIP, &"0c".repeat(32), 0));
    let wallet = Wallet::new(Some(root), storage, services).await.unwrap();
    (wallet, txid, id)
}

/// A payment of 1,000 satoshis, signed (`signAndProcess: true`), accepting
/// delayed broadcast or posted at once.
fn signed(delayed: bool) -> CreateActionArgs {
    let mut args = pay(1_000);
    args.options = Some(CreateActionOptions {
        sign_and_process: Some(true),
        accept_delayed_broadcast: Some(delayed),
        ..Default::default()
    });
    args
}

/// Every transaction, every request and every output's state, row by row:
/// what a refusal must leave as it was, and what the monitor's send pass
/// reads (a request `unsent`, its transaction `sending`).
async fn records(storage: &StorageSqlx) -> Vec<String> {
    let mut rows: Vec<String> = sqlx::query_scalar(
        "SELECT 'tx ' || transaction_id || ' ' || status || ' ' || COALESCE(txid, '-') \
         || ' outgoing=' || is_outgoing FROM transactions ORDER BY transaction_id",
    )
    .fetch_all(storage.pool())
    .await
    .unwrap();
    rows.extend(
        sqlx::query_scalar::<_, String>(
            "SELECT 'req ' || proven_tx_req_id || ' ' || status || ' ' || txid \
             || ' attempts=' || attempts FROM proven_tx_reqs ORDER BY proven_tx_req_id",
        )
        .fetch_all(storage.pool())
        .await
        .unwrap(),
    );
    rows.extend(
        sqlx::query_scalar::<_, String>(
            "SELECT 'out ' || output_id || ' spendable=' || spendable || ' spent_by=' \
             || COALESCE(spent_by, '-') FROM outputs ORDER BY output_id",
        )
        .fetch_all(storage.pool())
        .await
        .unwrap(),
    );
    rows
}

/// The monitor's send pass over `storage` (`send_waiting_transactions`),
/// posting through a mock broadcaster of its own that accepts: how many
/// transactions it sent, and how many posts the broadcaster took. Every
/// request the store holds is aged a day before it, so none is passed over
/// as too recent (the control below is posted by it).
async fn send_pass(storage: &StorageSqlx) -> (usize, usize) {
    let aged = Utc::now() - chrono::Duration::days(1);
    sqlx::query("UPDATE proven_tx_reqs SET created_at = ?, updated_at = ?")
        .bind(aged)
        .bind(aged)
        .execute(storage.writer())
        .await
        .unwrap();
    let broadcaster = Arc::new(MockWalletServices::builder().post_beef_success().build());
    WalletStorageProvider::set_services(storage, broadcaster.clone() as Arc<dyn WalletServices>);
    let sent = bsv_wallet_toolbox::MonitorStorage::send_waiting_transactions(
        storage,
        std::time::Duration::ZERO,
    )
    .await
    .unwrap();
    println!("the send pass: {sent:?}");
    let sent = sent
        .and_then(|s| s.send_with_results)
        .map_or(0, |results| results.len());
    (sent, broadcaster.call_count("post_beef"))
}

/// `args` through the gate's `create_action`: refused for funds with
/// nothing offered, and `held[]` the coin `txid`.0 alone, its word `lost`
/// and its reason [`LOST`]. A panic says what was built instead.
async fn assert_refused_lost(
    wallet: &Wallet<StorageSqlx, MockWalletServices>,
    args: CreateActionArgs,
    gate: &GateOptions,
    txid: &str,
) {
    let refusal = match create_action(wallet, args, "gate.local", gate).await {
        Err(GateError::Refused(refusal)) => refusal,
        Ok(made) => panic!(
            "the coin was offered: txid {:?}, a template {}",
            made.txid.map(hex::encode),
            made.signable_transaction.is_some()
        ),
        Err(other) => panic!("not the gate's refusal: {other}"),
    };
    println!("{} {refusal}", refusal.code());
    assert_eq!(refusal.code(), "INSUFFICIENT_FUNDS");
    assert_eq!(refusal.offered_satoshis, 0, "{refusal}");
    assert_eq!(refusal.held.len(), 1, "{refusal}");
    let coin = &refusal.held[0];
    assert_eq!(coin.outpoint, format!("{txid}.0"));
    assert_eq!((coin.word.as_str(), coin.reason.as_str()), ("lost", LOST));
}

/// (1) #646, the signed form: the only coin is mined and marked not
/// spendable with no spender, and the action is signed and accepts delayed
/// broadcast. The gate refuses for funds with nothing offered, `held[]` names
/// the coin `lost`, nothing is allocated, and the transactions and the
/// requests are as before the call: no `sending` transaction and no
/// `unsent` request, so the monitor's send pass, run here against a mock
/// broadcaster of its own, posts nothing. (The stable line's 0.18.9, which has
/// no hold, stores a signed spend of the coin `sending`, and that pass
/// posts it: #646's table.)
#[tokio::test]
async fn a_signed_action_under_delayed_broadcast_is_refused_lost_and_leaves_nothing_to_send() {
    let (wallet, lost, id) = a_wallet_with_one_lost_coin().await;
    let before = records(wallet.storage()).await;

    assert_refused_lost(&wallet, signed(true), &opts(1, true), &lost).await;

    let after = records(wallet.storage()).await;
    assert_eq!(after, before, "the transactions, the requests, the coins");
    assert_eq!(
        after,
        vec![
            format!("tx 1 completed {lost} outgoing=0"),
            format!("out {id} spendable=0 spent_by=-"),
        ],
        "one transaction, the coin's own; no request; the coin not allocated"
    );
    assert_eq!(wallet.services().call_count("post_beef"), 0);

    // The monitor's send pass, its broadcaster a mock that accepts.
    let (sent, posts) = send_pass(wallet.storage()).await;
    assert_eq!((sent, posts), (0, 0), "the send pass sent or posted");
    assert_eq!(records(wallet.storage()).await, before);
}

/// The control of (1): the same store with the flag on again. The signed
/// action under delayed broadcast is built over the coin, its transaction
/// stored `sending` with an `unsent` request and nothing posted at the
/// call; the monitor's send pass, run as in (1), then posts it once. So
/// (1)'s coin is one the wallet signs for, and (1)'s send pass is one that
/// posts what the store holds for it.
#[tokio::test]
async fn the_control_the_same_coin_spendable_is_signed_stored_sending_and_sent_by_the_pass() {
    let (wallet, coin_txid, id) = a_wallet_with_one_lost_coin().await;
    sqlx::query("UPDATE outputs SET spendable = 1 WHERE output_id = ?")
        .bind(id)
        .execute(wallet.storage().writer())
        .await
        .unwrap();

    let made = create_action(&wallet, signed(true), "gate.local", &opts(1, true))
        .await
        .unwrap();
    let txid = hex::encode(made.txid.expect("a signed transaction"));
    let after = records(wallet.storage()).await;
    println!("the control: {after:?}");
    assert_eq!(after[0], format!("tx 1 completed {coin_txid} outgoing=0"));
    assert_eq!(after[1], format!("tx 2 sending {txid} outgoing=1"));
    assert!(
        after[2].starts_with(&format!("req 1 unsent {txid} ")),
        "{after:?}"
    );
    assert!(after.contains(&format!("out {id} spendable=0 spent_by=2")));
    assert_eq!(wallet.services().call_count("post_beef"), 0);

    assert_eq!(send_pass(wallet.storage()).await, (1, 1), "sent, posted");
}

/// (2) #646, posted at once: the same action with no delayed broadcast.
/// Nothing is offered and `held[]` names the coin `lost`. The stable
/// line's 0.18.9 refuses this action for funds as well, with `50000 sats
/// offered, 0 coin(s) held back`, the coin unnamed (#646's table): what
/// the hold adds here is the word. Nothing allocated, stored or posted.
#[tokio::test]
async fn a_signed_action_posted_at_once_is_refused_lost_by_name() {
    let (wallet, lost, _) = a_wallet_with_one_lost_coin().await;
    let before = records(wallet.storage()).await;

    assert_refused_lost(&wallet, signed(false), &opts(1, true), &lost).await;
    assert_eq!(records(wallet.storage()).await, before);
    assert_eq!(wallet.services().call_count("post_beef"), 0);
}

/// The guard's rows: (output id, txid, held at).
async fn guard_rows(storage: &StorageSqlx) -> Vec<(i64, String, String)> {
    sqlx::query_as("SELECT output_id, txid, held_at FROM spend_guard_holds ORDER BY output_id")
        .fetch_all(storage.pool())
        .await
        .unwrap()
}

/// A wallet whose one coin is a received payment (taken in through
/// `internalizeAction`: purpose `receive`, its transaction `unproven`)
/// whose immediate post drew no accepting word: the note the guard reads,
/// `immediateBroadcastHint`, last on its request, the request `unsent`.
/// The coin is still spendable: the guard has not run. (wallet, the
/// payment's txid, the coin's id.)
async fn a_wallet_with_a_received_coin_no_broadcaster_took(
) -> (Wallet<StorageSqlx, MockWalletServices>, String, i64) {
    let root = PrivateKey::random();
    let (wallet, txids) = super::received_rule::wallet_with_received(&root, &[9_000]).await;
    let received = txids[0].clone();
    let note = serde_json::json!({"notes": [{
        "when": Utc::now().to_rfc3339(),
        "what": crate::server::post_word::IMMEDIATE_HINT,
        "outcome": "invalidTx",
        "attempts": 1,
        "words": ["ARC answered 465"],
        "nextReaskMinutes": 2
    }]})
    .to_string();
    let noted =
        sqlx::query("UPDATE proven_tx_reqs SET status = 'unsent', history = ? WHERE txid = ?")
            .bind(&note)
            .bind(&received)
            .execute(wallet.storage().writer())
            .await
            .unwrap()
            .rows_affected();
    assert_eq!(noted, 1, "the received transaction's request");
    let id: i64 = sqlx::query_scalar("SELECT output_id FROM outputs WHERE txid = ? AND vout = 0")
        .bind(&received)
        .fetch_one(wallet.storage().pool())
        .await
        .unwrap();
    let spendable: i64 = sqlx::query_scalar("SELECT spendable FROM outputs WHERE output_id = ?")
        .bind(id)
        .fetch_one(wallet.storage().pool())
        .await
        .unwrap();
    assert_eq!(spendable, 1, "spendable before the guard");
    (wallet, received, id)
}

/// (3) #646, the spend guard's hold: a received coin whose transaction
/// drew no accepting word on its immediate post. `spend_guard::run` holds
/// it by its own write: the flag off, no spender, its row in
/// `spend_guard_holds`. Then a signed action under `allowPending:
/// received`, the word that would offer a received coin with no proof,
/// posted at once and accepting delayed broadcast: refused both times,
/// nothing allocated, stored or posted, the guard's row untouched, and the
/// guard's next run quiet.
///
/// The refusal today: `INSUFFICIENT_FUNDS`, nothing offered, the coin in
/// `held[]` with the word `lost` and the reason `lost: the store marks it
/// not spendable`, the same word and reason as a coin `reconcile-outputs`
/// or `cleanup-abandoned` relinquished. The gate reads the flag before the
/// coin's creator and does not read `spend_guard_holds`, so the refusal
/// does not say the hold is the guard's, nor that it ends once a status
/// source holds the transaction.
#[tokio::test]
async fn a_received_coin_the_spend_guard_holds_is_refused_under_the_received_word() {
    let (wallet, received, id) = a_wallet_with_a_received_coin_no_broadcaster_took().await;
    let storage = wallet.storage();
    let posts_at_receipt = wallet.services().call_count("post_beef");

    // The guard's own write.
    let report = crate::spend_guard::run(storage).await.unwrap();
    assert_eq!(report.held, vec![received.clone()]);
    assert!(report.released.is_empty() && report.dropped.is_empty());
    let guarded = guard_rows(storage).await;
    assert_eq!(guarded.len(), 1);
    assert_eq!((guarded[0].0, &guarded[0].1), (id, &received));
    let before = records(storage).await;
    assert!(
        before.contains(&format!("out {id} spendable=0 spent_by=-")),
        "{before:?}"
    );

    let word = opts(1, true).with_allow_received(true);
    assert_eq!(word.allow_pending, AllowPending::Received);
    for delayed in [false, true] {
        println!("guard-held, delayed={delayed}:");
        assert_refused_lost(&wallet, signed(delayed), &word, &received).await;
        assert_eq!(records(storage).await, before, "delayed={delayed}");
        assert_eq!(guard_rows(storage).await, guarded, "delayed={delayed}");
    }
    assert_eq!(
        wallet.services().call_count("post_beef"),
        posts_at_receipt,
        "nothing posted"
    );

    // The guard again, as the next spend runs it: nothing to hold, nothing
    // released, its row as it wrote it.
    let again = crate::spend_guard::run(storage).await.unwrap();
    assert!(again.is_quiet(), "{again:?}");
    assert_eq!(guard_rows(storage).await, guarded);
    assert_eq!(records(storage).await, before);
}

/// The requests a served wallet's stub was sent: `METHOD path`, in order.
type Seen = Arc<std::sync::Mutex<Vec<String>>>;

/// The served router over [`a_store_with_one_lost_coin`], as `serve`
/// builds it. Its state is the concrete `Services` (the handler takes no
/// other), built by `test_support::mock_services`: this file's head says
/// what it can reach. The header service and the ARC are one loopback stub
/// that answers the coin's block and the tip and records every request.
/// (router, the served wallet, the coin's txid, the stub's record.)
async fn a_served_wallet_with_one_lost_coin() -> (
    axum::Router,
    crate::server::handlers::WalletState,
    String,
    Seen,
) {
    use crate::test_support::{header_answer, mock_services, serve_router};
    let root = PrivateKey::random();
    let (storage, lost, _) = a_store_with_one_lost_coin(&root).await;
    let roots = std::collections::HashMap::from([(MINED_AT, lost.clone())]);
    let seen = Seen::default();
    let stub = {
        let seen = seen.clone();
        axum::Router::new().fallback(move |method: axum::http::Method, uri: axum::http::Uri| {
            seen.lock()
                .unwrap()
                .push(format!("{method} {}", uri.path()));
            let answer = header_answer(&roots, &uri);
            async move { answer }
        })
    };
    // The stub serves for the test's life: its task ends with the runtime.
    let (url, _task) = serve_router(stub).await;
    let wallet: crate::server::handlers::WalletState = Arc::new(
        Wallet::new(Some(root), storage, mock_services(&url))
            .await
            .unwrap(),
    );
    let config = crate::server::ServerConfig::admitting(["test.local"]).unwrap();
    let router = crate::server::make_router(wallet.clone(), config);
    (router, wallet, lost, seen)
}

/// `POST /createAction` of one payment of 1,000 satoshis with `options`,
/// from the admitted origin: the status and the JSON answer.
async fn post_create_action(
    router: &axum::Router,
    options: &serde_json::Value,
) -> (u16, serde_json::Value) {
    use tower::ServiceExt;
    let body = serde_json::json!({
        "description": "pay someone",
        "outputs": [{"lockingScript": super::LOCK, "satoshis": 1000,
            "outputDescription": "payment"}],
        "options": options,
    });
    let request = axum::http::Request::post("/createAction")
        .header(axum::http::header::CONTENT_TYPE, "application/json")
        .header(axum::http::header::ORIGIN, "http://test.local")
        .body(axum::body::Body::from(body.to_string()))
        .unwrap();
    let response = router.clone().oneshot(request).await.unwrap();
    super::served_response(response).await
}

/// (4) #646, the served road: `POST /createAction` with
/// `options.acceptDelayedBroadcast: true` on a store seeded as in (1),
/// through the served router and its handler (the spend guard's run, then
/// the gate), signed by default and with `signAndProcess` spelled out.
/// The answer is 402 `INSUFFICIENT_FUNDS`, `offeredSatoshis` 0, and
/// `held[]` carries the coin with the word `lost`; the store is as before
/// and no post reached the broadcaster's stub.
#[tokio::test]
async fn the_served_create_action_under_delayed_broadcast_answers_402_with_the_coin_lost() {
    let (router, wallet, lost, seen) = a_served_wallet_with_one_lost_coin().await;
    let before = records(wallet.storage()).await;

    for options in [
        serde_json::json!({"acceptDelayedBroadcast": true}),
        serde_json::json!({"acceptDelayedBroadcast": true, "signAndProcess": true}),
    ] {
        let (status, answer) = post_create_action(&router, &options).await;
        println!("served, {options}: HTTP {status} {answer}");
        assert_eq!(status, 402, "{answer}");
        assert_eq!(answer["code"], "INSUFFICIENT_FUNDS");
        assert_eq!(answer["offeredSatoshis"], 0);
        assert_eq!(answer["held"].as_array().unwrap().len(), 1, "{answer}");
        let coin = &answer["held"][0];
        assert_eq!(coin["outpoint"], format!("{lost}.0"));
        assert_eq!(coin["word"], "lost");
        assert_eq!(coin["reason"], LOST);
        assert_eq!(coin["satoshis"], 50_000);
        assert_eq!(coin["origin"], "received");
        assert!(
            answer["message"]
                .as_str()
                .unwrap()
                .contains(&format!("{lost}.0 50000 sats: lost")),
            "{answer}"
        );
        assert_eq!(records(wallet.storage()).await, before, "the store");
    }
    let seen = seen.lock().unwrap().clone();
    println!("the stub was asked: {seen:?}");
    assert!(
        seen.iter().all(|request| request.starts_with("GET ")),
        "a post reached the stub: {seen:?}"
    );
}
