//! The tracker pass's hold of a claim (the stable line's 0.18.6, ported):
//! a claim a broadcaster called not final, held to be announced again at
//! its lock time, is announced and recorded with a chain tracker, and held
//! unannounced while no chain tracker is configured (`CHAINTRACKS_URL=off`),
//! since the wallet could not record it.

use super::tests::{opts, spendable, storage, tx_bytes, At};
use super::*;
use crate::test_support::txid_of;
use bsv_wallet_toolbox::services::mock::{
    error_post_beef_result, MockResponse, MockWalletServices,
};
use bsv_wallet_toolbox::services::GetMerklePathResult;

/// A claim held as not final, as `gift-claim --force` leaves it before
/// its lock time: the wallet (on the mock, whose tracker confirms), the
/// claim's plan and the lock time. The wallet's services answer the
/// claim's announce "not final", then accept the next post.
async fn a_held_claim() -> (
    bsv_wallet_toolbox::Wallet<StorageSqlx, MockWalletServices>,
    bsv_wallet_cli::gift::claim::ClaimPlan,
    u64,
) {
    use bsv_sdk::primitives::PrivateKey;
    use bsv_wallet_cli::gift::claim::build_claim_tx;
    use bsv_wallet_cli::gift::covenant::build_locking_script;
    use bsv_wallet_toolbox::services::mock::success_post_beef_result;
    use bsv_wallet_toolbox::Wallet;

    let root = PrivateKey::from_hex(&"01".repeat(32)).unwrap();
    let (deposit_priv, deposit_pub) = crate::brc29::deposit_keypair(&root).unwrap();
    let pay = crate::brc29::deposit_script(&root).unwrap();
    let lock = 1_600_000_000u64;
    let covenant = build_locking_script(&deposit_pub.to_compressed(), lock, 5000).unwrap();
    let deposit = tx_bytes(
        &[(&"11".repeat(32), 0)],
        &[(5000, covenant), (1000, pay.clone())],
    );
    let plan = build_claim_tx(&deposit, &deposit_priv, lock as u32).unwrap();
    let claim = hex::decode(&plan.claim_raw_hex).unwrap();
    let services = MockWalletServices::builder()
        .get_merkle_path_response(MockResponse::Sequence(vec![MockResponse::Success(
            GetMerklePathResult {
                name: Some("Services".to_string()),
                merkle_path: Some(
                    MerklePath::from_coinbase_txid(&txid_of(&deposit), 900_000)
                        .to_hex()
                        .unwrap(),
                ),
                header: None,
                error: None,
                notes: vec![],
            },
        )]))
        .post_beef_response(MockResponse::Sequence(vec![
            MockResponse::Success(vec![error_post_beef_result("Arcade", "476 non-final")]),
            MockResponse::Success(vec![success_post_beef_result(
                "Arcade",
                &[&plan.claim_txid],
            )]),
        ]))
        .build();
    let wallet = Wallet::new(Some(root), storage().await, services)
        .await
        .unwrap();
    let outcome = announce_claim(
        wallet.storage(),
        wallet.services(),
        &At(lock - 3600),
        &opts(),
        &deposit,
        &claim,
        &pay,
        lock,
        true,
    )
    .await
    .unwrap();
    assert!(outcome.record.is_none());
    assert_eq!(wallet.services().call_count("post_beef"), 1);
    assert_eq!(spendable(wallet.storage()).await, 0);
    (wallet, plan, lock)
}

/// Every column of the claim's tracked row, as stored.
async fn tracked_row(storage: &StorageSqlx, txid: &str) -> String {
    let columns: Vec<String> =
        sqlx::query_scalar("SELECT name FROM pragma_table_info('tracker_states') ORDER BY cid")
            .fetch_all(storage.pool())
            .await
            .unwrap();
    assert!(columns.len() >= 14, "{columns:?}");
    let quoted: Vec<String> = columns
        .iter()
        .map(|c| format!("'{c}=' || quote({c})"))
        .collect();
    sqlx::query_scalar(&format!(
        "SELECT {} FROM tracker_states WHERE txid = ?",
        quoted.join(" || ' ' || ")
    ))
    .bind(txid)
    .fetch_one(storage.pool())
    .await
    .unwrap()
}

