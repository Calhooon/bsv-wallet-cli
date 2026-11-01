//! The dead row's pause under the daemon's feed, its reports and a
//! could-not-look fault (#328, the owner's word of 2026-11-01).

use super::super::tests::{header, opts, path_of, FixedClock};
use super::*;
use bsv_wallet_toolbox::services::mock::MockWalletServices;

/// The pause a marked row waits without a header move, at its cap: 64
/// times the age threshold of 600 s, about 10.7 hours.
const PAUSE_CAP: Timestamp = (1 << MAX_BACKOFF_DOUBLINGS) * 600;

/// The daemon's options: the feed is held, so the pass keeps no ring and
/// reads no `moved_from`.
fn feed() -> TickOptions {
    TickOptions {
        feed_held: true,
        ..opts()
    }
}

async fn feed_pass(
    storage: &StorageSqlx,
    services: &MockWalletServices,
    now: Timestamp,
) -> TickReport {
    tick(storage, services, &FixedClock(now), &feed())
        .await
        .unwrap()
}

/// The feed's tip event at `height`, delivered at `cursor` and `now`.
async fn tip(
    storage: &StorageSqlx,
    services: &MockWalletServices,
    now: Timestamp,
    cursor: u64,
    height: u32,
) {
    let hash = format!("{height:064x}");
    let envelope = serde_json::json!({
        "v": 1, "kind": "tip", "height": height, "hash": hash, "time": 1_700_000_000u32,
        "header": {
            "version": 536_870_912, "previousHash": "00".repeat(32), "merkleRoot": "ef".repeat(32),
            "time": 1_700_000_000u32, "bits": 545_259_519_u32, "nonce": 0, "height": height,
            "hash": hash, "chainWork": format!("{}10", "00".repeat(31)),
        },
    });
    let event = decode_envelope(envelope.to_string().as_bytes()).unwrap();
    deliver_chain_event(storage, services, &FixedClock(now), &feed(), cursor, event)
        .await
        .unwrap();
}

/// A rejected row at 900 whose stored proof does not parse, its pause at
/// the cap, marked by one feed-driven pass at 100.
async fn marked_at_the_cap(txid: &str) -> (StorageSqlx, MockWalletServices) {
    let (storage, services) = proven_dead(txid, "rejected", 900).await;
    services.set_header_for_height(header(900, txid, 0));
    unparseable(&storage, txid).await;
    sqlx::query("UPDATE tracker_states SET asks = ? WHERE txid = ?")
        .bind(i64::from(MAX_BACKOFF_DOUBLINGS))
        .bind(txid)
        .execute(storage.writer())
        .await
        .unwrap();
    let first = feed_pass(&storage, &services, 100).await;
    assert_eq!(first.moved_from, None);
    assert_eq!(first.faults, vec![proof_stands_of(txid, "rejected")]);
    assert_eq!(
        mark_of(&storage, txid).await,
        (None, MAX_BACKOFF_DOUBLINGS + 1, Some(100 + PAUSE_CAP))
    );
    (storage, services)
}

#[tokio::test]
async fn a_marked_row_under_the_feed_clears_at_the_next_tip_not_after_its_pause() {
    assert_eq!(PAUSE_CAP, 38_400);
    let txid = "f1".repeat(32);
    let (storage, services) = marked_at_the_cap(&txid).await;
    // The toolbox repairs its path. Inside the pause, with no tip, the
    // row is not selected.
    let repaired = MerklePath::from_coinbase_txid(&txid, 900).to_binary();
    toolbox_path(&storage, &txid, &repaired).await;
    assert_idle(&feed_pass(&storage, &services, 101).await);

    // The feed's tip is the header move: the next pass checks the row
    // again, 38,400 s before its pause would have passed.
    tip(&storage, &services, 102, 1, 1_001).await;
    let after = feed_pass(&storage, &services, 103).await;
    const { assert!(103 < 100 + PAUSE_CAP) };
    assert_eq!(after.adopted_mined, vec![txid.clone()]);
    assert!(after.faults.is_empty());
    assert_eq!(
        mark_of(&storage, &txid).await,
        (Some(path_of(&txid, 900)), 0, None)
    );
}

#[tokio::test]
async fn a_tip_ends_only_a_marked_creators_pause() {
    let txid = "f2".repeat(32);
    let (storage, services) = marked_at_the_cap(&txid).await;
    // A row of the tick's own (an unproven transaction under its re-ask
    // pause) keeps its pause at the tip.
    sqlx::query(
        "INSERT INTO tracker_states (txid, word, asks, next_ask_at) VALUES (?, 'built', 3, 5000)",
    )
    .bind("f3".repeat(32))
    .execute(storage.writer())
    .await
    .unwrap();
    tip(&storage, &services, 102, 1, 1_001).await;
    assert_eq!(mark_of(&storage, &txid).await, (None, 7, Some(102)));
    assert_eq!(
        mark_of(&storage, &"f3".repeat(32)).await,
        (None, 3, Some(5000))
    );
    // A tip at or below the stored cursor was delivered before.
    sqlx::query("UPDATE tracker_states SET next_ask_at = 9000 WHERE txid = ?")
        .bind(&txid)
        .execute(storage.writer())
        .await
        .unwrap();
    tip(&storage, &services, 103, 1, 1_001).await;
    assert_eq!(mark_of(&storage, &txid).await, (None, 7, Some(9000)));
}
