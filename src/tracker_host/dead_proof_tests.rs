//! The tick reconciles a dead stored word with the toolbox's checked proof.

use super::tests::{header, opts, path_of, proven_coin, wallet_storage, FixedClock};
use super::*;
use bsv_wallet_toolbox::services::mock::MockWalletServices;

mod feed;

async fn dead_row(storage: &StorageSqlx, txid: &str, word: &str) -> TrackedRow {
    ensure_tables(storage).await.unwrap();
    let mut row = TrackedRow::new(txid);
    row.built = true;
    row.hints.push(StoredHint::of(&Hint::new(
        "node|accepted",
        HintStatus::Accepted,
        1,
    )));
    row.chain.push(
        serde_json::to_string(&ChainEnvelope::new(ChainEvent::Frozen {
            outpoint: bsv_tracker::Outpoint {
                txid: txid.to_string(),
                vout: 0,
            },
        }))
        .unwrap(),
    );
    row.verdict = match word {
        "conflicted" => Some(StoredVerdict::Conflicted {
            competitors: vec!["ee".repeat(32)],
        }),
        "rejected" => Some(StoredVerdict::Rejected {
            reason: "node refused the transaction".to_string(),
        }),
        "abandoned" => None,
        _ => panic!("not a dead word: {word}"),
    };
    let (state, fault) = row.replay(&opts().params(), &HeaderSnapshot::unread());
    assert_eq!(fault, None);
    store_row(storage, &row, &state, 10).await.unwrap();
    // No production caller feeds Abandon; seed its stored word directly.
    if word == "abandoned" {
        sqlx::query("UPDATE tracker_states SET word = 'abandoned' WHERE txid = ?")
            .bind(txid)
            .execute(storage.writer())
            .await
            .unwrap();
    }
    load_row(storage, txid).await.unwrap().unwrap()
}

async fn stored_bytes(storage: &StorageSqlx, txid: &str) -> String {
    sqlx::query_scalar(
        "SELECT json_array(txid, built, hints, bump, word, height, header_hash, reask, \
         asks, next_ask_at, reannounce_beef, reannounce_at, on_accept, chain, verdict, \
         follow_up, updated_at) FROM tracker_states WHERE txid = ?",
    )
    .bind(txid)
    .fetch_one(storage.pool())
    .await
    .unwrap()
}

fn services() -> MockWalletServices {
    let services = MockWalletServices::builder().build();
    services.set_tip_header(header(1_000, &"aa".repeat(32), 0));
    services
}

async fn pass(storage: &StorageSqlx, services: &impl WalletServices, now: Timestamp) -> TickReport {
    tick(storage, services, &FixedClock(now), &opts())
        .await
        .unwrap()
}

fn assert_idle(report: &TickReport) {
    assert_eq!((report.tracked, report.adopted, report.asked), (0, 0, 0));
    assert!(report.adopted_mined.is_empty() && report.adopted_proof_failed.is_empty());
    assert!(report.faults.is_empty());
}

/// The stored word, height and re-ask are `want`.
async fn assert_word(storage: &StorageSqlx, txid: &str, want: (&str, Option<u32>, Option<&str>)) {
    let (word, height, reask) = stored_word(storage, txid).await.unwrap().unwrap();
    assert_eq!((word.as_str(), height, reask.as_deref()), want);
}

/// A row's path and mark: `bump`, `asks`, `next_ask_at`.
async fn mark_of(storage: &StorageSqlx, txid: &str) -> (Option<String>, u32, Option<Timestamp>) {
    let row = load_row(storage, txid).await.unwrap().unwrap();
    (row.bump, row.asks, row.next_ask_at)
}

/// The header at 900 moves to one whose root is `root`, under a tip at
/// 901: the pass at 102 reads `moved_from` 900.
async fn moved_at_900(
    storage: &StorageSqlx,
    services: &MockWalletServices,
    root: &str,
) -> TickReport {
    services.set_header_for_height(header(900, root, 0));
    services.set_tip_header(super::tests::header_with_hash(901, &"aa".repeat(32), 0));
    let moved = pass(storage, services, 102).await;
    assert_eq!(moved.moved_from, Some(900));
    moved
}

