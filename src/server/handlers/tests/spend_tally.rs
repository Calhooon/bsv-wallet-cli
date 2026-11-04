//! #414 L6: the spend's row over a broadcast, for a page the seed does not
//! list: a refused broadcast retired leaves the tally as it found it, an
//! accepted one keeps its txid.

use super::mutants::{Presence, World};
use crate::store::origin_grants::{self as grants, Granting, Power};
use axum::http::StatusCode;
use serde_json::{json, Value};
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

/// The page's spend of 3,000 (the world's two outputs), answered within
/// 30 seconds: admitted, never held for an owner who never answers.
async fn spend(w: &World) -> (StatusCode, Option<String>, Value) {
    let (status, answer) = tokio::time::timeout(Duration::from_secs(30), w.create(json!({})))
        .await
        .expect("admitted within the ceiling, never held");
    (status, answer["code"].as_str().map(String::from), answer)
}

#[tokio::test]
async fn a_broadcast_refused_and_retired_leaves_no_row_and_the_next_spend_is_admitted() {
    // The spend fills the ceiling's whole month.
    let w = page(Presence::Absent, 3000).await;
    let retired = spend(&w).await;
    assert_eq!(
        (retired.0, retired.1.as_deref()),
        (StatusCode::BAD_GATEWAY, Some("BROADCAST_REJECTED")),
        "{}",
        retired.2
    );
    assert!(
        spends(&w).await.is_empty(),
        "the retired spend kept its row"
    );
    // The same spend again: had the row stayed, it would be past the ceiling
    // and held. Admitted, it reaches the spend gate, which holds the coin
    // its failed spender names.
    let again = spend(&w).await;
    assert_eq!(
        (again.0, again.1.as_deref()),
        (StatusCode::FORBIDDEN, Some("INPUT_NOT_OFFERED")),
        "{}",
        again.2
    );
    assert!(spends(&w).await.is_empty());
}

#[tokio::test]
async fn a_broadcast_spend_keeps_its_row_with_its_txid() {
    let w = page(Presence::Seen, 3000).await;
    let (status, code, answer) = spend(&w).await;
    assert_eq!((status, code), (StatusCode::OK, None), "{answer}");
    let txid = answer["txid"].as_str().unwrap().to_string();
    assert_eq!(spends(&w).await, vec![(Some(txid), None, 3000)]);
}
