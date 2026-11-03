//! The switch over the served router (#414, the design's L5b exit tests):
//! a disconnect's deny over a real socket (W4 (a)), the cap at its edges
//! (W14), a power granted, listed, revoked and refused after, the held
//! call's answers (the owner's routes, another process's write, the
//! shutdown), the seed, the host routes, the open doors and level 0 (W15
//! (i)) and the admin-reserved names.

use std::net::SocketAddr;
use std::time::Duration;

use super::*;
use crate::store::origin_grants::{self as grants, Answer, PendingRow, Power, Reply};
use crate::test_support::test_wallet;
use bsv_sdk::primitives::PrivateKey;
use tokio::io::AsyncWriteExt;

const PAGE: &str = "http://page.example";
const OTHER: &str = "http://other.example";

async fn wallet() -> (tempfile::TempDir, WalletState) {
    let dir = tempfile::TempDir::new().unwrap();
    let file = dir.path().join("switch.db");
    let storage = StorageSqlx::open(file.to_str().unwrap()).await.unwrap();
    let key = PrivateKey::from_hex(&"03".repeat(32)).unwrap();
    (dir, test_wallet(storage, "switch", key).await)
}

/// The router served by `axum::serve` on a bound `TcpListener` at
/// 127.0.0.1:0, as `run` serves it: its address and the config's held
/// calls. The seed lists `seeded`.
async fn served(wallet: &WalletState, seeded: &[&str]) -> (SocketAddr, powers::Held) {
    let config = ServerConfig::admitting(seeded.iter().copied()).unwrap();
    let held = config.held.clone();
    let app = make_router(wallet.clone(), config);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await });
    (addr, held)
}

/// A page's call on a connection of its own: the status, the CORS echo
/// and the body.
async fn page_call(
    addr: SocketAddr,
    origin: &str,
    path: &str,
    body: serde_json::Value,
) -> (u16, Option<String>, Option<String>, serde_json::Value) {
    let response = reqwest::Client::new()
        .post(format!("http://{addr}{path}"))
        .header("origin", origin)
        .json(&body)
        .send()
        .await
        .unwrap();
    let status = response.status().as_u16();
    let header = |name: &str| {
        response
            .headers()
            .get(name)
            .map(|v| v.to_str().unwrap().to_string())
    };
    let (echo, retry) = (header("access-control-allow-origin"), header("retry-after"));
    let body = response.json().await.unwrap_or(serde_json::Value::Null);
    (status, echo, retry, body)
}

fn basket(name: &str) -> serde_json::Value {
    json!({"basket": name})
}

fn signature(protocol: &str, level: u8) -> serde_json::Value {
    json!({"protocolID": [level, protocol], "keyID": "1", "counterparty": "self",
           "data": [1, 2, 3]})
}

async fn open_rows(wallet: &WalletState) -> Vec<PendingRow> {
    grants::listing(wallet.storage(), false)
        .await
        .unwrap()
        .pending
}