/// The pass adopted `txid` alone, as `mined` from the toolbox's path at
/// 900, with no fault and no merkle fetch, its mark cleared.
async fn assert_adopted_at_900(
    storage: &StorageSqlx,
    services: &MockWalletServices,
    report: &TickReport,
    txid: &str,
) {
    assert_eq!(report.adopted_mined, vec![txid.to_string()]);
    assert!(report.faults.is_empty());
    assert_eq!(services.call_count("get_merkle_path"), 0);
    assert_eq!(
        mark_of(storage, txid).await,
        (Some(path_of(txid, 900)), 0, None)
    );
}

async fn proven_dead(txid: &str, word: &str, height: u32) -> (StorageSqlx, MockWalletServices) {
    let (storage, user) = wallet_storage().await;
    proven_coin(&storage, user, txid, height, 1_000).await;
    dead_row(&storage, txid, word).await;
    (storage, services())
}

/// One pass at 100 reports the dead row's failing proof, and adopts nothing.
async fn rejected_stands_once(storage: &StorageSqlx, services: &impl WalletServices, txid: &str) {
    let report = pass(storage, services, 100).await;
    assert_eq!(report.faults, vec![proof_stands_of(txid, "rejected")]);
    assert!(report.adopted_mined.is_empty() && report.adopted_proof_failed.is_empty());
}

/// The inputs a revival or a mark leaves as they were.
fn assert_same_inputs(before: TrackedRow, after: TrackedRow) {
    assert_eq!(
        (after.built, after.hints, after.chain, after.verdict),
        (before.built, before.hints, before.chain, before.verdict)
    );
}

async fn assert_rejected_stands(storage: &StorageSqlx, services: &impl WalletServices, txid: &str) {
    let before = stored_bytes(storage, txid).await;
    rejected_stands_once(storage, services, txid).await;
    assert_eq!(stored_bytes(storage, txid).await, before);
    assert!(load_row(storage, txid)
        .await
        .unwrap()
        .unwrap()
        .bump
        .is_none());
}

/// One pass checks the dead row's failing proof once: the fault reported,
/// its `rejected` word and inputs standing, no `bump`, the crate's re-ask
/// `reask` and one ask with its pause stored, the gate's reading unmoved.
/// The next pass, inside the pause, does not select it.
async fn assert_marked(
    storage: &StorageSqlx,
    services: &MockWalletServices,
    txid: &str,
    reask: Option<&str>,
) {
    let before = load_row(storage, txid).await.unwrap().unwrap();
    let reading = creator(storage, services, txid).await;
    rejected_stands_once(storage, services, txid).await;
    assert_eq!(
        stored_word(storage, txid).await.unwrap().unwrap(),
        ("rejected".into(), None, reask.map(Into::into))
    );
    let after = load_row(storage, txid).await.unwrap().unwrap();
    assert_eq!(
        (after.bump.as_deref(), after.asks, after.next_ask_at),
        (None, 1, Some(700))
    );
    assert_same_inputs(before, after);
    assert_eq!(creator(storage, services, txid).await, reading);

    let marked = stored_bytes(storage, txid).await;
    let next = pass(storage, services, 101).await;
    assert_idle(&next);
    assert_eq!(stored_bytes(storage, txid).await, marked);
}

async fn assert_mined_readers(storage: &StorageSqlx, services: &MockWalletServices, txid: &str) {
    let body = served_word(storage, txid).await;
    assert_eq!(body["word"], "mined");
    assert_eq!(body["height"], 900);
    assert_eq!(
        body["evidence"],
        serde_json::json!({"root": txid, "height": 900})
    );
    let reading = creator(storage, services, txid).await;
    assert_eq!(
        (reading.word, reading.depth, reading.door, reading.offered()),
        ("mined", 101, true, true)
    );
}

