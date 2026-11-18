//! The toolbox's received-coin rule over coins its own `internalizeAction`
//! took in (purpose `receive`, the transaction `unproven`): the gate's
//! offer and the label `underReceivedRule` (#627, the port of the stable
//! line's 0.18.8).

use super::{
    action_rules, assess, create_action, create_action_from, reference_string, AllowPending,
    CreateActionArgs, GateError, StorageSqlx, UserCoins, Wallet, RECEIVED_PENDING_LABEL,
    TOOLBOX_RECEIVED_LABEL,
};
use super::{labels_of, list_args, opts, pay, reason_of, STORED_RECEIVED_LABEL};
use crate::tracker_host::tests::{header, header_with_hash};
use bsv_sdk::primitives::PrivateKey;
use bsv_sdk::transaction::MerklePath;
use bsv_sdk::wallet::{CreateActionOptions, WalletInterface};
use bsv_wallet_toolbox::services::mock::MockWalletServices;
use chrono::Utc;

/// The derivation of the received payments below.
const RECEIVED_PREFIX: &str = "cmVjZWl2ZWQtcHJlZml4";
const RECEIVED_SUFFIX: &str = "cmVjZWl2ZWQtc3VmZml4";
/// The header service's tip, and the storage's tracker's height.
const TIP: u32 = 900_000;
/// The payments' parents' proof: after Genesis, at least 100 below the tip
/// (bsv-rs #734).
const PARENT_HEIGHT: u32 = 620_639;

/// A wallet payment to `root` of `satoshis` from a random sender, as an
/// AtomicBEEF: the parent whole beside its BUMP at [`PARENT_HEIGHT`], the
/// payment itself
/// unproven. (BEEF, txid, the sender's key.)
fn received_payment(root: &PrivateKey, seed: u8, satoshis: u64) -> (Vec<u8>, String, String) {
    use bsv_sdk::transaction::{Beef, Transaction, TransactionInput, TransactionOutput};
    use bsv_sdk::wallet::{Counterparty, KeyDeriver, Protocol, SecurityLevel};
    let sender = PrivateKey::random().public_key();
    let derived = KeyDeriver::new(Some(root.clone()))
        .derive_public_key(
            &Protocol::new(SecurityLevel::Counterparty, "3241645161d8"),
            &format!("{RECEIVED_PREFIX} {RECEIVED_SUFFIX}"),
            &Counterparty::Other(sender.clone()),
            true,
        )
        .unwrap();
    let mut lock = vec![0x76, 0xa9, 0x14];
    lock.extend_from_slice(&derived.hash160());
    lock.extend_from_slice(&[0x88, 0xac]);
    // The parent's output is one a script `00` spends (the served door
    // tests' `beef_paying`), so the toolbox's spend verifier passes the
    // payment's input.
    let anyone = format!("14{}757551", "00".repeat(20));
    let input = |txid: &str| {
        let mut input = TransactionInput::new(txid.to_string(), 0);
        input.unlocking_script = Some(bsv_sdk::script::UnlockingScript::from_hex("00").unwrap());
        input
    };
    let mut parent = Transaction::new();
    parent.add_input(input(&hex::encode([seed; 32]))).unwrap();
    parent
        .add_output(TransactionOutput::new(
            satoshis + 1_000,
            bsv_sdk::script::LockingScript::from_hex(&anyone).unwrap(),
        ))
        .unwrap();
    let parent_txid = parent.id();
    let mut child = Transaction::new();
    child.set_version(1);
    child.add_input(input(&parent_txid)).unwrap();
    child
        .add_output(TransactionOutput::new(
            satoshis,
            bsv_sdk::script::LockingScript::from_binary(&lock).unwrap(),
        ))
        .unwrap();
    let txid = child.id();
    let mut beef = Beef::new();
    let bump = beef.merge_bump(MerklePath::from_coinbase_txid(&parent_txid, PARENT_HEIGHT));
    beef.merge_raw_tx(parent.to_binary(), Some(bump)).unwrap();
    beef.merge_raw_tx(child.to_binary(), None).unwrap();
    (beef.to_binary_atomic(&txid).unwrap(), txid, sender.to_hex())
}

