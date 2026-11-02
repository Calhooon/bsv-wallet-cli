//! A toolbox proof whose header a kept reorg took is not mined evidence
//! (the rule by the root, ruling (5) of 2026-10-20 19:4xZ, bsv-stack-lean
//! `docs/RULINGS-2026-10-12.md`: a `mined` word is `stale` when its root is
//! not the active header's root): the adoption pass checks the stored path
//! against the headers of the moment, so a creator it replays to another
//! word is neither adopted nor tracked as an unparseable path.

use super::super::tests::{header_with_hash, reorg_at_900};
use super::*;

/// A row of `txid` tracked without its path, marked to be checked again
/// at 50, keeping the reorg that took the header at 900 its toolbox path
/// checks in; `rejected` when the node refused it, else `built`.
async fn reorged_row(storage: &StorageSqlx, txid: &str, rejected: bool) {
    ensure_tables(storage).await.unwrap();
    let gone = header_with_hash(900, txid, 0).hash;
    let reorg = decode_envelope(reorg_at_900(&gone, txid).to_string().as_bytes()).unwrap();
    let mut row = TrackedRow::new(txid);
    row.chain
        .push(serde_json::to_string(&ChainEnvelope::new(reorg)).unwrap());
    row.verdict = rejected.then(|| StoredVerdict::Rejected {
        reason: "node refused the transaction".to_string(),
    });
    row.asks = 1;
    row.next_ask_at = Some(50);
    store_unread(storage, &row).await;
}

/// A creator of ours proven by the toolbox at 900 (its root the txid),
/// its tracker row `reorged_row`, the header service answering another
/// root at 900 under a tip at 1000.
async fn reorged(txid: &str, rejected: bool) -> (StorageSqlx, MockWalletServices) {
    let (storage, user) = wallet_storage().await;
    proven_coin(&storage, user, txid, 900, 1_000).await;
    reorged_row(&storage, txid, rejected).await;
    let services = services();
    services.set_header_for_height(header(900, &"ef".repeat(32), 0));
    (storage, services)
}

/// A dead row's proof that replays `stale` revives nothing: its
/// `rejected` stands with the reorg's re-ask, reported once, and it is not
/// adopted `mined`.
#[tokio::test]
async fn a_dead_rows_proof_a_kept_reorg_took_is_not_adopted_mined() {
    let txid = "f7".repeat(32);
    let (storage, services) = reorged(&txid, true).await;
    rejected_stands_once(&storage, &services, &txid).await;
    assert_word(&storage, &txid, ("rejected", None, Some("reorg@900"))).await;
}

/// A creator tracked without its path whose toolbox path checks but
/// replays `stale` is not adopted: the pass says the word the proof gave,
/// never that the path does not parse, and leaves the row's word and mark
/// as they were.
#[tokio::test]
async fn a_proof_that_replays_to_another_word_is_not_adopted_and_not_called_unparseable() {
    let txid = "f8".repeat(32);
    let (storage, services) = reorged(&txid, false).await;
    let report = pass(&storage, &services, 100).await;
    assert_eq!(
        report.faults,
        vec![format!(
            "{txid}: not adopted: the stored proof gave the word stale"
        )]
    );
    assert!(report.adopted_mined.is_empty());
    assert_word(&storage, &txid, ("built", None, None)).await;
    assert_eq!(mark_of(&storage, &txid).await, (None, 1, Some(50)));
}