/// A height-specific outage, independent of the tip. The same mock answers
/// that height after the caller flips `ready`; no courier is involved.
async fn header_service(
    txid: &str,
) -> (
    bsv_wallet_toolbox::Services,
    std::sync::Arc<std::sync::atomic::AtomicBool>,
    tokio::task::JoinHandle<()>,
) {
    use axum::{response::IntoResponse, routing::get, Json, Router};
    use std::sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    };

    let ready = Arc::new(AtomicBool::new(false));
    let available = ready.clone();
    let frame = |height, root: &str| {
        serde_json::json!({
            "status": "success", "value": {
                "version": 1, "previousHash": "00".repeat(32), "merkleRoot": root,
                "time": 1_700_000_000, "bits": 0x1d00_ffff, "nonce": 0,
                "height": height, "hash": "bb".repeat(32)
            }
        })
    };
    let tip = frame(1_000, &"aa".repeat(32));
    let block = frame(900, txid);
    let app = Router::new()
        .route(
            "/findChainTipHeaderHex",
            get(move || async move { Json(tip) }),
        )
        .route(
            "/findHeaderHexForHeight",
            get(move || {
                let block = block.clone();
                let available = available.clone();
                async move {
                    if available.load(Ordering::SeqCst) {
                        Json(block).into_response()
                    } else {
                        axum::http::StatusCode::SERVICE_UNAVAILABLE.into_response()
                    }
                }
            }),
        );
    let (url, task) = crate::test_support::serve_router(app).await;
    (crate::test_support::mock_services(&url), ready, task)
}

async fn creator(
    storage: &StorageSqlx,
    services: &impl WalletServices,
    txid: &str,
) -> CreatorReading {
    let path: Option<Vec<u8>> =
        sqlx::query_scalar("SELECT merkle_path FROM proven_txs WHERE txid = ?")
            .bind(txid)
            .fetch_optional(storage.pool())
            .await
            .unwrap();
    read_creators(storage, services, &opts(), &[(txid.to_string(), path)])
        .await
        .unwrap()
        .remove(txid)
        .unwrap()
}

