//! What the served `/createAction` does AFTER `Wallet::create_action` returns.
//!
//! # Why this exists
//!
//! The handler used to run `BroadcastVerifier::verify` INLINE after every
//! immediate broadcast, so a clean broadcast still paid for a full presence
//! probe before the client got its txid (measured on beta.zanaadu.com: 5.5 s
//! for a publish whose build+sign took 6 ms). That loop was there because the
//! toolbox once classified a definitive ARC/Arcade rejection (465 fee too low,
//! `REJECTED`) as a *transient* service error and handed back a phantom txid.
//!
//! Since bsv-wallet-toolbox-rs 0.3.55 a definitive rejection is a permanent
//! failure of `create_action` itself (tx failed, inputs released, the caller
//! gets an error), and an ACCEPTED broadcast is reported in
//! `sendWithResults[].status == "unproven"`. So the handler can now:
//!
//! * answer **immediately** when the broadcaster ACCEPTED the transaction, and
//!   run the presence verification in the **background** — on a definitive
//!   `Rejected` verdict the tx is failed and its inputs released through the
//!   toolbox's RELEASE-RULE path (`retire_undeliverable_txid`: alive-check,
//!   per-input chain verification, own outputs unspendable);
//! * keep the **inline** verification ONLY for an **ambiguous** result — a
//!   transient service fault where the transaction may or may not be out, the
//!   one case where the client must not be told "sent" on a coin flip.
//!
//! The decision (`disposition`) and the follow-up mechanics (`follow_up`) are
//! pure over injected closures so the timing contract is testable without a
//! network or a funded wallet.

use std::future::Future;

use bsv_sdk::wallet::{CreateActionResult, SendWithResultStatus};
use bsv_wallet_toolbox::{RetireOutcome, StorageSqlx, WalletServices};

use crate::broadcast_verify::BroadcastVerification;

/// How the immediate broadcast of a `create_action` went, as far as the wallet
/// could tell.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BroadcastDisposition {
    /// Nothing was broadcast: `noSend`, `acceptDelayedBroadcast`, or a deferred
    /// signing (`signableTransaction`) flow. Nothing to verify.
    NotBroadcast,
    /// The broadcaster ACCEPTED the transaction (2xx with a non-fatal status):
    /// `sendWithResults` reports it `unproven`.
    Accepted,
    /// The wallet could not tell: a transient service fault left the entry at
    /// `sending`, or the toolbox reported nothing for this txid.
    Ambiguous,
}

/// Classify a `create_action` result. Only immediate broadcasts are ever
/// verified; an accepted one is verified in the background, an ambiguous one
/// inline.
pub fn disposition(
    result: &CreateActionResult,
    no_send: bool,
    accept_delayed: bool,
) -> BroadcastDisposition {
    if no_send || accept_delayed || result.signable_transaction.is_some() {
        return BroadcastDisposition::NotBroadcast;
    }
    let Some(txid) = result.txid else {
        return BroadcastDisposition::NotBroadcast;
    };
    let entry = result
        .send_with_results
        .as_ref()
        .and_then(|rs| rs.iter().find(|r| r.txid == txid));
    match entry {
        Some(r) if matches!(r.status, SendWithResultStatus::Unproven) => {
            BroadcastDisposition::Accepted
        }
        _ => BroadcastDisposition::Ambiguous,
    }
}

/// What the request handler does once the follow-up returns.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FollowUp {
    /// Answer the client with the txid.
    Proceed,
    /// The inline verification found the transaction definitively absent; the
    /// tx has been retired and the client must see a failure.
    Rejected,
}

