//! A failing row is reported once when marked and again only on a
//! change of its state (#328 (2), the owner's word of 2026-11-01).

use super::*;

#[tokio::test]
async fn a_failing_row_is_reported_once_and_again_on_a_change_of_its_fault() {
    let txid = "f4".repeat(32);
    let (storage, services) = mismatched_at_900(&txid).await;
    let stands = vec![proof_stands_of(&txid, "conflicted")];
    assert_eq!(pass(&storage, &services, 100).await.faults, stands);

    // Two pause expiries, the same fault: checked each time, not reported.
    for (now, asks, next) in [(700, 2, 700 + 1_200), (1_900, 3, 1_900 + 2_400)] {
        let again = pass(&storage, &services, now).await;
        assert_eq!((again.tracked, again.faults.len()), (0, 0), "at {now}");
        assert_eq!(mark_of(&storage, &txid).await, (None, asks, Some(next)));
    }

    // The fault's class changes (the toolbox's path no longer parses):
    // reported a second time.
    unparseable(&storage, &txid).await;
    let changed = pass(&storage, &services, 4_300).await;
    assert_eq!(changed.faults, stands);
    assert_eq!(
        mark_of(&storage, &txid).await,
        (None, 4, Some(4_300 + 4_800))
    );
    assert!(pass(&storage, &services, 9_100).await.faults.is_empty());

    // The row offered again (the toolbox wrote another path that still
    // fails): reported a third time.
    let other = MerklePath::from_coinbase_txid(&txid, 901).to_binary();
    toolbox_path(&storage, &txid, &other).await;
    let offered = pass(&storage, &services, 9_100 + 9_600).await;
    assert_eq!(offered.faults, stands);
}

#[tokio::test]
async fn a_new_creator_without_its_path_is_reported_once() {
    let (storage, user) = wallet_storage().await;
    let services = services();
    let txid = "f5".repeat(32);
    proven_coin(&storage, user, &txid, 900, 1_000).await;
    services.set_header_for_height(header(900, &txid, 0));
    unparseable(&storage, &txid).await;
    let said = format!("{txid}: the stored proof does not parse; tracked without it");
    assert_eq!(pass(&storage, &services, 100).await.faults, vec![said]);
    for now in [700, 1_900] {
        assert!(pass(&storage, &services, now).await.faults.is_empty());
    }
    assert_eq!(
        mark_of(&storage, &txid).await,
        (None, 3, Some(1_900 + 2_400))
    );
}

#[tokio::test]
async fn a_dead_row_whose_mark_cleared_is_reported_again() {
    let txid = "f6".repeat(32);
    let (storage, services) = proven_dead(&txid, "rejected", 900).await;
    unparseable(&storage, &txid).await;
    let stands = vec![proof_stands_of(&txid, "rejected")];
    assert_eq!(pass(&storage, &services, 100).await.faults, stands);
    assert!(pass(&storage, &services, 700).await.faults.is_empty());

    // A step of the tick drops the mark (a hint's re-ask): the same fault
    // is a new mark again.
    sqlx::query("UPDATE tracker_states SET asks = 0, next_ask_at = NULL WHERE txid = ?")
        .bind(&txid)
        .execute(storage.writer())
        .await
        .unwrap();
    assert_eq!(pass(&storage, &services, 701).await.faults, stands);
}