async fn served_word(storage: &StorageSqlx, txid: &str) -> serde_json::Value {
    use axum::{body::Body, http::Request};
    use bsv_sdk::primitives::PrivateKey;
    use tower::ServiceExt;

    let wallet = crate::test_support::test_wallet(
        storage.clone(),
        "dead-proof-test",
        PrivateKey::from_hex(&"11".repeat(32)).unwrap(),
    )
    .await;
    let response = crate::server::make_router(wallet, crate::server::ServerConfig::default())
        .oneshot(
            Request::get(format!("/tx/{txid}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let (status, _, body) = crate::test_support::response_answer(response).await;
    assert_eq!(status, axum::http::StatusCode::OK);
    body
}

#[tokio::test]
async fn a_conflicted_row_with_a_toolbox_proof_is_mined_after_one_tick() {
    let txid = "c1".repeat(32);
    let (storage, services) = proven_dead(&txid, "conflicted", 900).await;
    let before = load_row(&storage, &txid).await.unwrap().unwrap();
    services.set_header_for_height(header(900, &txid, 0));
    assert_eq!(
        stored_word(&storage, &txid).await.unwrap().unwrap().0,
        "conflicted"
    );
    assert_eq!(served_word(&storage, &txid).await["word"], "conflicted");
    assert!(creator(&storage, &services, &txid).await.offered());

    let report = pass(&storage, &services, 100).await;
    let word = stored_word(&storage, &txid).await.unwrap().unwrap();
    println!("after one tick: {word:?}; {report:?}");
    assert_eq!(word, ("mined".into(), Some(900), Some("recheck".into())));
    assert_eq!(report.adopted_mined, vec![txid.clone()]);
    assert!(report.faults.is_empty());
    let after = load_row(&storage, &txid).await.unwrap().unwrap();
    assert_eq!(after.bump, Some(path_of(&txid, 900)));
    assert_eq!(
        after.header_hash,
        Some(header_of(&header(900, &txid, 0).to_binary()).unwrap().hash)
    );
    assert_same_inputs(before, after);
    assert_mined_readers(&storage, &services, &txid).await;

    let settled = stored_bytes(&storage, &txid).await;
    let next = pass(&storage, &services, 101).await;
    assert_eq!((next.tracked, next.adopted, next.asked), (0, 0, 0));
    assert!(next.adopted_mined.is_empty());
    assert_eq!(stored_bytes(&storage, &txid).await, settled);
    assert_eq!(services.call_count("get_merkle_path"), 0);
}

#[tokio::test]
async fn a_moved_header_clears_the_revived_rows_reask_after_one_fetch() {
    let txid = "cf".repeat(32);
    let (storage, _) = proven_dead(&txid, "conflicted", 900).await;
    let services = MockWalletServices::builder()
        .get_merkle_path_response(super::tests::proof_answer(Some(path_of(&txid, 900))))
        .build();
    services.set_tip_header(super::tests::header_with_hash(900, &txid, 0));
    services.set_header_for_height(header(900, &txid, 0));
    let revived = pass(&storage, &services, 100).await;
    assert_eq!(
        revived.adopted_mined.as_slice(),
        std::slice::from_ref(&txid)
    );
    assert_eq!(
        stored_word(&storage, &txid).await.unwrap().unwrap().2,
        Some("recheck".into())
    );
    let original = load_row(&storage, &txid).await.unwrap().unwrap();
    assert!(matches!(
        original.verdict,
        Some(StoredVerdict::Conflicted { .. })
    ));

    services.reset_history();
    services.set_header_for_height(header(900, &txid, 1));
    services.set_tip_header(super::tests::header_with_hash(901, &"aa".repeat(32), 0));
    let moved = pass(&storage, &services, 101).await;
    assert_eq!(
        (moved.moved_from, moved.tracked, moved.asked),
        (Some(900), 1, 1)
    );
    assert!(moved.faults.is_empty());
    assert_eq!(moved.mined.as_slice(), std::slice::from_ref(&txid));
    assert_eq!(services.call_count("get_header_for_height"), 1);
    assert_eq!(services.call_count("get_merkle_path"), 1);
    let word = stored_word(&storage, &txid).await.unwrap().unwrap();
    assert_eq!(word, ("mined".into(), Some(900), None));
    let fetched = load_row(&storage, &txid).await.unwrap().unwrap();
    assert_eq!((fetched.asks, fetched.verdict), (0, original.verdict));
    assert!(served_word(&storage, &txid).await["reask"].is_null());
    println!("moved tick: {word:?}; {moved:?}; one header read, one merkle call");

    let settled = stored_bytes(&storage, &txid).await;
    services.reset_history();
    let quiet = pass(&storage, &services, 102).await;
    assert_idle(&quiet);
    assert_eq!(quiet.moved_from, None);
    assert_eq!(stored_bytes(&storage, &txid).await, settled);
    assert_eq!(services.call_count("get_header_for_height"), 0);
    assert_eq!(services.call_count("get_merkle_path"), 0);
    println!("unmoved tick: {quiet:?}; stored word unchanged");
}

#[tokio::test]
async fn a_rejected_rows_mismatched_proof_is_marked_once_and_its_word_stands() {
    let txid = "c2".repeat(32);
    let (storage, services) = proven_dead(&txid, "rejected", 900).await;
    services.set_header_for_height(header(900, &"dd".repeat(32), 0));
    assert_marked(&storage, &services, &txid, Some("proof_failed@900")).await;
}

#[tokio::test]
async fn dead_rows_without_toolbox_proofs_are_never_loaded() {
    let (storage, user) = wallet_storage().await;
    let services = services();
    let mut before = Vec::new();
    for (byte, word) in [
        ("c3", "conflicted"),
        ("c4", "rejected"),
        ("c5", "abandoned"),
    ] {
        let txid = byte.repeat(32);
        proven_coin(&storage, user, &txid, 900, 1_000).await;
        dead_row(&storage, &txid, word).await;
        before.push((txid.clone(), stored_bytes(&storage, &txid).await));
    }
    sqlx::query("UPDATE transactions SET proven_tx_id = NULL")
        .execute(storage.writer())
        .await
        .unwrap();
    sqlx::query("DELETE FROM proven_txs")
        .execute(storage.writer())
        .await
        .unwrap();
    let report = pass(&storage, &services, 100).await;
    assert_idle(&report);
    for (txid, bytes) in before {
        assert_eq!(stored_bytes(&storage, &txid).await, bytes);
    }
}

#[tokio::test]
async fn an_unanswered_header_retries_the_dead_row_on_the_next_tick() {
    let txid = "c6".repeat(32);
    let (storage, _) = proven_dead(&txid, "conflicted", 900).await;
    let before = stored_bytes(&storage, &txid).await;
    let (services, ready, task) = header_service(&txid).await;
    let report = pass(&storage, &services, 100).await;
    assert!(report.faults.is_empty() && report.adopted_mined.is_empty());
    assert_eq!(stored_bytes(&storage, &txid).await, before);
    ready.store(true, std::sync::atomic::Ordering::SeqCst);
    let next = pass(&storage, &services, 101).await;
    task.abort();
    assert!(task.await.unwrap_err().is_cancelled());
    assert_eq!(next.adopted_mined, vec![txid.clone()]);
    assert_eq!(
        stored_word(&storage, &txid).await.unwrap().unwrap().0,
        "mined"
    );
}

#[tokio::test]
async fn an_unparseable_toolbox_proof_marks_the_dead_row_once() {
    let txid = "c7".repeat(32);
    let (storage, services) = proven_dead(&txid, "rejected", 900).await;
    unparseable(&storage, &txid).await;
    // No proof parsed, so the crate gives no re-ask and the stored mark is
    // the pause. The query's second clearing rule reads the toolbox's
    // `proven_txs.height`, not the path's, so a header move at or below it
    // checks the row again too (the next test).
    assert_marked(&storage, &services, &txid, None).await;
}

#[tokio::test]
async fn a_header_move_at_the_toolbox_height_checks_an_unparseable_dead_row_again() {
    let txid = "dd".repeat(31) + "e0";
    let (storage, services) = mismatched_at_900(&txid).await;
    unparseable(&storage, &txid).await;
    let stands = vec![proof_stands_of(&txid, "conflicted")];
    let first = pass(&storage, &services, 100).await;
    assert_eq!(first.faults, stands);
    assert_word(&storage, &txid, ("conflicted", None, None)).await;
    assert_idle(&pass(&storage, &services, 101).await);

    // The header at 900, the toolbox's `proven_txs.height`, moves.
    let moved = moved_at_900(&storage, &services, &"ef".repeat(32)).await;
    assert_eq!(moved.faults, stands);
    assert!(moved.adopted_mined.is_empty());
    assert_eq!(mark_of(&storage, &txid).await, (None, 2, Some(102 + 1_200)));
    assert_word(&storage, &txid, ("conflicted", None, None)).await;
    assert_idle(&pass(&storage, &services, 103).await);
}

fn proof_stands_of(txid: &str, word: &str) -> String {
    format!("{txid}: the stored proof does not check; its {word} stands")
}

async fn unparseable(storage: &StorageSqlx, txid: &str) {
    toolbox_path(storage, txid, &[0]).await;
}

/// The toolbox's `proven_txs.merkle_path` for `txid`, rewritten in place.
async fn toolbox_path(storage: &StorageSqlx, txid: &str, path: &[u8]) {
    sqlx::query("UPDATE proven_txs SET merkle_path = ? WHERE txid = ?")
        .bind(path)
        .bind(txid)
        .execute(storage.writer())
        .await
        .unwrap();
}

#[tokio::test]
async fn a_toolbox_proof_above_the_tip_keeps_the_dead_row_untouched() {
    let txid = "c8".repeat(32);
    let (storage, services) = proven_dead(&txid, "rejected", 1_001).await;
    services.set_header_for_height(header(1_001, &txid, 0));
    assert_rejected_stands(&storage, &services, &txid).await;
}

#[tokio::test]
async fn a_dead_row_already_holding_a_bump_is_not_readopted() {
    let txid = "c9".repeat(32);
    let (storage, services) = proven_dead(&txid, "rejected", 900).await;
    sqlx::query("UPDATE tracker_states SET bump = ? WHERE txid = ?")
        .bind(path_of(&txid, 900))
        .bind(&txid)
        .execute(storage.writer())
        .await
        .unwrap();
    let before = stored_bytes(&storage, &txid).await;
    services.set_header_for_height(header(900, &txid, 0));
    let report = pass(&storage, &services, 100).await;
    assert_idle(&report);
    assert_eq!(stored_bytes(&storage, &txid).await, before);
}

/// The `i`th proven creator at height `100 + i`, its header's root `root`
/// (its own txid when `None`), with a dead row when `word` names one.
async fn numbered(
    storage: &StorageSqlx,
    user: i64,
    services: &MockWalletServices,
    i: usize,
    salt: &str,
    root: Option<&str>,
    word: Option<&str>,
) -> String {
    let txid = format!("{i:08x}{}", salt.repeat(28));
    let height = 100 + u32::try_from(i).unwrap();
    proven_coin(storage, user, &txid, height, 1_000).await;
    services.set_header_for_height(header(height, root.unwrap_or(&txid), 0));
    if let Some(word) = word {
        dead_row(storage, &txid, word).await;
    }
    txid
}

#[tokio::test]
async fn one_adoption_limit_covers_new_and_all_three_dead_words() {
    let (storage, user) = wallet_storage().await;
    let services = services();
    let mut txids = Vec::new();
    for i in 0..MAX_ADOPT_PER_PASS + 3 {
        let word = [
            None,
            Some("conflicted"),
            Some("rejected"),
            Some("abandoned"),
        ][i % 4];
        txids.push(numbered(&storage, user, &services, i, "ab", None, word).await);
    }
    // New creators lead the selection, then the dead rows, each oldest first.
    let (new, dead): (Vec<_>, Vec<_>) =
        txids.into_iter().enumerate().partition(|(i, _)| i % 4 == 0);
    let txids: Vec<String> = new.into_iter().chain(dead).map(|(_, txid)| txid).collect();
    let report = pass(&storage, &services, 100).await;
    assert_eq!(report.adopted_mined, txids[..MAX_ADOPT_PER_PASS]);
    assert!(report.faults.is_empty());
    assert_eq!(report.tracked, 0);
    let next = pass(&storage, &services, 101).await;
    assert_eq!(next.adopted_mined, txids[MAX_ADOPT_PER_PASS..]);
    assert!(next.faults.is_empty());
    let done = pass(&storage, &services, 102).await;
    assert!(done.adopted_mined.is_empty());
    assert_eq!((done.tracked, done.asked), (0, 0));
}

#[tokio::test]
async fn dead_rows_whose_proofs_keep_failing_never_hold_back_a_new_creator() {
    let (storage, user) = wallet_storage().await;
    let services = services();
    let mismatched = "dd".repeat(32);
    for i in 0..MAX_ADOPT_PER_PASS {
        let root = Some(mismatched.as_str());
        numbered(&storage, user, &services, i, "cd", root, Some("rejected")).await;
    }
    let fresh = "ce".repeat(32);
    proven_coin(&storage, user, &fresh, 900, 1_000).await;
    services.set_header_for_height(header(900, &fresh, 0));
    let report = pass(&storage, &services, 100).await;
    assert_eq!(report.adopted_mined, vec![fresh.clone()]);
    assert_eq!(report.faults.len(), MAX_ADOPT_PER_PASS - 1);
    assert_eq!(
        stored_word(&storage, &fresh).await.unwrap().unwrap().0,
        "mined"
    );
    // Each failing dead row is reported once: the one the limit left out
    // now, and none of the marked rows again inside their pause.
    let next = pass(&storage, &services, 101).await;
    assert!(next.adopted_mined.is_empty());
    assert_eq!(next.faults.len(), 1);
    let quiet = pass(&storage, &services, 102).await;
    assert!(quiet.faults.is_empty() && quiet.adopted_mined.is_empty());
}

#[tokio::test]
async fn unparseable_proofs_never_hold_back_a_new_creator_or_a_revival() {
    let (storage, user) = wallet_storage().await;
    let services = services();
    let mut broken = Vec::new();
    for i in 0..MAX_ADOPT_PER_PASS {
        let txid = numbered(&storage, user, &services, i, "da", None, None).await;
        unparseable(&storage, &txid).await;
        broken.push(txid);
    }
    let reading = creator(&storage, &services, &broken[0]).await;
    let fresh = "db".repeat(32);
    proven_coin(&storage, user, &fresh, 900, 1_000).await;
    let revived = "dc".repeat(32);
    proven_coin(&storage, user, &revived, 901, 1_000).await;
    dead_row(&storage, &revived, "conflicted").await;
    services.set_header_for_height(header(900, &fresh, 0));
    services.set_header_for_height(header(901, &revived, 0));

    let report = pass(&storage, &services, 100).await;
    let said: Vec<String> = broken
        .iter()
        .map(|txid| format!("{txid}: the stored proof does not parse; tracked without it"))
        .collect();
    assert_eq!(report.faults, said);
    assert!(report.adopted_mined.is_empty() && report.adopted_proof_failed.is_empty());
    for txid in &broken {
        let row = load_row(&storage, txid).await.unwrap().unwrap();
        assert_eq!(
            (row.built, row.bump, row.asks, row.next_ask_at),
            (false, None, 1, Some(700))
        );
        assert_eq!(
            stored_word(&storage, txid).await.unwrap().unwrap(),
            ("unknown".into(), None, None)
        );
    }
    assert_eq!(creator(&storage, &services, &broken[0]).await, reading);

    let next = pass(&storage, &services, 101).await;
    println!("second pass: {}", next.summary());
    assert_eq!(next.adopted_mined, vec![fresh.clone(), revived.clone()]);
    assert!(next.faults.is_empty());
    assert_eq!((next.tracked, next.asked), (MAX_ADOPT_PER_PASS, 0));
    assert_eq!(services.call_count("get_merkle_path"), 0);
    for (txid, height) in [(&fresh, 900), (&revived, 901)] {
        let word = stored_word(&storage, txid).await.unwrap().unwrap();
        assert_eq!((word.0.as_str(), word.1), ("mined", Some(height)));
    }
}

#[tokio::test]
async fn a_new_creator_tracked_without_its_path_is_adopted_after_the_toolbox_repairs_it() {
    let (storage, user) = wallet_storage().await;
    let services = services();
    let txid = "e1".repeat(32);
    proven_coin(&storage, user, &txid, 900, 1_000).await;
    services.set_header_for_height(header(900, &txid, 0));
    // The toolbox holds the path as TSC JSON, which does not parse as BUMP.
    let json = format!(r#"{{"index":0,"txOrId":"{txid}","nodes":[]}}"#);
    toolbox_path(&storage, &txid, json.as_bytes()).await;

    let first = pass(&storage, &services, 100).await;
    let said = format!("{txid}: the stored proof does not parse; tracked without it");
    assert_eq!(first.faults, vec![said]);
    assert_eq!(mark_of(&storage, &txid).await, (None, 1, Some(700)));
    // The row is served: its word, where with no row `/tx/{txid}` was 404.
    assert_eq!(served_word(&storage, &txid).await["word"], "unknown");

    // The toolbox's JSON-to-BUMP repair rewrites the path in place.
    let repaired = MerklePath::from_coinbase_txid(&txid, 900).to_binary();
    toolbox_path(&storage, &txid, &repaired).await;
    // Inside the pause the row is not selected: tracked, never adopted.
    let inside = pass(&storage, &services, 101).await;
    assert!(inside.adopted_mined.is_empty() && inside.faults.is_empty());
    assert_word(&storage, &txid, ("unknown", None, None)).await;

    // The pause passed: the repaired path is read and checks.
    let due = pass(&storage, &services, 100_000).await;
    assert_adopted_at_900(&storage, &services, &due, &txid).await;
    assert_mined_readers(&storage, &services, &txid).await;

    let later = pass(&storage, &services, 10_000_000).await;
    assert!(later.adopted_mined.is_empty() && later.faults.is_empty());
    assert_word(&storage, &txid, ("mined", Some(900), None)).await;
}

#[tokio::test]
async fn a_made_creator_tracked_without_its_path_reads_built_with_its_door_and_reason_unchanged() {
    use crate::spend_gate::{hold_reason, GateOptions, Origin};

    let (storage, user) = wallet_storage().await;
    let services = services();
    let txid = "e2".repeat(32);
    proven_coin(&storage, user, &txid, 900, 1_000).await;
    sqlx::query("UPDATE transactions SET is_outgoing = 1 WHERE txid = ?")
        .bind(&txid)
        .execute(storage.writer())
        .await
        .unwrap();
    unparseable(&storage, &txid).await;
    let reasons = |reading: &CreatorReading| {
        [true, false].map(|own_change| {
            let opts = GateOptions {
                own_change,
                ..GateOptions::default()
            };
            hold_reason(reading, Origin::Made, (&txid, 0), &opts)
        })
    };
    let before = creator(&storage, &services, &txid).await;
    assert_eq!(before.word, word_label(&Word::Unknown));

    pass(&storage, &services, 100).await;
    let row = load_row(&storage, &txid).await.unwrap().unwrap();
    assert_eq!((row.built, row.bump, row.asks), (true, None, 1));
    let after = creator(&storage, &services, &txid).await;
    // The word is the made row's: `built` where no row read `unknown`;
    // the gate shows it in a refusal's `held[].word`.
    assert_eq!(after.word, word_label(&Word::Built));
    assert_eq!(
        (after.door, &after.fault, after.depth, after.suspect),
        (before.door, &before.fault, before.depth, before.suspect)
    );
    assert!(!after.door && after.fault.is_none());
    assert_eq!(reasons(&after), reasons(&before));
    assert!(reasons(&after)[1].is_some());
}

/// A conflicted row at 900 whose stored proof misses the header there.
async fn mismatched_at_900(txid: &str) -> (StorageSqlx, MockWalletServices) {
    let (storage, _) = proven_dead(txid, "conflicted", 900).await;
    let services = MockWalletServices::builder().build();
    let root = "dd".repeat(32);
    services.set_tip_header(super::tests::header_with_hash(900, &root, 0));
    services.set_header_for_height(header(900, &root, 0));
    (storage, services)
}

#[tokio::test]
async fn a_marked_row_is_checked_again_when_its_header_moves_and_revived() {
    let txid = "dd".repeat(31) + "de";
    let (storage, services) = mismatched_at_900(&txid).await;
    let first = pass(&storage, &services, 100).await;
    assert_eq!(first.faults.len(), 1);
    assert_eq!(
        stored_word(&storage, &txid).await.unwrap().unwrap(),
        ("conflicted".into(), None, Some("proof_failed@900".into()))
    );
    assert_idle(&pass(&storage, &services, 101).await);

    // The header at 900 moves to one whose root is the proof's.
    let moved = moved_at_900(&storage, &services, &txid).await;
    println!("moved pass: {}", moved.summary());
    assert_adopted_at_900(&storage, &services, &moved, &txid).await;
    assert_eq!(
        stored_word(&storage, &txid).await.unwrap().unwrap().0,
        "mined"
    );
}

#[tokio::test]
async fn a_marked_row_is_checked_again_after_its_pause_and_reads_a_new_toolbox_path() {
    let txid = "dd".repeat(31) + "df";
    let (storage, services) = mismatched_at_900(&txid).await;
    assert_eq!(pass(&storage, &services, 100).await.faults.len(), 1);
    assert_idle(&pass(&storage, &services, 699).await);

    // The pause passed: one more check, still failing, a doubled pause.
    let again = pass(&storage, &services, 700).await;
    assert_eq!(again.faults.len(), 1);
    let row = load_row(&storage, &txid).await.unwrap().unwrap();
    assert_eq!((row.asks, row.next_ask_at), (2, Some(700 + 1_200)));
    assert_eq!(
        stored_word(&storage, &txid).await.unwrap().unwrap().0,
        "conflicted"
    );
    assert_idle(&pass(&storage, &services, 1_899).await);

    // The toolbox rewrote its path (a re-anchor at 901): the next check reads it.
    sqlx::query("UPDATE proven_txs SET height = 901, merkle_path = ? WHERE txid = ?")
        .bind(MerklePath::from_coinbase_txid(&txid, 901).to_binary())
        .bind(&txid)
        .execute(storage.writer())
        .await
        .unwrap();
    services.set_tip_header(super::tests::header_with_hash(901, &"aa".repeat(32), 0));
    services.set_header_for_height(header(901, &txid, 0));
    let revived = pass(&storage, &services, 1_900).await;
    assert_eq!(revived.adopted_mined, vec![txid.clone()]);
    assert!(revived.faults.is_empty());
    assert_eq!(
        stored_word(&storage, &txid).await.unwrap().unwrap(),
        ("mined".into(), Some(901), Some("recheck".into()))
    );
}