/// A wallet of `root` holding one received unproven payment per entry of
/// `amounts`, each taken in through the toolbox's own `internalizeAction`
/// (purpose `receive`, its transaction `unproven`), the mock's broadcasters
/// accepting each. The header service's tip is [`TIP`] and carries none of
/// them; the storage's proof gate is never raised (no monitor runs, as
/// under `serve`). The payments' txids, in order.
pub(super) async fn wallet_with_received(
    root: &PrivateKey,
    amounts: &[u64],
) -> (Wallet<StorageSqlx, MockWalletServices>, Vec<String>) {
    use crate::test_support::TestChainTracker;
    use bsv_sdk::wallet::{InternalizeActionArgs, InternalizeOutput, WalletPayment};
    use bsv_wallet_toolbox::services::mock::{success_post_beef_result, MockResponse};
    let (storage, _, _) =
        crate::test_support::coins::storage_for(&root.public_key().to_hex()).await;
    let payments: Vec<_> = amounts
        .iter()
        .enumerate()
        .map(|(i, s)| received_payment(root, 0xe0 + i as u8, *s))
        .collect();
    let txids: Vec<String> = payments.iter().map(|p| p.1.clone()).collect();
    let refs: Vec<&str> = txids.iter().map(String::as_str).collect();
    storage
        .set_chain_tracker(std::sync::Arc::new(TestChainTracker::always_valid(TIP)))
        .await;
    let services = MockWalletServices::builder()
        .post_beef_response(MockResponse::Success(vec![success_post_beef_result(
            "arc", &refs,
        )]))
        .build();
    services.set_tip_header(header_with_hash(TIP, &"0e".repeat(32), 0));
    let wallet = Wallet::new(Some(root.clone()), storage, services)
        .await
        .unwrap();
    for (beef, _, sender) in payments {
        wallet
            .internalize_action(
                InternalizeActionArgs {
                    tx: beef,
                    outputs: vec![InternalizeOutput {
                        output_index: 0,
                        protocol: "wallet payment".to_string(),
                        payment_remittance: Some(WalletPayment {
                            derivation_prefix: RECEIVED_PREFIX.to_string(),
                            derivation_suffix: RECEIVED_SUFFIX.to_string(),
                            sender_identity_key: sender,
                        }),
                        insertion_remittance: None,
                    }],
                    description: "a payment received".to_string(),
                    labels: None,
                    seek_permission: None,
                },
                Some("gate.local"),
            )
            .await
            .unwrap();
    }
    for txid in &txids {
        let (purpose, status): (String, String) = sqlx::query_as(
            "SELECT o.purpose, t.status FROM outputs o JOIN transactions t \
             ON o.transaction_id = t.transaction_id WHERE o.txid = ? AND o.vout = 0",
        )
        .bind(txid)
        .fetch_one(wallet.storage().pool())
        .await
        .unwrap();
        assert_eq!((purpose.as_str(), status.as_str()), ("receive", "unproven"));
    }
    (wallet, txids)
}

/// A payment of `satoshis`, signed and not sent.
pub(super) fn pay_signed(satoshis: u64, labels: Option<Vec<String>>) -> CreateActionArgs {
    let mut args = pay(satoshis);
    args.labels = labels;
    args.options = Some(CreateActionOptions {
        sign_and_process: Some(true),
        no_send: Some(true),
        ..Default::default()
    });
    args
}

/// The action holding the received coin `txid:0`, if any.
pub(super) async fn spender_of(
    wallet: &Wallet<StorageSqlx, MockWalletServices>,
    txid: &str,
) -> Option<i64> {
    sqlx::query_scalar("SELECT spent_by FROM outputs WHERE txid = ? AND vout = 0")
        .bind(txid)
        .fetch_one(wallet.storage().pool())
        .await
        .unwrap()
}

