//! #414 L6: the spend's row over a broadcast, for a page the seed does not
//! list: a refused broadcast retired leaves the tally as it found it, an
//! accepted one keeps its txid.

use super::mutants::{Presence, World};
use crate::store::origin_grants::{self as grants, Granting, Power};
use axum::http::StatusCode;
use serde_json::json;
use std::time::Duration;

/// The page asked per power, its month's ceiling `amount`.
async fn page(presence: Presence, amount: i64) -> World {
    let w = World::serving(presence, false, None).await;
    let granting = Granting {
        originator: "handler.test",
        power: &Power::Spending,
        amount: Some(amount),
        expiry: 0,
    };
    let now = crate::server::owner_door::now().unwrap();
    grants::grant(w.wallet.storage(), &granting, now)
        .await
        .unwrap();
    w
}

async fn spends(w: &World) -> Vec<(Option<String>, Option<String>, i64)> {
    sqlx::query_as("SELECT txid, reference, satoshis FROM origin_spends ORDER BY spend_id")
        .fetch_all(w.wallet.storage().pool())
        .await
        .unwrap()
}

#[tokio::test]
async fn a_broadcast_refused_and_retired_leaves_no_row_and_the_next_spend_is_admitted() {
    // The call's outputs sum to 3,000, the ceiling's whole month.
    let w = page(Presence::Absent, 3000).await;
    let (status, answer) = w.create(json!({})).await;
    assert_eq!(status, StatusCode::BAD_GATEWAY, "{answer}");
    assert_eq!(answer["code"], "BROADCAST_REJECTED");
    assert!(
        spends(&w).await.is_empty(),
        "the retired spend kept its row"
    );
    // The same spend again: had the row stayed, 3,000 more would be past the
    // ceiling and held for an owner who never answers. Admitted, it reaches
    // the spend gate, which holds the coin its failed spender names.
    let (status, answer) = tokio::time::timeout(Duration::from_secs(30), w.create(json!({})))
        .await
        .expect("admitted within the ceiling, never held");
    assert_eq!(
        (status, answer["code"].as_str()),
        (StatusCode::FORBIDDEN, Some("INPUT_NOT_OFFERED")),
        "{answer}"
    );
    assert!(spends(&w).await.is_empty());
}

#[tokio::test]
async fn a_broadcast_spend_keeps_its_row_with_its_txid() {
    let w = page(Presence::Seen, 3000).await;
    let (status, answer) = w.create(json!({})).await;
    assert_eq!(status, StatusCode::OK, "{answer}");
    let txid = answer["txid"].as_str().unwrap().to_string();
    assert_eq!(spends(&w).await, vec![(Some(txid), None, 3000)]);
}
