//! The probe record over the served router (#539): a page's answers with
//! no row counted per door and level-0 power, off the call's path (a call
//! answers while the store's writer is held, and a failed write is the
//! log's, never the page's), never a grant or a pending row, and none for
//! the seed.

use std::time::Duration;

use axum::body::Body;
use axum::http::Request;
use tower::ServiceExt;

use super::owner_door_tests::wallet;
use super::*;
use crate::store::origin_grants::{self as grants, Power, ProbeRow, Standing};

const PAGE: &str = "http://new.example";

/// The router with the seed `seeded`, and its probe queue.
fn served(wallet: &WalletState, seeded: &[&str]) -> (Router, probes::Probes) {
    let config = ServerConfig::admitting(seeded.iter().copied()).unwrap();
    let probes = config.probes.clone();
    (make_router(wallet.clone(), config), probes)
}

/// A page's call from `origin`: its status.
async fn call(router: &Router, origin: &str, path: &str, body: Option<serde_json::Value>) -> u16 {
    let request = Request::builder()
        .method(if body.is_some() { "POST" } else { "GET" })
        .uri(path)
        .header("origin", origin)
        .header("content-type", "application/json");
    let body = body.map_or_else(Body::empty, |b| Body::from(b.to_string()));
    let response = router.clone().oneshot(request.body(body).unwrap()).await;
    response.unwrap().status().as_u16()
}

fn level_zero(name: &str) -> serde_json::Value {
    json!({"protocolID": [0, name], "keyID": "1", "counterparty": "self", "data": [1, 2, 3]})
}

/// `(originator, door, power, count)` of each row.
async fn counted(wallet: &WalletState) -> Vec<(String, String, Option<String>, i64)> {
    let rows = grants::probes::probes(wallet.storage()).await.unwrap();
    rows.into_iter()
        .map(|r: ProbeRow| (r.originator, r.door, r.power, r.count))
        .collect()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_pages_open_doors_and_level_zero_are_counted_never_granted() {
    let (_dir, wallet) = wallet().await;
    let (router, probes) = served(&wallet, &["seeded.example"]);
    assert_eq!(call(&router, PAGE, "/getVersion", None).await, 200);
    assert_eq!(
        call(&router, "http://new.example:8080", "/getVersion", None).await,
        200
    );
    let signed = call(
        &router,
        PAGE,
        "/createSignature",
        Some(level_zero("wire probe")),
    )
    .await;
    assert_eq!(signed, 200);
    // The seed's answers are its own, never a probe.
    assert_eq!(
        call(&router, "http://seeded.example", "/getVersion", None).await,
        200
    );
    probes.settled().await;
    let zero = Power::level_zero_key("wire probe");
    let page = |door: &str, power: Option<String>, count| {
        ("new.example".to_string(), door.to_string(), power, count)
    };
    assert_eq!(
        counted(&wallet).await,
        vec![
            page("createSignature", zero, 1),
            page("getVersion", None, 2)
        ]
    );
    let listed = crate::server::powers_tests::listed(wallet.storage(), true).await;
    assert!(listed.grants.is_empty() && listed.pending.is_empty());
    // The record is never read as a grant: a level-1 power stands unasked.
    let power = Power::protocol(false, 1, "wire probe", None).unwrap();
    let standing = grants::lookup(wallet.storage(), "new.example", &power, 0).await;
    assert_eq!(standing.unwrap(), Standing::None);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn no_call_waits_on_the_record_and_a_failed_write_is_logged_not_served() {
    let (_dir, wallet) = wallet().await;
    let (router, probes) = served(&wallet, &[]);
    grants::ensure(wallet.storage()).await.unwrap();
    // The writer held: the open door answers at once, the record waits.
    let held = wallet.storage().write_tx().await.unwrap();
    let answered = tokio::time::timeout(
        Duration::from_secs(2),
        call(&router, PAGE, "/getNetwork", None),
    )
    .await;
    assert_eq!(answered.expect("the call waited on the record"), 200);
    drop(held);
    probes.settled().await;
    assert_eq!(counted(&wallet).await.len(), 1);
    // A write the store refuses: the page is answered, the queue drains.
    sqlx::query(
        "CREATE TRIGGER refuse_probes BEFORE UPDATE ON origin_probes \
         BEGIN SELECT RAISE(ABORT, 'refused'); END",
    )
    .execute(wallet.storage().writer())
    .await
    .unwrap();
    assert_eq!(call(&router, PAGE, "/getNetwork", None).await, 200);
    probes.settled().await;
    assert_eq!(counted(&wallet).await[0].3, 1, "the refused write is lost");
}