/// The tracker's own pass over the received payment `txid`: its proof at
/// `height` is asked for once its age is due and checks against the
/// header service's header; the wallet's storage is handed the proof and,
/// its proof gate never raised (no monitor ran, as under `serve`), defers
/// it. The tracker's row then holds a checked proof while the toolbox's
/// transaction stays `unproven`: the state the gate reads as mined.
pub(super) async fn tracker_proves_received(
    wallet: &Wallet<StorageSqlx, MockWalletServices>,
    txid: &str,
    height: u32,
) {
    use crate::tracker_host::tests::{opts as tick_opts, path_of, proof_answer, FixedClock};
    let services = MockWalletServices::builder()
        .get_merkle_path_response(proof_answer(Some(path_of(txid, height))))
        .build();
    services.set_header_for_height(header(height, txid, 0));
    services.set_tip_header(header_with_hash(TIP, &"0e".repeat(32), 0));
    wallet
        .services()
        .set_header_for_height(header(height, txid, 0));
    // The broadcaster's acceptance of the payment, as the toolbox's
    // broadcast memory records it when `internalizeAction` posts it.
    wallet
        .storage()
        .record_broadcast_status(
            txid,
            "arcade",
            bsv_wallet_toolbox::BROADCAST_STATUS_ACCEPTED,
        )
        .await
        .unwrap();
    let now = Utc::now().timestamp() as u64;
    let mut mined = Vec::new();
    let mut stored = Vec::new();
    for at in [now, now + 601, now + 1_300] {
        let report =
            crate::tracker_host::tick(wallet.storage(), &services, &FixedClock(at), &tick_opts())
                .await
                .unwrap();
        mined.extend(report.mined);
        stored.extend(report.stored);
    }
    assert_eq!(
        mined,
        vec![txid.to_string()],
        "the tracker checked the proof"
    );
    assert_eq!(stored.len(), 1, "{stored:?}");
    assert!(
        stored[0].contains("DeferredAboveProcessedHeight"),
        "{stored:?}"
    );
    let (status, proven): (String, Option<i64>) =
        sqlx::query_as("SELECT status, proven_tx_id FROM transactions WHERE txid = ?")
            .bind(txid)
            .fetch_one(wallet.storage().pool())
            .await
            .unwrap();
    assert_eq!((status.as_str(), proven), ("unproven", None));
}

/// The labels stored on the action `txid`, sorted.
async fn labels_of_txid(storage: &StorageSqlx, txid: &str) -> Vec<String> {
    sqlx::query_scalar(
        "SELECT l.label FROM transactions t \
         JOIN tx_labels_map m ON m.transaction_id = t.transaction_id AND m.is_deleted = 0 \
         JOIN tx_labels l ON l.tx_label_id = m.tx_label_id AND l.is_deleted = 0 \
         WHERE t.txid = ? ORDER BY l.label",
    )
    .bind(txid)
    .fetch_all(storage.pool())
    .await
    .unwrap()
}

/// The opening line of the refusal with no word over a wallet whose one
/// coin is a received unproven payment of 9,000 (a line per held coin
/// follows): the gate's own answer, unchanged by #627.
const NO_WORD_REFUSAL: &str = "Insufficient funds: no offerable coin covers the action (Insufficient funds: need 1005, have 0; 0 sats offered, 1 coin(s) held back)";

fn assert_no_word_refusal(err: GateError, txid: &str) {
    let GateError::Refused(refusal) = err else {
        panic!("not the gate's refusal: {err}");
    };
    assert_eq!(refusal.code(), "INSUFFICIENT_FUNDS");
    let message = refusal.to_string();
    assert_eq!(message.lines().next(), Some(NO_WORD_REFUSAL), "{message}");
    assert_eq!(message.lines().count(), 2, "{message}");
    assert_eq!(refusal.offered_satoshis, 0);
    assert_eq!(refusal.held.len(), 1);
    assert!(refusal.held[0].outpoint.starts_with(txid));
    assert_eq!(
        refusal.held[0].reason,
        "depth 1: 0 blocks; received, no proof; offered under allowPending: received"
    );
}

/// The listing a caller of the served `/listActions` sees of the actions
/// labelled `label`, with `includeLabels` (`handlers::list_actions`,
/// `with_rules`).
async fn served_listing(
    wallet: &Wallet<StorageSqlx, MockWalletServices>,
    label: &str,
) -> serde_json::Value {
    let mut args = list_args();
    args.labels = vec![label.to_string()];
    args.include_labels = Some(true);
    let list = wallet.list_actions(args, Some("gate.local")).await.unwrap();
    let coins = UserCoins::from_wallet(wallet).await.unwrap();
    let rules = action_rules(wallet.storage(), coins.user_id())
        .await
        .unwrap();
    crate::server::handlers::with_rules(serde_json::to_value(&list).unwrap(), &rules)
}

