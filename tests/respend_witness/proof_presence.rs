//! #247: proof evidence closes the respend door before a status lookup.

use super::*;

#[tokio::test]
async fn an_unfail_request_refuses_respend() {
    let f = fixture().await;
    // A recovery request can coexist with an unproven transaction whose
    // earlier doubleSpend batch missed the transaction's failed write.
    sqlx::query("UPDATE proven_tx_reqs SET status='unfail' WHERE txid=?")
        .bind(&f.phantom)
        .execute(f.wallet.storage().writer())
        .await
        .unwrap();
    assert_proof_refuses(&f, "unfail").await;
}

#[tokio::test]
async fn an_unproven_request_with_a_proof_link_refuses_respend() {
    let f = fixture().await;
    // Isolate the link: the fixture's existing proof satisfies the foreign
    // key, while no proven_txs row matches the phantom's txid.
    sqlx::query("UPDATE proven_tx_reqs SET status='unproven',proven_tx_id=(SELECT proven_tx_id FROM proven_txs WHERE txid=?) WHERE txid=?")
        .bind(&f.funding)
        .bind(&f.phantom)
        .execute(f.wallet.storage().writer())
        .await
        .unwrap();
    assert_proof_refuses(&f, "unproven").await;
}

#[tokio::test]
async fn a_stored_proof_without_a_request_link_refuses_respend() {
    let f = fixture().await;
    let s = f.wallet.storage();
    sqlx::query("UPDATE proven_tx_reqs SET status='unproven',proven_tx_id=NULL WHERE txid=?")
        .bind(&f.phantom)
        .execute(s.writer())
        .await
        .unwrap();
    sqlx::query("INSERT INTO proven_txs (txid,height,idx,block_hash,merkle_root,merkle_path,raw_tx,created_at,updated_at) SELECT txid,870900,0,?,?,?,raw_tx,created_at,updated_at FROM transactions WHERE transaction_id=?")
        .bind("aa".repeat(32))
        .bind(&f.phantom)
        .bind(proof(&f.phantom))
        .bind(f.phantom_row)
        .execute(s.writer())
        .await
        .unwrap();
    assert_proof_refuses(&f, "unproven").await;
}

async fn assert_proof_refuses(f: &world::Fixture, request_word: &str) {
    let s = f.wallet.storage();
    let before = words(f).await;
    assert_eq!(before, ("unproven".into(), request_word.into()));
    let history: String = sqlx::query_scalar("SELECT history FROM proven_tx_reqs WHERE txid=?")
        .bind(&f.phantom)
        .fetch_one(s.pool())
        .await
        .unwrap();
    let transactions: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM transactions")
        .fetch_one(s.pool())
        .await
        .unwrap();
    let absent = status_source(&f.phantom, "unknown", BroadcastStatus::Unknown);
    headed(&absent, &f.funding);
    let err = build_and_post(s, &absent, &f.root, &f.phantom, &["mock"])
        .await
        .unwrap_err();
    assert_eq!(
        err.to_string(),
        format!(
            "respend refused: a proof is present for {}: not a phantom",
            f.phantom
        )
    );
    for method in [
        "get_broadcaster_statuses",
        "get_status_for_txids",
        "post_beef",
    ] {
        assert_eq!(absent.call_count(method), 0, "{method}");
    }
    assert_eq!(words(f).await, before);
    assert_eq!(coin_state(f, f.coin).await, (false, Some(f.phantom_row)));
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM transactions")
            .fetch_one(s.pool())
            .await
            .unwrap(),
        transactions,
        "no replacement was built"
    );
    assert_eq!(
        sqlx::query_scalar::<_, String>("SELECT history FROM proven_tx_reqs WHERE txid=?")
            .bind(&f.phantom)
            .fetch_one(s.pool())
            .await
            .unwrap(),
        history,
        "no competitor was queued"
    );
}
