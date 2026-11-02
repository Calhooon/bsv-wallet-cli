//! The stale follow-up's announce, judged by the host's rules that the
//! mutation oracle found unjudged (the owner's word of 2026-11-01, 23.5
//! (c); the second batch of `src/tracker_host.rs`'s survivors of night
//! 41340299977): the BEEF announced once is the wallet's own stored
//! transaction with its inputs, and a refused announce is the node's
//! verdict only when the node judged the transaction itself (the charter's
//! evidence rule: a verdict is evidence, a service fault is "could not
//! look").

use super::tests::wallet_storage;
use super::*;
use crate::test_support::{p2pkh, raw_tx, txid_of};
use bsv_wallet_toolbox::services::mock::success_post_beef_result;
use bsv_wallet_toolbox::services::PostTxResultForTxid;
use chrono::Utc;

/// One broadcaster's answer on `txid` that did not take it: its status
/// `error`, the node's text `data`, and the flags the broadcaster set.
fn refused(txid: &str, data: &str, service_error: bool, orphan_mempool: bool) -> PostBeefResult {
    PostBeefResult {
        name: "Arcade".to_string(),
        status: "error".to_string(),
        txid_results: vec![PostTxResultForTxid {
            txid: txid.to_string(),
            status: "error".to_string(),
            double_spend: false,
            competing_txs: None,
            data: Some(data.to_string()),
            orphan_mempool,
            service_error,
            block_hash: None,
            block_height: None,
            notes: vec![],
        }],
        error: None,
        notes: vec![],
    }
}

/// The evidence rule's verdict (the tracker charter, section 2): a node
/// that refused the transaction itself is the `rejected` verdict, its
/// reason the broadcaster and the node's own words.
#[test]
fn a_nodes_refusal_of_the_transaction_is_its_rejected_verdict() {
    let txid = "c1".repeat(32);
    let verdict = node_verdict(
        &[refused(
            &txid,
            "bad-txns-inputs-missingorspent",
            false,
            false,
        )],
        &txid,
    );
    assert_eq!(
        verdict,
        Some(NodeVerdict::Rejected {
            reason: "Arcade: bad-txns-inputs-missingorspent".to_string(),
        })
    );
}

/// "Could not look" is never a verdict (the charter's evidence rule; the
/// explorer ladder's fault): a broadcaster's service fault, a missing
/// parent (orphan mempool) and "not final" each leave the word where it
/// was, as does any broadcaster that took the transaction.
#[test]
fn a_service_fault_a_missing_parent_or_not_final_is_no_verdict() {
    let txid = "c2".repeat(32);
    let cases = [
        (
            "a service fault",
            refused(&txid, "503 unavailable", true, false),
        ),
        (
            "a missing parent",
            refused(&txid, "missing inputs", false, true),
        ),
        ("not final", refused(&txid, "476 non-final", false, false)),
    ];
    for (what, answer) in cases {
        assert_eq!(node_verdict(&[answer], &txid), None, "{what}");
    }
    let taken = [
        refused(&txid, "bad-txns-inputs-missingorspent", false, false),
        success_post_beef_result("Taal", &[&txid]),
    ];
    assert_eq!(node_verdict(&taken, &txid), None, "one broadcaster took it");
}

/// A transaction row of the wallet's own, `raw` with its stored input BEEF.
async fn own_tx(storage: &StorageSqlx, user_id: i64, raw: &[u8], input_beef: &[u8]) {
    sqlx::query(
        "INSERT INTO transactions (user_id, status, reference, is_outgoing, satoshis, version, lock_time, description, txid, raw_tx, input_beef, created_at, updated_at) \
         VALUES (?, 'unproven', ?, 1, 0, 1, 0, 'd', ?, ?, ?, ?, ?)",
    )
    .bind(user_id)
    .bind(&txid_of(raw)[..8])
    .bind(txid_of(raw))
    .bind(raw)
    .bind(input_beef)
    .bind(Utc::now())
    .bind(Utc::now())
    .execute(storage.writer())
    .await
    .unwrap();
}

/// The txids a BEEF carries, in its order.
fn txids_of(beef: &[u8]) -> Vec<String> {
    Beef::from_binary(beef)
        .unwrap()
        .txs
        .iter()
        .map(|tx| tx.txid())
        .collect()
}

/// The one announce of a stale transaction (the charter's section 4, the
/// follow-up) sends the wallet's own stored transaction with the inputs
/// its stored BEEF holds, so a node can validate it; an empty stored input
/// BEEF reads as none, the transaction alone.
#[tokio::test]
async fn the_announced_beef_is_the_stored_transaction_with_its_inputs() {
    let (storage, user) = wallet_storage().await;
    let parent = raw_tx(&[(&"11".repeat(32), 0)], &[(2_000, p2pkh([1; 20]))]);
    let child = raw_tx(&[(&txid_of(&parent), 0)], &[(1_500, p2pkh([2; 20]))]);
    let mut inputs = Beef::new();
    inputs.merge_raw_tx(parent.clone(), None);
    own_tx(&storage, user, &child, &inputs.to_binary()).await;

    let beef = stored_beef(&storage, &txid_of(&child))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(txids_of(&beef), vec![txid_of(&parent), txid_of(&child)]);

    let lone = raw_tx(&[(&"22".repeat(32), 1)], &[(900, p2pkh([3; 20]))]);
    own_tx(&storage, user, &lone, &[]).await;
    let beef = stored_beef(&storage, &txid_of(&lone))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(txids_of(&beef), vec![txid_of(&lone)]);

    assert_eq!(stored_beef(&storage, &"33".repeat(32)).await.unwrap(), None);
}