/// #627, the test that decides: a received coin the gate admits WITHOUT
/// the caller's word, because the tracker's row holds a checked proof at
/// depth (the tracker's own pass, its proof handed to the storage and
/// deferred), while the toolbox's own record of the transaction is not yet
/// `completed`. A plain spend with no `allowPending`. At the port's base
/// (e058eab) the gate offered the coin and the toolbox's selection passed
/// it over for want of its label: the gate's 402, "Insufficient funds: no
/// offerable coin covers the action (Insufficient funds: need 1005, have 0;
/// 9000 sats offered, 0 coin(s) held back)", nothing held. Now the offer
/// carries the toolbox's label alone, and the spend signs.
#[tokio::test]
async fn a_received_coin_the_gate_reads_as_mined_with_no_word() {
    let root = PrivateKey::random();
    let (wallet, txids) = wallet_with_received(&root, &[9_000]).await;
    let received = &txids[0];
    tracker_proves_received(&wallet, received, TIP - 1).await;

    let gate = opts(1, true);
    let coins = UserCoins::from_wallet(&wallet).await.unwrap();
    let read = assess(coins, wallet.services(), &gate, None).await.unwrap();
    assert_eq!(read.offered.len(), 1, "the gate offers it with no word");
    assert!(read.held.is_empty());

    let made = create_action(&wallet, pay_signed(1_000, None), "gate.local", &gate)
        .await
        .expect("the offered coin is selected");
    let txid = bsv_sdk::primitives::to_hex(&made.txid.expect("signed"));
    assert!(made.tx.is_some(), "the spend signs");
    assert!(spender_of(&wallet, received).await.is_some());
    // The toolbox's label alone: no word was given, so no record of one.
    assert_eq!(
        labels_of_txid(wallet.storage(), &txid).await,
        vec![STORED_RECEIVED_LABEL]
    );
}

/// The same road on the `--from` door (`create_action_from`, CLI #313),
/// which hands its offer to the toolbox on its own: the named received
/// coin, read as mined with no word, is spent and its action carries the
/// toolbox's label alone.
#[tokio::test]
async fn the_from_door_carries_the_toolbox_label_alone_for_a_coin_read_as_mined() {
    let root = PrivateKey::random();
    let (wallet, txids) = wallet_with_received(&root, &[9_000]).await;
    let received = &txids[0];
    tracker_proves_received(&wallet, received, TIP - 1).await;
    let made = create_action_from(
        &wallet,
        pay_signed(1_000, Some(vec!["mine".to_string()])),
        "gate.local",
        &opts(1, true),
        &[(received.clone(), 0)],
    )
    .await
    .expect("the named coin is spent");
    let txid = bsv_sdk::primitives::to_hex(&made.txid.expect("signed"));
    assert!(spender_of(&wallet, received).await.is_some());
    assert_eq!(
        labels_of_txid(wallet.storage(), &txid).await,
        vec!["mine", STORED_RECEIVED_LABEL]
    );
}