/// Run the post-broadcast verification for `txid` according to `disposition`.
///
/// * `NotBroadcast`: nothing happens.
/// * `Accepted`: `verify` is SPAWNED; this function returns at once. If the
///   verdict is `Rejected`, `on_rejected` runs in that background task.
/// * `Ambiguous`: `verify` is AWAITED; a `Rejected` verdict runs `on_rejected`
///   before returning [`FollowUp::Rejected`].
///
/// A `Confirmed` or `Inconclusive` verdict never touches the transaction: an
/// absence must be definitive before any money moves (see `broadcast_verify`).
pub async fn follow_up<V, VF, R, RF>(
    disposition: BroadcastDisposition,
    txid: String,
    verify: V,
    on_rejected: R,
) -> FollowUp
where
    V: FnOnce(String) -> VF + Send + 'static,
    VF: Future<Output = BroadcastVerification> + Send + 'static,
    R: FnOnce(String) -> RF + Send + 'static,
    RF: Future<Output = ()> + Send + 'static,
{
    match disposition {
        BroadcastDisposition::NotBroadcast => FollowUp::Proceed,
        BroadcastDisposition::Accepted => {
            tokio::spawn(async move {
                match verify(txid.clone()).await {
                    BroadcastVerification::Confirmed => {
                        tracing::debug!(txid = %txid, "post-broadcast verification: present");
                    }
                    BroadcastVerification::Inconclusive => {
                        tracing::info!(
                            txid = %txid,
                            "post-broadcast verification: inconclusive (kept; the reconcile sweeps decide later)"
                        );
                    }
                    BroadcastVerification::Rejected => {
                        tracing::warn!(
                            txid = %txid,
                            "post-broadcast verification: ACCEPTED by the broadcaster but definitively absent afterwards — retiring"
                        );
                        on_rejected(txid).await;
                    }
                }
            });
            FollowUp::Proceed
        }
        BroadcastDisposition::Ambiguous => match verify(txid.clone()).await {
            BroadcastVerification::Rejected => {
                tracing::warn!(
                    txid = %txid,
                    "ambiguous broadcast verified definitively absent — retiring and failing the request"
                );
                on_rejected(txid).await;
                FollowUp::Rejected
            }
            BroadcastVerification::Confirmed | BroadcastVerification::Inconclusive => {
                FollowUp::Proceed
            }
        },
    }
}

