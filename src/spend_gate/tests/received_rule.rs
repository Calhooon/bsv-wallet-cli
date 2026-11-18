//! The toolbox's received-coin rule over coins its own `internalizeAction`
//! took in (purpose `receive`, the transaction `unproven`): the gate's
//! offer and the label `underReceivedRule` (#627, the port of the stable
//! line's 0.18.8).

use super::{assess, create_action, CreateActionArgs, GateError, StorageSqlx, UserCoins, Wallet};
use super::{opts, pay};
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
pub(super) const TIP: u32 = 900_000;
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

/// #627, the test that decides: a received coin the gate admits WITHOUT
/// the caller's word, because the tracker's row holds a checked proof at
/// depth (the tracker's own pass, its proof handed to the storage and
/// deferred), while the toolbox's own record of the transaction is not yet
/// `completed`. A plain spend with no `allowPending`. At the port's base
/// (e058eab) the gate offers the coin and the toolbox's selection passes it
/// over for want of its label: the gate's 402 with 9,000 offered and
/// nothing held.
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

    let err = create_action(&wallet, pay_signed(1_000, None), "gate.local", &gate)
        .await
        .expect_err("at the base the toolbox passes the offered coin over");
    let GateError::Refused(refusal) = &err else {
        panic!("not the gate's refusal: {err}");
    };
    assert_eq!(refusal.code(), "INSUFFICIENT_FUNDS");
    assert_eq!(
        err.to_string(),
        "Insufficient funds: no offerable coin covers the action (Insufficient funds: need 1005, have 0; 9000 sats offered, 0 coin(s) held back)"
    );
    assert_eq!(refusal.offered_satoshis, 9_000);
    assert!(refusal.held.is_empty());
    assert_eq!(spender_of(&wallet, received).await, None);
}