/// Under the door's word the gate offers a received unproven coin and
/// passes the toolbox's label beside its own, so the coin is selected and
/// the spend signs; with no word the gate's answer is unchanged and the
/// coin stays spendable and unreserved. The gate's offer alone, without
/// the toolbox's label, is refused by the toolbox.
#[tokio::test]
async fn under_the_word_a_received_coin_is_selected_and_the_spend_signs() {
    let root = PrivateKey::random();
    let (wallet, txids) = wallet_with_received(&root, &[9_000]).await;
    let received = &txids[0];
    let gate = || opts(1, true);

    let err = create_action(&wallet, pay_signed(1_000, None), "gate.local", &gate())
        .await
        .expect_err("held with no word");
    assert_no_word_refusal(err, received);
    assert_eq!(spender_of(&wallet, received).await, None);

    // The gate's offer of the coin without the toolbox's label: passed over.
    let coins = UserCoins::from_wallet(&wallet).await.unwrap();
    let offered = assess(
        coins,
        wallet.services(),
        &gate().with_allow_received(true),
        None,
    )
    .await
    .unwrap();
    assert_eq!(offered.offered.len(), 1);
    let bare = wallet
        .create_action_offering(
            pay_signed(1_000, Some(vec![RECEIVED_PENDING_LABEL.to_string()])),
            Some("gate.local"),
            &offered.offer(),
            &[],
        )
        .await
        .expect_err("the toolbox passes the coin over without its label");
    let bsv_sdk::Error::WalletRefusal { class, message } = &bare else {
        panic!("not the toolbox's typed refusal: {bare:?}");
    };
    assert_eq!(*class, bsv_sdk::WalletRefusalClass::InsufficientFunds);
    assert_eq!(message, "Insufficient funds: need 1005, have 0");
    assert_eq!(spender_of(&wallet, received).await, None);

    let made = create_action(
        &wallet,
        pay_signed(1_000, Some(vec!["mine".to_string()])),
        "gate.local",
        &gate().with_allow_received(true),
    )
    .await
    .expect("selected under the word");
    let txid = bsv_sdk::primitives::to_hex(&made.txid.expect("signed"));
    assert!(made.tx.is_some(), "the spend signs");
    assert!(spender_of(&wallet, received).await.is_some());
    assert_eq!(
        labels_of_txid(wallet.storage(), &txid).await,
        vec!["mine", RECEIVED_PENDING_LABEL, STORED_RECEIVED_LABEL]
    );
}

/// The scoped word over two received unproven coins the toolbox took in:
/// only the named coin is offered and selected; the other, held "not
/// named", stays unspent; a payment the named coin cannot cover is the
/// gate's refusal with the other listed as not named.
#[tokio::test]
async fn under_the_scoped_word_only_the_named_received_coin_is_selected() {
    let root = PrivateKey::random();
    let (wallet, txids) = wallet_with_received(&root, &[5_000, 9_000]).await;
    let (named, other) = (&txids[0], &txids[1]);
    let scoped =
        || opts(1, true).with_allow_pending(AllowPending::ReceivedOnly(vec![(named.clone(), 0)]));

    let err = create_action(&wallet, pay_signed(7_000, None), "gate.local", &scoped())
        .await
        .expect_err("the named coin alone does not cover 7,000");
    let GateError::Refused(refusal) = err else {
        panic!("not the gate's refusal: {err}");
    };
    assert_eq!(refusal.code(), "INSUFFICIENT_FUNDS");
    assert_eq!(refusal.offered_satoshis, 5_000);
    let held = refusal
        .held
        .iter()
        .find(|c| c.outpoint.starts_with(other.as_str()))
        .unwrap();
    assert_eq!(
        held.reason,
        "depth 1: 0 blocks; received, no proof; not named under allowPending: received"
    );

    let made = create_action(&wallet, pay_signed(1_000, None), "gate.local", &scoped())
        .await
        .expect("the named coin funds it");
    let txid = bsv_sdk::primitives::to_hex(&made.txid.expect("signed"));
    assert!(made.tx.is_some(), "the spend signs");
    assert!(spender_of(&wallet, named).await.is_some());
    assert_eq!(spender_of(&wallet, other).await, None);
    assert_eq!(
        labels_of_txid(wallet.storage(), &txid).await,
        vec![RECEIVED_PENDING_LABEL, STORED_RECEIVED_LABEL]
    );
}

/// A caller's own `underReceivedRule`, in spellings the toolbox reads as
/// the rule, without the door's word, over a received unproven coin: the
/// labels are stripped before the door, the coin is not offered, and the
/// answer is the one with no label. Under the word the label is the
/// gate's, written once.
#[tokio::test]
async fn a_callers_own_received_rule_label_does_not_get_the_coin() {
    let root = PrivateKey::random();
    let (wallet, txids) = wallet_with_received(&root, &[9_000]).await;
    let received = &txids[0];
    let forged = || {
        Some(vec![
            TOOLBOX_RECEIVED_LABEL.to_string(),
            "underreceivedrule".to_string(),
            "UNDERRECEIVEDRULE".to_string(),
            " underReceivedRule\t".to_string(),
            "\u{feff}underReceivedRule\n".to_string(),
        ])
    };
    let err = create_action(
        &wallet,
        pay_signed(1_000, forged()),
        "gate.local",
        &opts(1, true),
    )
    .await
    .expect_err("a caller's label grants nothing");
    assert_no_word_refusal(err, received);
    assert_eq!(spender_of(&wallet, received).await, None);

    let made = create_action(
        &wallet,
        pay_signed(1_000, forged()),
        "gate.local",
        &opts(1, true).with_allow_received(true),
    )
    .await
    .expect("under the word");
    let txid = bsv_sdk::primitives::to_hex(&made.txid.expect("signed"));
    assert_eq!(
        labels_of_txid(wallet.storage(), &txid).await,
        vec![RECEIVED_PENDING_LABEL, STORED_RECEIVED_LABEL]
    );
}