/// Fail a definitively-absent broadcast and give its inputs back, through the
/// toolbox's RELEASE-RULE path (`StorageSqlx::retire_undeliverable_txid`): the
/// tx is alive-checked first, each input is released only on its own chain
/// verification, the tx's own outputs go unspendable and the tx is `failed`
/// with its req `invalid` (the unfail canary keeps re-checking it). Every
/// outcome is logged; nothing here can fail the request that already answered.
pub async fn retire_rejected_broadcast(
    storage: &StorageSqlx,
    services: &dyn WalletServices,
    txid: &str,
) -> Option<RetireOutcome> {
    match storage
        .retire_undeliverable_txid(services, txid, "invalid")
        .await
    {
        Ok(Some(outcome @ RetireOutcome::Retired { restored, kept })) => {
            tracing::warn!(
                txid = %txid,
                restored,
                kept,
                "retired absent broadcast: tx failed, {} input(s) released (chain-verified), {} kept locked",
                restored,
                kept
            );
            Some(outcome)
        }
        Ok(Some(RetireOutcome::Alive)) => {
            tracing::info!(
                txid = %txid,
                "absent per the probe but known to the status service — kept (promoted), nothing released"
            );
            Some(RetireOutcome::Alive)
        }
        Ok(None) => {
            tracing::warn!(
                txid = %txid,
                "absent broadcast has no proven_tx_req — nothing to retire"
            );
            None
        }
        Err(e) => {
            tracing::error!(txid = %txid, error = %e, "failed to retire absent broadcast");
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bsv_sdk::wallet::{SendWithResult, SignableTransaction};
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;
    use std::time::{Duration, Instant};

    const TXID_HEX: &str = "0000000000000000000000000000000000000000000000000000000000000001";

    fn txid_bytes() -> [u8; 32] {
        let mut t = [0u8; 32];
        t[31] = 1;
        t
    }

    fn result(status: Option<SendWithResultStatus>) -> CreateActionResult {
        let txid = txid_bytes();
        CreateActionResult {
            txid: Some(txid),
            tx: Some(vec![1, 0, 0, 0]),
            no_send_change: None,
            send_with_results: status.map(|s| vec![SendWithResult { txid, status: s }]),
            signable_transaction: None,
            input_type: None,
            inputs: None,
            reference_number: None,
            beef: None,
        }
    }

    // ---- disposition ------------------------------------------------------

    #[test]
    fn accepted_broadcast_reports_unproven() {
        assert_eq!(
            disposition(&result(Some(SendWithResultStatus::Unproven)), false, false),
            BroadcastDisposition::Accepted
        );
    }

    #[test]
    fn transient_fault_leaves_sending_which_is_ambiguous() {
        assert_eq!(
            disposition(&result(Some(SendWithResultStatus::Sending)), false, false),
            BroadcastDisposition::Ambiguous
        );
    }

    #[test]
    fn no_report_for_our_txid_is_ambiguous() {
        // An older toolbox (or a missing entry) must fall back to the safe path.
        assert_eq!(
            disposition(&result(None), false, false),
            BroadcastDisposition::Ambiguous
        );
        let mut other = result(Some(SendWithResultStatus::Unproven));
        other.send_with_results.as_mut().unwrap()[0].txid = [9u8; 32];
        assert_eq!(
            disposition(&other, false, false),
            BroadcastDisposition::Ambiguous
        );
    }

    #[test]
    fn nothing_to_verify_when_nothing_was_broadcast() {
        let accepted = result(Some(SendWithResultStatus::Unproven));
        assert_eq!(
            disposition(&accepted, true, false),
            BroadcastDisposition::NotBroadcast
        );
        assert_eq!(
            disposition(&accepted, false, true),
            BroadcastDisposition::NotBroadcast
        );
        let mut deferred = result(Some(SendWithResultStatus::Unproven));
        deferred.signable_transaction = Some(SignableTransaction {
            tx: vec![1],
            reference: b"ref".to_vec(),
        });
        assert_eq!(
            disposition(&deferred, false, false),
            BroadcastDisposition::NotBroadcast
        );
        let mut no_txid = result(Some(SendWithResultStatus::Unproven));
        no_txid.txid = None;
        assert_eq!(
            disposition(&no_txid, false, false),
            BroadcastDisposition::NotBroadcast
        );
    }

    // ---- follow_up timing contract -----------------------------------------

    #[tokio::test]
    async fn accepted_broadcast_answers_before_the_verification_finishes() {
        // The verifier takes 300 ms and comes back Rejected. The handler must
        // return long before that (the probe was SPAWNED, not awaited), and the
        // retire hook must still fire afterwards, in the background.
        let (tx, rx) = tokio::sync::oneshot::channel::<String>();
        let started = Instant::now();
        let outcome = follow_up(
            BroadcastDisposition::Accepted,
            TXID_HEX.to_string(),
            |_txid| async {
                tokio::time::sleep(Duration::from_millis(300)).await;
                BroadcastVerification::Rejected
            },
            move |txid| async move {
                tx.send(txid).ok();
            },
        )
        .await;
        let elapsed = started.elapsed();
        assert_eq!(outcome, FollowUp::Proceed);
        assert!(
            elapsed < Duration::from_millis(150),
            "an accepted broadcast must not wait for the verifier (took {:?})",
            elapsed
        );
        let retired = tokio::time::timeout(Duration::from_secs(2), rx)
            .await
            .expect("the background verification must run to its verdict")
            .expect("retire hook fired");
        assert_eq!(retired, TXID_HEX);
        assert!(
            started.elapsed() >= Duration::from_millis(300),
            "the verdict arrives only after the probe window"
        );
    }

    #[tokio::test]
    async fn accepted_and_present_never_retires() {
        let fired = Arc::new(AtomicBool::new(false));
        let f = fired.clone();
        let outcome = follow_up(
            BroadcastDisposition::Accepted,
            TXID_HEX.to_string(),
            |_txid| async { BroadcastVerification::Confirmed },
            move |_txid| async move {
                f.store(true, Ordering::SeqCst);
            },
        )
        .await;
        assert_eq!(outcome, FollowUp::Proceed);
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert!(!fired.load(Ordering::SeqCst));
    }

    #[tokio::test]
    async fn accepted_and_inconclusive_never_retires() {
        // An absence that is not definitive must never release money.
        let fired = Arc::new(AtomicBool::new(false));
        let f = fired.clone();
        follow_up(
            BroadcastDisposition::Accepted,
            TXID_HEX.to_string(),
            |_txid| async { BroadcastVerification::Inconclusive },
            move |_txid| async move {
                f.store(true, Ordering::SeqCst);
            },
        )
        .await;
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert!(!fired.load(Ordering::SeqCst));
    }

    #[tokio::test]
    async fn ambiguous_broadcast_is_verified_inline_and_a_rejection_fails_the_request() {
        // The one case that still blocks: the wallet could not tell whether the
        // tx went out. The verifier is AWAITED, and a definitive absence retires
        // the tx BEFORE the handler answers with a failure.
        let fired = Arc::new(AtomicBool::new(false));
        let f = fired.clone();
        let started = Instant::now();
        let outcome = follow_up(
            BroadcastDisposition::Ambiguous,
            TXID_HEX.to_string(),
            |_txid| async {
                tokio::time::sleep(Duration::from_millis(200)).await;
                BroadcastVerification::Rejected
            },
            move |txid| async move {
                assert_eq!(txid, TXID_HEX);
                f.store(true, Ordering::SeqCst);
            },
        )
        .await;
        assert_eq!(outcome, FollowUp::Rejected);
        assert!(
            started.elapsed() >= Duration::from_millis(200),
            "an ambiguous broadcast must wait for the verdict"
        );
        assert!(
            fired.load(Ordering::SeqCst),
            "the tx is retired before the failure is returned"
        );
    }

    #[tokio::test]
    async fn ambiguous_broadcast_proceeds_on_confirmed_or_inconclusive() {
        for verdict in [
            BroadcastVerification::Confirmed,
            BroadcastVerification::Inconclusive,
        ] {
            let fired = Arc::new(AtomicBool::new(false));
            let f = fired.clone();
            let outcome = follow_up(
                BroadcastDisposition::Ambiguous,
                TXID_HEX.to_string(),
                move |_txid| async move { verdict },
                move |_txid| async move {
                    f.store(true, Ordering::SeqCst);
                },
            )
            .await;
            assert_eq!(outcome, FollowUp::Proceed);
            assert!(!fired.load(Ordering::SeqCst));
        }
    }

    #[tokio::test]
    async fn nothing_broadcast_means_nothing_verified() {
        let probed = Arc::new(AtomicBool::new(false));
        let p = probed.clone();
        let outcome = follow_up(
            BroadcastDisposition::NotBroadcast,
            TXID_HEX.to_string(),
            move |_txid| async move {
                p.store(true, Ordering::SeqCst);
                BroadcastVerification::Rejected
            },
            |_txid| async {},
        )
        .await;
        assert_eq!(outcome, FollowUp::Proceed);
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert!(!probed.load(Ordering::SeqCst));
    }

    // ---- the retire hook against real storage ------------------------------

    /// The exact storage the handler's hook runs against: an `unproven` tx
    /// (broadcast accepted), req `unmined`, one input locked, one change output.
    async fn seeded_storage() -> (StorageSqlx, i64, i64) {
        use bsv_wallet_toolbox::WalletStorageWriter;

        let storage = StorageSqlx::in_memory().await.expect("in-memory storage");
        let storage_key = "02".to_string() + &"ab".repeat(32);
        storage
            .migrate("follow-up-tests", &storage_key)
            .await
            .expect("migrate");
        storage.make_available().await.expect("make_available");
        let identity = "02".to_string() + &"cd".repeat(32);
        let (user, _) = storage.find_or_insert_user(&identity).await.expect("user");
        let basket = storage
            .find_or_create_default_basket(user.user_id)
            .await
            .expect("basket");
        let now = chrono::Utc::now();
        let lock = hex::decode("76a914dbc0a7c84983c5bf199b7b2d41b3acf0408ee5aa88ac").unwrap();
        let parent_txid = "aa".repeat(32);

        let parent_id = sqlx::query(
            "INSERT INTO transactions (user_id, status, reference, is_outgoing, satoshis, version, lock_time, description, txid, raw_tx, created_at, updated_at) \
             VALUES (?, 'completed', 'parent', 0, 50000, 1, 0, 'parent', ?, X'01000000', ?, ?)",
        )
        .bind(user.user_id)
        .bind(&parent_txid)
        .bind(now)
        .bind(now)
        .execute(storage.pool())
        .await
        .unwrap()
        .last_insert_rowid();
        let tx_id = sqlx::query(
            "INSERT INTO transactions (user_id, status, reference, is_outgoing, satoshis, version, lock_time, description, txid, raw_tx, created_at, updated_at) \
             VALUES (?, 'unproven', 'ours', 1, -2000, 1, 0, 'ours', ?, X'01000000', ?, ?)",
        )
        .bind(user.user_id)
        .bind(TXID_HEX)
        .bind(now)
        .bind(now)
        .execute(storage.pool())
        .await
        .unwrap()
        .last_insert_rowid();
        let input_id = sqlx::query(
            "INSERT INTO outputs (user_id, transaction_id, basket_id, vout, satoshis, locking_script, txid, type, spendable, change, spent_by, provided_by, purpose, output_description, created_at, updated_at) \
             VALUES (?, ?, ?, 0, 50000, ?, ?, 'P2PKH', 0, 1, ?, 'storage', 'change', 'input', ?, ?)",
        )
        .bind(user.user_id)
        .bind(parent_id)
        .bind(basket.basket_id)
        .bind(&lock)
        .bind(&parent_txid)
        .bind(tx_id)
        .bind(now)
        .bind(now)
        .execute(storage.pool())
        .await
        .unwrap()
        .last_insert_rowid();
        let own_id = sqlx::query(
            "INSERT INTO outputs (user_id, transaction_id, basket_id, vout, satoshis, locking_script, txid, type, spendable, change, provided_by, purpose, output_description, created_at, updated_at) \
             VALUES (?, ?, ?, 0, 48000, ?, ?, 'P2PKH', 1, 1, 'storage', 'change', 'our change', ?, ?)",
        )
        .bind(user.user_id)
        .bind(tx_id)
        .bind(basket.basket_id)
        .bind(&lock)
        .bind(TXID_HEX)
        .bind(now)
        .bind(now)
        .execute(storage.pool())
        .await
        .unwrap()
        .last_insert_rowid();
        sqlx::query(
            "INSERT INTO proven_tx_reqs (txid, status, attempts, history, notified, notify, raw_tx, created_at, updated_at) \
             VALUES (?, 'unmined', 0, '{}', 0, '{}', X'01000000', ?, ?)",
        )
        .bind(TXID_HEX)
        .bind(now)
        .bind(now)
        .execute(storage.pool())
        .await
        .unwrap();
        (storage, input_id, own_id)
    }

    async fn output_state(storage: &StorageSqlx, id: i64) -> (i64, Option<i64>) {
        sqlx::query_as("SELECT spendable, spent_by FROM outputs WHERE output_id = ?")
            .bind(id)
            .fetch_one(storage.pool())
            .await
            .unwrap()
    }

    #[tokio::test]
    async fn async_rejection_marks_the_tx_failed_and_releases_verified_inputs() {
        use bsv_wallet_toolbox::services::mock::MockWalletServices;

        let (storage, input_id, own_id) = seeded_storage().await;
        // The chain oracle: the tx is unknown (not alive), its input is unspent.
        let services = MockWalletServices::new();

        // Drive the exact handler wiring: an ACCEPTED broadcast whose background
        // verification comes back Rejected runs the retire hook.
        let storage = Arc::new(storage);
        let services = Arc::new(services);
        let (done_tx, done_rx) = tokio::sync::oneshot::channel::<Option<RetireOutcome>>();
        let (s, v) = (storage.clone(), services.clone());
        follow_up(
            BroadcastDisposition::Accepted,
            TXID_HEX.to_string(),
            |_txid| async { BroadcastVerification::Rejected },
            move |txid| async move {
                let outcome = retire_rejected_broadcast(&s, &*v, &txid).await;
                done_tx.send(outcome).ok();
            },
        )
        .await;
        let outcome = tokio::time::timeout(Duration::from_secs(5), done_rx)
            .await
            .expect("background retire runs")
            .expect("hook fired");
        assert_eq!(
            outcome,
            Some(RetireOutcome::Retired {
                restored: 1,
                kept: 0
            })
        );

        let status: String = sqlx::query_scalar("SELECT status FROM transactions WHERE txid = ?")
            .bind(TXID_HEX)
            .fetch_one(storage.pool())
            .await
            .unwrap();
        assert_eq!(status, "failed");
        let req: String = sqlx::query_scalar("SELECT status FROM proven_tx_reqs WHERE txid = ?")
            .bind(TXID_HEX)
            .fetch_one(storage.pool())
            .await
            .unwrap();
        assert_eq!(req, "invalid");
        assert_eq!(
            output_state(&storage, input_id).await,
            (1, None),
            "the chain-verified input is back in coin selection"
        );
        assert_eq!(
            output_state(&storage, own_id).await,
            (0, None),
            "the failed tx's change can never fund anything"
        );
    }

    #[tokio::test]
    async fn retire_keeps_an_input_the_chain_cannot_vouch_for() {
        use bsv_wallet_toolbox::services::mock::{MockResponse, MockWalletServices};

        let (storage, input_id, _own_id) = seeded_storage().await;
        let services = MockWalletServices::builder()
            .is_utxo_response(MockResponse::Success(false))
            .build();
        let outcome = retire_rejected_broadcast(&storage, &services, TXID_HEX).await;
        assert_eq!(
            outcome,
            Some(RetireOutcome::Retired {
                restored: 0,
                kept: 1
            })
        );
        let (spendable, spent_by) = output_state(&storage, input_id).await;
        assert_eq!(spendable, 0, "an unknown never releases money");
        assert!(spent_by.is_some());
    }

    #[tokio::test]
    async fn retire_of_an_unknown_txid_is_a_logged_noop() {
        use bsv_wallet_toolbox::services::mock::MockWalletServices;

        let (storage, input_id, _own_id) = seeded_storage().await;
        let outcome =
            retire_rejected_broadcast(&storage, &MockWalletServices::new(), &"ee".repeat(32)).await;
        assert_eq!(outcome, None);
        assert_eq!(output_state(&storage, input_id).await.0, 0);
    }
}