/// The stable line's 0.18.6, ported: with no chain tracker
/// (`CHAINTRACKS_URL=off`) the toolbox's door refuses every
/// `internalizeAction` (#509), so a held claim whose time has come is
/// not announced: nothing is posted, the row is read back as it was,
/// column by column, and the pass names the claim in its faults. Nothing
/// is owed to the wallet and nothing is recorded. Red without the hold:
/// the pass posted the claim and the wallet then refused to record it.
#[tokio::test]
async fn a_held_claim_is_not_announced_while_no_chain_tracker_is_configured() {
    use crate::test_support::NoTracker;
    use bsv_wallet_toolbox::services::mock::success_post_beef_result;

    let (wallet, plan, lock) = a_held_claim().await;
    let txid = plan.claim_txid.clone();
    let before = tracked_row(wallet.storage(), &txid).await;
    assert!(before.contains("on_accept='"), "{before}");
    // Services with no tracker whose broadcasters would take it.
    let off = NoTracker(
        MockWalletServices::builder()
            .post_beef_response(MockResponse::Success(vec![success_post_beef_result(
                "Arcade",
                &[&txid],
            )]))
            .build(),
    );

    for at in [lock, lock + 600] {
        let mut report = tracker_host::tick(wallet.storage(), &off, &At(at), &opts())
            .await
            .unwrap();
        assert_eq!(off.call_count("post_beef"), 0, "nothing posted at {at}");
        assert!(report.reannounced.is_empty(), "{report:?}");
        assert!(report.not_final.is_empty(), "{report:?}");
        assert_eq!(
            report.faults,
            vec![format!(
                "{txid}: {}",
                tracker_host::CLAIM_HELD_WITHOUT_A_TRACKER
            )]
        );
        assert!(report.faults[0].contains("no chain tracker is configured"));
        assert!(report.faults[0].contains("CHAINTRACKS_URL"));
        assert!(!report.is_quiet());
        tracker_host::record_accepted(&wallet, &mut report).await;
        assert!(report.stored.is_empty(), "{report:?}");
        assert_eq!(tracked_row(wallet.storage(), &txid).await, before);
    }
    assert_eq!(wallet.services().call_count("post_beef"), 1, "the announce");
    assert_eq!(spendable(wallet.storage()).await, 0);
}

/// A claim held as not final is recorded when its re-announce is
/// accepted, by the caller that holds the wallet, and not before. The
/// control of the hold below (the stable line's 0.18.6
/// `a_held_claim_with_a_chain_tracker_is_announced_once_and_recorded`):
/// with a chain tracker (the mock's) the claim is posted once at its
/// time, accepted, and recorded in the wallet.
#[tokio::test]
async fn a_held_claim_is_recorded_when_its_re_announce_is_accepted() {
    let (wallet, plan, lock) = a_held_claim().await;
    let mut report = tracker_host::tick(wallet.storage(), wallet.services(), &At(lock), &opts())
        .await
        .unwrap();
    assert_eq!(
        wallet.services().call_count("post_beef"),
        2,
        "the announce, and one re-announce"
    );
    assert_eq!(report.reannounced, vec![plan.claim_txid.clone()]);
    assert!(report.faults.is_empty(), "{report:?}");
    tracker_host::record_accepted(&wallet, &mut report).await;
    assert_eq!(
        report.stored,
        vec![format!("{}: recorded in the wallet", plan.claim_txid)]
    );
    assert_eq!(
        spendable(wallet.storage()).await as u64,
        plan.covenant.amount + plan.change
    );
}