/// The same spend as the test that decides, once the toolbox has completed
/// the coin's transaction (its proof gate raised and the proof stored, as
/// the monitor does under `daemon`): the toolbox selects the coin without
/// its label, and the action carries the caller's label alone.
#[tokio::test]
async fn a_received_coin_the_toolbox_completed_is_spent_with_no_label() {
    use bsv_wallet_toolbox::MonitorStorage;
    let root = PrivateKey::random();
    let (wallet, txids) = wallet_with_received(&root, &[9_000]).await;
    let received = &txids[0];
    tracker_proves_received(&wallet, received, TIP - 1).await;
    let mut tracker = crate::test_support::TestChainTracker::new(TIP);
    tracker.add_root(TIP - 1, received.clone());
    wallet
        .storage()
        .set_chain_tracker(std::sync::Arc::new(tracker))
        .await;
    wallet
        .storage()
        .set_max_acceptable_proof_height(TIP - 1)
        .await
        .unwrap();
    let outcome = wallet
        .storage()
        .ingest_merkle_proof(
            received,
            &MerklePath::from_coinbase_txid(received, TIP - 1)
                .to_binary()
                .unwrap(),
            TIP - 1,
            &"bb".repeat(32),
            Some(received),
        )
        .await
        .unwrap();
    assert!(
        format!("{outcome:?}").starts_with("Ingested"),
        "{outcome:?}"
    );
    let status: String = sqlx::query_scalar("SELECT status FROM transactions WHERE txid = ?")
        .bind(received)
        .fetch_one(wallet.storage().pool())
        .await
        .unwrap();
    assert_eq!(status, "completed");

    let made = create_action(
        &wallet,
        pay_signed(1_000, Some(vec!["mine".to_string()])),
        "gate.local",
        &opts(1, true),
    )
    .await
    .expect("a completed received coin is selected as before");
    let txid = bsv_sdk::primitives::to_hex(&made.txid.expect("signed"));
    assert!(spender_of(&wallet, received).await.is_some());
    assert_eq!(labels_of_txid(wallet.storage(), &txid).await, vec!["mine"]);
}

/// An offer that holds no received coin the toolbox has not completed
/// carries no label, whatever else the wallet holds: a mined coin is
/// offered and funds the spend while a received unproven coin, held with
/// no word, is outside the offer.
#[tokio::test]
async fn an_offer_with_no_received_unproven_coin_carries_no_label() {
    let root = PrivateKey::random();
    let (wallet, txids) = wallet_with_received(&root, &[9_000]).await;
    let received = &txids[0];
    let coins = UserCoins::from_wallet(&wallet).await.unwrap();
    let basket = wallet
        .storage()
        .find_or_create_default_basket(coins.user_id())
        .await
        .unwrap()
        .basket_id;
    let (mined, mined_id) = crate::test_support::coins::mined_coin(
        wallet.storage(),
        (coins.user_id(), basket),
        0xa5,
        20_000,
        TIP - 50,
    )
    .await;
    wallet
        .services()
        .set_header_for_height(header(TIP - 50, &mined, 0));

    let gate = opts(1, true);
    let read = assess(coins, wallet.services(), &gate, None).await.unwrap();
    assert_eq!(read.offer(), vec![mined_id]);
    assert!(
        reason_of(&read, received).is_some(),
        "the received coin is held"
    );

    // A template: the mined coin's lock is no key of this wallet's.
    let mut args = pay(1_000);
    args.options = Some(CreateActionOptions {
        sign_and_process: Some(false),
        ..Default::default()
    });
    let made = create_action(&wallet, args, "gate.local", &gate)
        .await
        .expect("the mined coin funds it");
    let reference = reference_string(&made.signable_transaction.expect("a template").reference);
    let spent: Option<i64> = sqlx::query_scalar("SELECT spent_by FROM outputs WHERE output_id = ?")
        .bind(mined_id)
        .fetch_one(wallet.storage().pool())
        .await
        .unwrap();
    assert!(spent.is_some(), "the mined coin funds it");
    assert_eq!(spender_of(&wallet, received).await, None);
    assert!(labels_of(wallet.storage(), &reference).await.is_empty());
}

