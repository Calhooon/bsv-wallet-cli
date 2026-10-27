//! The tick reconciles a dead stored word with the toolbox's checked proof.

use super::tests::{header, opts, path_of, proven_coin, wallet_storage, FixedClock};
use super::*;
use bsv_wallet_toolbox::services::mock::MockWalletServices;

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

async fn proven_dead(txid: &str, word: &str, height: u32) -> (StorageSqlx, MockWalletServices) {
    let (storage, user) = wallet_storage().await;
    proven_coin(&storage, user, txid, height, 1_000).await;
    dead_row(&storage, txid, word).await;
    (storage, services())
}

async fn assert_rejected_stands(storage: &StorageSqlx, services: &impl WalletServices, txid: &str) {
    let before = stored_bytes(storage, txid).await;
    let report = pass(storage, services, 100).await;
    assert_eq!(
        report.faults,
        vec![format!(
            "{txid}: the stored proof does not check; its rejected stands"
        )]
    );
    assert!(report.adopted_mined.is_empty() && report.adopted_proof_failed.is_empty());
    assert_eq!(stored_bytes(storage, txid).await, before);
    assert!(load_row(storage, txid)
        .await
        .unwrap()
        .unwrap()
        .bump
        .is_none());
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
    read_creators(
        storage,
        services,
        &opts(),
        &[(
            txid.to_string(),
            Some(MerklePath::from_coinbase_txid(txid, 900).to_binary()),
        )],
    )
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
    assert_eq!(
        (after.built, after.hints, after.chain, after.verdict),
        (before.built, before.hints, before.chain, before.verdict)
    );
    assert_mined_readers(&storage, &services, &txid).await;

    let settled = stored_bytes(&storage, &txid).await;
    let next = pass(&storage, &services, 101).await;
    assert_eq!((next.tracked, next.adopted, next.asked), (0, 0, 0));
    assert!(next.adopted_mined.is_empty());
    assert_eq!(stored_bytes(&storage, &txid).await, settled);
    assert_eq!(services.call_count("get_merkle_path"), 0);
}

#[tokio::test]
async fn a_rejected_rows_mismatched_proof_leaves_its_word_and_null_bump() {
    let txid = "c2".repeat(32);
    let (storage, services) = proven_dead(&txid, "rejected", 900).await;
    services.set_header_for_height(header(900, &"dd".repeat(32), 0));
    assert_rejected_stands(&storage, &services, &txid).await;
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
async fn an_unparseable_toolbox_proof_keeps_the_dead_row_untouched() {
    let txid = "c7".repeat(32);
    let (storage, services) = proven_dead(&txid, "rejected", 900).await;
    sqlx::query("UPDATE proven_txs SET merkle_path = X'00' WHERE txid = ?")
        .bind(&txid)
        .execute(storage.writer())
        .await
        .unwrap();
    assert_rejected_stands(&storage, &services, &txid).await;
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

#[tokio::test]
async fn one_adoption_limit_covers_new_and_all_three_dead_words() {
    let (storage, user) = wallet_storage().await;
    let services = services();
    let mut txids = Vec::new();
    for i in 0..MAX_ADOPT_PER_PASS + 3 {
        let txid = format!("{i:08x}{}", "ab".repeat(28));
        let height = 100 + u32::try_from(i).unwrap();
        proven_coin(&storage, user, &txid, height, 1_000).await;
        services.set_header_for_height(header(height, &txid, 0));
        if let Some(word) = [
            None,
            Some("conflicted"),
            Some("rejected"),
            Some("abandoned"),
        ][i % 4]
        {
            dead_row(&storage, &txid, word).await;
        }
        txids.push(txid);
    }
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