/// Wait until `rows` open rows exist (a held call has written its row).
async fn until_open(wallet: &WalletState, rows: usize) -> Vec<PendingRow> {
    for _ in 0..500 {
        let open = open_rows(wallet).await;
        if open.len() == rows {
            return open;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    panic!(
        "{rows} open rows never appeared: {:?}",
        open_rows(wallet).await
    );
}

async fn answer(wallet: &WalletState, id: i64, answer: Answer) {
    let reply = Reply {
        answer,
        expiry: 0,
        amount: None,
    };
    let now = owner_door::now().unwrap();
    grants::answer(wallet.storage(), id, &reply, now)
        .await
        .unwrap();
}

/// The design's L5b exit test (W4 (a)): two held calls of one originator
/// on two powers, each on its own connection; one connection closed by the
/// client; both rows gone and the other call answered 400 `Permission
/// denied.`; a held call of another originator untouched; a call of the
/// same originator opened after the close keeps its new row, held.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_disconnect_denies_its_originators_held_calls_over_a_real_socket() {
    let (_dir, wallet) = wallet().await;
    let (addr, held) = served(&wallet, &[]).await;

    // The call whose caller leaves: a raw HTTP/1.1 request, written whole.
    let mut leaving = tokio::net::TcpStream::connect(addr).await.unwrap();
    let body = basket("one").to_string();
    let request = format!(
        "POST /listOutputs HTTP/1.1\r\nHost: {addr}\r\nOrigin: {PAGE}\r\n\
         Content-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}",
        body.len()
    );
    leaving.write_all(request.as_bytes()).await.unwrap();
    until_open(&wallet, 1).await;
    let staying = tokio::spawn(page_call(
        addr,
        PAGE,
        "/createSignature",
        signature("two two", 2),
    ));
    until_open(&wallet, 2).await;
    let other = tokio::spawn(page_call(addr, OTHER, "/listOutputs", basket("one")));
    let open = until_open(&wallet, 3).await;
    let other_id = open
        .iter()
        .find(|r| r.originator == "other.example")
        .unwrap()
        .pending_id;
    assert_eq!(held.rows_of("page.example").len(), 2);

    // The client closes its connection fully, after the request is written.
    leaving.shutdown().await.unwrap();
    drop(leaving);

    let (status, echo, _, body) = staying.await.unwrap();
    assert_eq!(
        (status, body["code"].as_str(), body["message"].as_str()),
        (
            400,
            Some("ERR_PERMISSION_DENIED"),
            Some("Permission denied.")
        )
    );
    assert_eq!(echo.as_deref(), Some(PAGE), "the page reads its denial");
    let open = open_rows(&wallet).await;
    assert_eq!(
        open.iter().map(|r| r.pending_id).collect::<Vec<_>>(),
        [other_id],
        "both of page.example's rows deleted, other.example's kept"
    );
    let all = grants::listing(wallet.storage(), true).await.unwrap();
    assert!(
        all.pending.iter().all(|r| r.originator != "page.example"),
        "no row kept, answered or open: {:?}",
        all.pending
    );
    assert!(all.grants.is_empty(), "a disconnect writes no grant");
    assert!(!other.is_finished(), "other.example's call is still held");

    // A call of the same page after the close opens a new prompt and is
    // held: the snapshot bounded the withdrawal.
    let again = tokio::spawn(page_call(addr, PAGE, "/listOutputs", basket("one")));
    let open = until_open(&wallet, 2).await;
    let again_id = open
        .iter()
        .find(|r| r.originator == "page.example")
        .unwrap()
        .pending_id;
    tokio::time::sleep(Duration::from_millis(600)).await;
    assert_eq!(open_rows(&wallet).await.len(), 2, "the new row stays open");
    assert!(!again.is_finished());

    answer(&wallet, other_id, Answer::Deny).await;
    answer(&wallet, again_id, Answer::Grant).await;
    assert_eq!(other.await.unwrap().0, 400);
    let (status, _, _, body) = again.await.unwrap();
    assert_eq!(status, 200, "{body}");
    assert_eq!(held.counts("page.example"), (0, 0));
}

/// The cap (W14) at its edges: an originator's third held call and the
/// wallet's fifth are answered 429 at once with `Retry-After: 5` and the
/// echo, writing no row; an answer frees its place.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_cap_refuses_an_originators_third_held_call_and_the_fifth_in_all() {
    let (_dir, wallet) = wallet().await;
    let (addr, held) = served(&wallet, &[]).await;
    let first = tokio::spawn(page_call(addr, PAGE, "/listOutputs", basket("one")));
    until_open(&wallet, 1).await;
    // A joined call holds a connection: it counts.
    let joined = tokio::spawn(page_call(addr, PAGE, "/listOutputs", basket("one")));
    for _ in 0..500 {
        if held.counts("page.example").0 == 2 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert_eq!(held.counts("page.example"), (2, 2));

    let (status, echo, retry, body) = page_call(addr, PAGE, "/listOutputs", basket("two")).await;
    assert_eq!(
        (status, body["code"].as_str(), body["message"].as_str()),
        (
            429,
            Some("ERR_PERMISSION_PENDING_LIMIT"),
            Some(powers::PENDING_LIMIT)
        )
    );
    assert_eq!((echo.as_deref(), retry.as_deref()), (Some(PAGE), Some("5")));
    assert_eq!(
        open_rows(&wallet).await.len(),
        1,
        "the refused call wrote no row"
    );

    let second = tokio::spawn(page_call(addr, OTHER, "/listOutputs", basket("one")));
    let third = tokio::spawn(page_call(
        addr,
        "http://third.example",
        "/listOutputs",
        basket("one"),
    ));
    let open = until_open(&wallet, 3).await;
    assert_eq!(held.counts("other.example"), (1, 4));
    let (status, _, _, _) =
        page_call(addr, "http://fifth.example", "/listOutputs", basket("one")).await;
    assert_eq!(status, 429, "the fifth held call in all");

    // The owner's answer frees the page's two places at once.
    let page_id = open
        .iter()
        .find(|r| r.originator == "page.example")
        .unwrap()
        .pending_id;
    answer(&wallet, page_id, Answer::Deny).await;
    assert_eq!(first.await.unwrap().0, 400);
    assert_eq!(joined.await.unwrap().0, 400);
    let fifth = tokio::spawn(page_call(
        addr,
        "http://fifth.example",
        "/listOutputs",
        basket("one"),
    ));
    let open = until_open(&wallet, 3).await;
    for row in open {
        answer(&wallet, row.pending_id, Answer::Deny).await;
    }
    for call in [second, third, fifth] {
        assert_eq!(call.await.unwrap().0, 400);
    }
    assert_eq!(held.counts("page.example"), (0, 0));
}

/// The issue's exit test: a power granted from the owner's route while
/// its call is held, listed, revoked, and refused after at once with no
/// pending row; the page's other powers untouched.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_power_granted_listed_revoked_and_refused_after() {
    let (_dir, wallet) = wallet().await;
    let (addr, _) = served(&wallet, &[]).await;
    let owner = reqwest::Client::new();
    let held = tokio::spawn(page_call(addr, PAGE, "/listOutputs", basket("todo items")));
    let open = until_open(&wallet, 1).await;
    assert_eq!(
        (
            open[0].kind.as_str(),
            open[0].key.as_str(),
            open[0].usage.as_deref()
        ),
        ("basket", r#"["todo items"]"#, Some("listing"))
    );
    let answered = owner
        .post(format!("http://{addr}/origins/answer"))
        .json(&json!({"pendingId": open[0].pending_id, "answer": "grant"}))
        .send()
        .await
        .unwrap();
    assert_eq!(answered.status(), 200);
    let (status, echo, _, body) = held.await.unwrap();
    assert_eq!((status, echo.as_deref()), (200, Some(PAGE)), "{body}");
    assert!(body["outputs"].is_array());

    let listed: serde_json::Value = owner
        .get(format!("http://{addr}/origins"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(listed["grants"][0]["key"], r#"["todo items"]"#);
    assert_eq!(listed["grants"][0]["expiry"], 0);
    assert_eq!(listed["pending"][0]["answer"], "grant");
    let (status, ..) = page_call(addr, PAGE, "/listOutputs", basket("todo items")).await;
    assert_eq!(status, 200, "granted: answered at once");

    let revoked = owner
        .post(format!("http://{addr}/origins/revoke"))
        .json(&json!({"originator": "page.example", "kind": "basket", "key": r#"["todo items"]"#}))
        .send()
        .await
        .unwrap();
    assert_eq!(revoked.status(), 200);
    for origin in [PAGE, "http://page.example:8080"] {
        let (status, echo, _, body) =
            page_call(addr, origin, "/listOutputs", basket("todo items")).await;
        assert_eq!(
            (status, body["message"].as_str(), echo.as_deref()),
            (400, Some(origin_gate::DENIED), Some(origin))
        );
    }
    assert!(
        open_rows(&wallet).await.is_empty(),
        "a revoked power writes no row"
    );
    // Another power of the page is asked as before.
    let other = tokio::spawn(page_call(addr, PAGE, "/listOutputs", basket("other")));
    let open = until_open(&wallet, 1).await;
    answer(&wallet, open[0].pending_id, Answer::Deny).await;
    let (status, _, _, body) = other.await.unwrap();
    assert_eq!(
        (status, body["message"].as_str()),
        (400, Some("Permission denied."))
    );
}

/// A held call reads its row every 250 ms: an answer another process
/// writes (the CLI, the inspector) reaches it with no wake. An expired
/// grant is held as a renewal.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_expired_grant_is_held_as_a_renewal_and_another_process_answers_it() {
    let (_dir, wallet) = wallet().await;
    let (addr, _) = served(&wallet, &[]).await;
    let power = Power::protocol(false, 2, "renewed gate", None).unwrap();
    let granting = grants::Granting {
        originator: "page.example",
        power: &power,
        amount: None,
        expiry: 1,
    };
    grants::grant(wallet.storage(), &granting, 1).await.unwrap();
    let call = tokio::spawn(page_call(
        addr,
        PAGE,
        "/createSignature",
        signature("renewed gate", 2),
    ));
    let open = until_open(&wallet, 1).await;
    assert!(open[0].renewal, "an expired grant is a renewal");
    assert_eq!(open[0].usage.as_deref(), Some("signing"));
    answer(&wallet, open[0].pending_id, Answer::Grant).await;
    let (status, _, _, body) = call.await.unwrap();
    assert_eq!(status, 200, "{body}");
    assert!(body["signature"].is_array());
}

/// The shutdown (W13): a held call answers 503 `WALLET_BRIDGE_UNAVAILABLE`
/// in the port's carrier and its row is withdrawn; a call that would be
/// held after it is answered so at once.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_held_call_answers_the_shutdown_and_withdraws_its_row() {
    let (_dir, wallet) = wallet().await;
    let (addr, held) = served(&wallet, &[]).await;
    let call = tokio::spawn(page_call(addr, PAGE, "/listOutputs", basket("one")));
    until_open(&wallet, 1).await;
    held.shut_down();
    let (status, echo, _, body) = call.await.unwrap();
    assert_eq!(
        (status, body["code"].as_str(), body["message"].as_str()),
        (
            503,
            Some("WALLET_BRIDGE_UNAVAILABLE"),
            Some(powers::SHUTTING_DOWN)
        )
    );
    assert_eq!(echo.as_deref(), Some(PAGE));
    assert!(grants::listing(wallet.storage(), true)
        .await
        .unwrap()
        .pending
        .is_empty());
    let (status, ..) = page_call(addr, PAGE, "/listOutputs", basket("two")).await;
    assert_eq!(status, 503);
    assert!(open_rows(&wallet).await.is_empty());
}

/// The seed (question 6): a listed originator is admitted at every door
/// and host route with no row, the admin-reserved names included; a page
/// it does not list is refused at a host route (no kind names one, the
/// pattern `/tx/{txid}` included) and asked per power at a door; the
/// owner, with no `Origin` from loopback, reads the host routes.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_seed_admits_every_door_and_host_route_and_a_page_reads_no_host_route() {
    let (_dir, wallet) = wallet().await;
    let (addr, _) = served(&wallet, &["seeded.example"]).await;
    let (status, ..) = page_call(
        addr,
        "http://seeded.example",
        "/listOutputs",
        basket("default"),
    )
    .await;
    assert_eq!(
        status, 200,
        "the seed is not refused an admin-reserved basket"
    );
    let get = |origin: Option<&str>, path: &str| {
        let mut request = reqwest::Client::new().get(format!("http://{addr}{path}"));
        if let Some(origin) = origin {
            request = request.header("origin", origin);
        }
        request.send()
    };
    let txid = "ab".repeat(32);
    for path in ["/permission-audit", "/chain-feed", &format!("/tx/{txid}")] {
        let seeded = get(Some("http://seeded.example:8080"), path).await.unwrap();
        assert_ne!(seeded.status(), 400, "{path}: the seed reads a host route");
        let owner = get(None, path).await.unwrap();
        assert_ne!(owner.status(), 400, "{path}: the owner reads a host route");
        let page = get(Some(PAGE), path).await.unwrap();
        assert_eq!(page.status(), 400, "{path}");
        assert_eq!(
            page.headers().get("access-control-allow-origin").unwrap(),
            PAGE
        );
        let body: serde_json::Value = page.json().await.unwrap();
        assert_eq!(
            (body["code"].as_str(), body["message"].as_str()),
            (Some("ERR_PERMISSION_DENIED"), Some(origin_gate::DENIED)),
            "{path}"
        );
    }
    assert!(
        open_rows(&wallet).await.is_empty(),
        "nothing asked, nothing written"
    );
}

/// W15 (i): the open doors and a level-0 protocol answer a page with a
/// parsed `Origin` and an agreeing originator, with no row; an
/// admin-reserved name is refused at once in the reference's words; a
/// disagreeing originator is refused before any row is read or written.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_open_doors_and_level_zero_answer_a_page_with_no_row() {
    let (_dir, wallet) = wallet().await;
    let (addr, _) = served(&wallet, &[]).await;
    for path in [
        "/isAuthenticated",
        "/getVersion",
        "/getNetwork",
        "/waitForAuthentication",
    ] {
        let response = reqwest::Client::new()
            .get(format!("http://{addr}{path}"))
            .header("origin", PAGE)
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), 200, "{path}");
        assert_eq!(
            response
                .headers()
                .get("access-control-allow-origin")
                .unwrap(),
            PAGE
        );
    }
    let (status, echo, _, body) =
        page_call(addr, PAGE, "/createSignature", signature("open gate", 0)).await;
    assert_eq!((status, echo.as_deref()), (200, Some(PAGE)), "{body}");
    let (status, _, _, body) = page_call(addr, PAGE, "/listOutputs", basket(" Default")).await;
    assert_eq!(
        (status, body["code"].as_str(), body["message"].as_str()),
        (
            400,
            Some("ERR_PERMISSION_DENIED"),
            Some("Basket \u{201c}default\u{201d} is admin-only.")
        )
    );
    let (status, _, _, body) =
        page_call(addr, PAGE, "/createSignature", signature("admin gate", 2)).await;
    assert_eq!(
        (status, body["message"].as_str()),
        (
            400,
            Some("Protocol \u{201c}admin gate\u{201d} is admin-only.")
        )
    );
    let disagreeing = reqwest::Client::new()
        .post(format!("http://{addr}/listOutputs"))
        .header("origin", "http://evil.example")
        .header("originator", "page.example")
        .json(&basket("one"))
        .send()
        .await
        .unwrap();
    assert_eq!(disagreeing.status(), 400);
    assert!(
        grants::listing(wallet.storage(), true)
            .await
            .unwrap()
            .pending
            .is_empty(),
        "no row for an open door, level 0, an admin name or a disagreement"
    );
}