/// Under the word, a received coin the gate reads as mined is spent with
/// the word's two labels, the toolbox's written once.
#[tokio::test]
async fn under_the_word_a_received_coin_read_as_mined_carries_the_label_once() {
    let root = PrivateKey::random();
    let (wallet, txids) = wallet_with_received(&root, &[9_000]).await;
    tracker_proves_received(&wallet, &txids[0], TIP - 1).await;
    let made = create_action(
        &wallet,
        pay_signed(1_000, None),
        "gate.local",
        &opts(1, true).with_allow_received(true),
    )
    .await
    .unwrap();
    let txid = bsv_sdk::primitives::to_hex(&made.txid.unwrap());
    assert_eq!(
        labels_of_txid(wallet.storage(), &txid).await,
        vec![RECEIVED_PENDING_LABEL, STORED_RECEIVED_LABEL]
    );
}

/// What a caller of `listActions` sees of an action made under the word:
/// with `includeLabels` both labels the gate passed beside the caller's,
/// and `underRule` `received-pending`.
#[tokio::test]
async fn a_served_listing_shows_the_received_rule_label_under_the_word() {
    let root = PrivateKey::random();
    let (wallet, _) = wallet_with_received(&root, &[9_000]).await;
    let made = create_action(
        &wallet,
        pay_signed(1_000, Some(vec!["mine".to_string()])),
        "gate.local",
        &opts(1, true).with_allow_received(true),
    )
    .await
    .unwrap();
    let txid = bsv_sdk::primitives::to_hex(&made.txid.unwrap());
    let shown = served_listing(&wallet, TOOLBOX_RECEIVED_LABEL).await;
    let actions = shown["actions"].as_array().unwrap();
    assert_eq!(actions.len(), 1, "{shown}");
    assert_eq!(actions[0]["txid"], txid);
    assert_eq!(actions[0]["underRule"], "received-pending");
    let mut labels: Vec<String> = serde_json::from_value(actions[0]["labels"].clone()).unwrap();
    labels.sort();
    assert_eq!(
        labels,
        vec!["mine", RECEIVED_PENDING_LABEL, STORED_RECEIVED_LABEL]
    );
}

/// What a caller of `listActions` sees of the decided spend, made with no
/// word: with `includeLabels` the toolbox's label alone beside the
/// caller's, and no `underRule` (the caller asked for no rule).
#[tokio::test]
async fn a_served_listing_shows_the_toolbox_label_alone_with_no_word() {
    let root = PrivateKey::random();
    let (wallet, txids) = wallet_with_received(&root, &[9_000]).await;
    tracker_proves_received(&wallet, &txids[0], TIP - 1).await;
    let made = create_action(
        &wallet,
        pay_signed(1_000, Some(vec!["mine".to_string()])),
        "gate.local",
        &opts(1, true),
    )
    .await
    .unwrap();
    let txid = bsv_sdk::primitives::to_hex(&made.txid.unwrap());
    let shown = served_listing(&wallet, TOOLBOX_RECEIVED_LABEL).await;
    let actions = shown["actions"].as_array().unwrap();
    assert_eq!(actions.len(), 1, "{shown}");
    assert_eq!(actions[0]["txid"], txid);
    assert!(actions[0].get("underRule").is_none(), "{shown}");
    let mut labels: Vec<String> = serde_json::from_value(actions[0]["labels"].clone()).unwrap();
    labels.sort();
    assert_eq!(labels, vec!["mine", STORED_RECEIVED_LABEL]);
}
