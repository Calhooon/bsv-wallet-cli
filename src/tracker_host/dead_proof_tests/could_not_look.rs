//! A could-not-look fault takes the age re-ask's backoff and no mark
//! (#328 (3), the owner's word of 2026-11-01).

use super::*;

#[tokio::test]
async fn a_creator_answered_unavailable_takes_the_backoff_and_no_mark() {
    let (storage, user) = wallet_storage().await;
    let services = services();
    // Its block is above the service's tip (1,000): no header answers.
    let txid = "f7".repeat(32);
    proven_coin(&storage, user, &txid, 1_001, 1_000).await;
    // Beside it, a creator whose stored proof does not parse.
    let unparsed = "f8".repeat(32);
    proven_coin(&storage, user, &unparsed, 900, 1_000).await;
    unparseable(&storage, &unparsed).await;

    let first = pass(&storage, &services, 100).await;
    let said = format!("{txid}: not adopted: Unavailable(1001)");
    assert!(first.faults.contains(&said), "{:?}", first.faults);
    // No mark and no row: the fault is no verdict. The parse fault marks.
    assert!(load_row(&storage, &txid).await.unwrap().is_none());
    assert_eq!(mark_of(&storage, &unparsed).await, (None, 1, Some(700)));

    // Inside the backoff (600 s, the age re-ask's pause at no asks): not
    // selected, nothing reported.
    assert!(pass(&storage, &services, 699).await.faults.is_empty());
    // The backoff passed: selected, the same fault, the backoff doubled,
    // not reported again (#328 (2)).
    let due = pass(&storage, &services, 700).await;
    assert!(due.faults.is_empty(), "{:?}", due.faults);
    assert!(load_row(&storage, &txid).await.unwrap().is_none());

    // The header service answers at 1,001: inside the doubled backoff
    // (1,200 s) nothing is adopted; the next pass after it adopts it.
    services.set_tip_header(header(1_001, &"aa".repeat(32), 0));
    services.set_header_for_height(header(1_001, &txid, 0));
    let inside = pass(&storage, &services, 1_899).await;
    assert!(inside.adopted_mined.is_empty() && inside.faults.is_empty());
    let adopted = pass(&storage, &services, 1_900).await;
    assert_eq!(adopted.adopted_mined, vec![txid.clone()]);
    assert_word(&storage, &txid, ("mined", Some(1_001), None)).await;
}

#[tokio::test]
async fn an_unanswered_header_retries_the_dead_row_after_the_backoff() {
    let txid = "c6".repeat(32);
    let (storage, _) = proven_dead(&txid, "conflicted", 900).await;
    let before = stored_bytes(&storage, &txid).await;
    let (services, ready, task) = header_service(&txid).await;
    let report = pass(&storage, &services, 100).await;
    assert!(report.faults.is_empty() && report.adopted_mined.is_empty());
    assert_eq!(stored_bytes(&storage, &txid).await, before);
    ready.store(true, std::sync::atomic::Ordering::SeqCst);
    // Could not look takes the age re-ask's backoff (#328 (3)): not
    // selected inside it, checked again when it passes.
    assert!(pass(&storage, &services, 699)
        .await
        .adopted_mined
        .is_empty());
    assert_eq!(stored_bytes(&storage, &txid).await, before);
    let next = pass(&storage, &services, 700).await;
    task.abort();
    assert!(task.await.unwrap_err().is_cancelled());
    assert_eq!(next.adopted_mined, vec![txid.clone()]);
    assert_eq!(
        stored_word(&storage, &txid).await.unwrap().unwrap().0,
        "mined"
    );
}
