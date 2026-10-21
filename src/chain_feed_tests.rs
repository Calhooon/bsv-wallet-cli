//! The feed loop against a scripted feed: an in-process HTTP stub serving
//! the page and the stream (chaintracks-cloudflare v0.4.1
//! `docs/CHAIN-EVENTS.md`, "Delivery and cursors"), the tracker's rows in
//! an in-memory wallet, and the toolbox's mock services as the daemon's
//! headers and proofs.

use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::extract::{Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::Router;
use bsv_wallet_toolbox::services::mock::MockWalletServices;
use bsv_wallet_toolbox::{StorageSqlx, BROADCAST_STATUS_MINED};
use chrono::Utc;
use serde_json::{json, Value};

use super::*;
use crate::tracker_host::tests::{
    header, header_with_hash, insert_tx, opts, path_of, proof_answer, proven_coin, wallet_storage,
    FixedClock,
};
use crate::tracker_host::{feed_cursor, stored_word, tick};

/// What the stub serves, in order, and what it was asked.
#[derive(Default)]
struct Script {
    pages: VecDeque<(u16, String)>,
    streams: VecDeque<(u16, String)>,
    /// `since` of each page request.
    page_since: Vec<String>,
    /// `since` and `Last-Event-ID` of each stream request.
    stream_seen: Vec<(String, Option<String>)>,
}

type Shared = Arc<Mutex<Script>>;

/// The feed's own refusal after the scripted connections: an `event:
/// error`, then EOF, which stops the loop (and so the test).
const FEED_UNAVAILABLE: &str = "event: error\ndata: {\"code\":\"feedUnavailable\"}\n\n";

async fn page(State(s): State<Shared>, Query(q): Query<HashMap<String, String>>) -> Response {
    let since = q.get("since").cloned().unwrap_or_default();
    let mut s = s.lock().unwrap();
    s.page_since.push(since.clone());
    let (code, body) = s.pages.pop_front().unwrap_or_else(|| {
        (
            200,
            json!({"v": 1, "cursor": since, "events": [], "hasMore": false}).to_string(),
        )
    });
    (
        StatusCode::from_u16(code).unwrap(),
        [
            ("content-type", "application/json"),
            ("x-chain-event-version", "2"),
        ],
        body,
    )
        .into_response()
}

async fn stream(
    State(s): State<Shared>,
    Query(q): Query<HashMap<String, String>>,
    headers: HeaderMap,
) -> Response {
    let mut s = s.lock().unwrap();
    s.stream_seen.push((
        q.get("since").cloned().unwrap_or_default(),
        headers
            .get("last-event-id")
            .map(|v| v.to_str().unwrap().to_string()),
    ));
    let (code, body) = s
        .streams
        .pop_front()
        .unwrap_or_else(|| (200, FEED_UNAVAILABLE.to_string()));
    (
        StatusCode::from_u16(code).unwrap(),
        [
            ("content-type", "text/event-stream"),
            ("x-chain-event-version", "2"),
        ],
        body,
    )
        .into_response()
}

async fn stub(script: Script) -> (String, Shared) {
    let shared: Shared = Arc::new(Mutex::new(script));
    let app = Router::new()
        .route("/events", get(page))
        .route("/events/stream", get(stream))
        .with_state(shared.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    (format!("http://{addr}"), shared)
}

fn fast() -> FeedOptions {
    FeedOptions {
        min_pause: Duration::from_millis(5),
        max_pause: Duration::from_millis(20),
    }
}

/// A public event header (all nine fields).
fn eh(height: u32, hash: &str, previous: &str, root: &str) -> Value {
    json!({
        "version": 536870912, "previousHash": previous, "merkleRoot": root,
        "time": 1_700_000_000u32, "bits": 545259519u32, "nonce": 0, "height": height,
        "hash": hash, "chainWork": format!("{}10", "00".repeat(31)),
    })
}

fn page_of(events: &[(u64, Value)], has_more: bool) -> (u16, String) {
    let cursor = events
        .last()
        .map(|(c, _)| c.to_string())
        .unwrap_or_default();
    let events: Vec<Value> = events
        .iter()
        .map(|(c, e)| json!({"cursor": c.to_string(), "event": e}))
        .collect();
    (
        200,
        json!({"v": 1, "cursor": cursor, "events": events, "hasMore": has_more}).to_string(),
    )
}

fn sse(events: &[(u64, Value)]) -> String {
    let mut out = String::new();
    for (cursor, event) in events {
        out.push_str(&format!("id: {cursor}\ndata: {event}\n\n: heartbeat\n\n"));
    }
    out
}

/// A wallet whose one transaction `T` is `mined` at 900 by a checked
/// proof (one tracker pass), with the services that proved it.
async fn mined_at_900() -> (StorageSqlx, MockWalletServices, String) {
    let (storage, user) = wallet_storage().await;
    let txid = "c3".repeat(32);
    insert_tx(&storage, user, &txid, "unproven").await;
    storage
        .record_broadcast_status(&txid, "arcade", BROADCAST_STATUS_MINED)
        .await
        .unwrap();
    let services = MockWalletServices::builder()
        .get_merkle_path_response(proof_answer(Some(path_of(&txid, 900))))
        .build();
    services.set_header_for_height(header(900, &txid, 0));
    services.set_tip_header(header_with_hash(905, &"0a".repeat(32), 0));
    tick(&storage, &services, &clock(), &opts()).await.unwrap();
    assert_eq!(word(&storage, &txid).await.0, "mined");
    (storage, services, txid)
}

fn clock() -> FixedClock {
    FixedClock(Utc::now().timestamp() as u64)
}

async fn word(storage: &StorageSqlx, txid: &str) -> (String, Option<u32>, Option<String>) {
    stored_word(storage, txid).await.unwrap().unwrap()
}

async fn follow(base: &str, storage: &StorageSqlx, services: &MockWalletServices) -> FeedStatus {
    let status = tokio::time::timeout(
        Duration::from_secs(20),
        run(base, storage, services, &clock(), &opts(), &fast()),
    )
    .await
    .expect("the scripted feed ends with an error event");
    println!("feed status: {}", serde_json::to_string(&status).unwrap());
    assert_eq!(read_status(storage).await.unwrap().as_ref(), Some(&status));
    status
}

fn fault_kind(status: &FeedStatus) -> &str {
    status.fault.as_ref().map(|f| f.kind.as_str()).unwrap_or("")
}

/// A reorg replacing T's block arrives on the feed: T's word goes `stale`
/// with `Reask::Reorg` at the replaced height, the cursor advances, and
/// the next pass asks for the new proof and the word is `mined` again.
#[tokio::test]
async fn a_reorg_on_the_feed_stales_the_word_and_the_new_proof_mines_it() {
    let (storage, services, txid) = mined_at_900().await;
    let prior = "b1".repeat(32);
    let reorg = json!({
        "v": 1, "kind": "reorg", "forkHeight": 900, "depth": 1,
        "deactivatedHeaders": [eh(900, &prior, &"b0".repeat(32), &txid)],
        "newTip": eh(901, &"b3".repeat(32), &"b2".repeat(32), &"ee".repeat(32)),
    });
    // The header service has moved: 900 is another block now.
    services.set_header_for_height(header(900, &"ee".repeat(32), 7));
    let (base, shared) = stub(Script {
        pages: [page_of(&[(1, reorg)], false)].into(),
        ..Script::default()
    })
    .await;

    let status = follow(&base, &storage, &services).await;
    assert_eq!(fault_kind(&status), "feedUnavailable");
    assert_eq!(status.cursor, "1");
    assert_eq!(status.fault.as_ref().unwrap().cursor, "1");
    assert_eq!(feed_cursor(&storage).await.unwrap(), 1);
    assert_eq!(
        word(&storage, &txid).await,
        ("stale".to_string(), None, Some("reorg@900".to_string()))
    );
    assert_eq!(shared.lock().unwrap().page_since, vec!["0".to_string()]);

    // The new proof: T is in the block at 901 on the active chain.
    let services = MockWalletServices::builder()
        .get_merkle_path_response(proof_answer(Some(path_of(&txid, 901))))
        .build();
    services.set_header_for_height(header(900, &"ee".repeat(32), 7));
    services.set_header_for_height(header(901, &txid, 0));
    services.set_tip_header(header_with_hash(905, &"0a".repeat(32), 0));
    let report = tick(&storage, &services, &clock(), &opts()).await.unwrap();
    println!("{}", report.summary());
    assert_eq!(report.mined, vec![txid.clone()]);
    assert_eq!(
        word(&storage, &txid).await,
        ("mined".to_string(), Some(901), None)
    );
}

/// A fork at T's height marks it suspect (`Reask::Fork`), its word still
/// `mined`; the next tip rechecks it against the headers and clears it.
/// Each step is one daemon run, from the stored cursor.
#[tokio::test]
async fn a_fork_marks_the_word_suspect_and_a_tip_rechecks_it() {
    let (storage, services, txid) = mined_at_900().await;
    let fork = json!({
        "v": 1, "kind": "fork", "height": 900, "depth": 1,
        "competingTips": [
            eh(900, &"d1".repeat(32), &"d0".repeat(32), &txid),
            eh(900, &"d2".repeat(32), &"d0".repeat(32), &"ee".repeat(32)),
        ],
    });
    let (base, _) = stub(Script {
        pages: [page_of(&[(1, fork)], false)].into(),
        ..Script::default()
    })
    .await;
    follow(&base, &storage, &services).await;
    assert_eq!(
        word(&storage, &txid).await,
        ("mined".to_string(), Some(900), Some("fork@900".to_string()))
    );

    let tip_header = eh(901, &"d3".repeat(32), &"d1".repeat(32), &"ef".repeat(32));
    let tip = json!({
        "v": 1, "kind": "tip", "height": 901, "hash": "d3".repeat(32),
        "time": 1_700_000_000u32, "header": tip_header,
    });
    let (base, shared) = stub(Script {
        pages: [page_of(&[(2, tip)], false)].into(),
        ..Script::default()
    })
    .await;
    let status = follow(&base, &storage, &services).await;
    assert_eq!(status.cursor, "2");
    assert_eq!(shared.lock().unwrap().page_since, vec!["1".to_string()]);
    assert_eq!(
        word(&storage, &txid).await,
        ("mined".to_string(), Some(900), None)
    );
}

/// The split draft (bsv-stack-lean
/// `corpus/scenarios/tracker-split-observation-writes-no-word.md`): a split
/// above T's block touches nothing; a split at T's height marks it suspect
/// with `Reask::Fork` and moves no word; `splitResolved` rechecks it
/// through the headers and T is `mined` at its height throughout.
#[tokio::test]
async fn a_split_and_its_resolution_move_no_word() {
    let (storage, services, txid) = mined_at_900().await;
    let split = |fork_height: u32| {
        json!({
            "v": 2, "kind": "split", "key": format!("split:{}", "5a".repeat(32)),
            "forkHeight": fork_height,
            "tips": [
                {"hash": "a1".repeat(32), "height": fork_height + 2,
                 "chainWork": format!("{}c4", "00".repeat(31)), "rungs": ["bitails", "woc"]},
                {"hash": "a2".repeat(32), "height": fork_height + 2,
                 "chainWork": format!("{}c4", "00".repeat(31)), "rungs": ["arcade"]},
            ],
        })
    };
    let resolved = |fork_height: u32| {
        json!({"v": 2, "kind": "splitResolved",
               "key": format!("split:{}", "5a".repeat(32)), "forkHeight": fork_height})
    };

    // Columns 1 to 3: the split above T's block.
    let (base, _) = stub(Script {
        streams: [(200, sse(&[(1, split(901)), (2, resolved(901))]))].into(),
        ..Script::default()
    })
    .await;
    follow(&base, &storage, &services).await;
    assert_eq!(
        word(&storage, &txid).await,
        ("mined".to_string(), Some(900), None)
    );

    // Column 4: the split at T's height, then its resolution.
    let (base, _) = stub(Script {
        streams: [(200, sse(&[(3, split(900))]))].into(),
        ..Script::default()
    })
    .await;
    follow(&base, &storage, &services).await;
    assert_eq!(
        word(&storage, &txid).await,
        ("mined".to_string(), Some(900), Some("fork@900".to_string()))
    );
    let (base, _) = stub(Script {
        streams: [(200, sse(&[(4, resolved(900))]))].into(),
        ..Script::default()
    })
    .await;
    let status = follow(&base, &storage, &services).await;
    assert_eq!(status.cursor, "4");
    assert_eq!(
        word(&storage, &txid).await,
        ("mined".to_string(), Some(900), None)
    );
}

/// F2 of the stack's coin read (bsv-stack-lean #164), the scenario
/// `tracker-split-observation-writes-no-word.md` column 4 on a coin whose
/// creator the toolbox's monitor proved first (`completed`, a stored
/// proof): the daemon's served pass adopts it `mined`, a `split` at its
/// height marks it suspect and the gate holds the coin, and
/// `splitResolved` rechecks it through the headers and the coin is offered
/// again. No word moves.
#[tokio::test]
async fn a_split_at_an_adopted_creators_height_holds_its_coin_until_resolved() {
    let (storage, user) = wallet_storage().await;
    let txid = "9b".repeat(32);
    proven_coin(&storage, user, &txid, 900, 3_000).await;
    let services = MockWalletServices::builder().build();
    services.set_header_for_height(header(900, &txid, 0));
    services.set_tip_header(header_with_hash(900, &"0a".repeat(32), 0));
    crate::broadcast_reconcile::run_served_pass(&storage, &services, &clock(), &opts(), None)
        .await
        .unwrap();
    assert_eq!(
        word(&storage, &txid).await,
        ("mined".to_string(), Some(900), None)
    );
    let gate = crate::spend_gate::GateOptions {
        spend_depth: 1,
        own_change: true,
        allow_received: false,
    };
    let offered = |a: &crate::spend_gate::Assessment| a.offered_satoshis();
    let a = crate::spend_gate::assess(&storage, &services, &gate, None)
        .await
        .unwrap();
    println!("[before] offered {} held {:?}", offered(&a), a.held);
    assert_eq!((offered(&a), a.held.len()), (3_000, 0));

    let split = json!({
        "v": 2, "kind": "split", "key": format!("split:{}", "5a".repeat(32)),
        "forkHeight": 900,
        "tips": [
            {"hash": "a1".repeat(32), "height": 902,
             "chainWork": format!("{}c4", "00".repeat(31)), "rungs": ["bitails", "woc"]},
            {"hash": "a2".repeat(32), "height": 902,
             "chainWork": format!("{}c4", "00".repeat(31)), "rungs": ["arcade"]},
        ],
    });
    let (base, _) = stub(Script {
        streams: [(200, sse(&[(1, split)]))].into(),
        ..Script::default()
    })
    .await;
    follow(&base, &storage, &services).await;
    assert_eq!(
        word(&storage, &txid).await,
        ("mined".to_string(), Some(900), Some("fork@900".to_string()))
    );
    let a = crate::spend_gate::assess(&storage, &services, &gate, None)
        .await
        .unwrap();
    println!(
        "[during the split] offered {} held {:?}",
        offered(&a),
        a.held
    );
    assert_eq!(offered(&a), 0);
    assert_eq!(a.held.len(), 1);
    assert_eq!(a.held[0].outpoint, format!("{txid}.0"));
    assert_eq!(
        a.held[0].reason,
        "suspect: a fork or an unresolved split at or below its height"
    );
    assert_eq!(
        crate::tracker_host::open_split_floor(&storage)
            .await
            .unwrap(),
        Some(900)
    );

    let resolved = json!({"v": 2, "kind": "splitResolved",
        "key": format!("split:{}", "5a".repeat(32)), "forkHeight": 900});
    let (base, _) = stub(Script {
        streams: [(200, sse(&[(2, resolved)]))].into(),
        ..Script::default()
    })
    .await;
    follow(&base, &storage, &services).await;
    assert_eq!(
        word(&storage, &txid).await,
        ("mined".to_string(), Some(900), None)
    );
    let a = crate::spend_gate::assess(&storage, &services, &gate, None)
        .await
        .unwrap();
    println!(
        "[after splitResolved] offered {} held {:?}",
        offered(&a),
        a.held
    );
    assert_eq!((offered(&a), a.held.len()), (3_000, 0));
    assert_eq!(
        crate::tracker_host::open_split_floor(&storage)
            .await
            .unwrap(),
        None
    );
}

/// F6 of the stack's coin read: while following, the stored status is
/// rewritten after every delivery (N = 1), so `status.cursor` is the
/// stored cursor and `delivered` and `updatedAt` move with it.
#[tokio::test]
async fn the_status_is_rewritten_after_every_delivery() {
    let (storage, services, _) = mined_at_900().await;
    let at = FixedClock(1_800_000_000);
    let mut feed = Feed {
        base: "http://127.0.0.1:9".to_string(),
        client: reqwest::Client::new(),
        storage: &storage,
        services: &services,
        clock: &at,
        opts: &opts(),
        status: FeedStatus {
            source: "http://127.0.0.1:9".into(),
            state: "following".into(),
            cursor: "0".into(),
            delivered: 0,
            reconnects: 0,
            fault: None,
            last_error: None,
            updated_at: 0,
        },
    };
    for cursor in 1..=3u64 {
        let height = 905 + cursor as u32;
        let tip = json!({
            "v": 1, "kind": "tip", "height": height, "hash": "d3".repeat(32),
            "time": 1_700_000_000u32,
            "header": eh(height, &"d3".repeat(32), &"d1".repeat(32), &"ef".repeat(32)),
        });
        feed.deliver(&cursor.to_string(), tip.to_string().as_bytes())
            .await
            .map_err(|_| "a delivery")
            .unwrap();
        let stored = read_status(&storage).await.unwrap().unwrap();
        println!(
            "after {cursor}: {}",
            serde_json::to_string(&stored).unwrap()
        );
        assert_eq!(stored.state, "following");
        assert_eq!(
            stored.cursor,
            feed_cursor(&storage).await.unwrap().to_string()
        );
        assert_eq!(stored.delivered, cursor);
        assert_eq!(stored.updated_at, 1_800_000_000);
    }
}

/// An envelope the decoder refuses stops the loop at the stored cursor:
/// the status names it and the offending cursor, and the next envelope in
/// the same page is not delivered (no skip).
#[tokio::test]
async fn a_decode_fault_stops_the_loop_at_its_cursor() {
    let (storage, services, txid) = mined_at_900().await;
    let tip_age = json!({"v": 1, "kind": "tipAge", "seconds": 600,
        "tip": eh(905, &"0a".repeat(32), &"09".repeat(32), &"0b".repeat(32))});
    let unknown = json!({"v": 3, "kind": "tip"});
    let fork = json!({
        "v": 1, "kind": "fork", "height": 900, "depth": 1,
        "competingTips": [
            eh(900, &"d1".repeat(32), &"d0".repeat(32), &txid),
            eh(900, &"d2".repeat(32), &"d0".repeat(32), &"ee".repeat(32)),
        ],
    });
    let (base, shared) = stub(Script {
        pages: [page_of(&[(1, tip_age), (2, unknown), (3, fork)], false)].into(),
        ..Script::default()
    })
    .await;
    let status = follow(&base, &storage, &services).await;
    let fault = status.fault.clone().unwrap();
    assert_eq!(fault.kind, "decode");
    assert_eq!(fault.cursor, "1", "stopped at the last delivered cursor");
    assert_eq!(fault.event_cursor.as_deref(), Some("2"));
    assert!(
        fault.detail.contains("UnknownVersion(3)"),
        "{}",
        fault.detail
    );
    assert_eq!(status.state, "stopped");
    assert_eq!(feed_cursor(&storage).await.unwrap(), 1);
    assert_eq!(
        word(&storage, &txid).await.2,
        None,
        "the fork after it is not delivered"
    );
    assert!(
        shared.lock().unwrap().stream_seen.is_empty(),
        "no stream past a fault"
    );
}

/// A cursor ahead of the journal (HTTP 409 `cursorAhead`) is reported and
/// stops the loop.
#[tokio::test]
async fn cursor_ahead_is_reported() {
    let (storage, services, _) = mined_at_900().await;
    let (base, shared) = stub(Script {
        pages: [(409, json!({"code": "cursorAhead"}).to_string())].into(),
        ..Script::default()
    })
    .await;
    let status = follow(&base, &storage, &services).await;
    let fault = status.fault.unwrap();
    assert_eq!(fault.kind, "cursorAhead");
    assert_eq!(fault.cursor, "0");
    assert!(fault.detail.contains("409"), "{}", fault.detail);
    assert!(shared.lock().unwrap().stream_seen.is_empty());
}

/// A stream that ends without an error event reconnects: the page from
/// the stored cursor, then the stream with `Last-Event-ID`. A repeated
/// envelope (delivery is at least once) is delivered once.
#[tokio::test]
async fn a_dropped_stream_reconnects_with_last_event_id() {
    let (storage, services, _) = mined_at_900().await;
    let tip_age = |seconds: u64| {
        json!({"v": 1, "kind": "tipAge", "seconds": seconds,
               "tip": eh(905, &"0a".repeat(32), &"09".repeat(32), &"0b".repeat(32))})
    };
    let (base, shared) = stub(Script {
        streams: [
            (200, sse(&[(1, tip_age(10)), (2, tip_age(20))])),
            (
                200,
                sse(&[(2, tip_age(20)), (3, tip_age(30))]) + FEED_UNAVAILABLE,
            ),
        ]
        .into(),
        ..Script::default()
    })
    .await;
    let status = follow(&base, &storage, &services).await;
    assert_eq!(fault_kind(&status), "feedUnavailable");
    assert_eq!(status.cursor, "3");
    assert_eq!(status.delivered, 3);
    assert_eq!(status.reconnects, 1);
    assert_eq!(feed_cursor(&storage).await.unwrap(), 3);
    let s = shared.lock().unwrap();
    assert_eq!(s.page_since, vec!["0".to_string(), "2".to_string()]);
    assert_eq!(
        s.stream_seen,
        vec![
            ("0".to_string(), None),
            ("2".to_string(), Some("2".to_string())),
        ]
    );
}

#[test]
fn an_sse_message_is_read_with_its_comments_dropped() {
    assert_eq!(
        SseMessage::parse(": heartbeat\nid: 42\ndata: {\"v\":1}\n\n"),
        SseMessage {
            id: Some("42".into()),
            event: None,
            data: Some("{\"v\":1}".into()),
        }
    );
    assert_eq!(
        SseMessage::parse("event: error\ndata: a\ndata: b\n\n").data,
        Some("a\nb".into())
    );
}

// ---------------------------------------------------------------------------
// The reorg below the user (bsv-stack-lean
// `corpus/scenarios/2026-10-20-reorg-below-the-user.md`; rulings (5) of
// 2026-10-20 19:4xZ and 7 of 20:1xZ, `docs/RULINGS-2026-10-12.md`)
// ---------------------------------------------------------------------------

/// The hash the wallet's row holds for T's checked header.
async fn checked_hash(storage: &StorageSqlx, txid: &str) -> String {
    sqlx::query_scalar("SELECT header_hash FROM tracker_states WHERE txid = ?")
        .bind(txid)
        .fetch_one(storage.pool())
        .await
        .unwrap()
}

async fn follow_up_of(storage: &StorageSqlx, txid: &str) -> Option<String> {
    sqlx::query_scalar("SELECT follow_up FROM tracker_states WHERE txid = ?")
        .bind(txid)
        .fetch_one(storage.pool())
        .await
        .unwrap()
}

/// The reorg of the draft's step 3 on the feed: `reorg(900, 1, [A1], B2)`.
fn reorg_of_a1(a1_hash: &str, txid: &str) -> Value {
    json!({
        "v": 1, "kind": "reorg", "forkHeight": 900, "depth": 1,
        "deactivatedHeaders": [eh(900, a1_hash, &"b0".repeat(32), txid)],
        "newTip": eh(901, &"b3".repeat(32), &"b2".repeat(32), &"ee".repeat(32)),
    })
}

/// The pass's options with the feed held. The daemon also runs the
/// monitor (`monitor_runs`); these keep the pass's own follow-up.
fn held() -> TickOptions {
    TickOptions {
        feed_held: true,
        ..opts()
    }
}

/// T `stale` with `reorg@900` after the draft's step 3: mined at 900 on
/// A1, then the reorg on the feed with the header service on B1.
async fn stale_at_900() -> (StorageSqlx, String) {
    let (storage, services, txid) = mined_at_900().await;
    let a1 = checked_hash(&storage, &txid).await;
    services.set_header_for_height(header(900, &"ee".repeat(32), 7));
    let (base, _) = stub(Script {
        pages: [page_of(&[(1, reorg_of_a1(&a1, &txid))], false)].into(),
        ..Script::default()
    })
    .await;
    follow(&base, &storage, &services).await;
    assert_eq!(
        word(&storage, &txid).await,
        ("stale".to_string(), None, Some("reorg@900".to_string()))
    );
    (storage, txid)
}

/// Step 3, ruling (5): the reorg arrives before the header service answers
/// the new branch (it still gives A1 at 900). The crate's recheck has no
/// header of the new branch to read, so T stands suspect with
/// `Reask::Recheck`, never `mined` on A1's header and never `stale` by the
/// height alone. The next tip, with the service on B1, rechecks it: A1's
/// root is gone, T is `stale` with `reorg@900`.
#[tokio::test]
async fn step_3_a_reorg_before_the_service_moves_leaves_the_word_suspect_until_the_next_tip() {
    let (storage, services, txid) = mined_at_900().await;
    let a1 = checked_hash(&storage, &txid).await;
    let (base, _) = stub(Script {
        pages: [page_of(&[(1, reorg_of_a1(&a1, &txid))], false)].into(),
        ..Script::default()
    })
    .await;
    follow(&base, &storage, &services).await;
    assert_eq!(
        word(&storage, &txid).await,
        ("mined".to_string(), Some(900), Some("recheck".to_string()))
    );

    services.set_header_for_height(header(900, &"ee".repeat(32), 7));
    let tip = json!({
        "v": 1, "kind": "tip", "height": 902, "hash": "b4".repeat(32),
        "time": 1_700_000_000u32,
        "header": eh(902, &"b4".repeat(32), &"b3".repeat(32), &"ef".repeat(32)),
    });
    let (base, _) = stub(Script {
        pages: [page_of(&[(2, tip)], false)].into(),
        ..Script::default()
    })
    .await;
    follow(&base, &storage, &services).await;
    assert_eq!(
        word(&storage, &txid).await,
        ("stale".to_string(), None, Some("reorg@900".to_string()))
    );
}

/// Ruling (5): a reorg whose new block at 900 still carries T's root (B1
/// holds the same transactions under another header) moves nothing: T is
/// `mined` at 900 with no re-ask, and the daemon's pass, holding the feed,
/// asks no proof and reads no ring.
#[tokio::test]
async fn a_reorg_whose_new_block_carries_the_root_moves_nothing_and_fetches_nothing() {
    let (storage, services, txid) = mined_at_900().await;
    let a1 = checked_hash(&storage, &txid).await;
    services.set_header_for_height(header(900, &txid, 7));
    let (base, _) = stub(Script {
        pages: [page_of(&[(1, reorg_of_a1(&a1, &txid))], false)].into(),
        ..Script::default()
    })
    .await;
    follow(&base, &storage, &services).await;
    assert_eq!(
        word(&storage, &txid).await,
        ("mined".to_string(), Some(900), None)
    );

    services.reset_history();
    let report = tick(&storage, &services, &clock(), &held()).await.unwrap();
    assert_eq!(report.asked, 0);
    assert_eq!(report.tracked, 0, "{}", report.summary());
    assert_eq!(report.moved_from, None);
    assert_eq!(services.call_count("get_merkle_path"), 0);
}

/// Step 4 (i), ruling 7d (c): T is `stale`; the pass reads its status at
/// the broadcaster once, the broadcaster knows it (seen), and nothing is
/// announced. T stays `stale` with `reorg@900` (its root moved, so it is
/// asked for a proof), now on the tier `seen`. The next pass of the same
/// episode reads no status again.
#[tokio::test]
async fn step_4_i_a_broadcaster_that_knows_the_stale_transaction_is_not_told_again() {
    let (storage, txid) = stale_at_900().await;
    let services = MockWalletServices::builder()
        .get_merkle_path_response(proof_answer(None))
        .broadcaster_statuses(vec![(
            "arcade".to_string(),
            bsv_wallet_toolbox::BroadcastStatus::Seen,
        )])
        .build();
    services.set_header_for_height(header(900, &"ee".repeat(32), 7));
    services.set_tip_header(header_with_hash(905, &"0a".repeat(32), 0));

    let report = tick(&storage, &services, &clock(), &held()).await.unwrap();
    println!("{}", report.summary());
    assert_eq!(report.stale_known, vec![txid.clone()]);
    assert!(report.stale_announced.is_empty());
    assert_eq!(services.call_count("get_broadcaster_statuses"), 1);
    assert_eq!(services.call_count("post_beef"), 0);
    assert_eq!(
        services.call_count("get_merkle_path"),
        1,
        "the re-ask of a moved root"
    );
    assert_eq!(
        word(&storage, &txid).await,
        ("stale".to_string(), None, Some("reorg@900".to_string()))
    );
    assert_eq!(
        crate::tracker_host::stored_tier(&storage, &txid)
            .await
            .unwrap(),
        Some("seen")
    );

    let later = FixedClock(clock().0 + 10_000);
    tick(&storage, &services, &later, &held()).await.unwrap();
    assert_eq!(services.call_count("get_broadcaster_statuses"), 1);
    assert_eq!(services.call_count("post_beef"), 0);
}

/// Step 4 (ii) and step 7, ruling 7d (c): no broadcaster knows the `stale`
/// T, so it is announced once (its stored BEEF); a later pass of the same
/// episode announces nothing. Step 7: T's new path at 902 checks, T is
/// `mined` at 902 and the episode's mark is gone.
#[tokio::test]
async fn step_4_ii_a_stale_transaction_no_broadcaster_knows_is_announced_once() {
    let (storage, txid) = stale_at_900().await;
    let services = MockWalletServices::builder()
        .get_merkle_path_response(proof_answer(None))
        .broadcaster_statuses(vec![(
            "arcade".to_string(),
            bsv_wallet_toolbox::BroadcastStatus::Unknown,
        )])
        .build();
    services.set_header_for_height(header(900, &"ee".repeat(32), 7));
    services.set_tip_header(header_with_hash(905, &"0a".repeat(32), 0));

    let report = tick(&storage, &services, &clock(), &held()).await.unwrap();
    println!("{}", report.summary());
    assert_eq!(report.stale_announced, vec![txid.clone()]);
    assert_eq!(services.call_count("post_beef"), 1);
    assert_eq!(
        follow_up_of(&storage, &txid).await.as_deref(),
        Some("announced")
    );
    assert_eq!(word(&storage, &txid).await.0, "stale");

    let later = FixedClock(clock().0 + 10_000);
    tick(&storage, &services, &later, &held()).await.unwrap();
    assert_eq!(services.call_count("get_broadcaster_statuses"), 1);
    assert_eq!(
        services.call_count("post_beef"),
        1,
        "never a second announce"
    );

    let services = MockWalletServices::builder()
        .get_merkle_path_response(proof_answer(Some(path_of(&txid, 902))))
        .build();
    services.set_header_for_height(header(900, &"ee".repeat(32), 7));
    services.set_header_for_height(header(902, &txid, 0));
    services.set_tip_header(header_with_hash(905, &"0a".repeat(32), 0));
    let later = FixedClock(clock().0 + 100_000);
    let report = tick(&storage, &services, &later, &held()).await.unwrap();
    assert_eq!(report.mined, vec![txid.clone()]);
    assert_eq!(
        word(&storage, &txid).await,
        ("mined".to_string(), Some(902), None)
    );
    assert_eq!(follow_up_of(&storage, &txid).await, None);
}

/// One rule, not two (ruling 7d (c) with toolbox 0.10.0's
/// `reask_lost_proof`): in the daemon the toolbox's monitor runs its reorg
/// and review tasks, which demote the stored proof and re-ask the courier
/// once. The served pass then makes no status read and no announce for the
/// `stale` episode, on this pass or the next: one status read and at most
/// one announce across both. The episode's re-ask for a proof still runs.
#[tokio::test]
async fn in_the_daemon_a_stale_episode_is_followed_up_once_by_the_monitor_not_twice() {
    use bsv_wallet_toolbox::monitor::reorg_ops::{reprove_anchor, LostProofReask, ReproveOutcome};
    use bsv_wallet_toolbox::storage::ProvenTxAnchor;

    let (storage, txid) = stale_at_900().await;
    let services = MockWalletServices::builder()
        .get_merkle_path_response(proof_answer(None))
        .broadcaster_statuses(vec![(
            "arcade".to_string(),
            bsv_wallet_toolbox::BroadcastStatus::Seen,
        )])
        .build();
    services.set_header_for_height(header(900, &"ee".repeat(32), 7));
    services.set_tip_header(header_with_hash(905, &"0a".repeat(32), 0));

    // The toolbox's stored proof of T on A1 (the fixture's bytes do not
    // hash to T, so the pass's ingest stored none), as its monitor holds it.
    let anchor = ProvenTxAnchor {
        txid: txid.clone(),
        height: 900,
        block_hash: checked_hash(&storage, &txid).await,
        merkle_root: txid.clone(),
    };
    sqlx::query(
        "INSERT INTO proven_txs (txid, height, idx, block_hash, merkle_root, merkle_path, raw_tx) \
         VALUES (?, 900, 0, ?, ?, X'00', X'01000000')",
    )
    .bind(&anchor.txid)
    .bind(&anchor.block_hash)
    .bind(&anchor.merkle_root)
    .execute(storage.writer())
    .await
    .unwrap();

    // The monitor's path: the header read finds the root gone, the proof is
    // demoted, the courier asked once.
    assert_eq!(
        reprove_anchor(&storage, &services, &anchor).await,
        ReproveOutcome::Demoted {
            reask: LostProofReask::Known
        }
    );
    assert_eq!(services.call_count("get_broadcaster_statuses"), 1);

    // The served pass, as the daemon runs it: the episode is the monitor's.
    let daemon = TickOptions {
        monitor_runs: true,
        ..held()
    };
    let report = tick(&storage, &services, &clock(), &daemon).await.unwrap();
    assert_eq!(report.stale_to_monitor, vec![txid.clone()]);
    assert!(report.stale_known.is_empty() && report.stale_announced.is_empty());
    assert_eq!(
        follow_up_of(&storage, &txid).await.as_deref(),
        Some("monitor")
    );
    let later = FixedClock(clock().0 + 10_000);
    let report = tick(&storage, &services, &later, &daemon).await.unwrap();
    assert!(report.stale_to_monitor.is_empty(), "marked once an episode");
    assert_eq!(
        services.call_count("get_broadcaster_statuses"),
        1,
        "one status read across both"
    );
    assert_eq!(services.call_count("post_beef"), 0, "at most one announce");
    assert_eq!(word(&storage, &txid).await.0, "stale");
}

/// Step 8's evidence through the announce, ruling 7d (c): no broadcaster
/// knows the `stale` T, and the one announce is refused by the node as a
/// double spend naming X. The refusal writes the node's verdict: T is
/// `conflicted([X])`, and the word stands on the next load.
#[tokio::test]
async fn a_refused_announce_of_a_stale_transaction_writes_the_nodes_verdict() {
    let (storage, txid) = stale_at_900().await;
    let x = "d9".repeat(32);
    let services = MockWalletServices::builder()
        .get_merkle_path_response(proof_answer(None))
        .broadcaster_statuses(vec![(
            "arcade".to_string(),
            bsv_wallet_toolbox::BroadcastStatus::Unknown,
        )])
        .post_beef_double_spend(&txid, &x)
        .build();
    services.set_header_for_height(header(900, &"ee".repeat(32), 7));
    services.set_tip_header(header_with_hash(905, &"0a".repeat(32), 0));

    let report = tick(&storage, &services, &clock(), &held()).await.unwrap();
    println!("{}", report.summary());
    assert_eq!(report.verdicts, vec![format!("{txid}: conflicted")]);
    assert_eq!(services.call_count("post_beef"), 1);
    assert_eq!(word(&storage, &txid).await.0, "conflicted");

    let later = FixedClock(clock().0 + 10_000);
    tick(&storage, &services, &later, &held()).await.unwrap();
    assert_eq!(word(&storage, &txid).await.0, "conflicted");
    assert_eq!(services.call_count("post_beef"), 1);
}
