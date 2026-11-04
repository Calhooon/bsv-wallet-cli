//! The switch over the served router (#414, the design's L5b exit tests):
//! a disconnect's deny over a real socket (W4 (a)), the cap at its edges
//! (W14), a power granted, listed, revoked and refused after, the held
//! call's answers (the owner's routes, another process's write, the
//! shutdown), the seed, the host routes, the open doors and level 0 (W15
//! (i)) and the admin-reserved names.

use std::net::SocketAddr;
use std::time::Duration;

use super::owner_door_tests::wallet;
use super::*;
use crate::store::origin_grants::{self as grants, Answer, PendingRow, Power, Reply};
use crate::test_support::serve_router;
use tokio::io::AsyncWriteExt;

pub(super) const PAGE: &str = "http://page.example";
pub(super) const OTHER: &str = "http://other.example";

/// A call's answer as a page reads it: the status, the CORS echo, the
/// `Retry-After` and the body.
pub(super) type Read = (u16, Option<String>, Option<String>, serde_json::Value);

/// The router served by `axum::serve` on a bound `TcpListener` at
/// 127.0.0.1:0 (`serve_router`), as `run` serves it: its address and the
/// config's held calls. The seed lists `seeded`.
async fn served(wallet: &WalletState, seeded: &[&str]) -> (SocketAddr, powers::Held) {
    let config = ServerConfig::admitting(seeded.iter().copied()).unwrap();
    let held = config.held.clone();
    (serve_at(wallet, config).await, held)
}

/// `config`'s router served on a loopback listener: its address.
pub(super) async fn serve_at(wallet: &WalletState, config: ServerConfig) -> SocketAddr {
    let (url, _task) = serve_router(make_router(wallet.clone(), config)).await;
    url.trim_start_matches("http://").parse().unwrap()
}

/// The router's URL for `path`.
pub(super) fn at(addr: SocketAddr, path: &str) -> String {
    format!("http://{addr}{path}")
}

/// A response as a page reads it.
pub(super) async fn read(response: reqwest::Response) -> Read {
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

/// A page's POST on a connection of its own.
pub(super) async fn page_call(
    addr: SocketAddr,
    origin: &str,
    path: &str,
    body: serde_json::Value,
) -> Read {
    let request = reqwest::Client::new().post(at(addr, path));
    read(
        request
            .header("origin", origin)
            .json(&body)
            .send()
            .await
            .unwrap(),
    )
    .await
}

/// A GET from `origin`, or from the owner's side with none.
pub(super) async fn get_from(addr: SocketAddr, origin: Option<&str>, path: &str) -> Read {
    let mut request = reqwest::Client::new().get(at(addr, path));
    if let Some(origin) = origin {
        request = request.header("origin", origin);
    }
    read(request.send().await.unwrap()).await
}

/// One of the owner's routes, from the owner's side: its status.
async fn owner_post(addr: SocketAddr, path: &str, body: serde_json::Value) -> u16 {
    let request = reqwest::Client::new().post(at(addr, path)).json(&body);
    read(request.send().await.unwrap()).await.0
}

/// The status, code and message of a refusal.
fn refusal(read: &Read) -> (u16, Option<&str>, Option<&str>) {
    (read.0, read.3["code"].as_str(), read.3["message"].as_str())
}

fn basket(name: &str) -> serde_json::Value {
    json!({"basket": name})
}

pub(super) fn signature(protocol: &str, level: u8) -> serde_json::Value {
    json!({"protocolID": [level, protocol], "keyID": "1", "counterparty": "self",
           "data": [1, 2, 3]})
}

async fn open_rows(wallet: &WalletState) -> Vec<PendingRow> {
    grants::listing(wallet.storage(), false)
        .await
        .unwrap()
        .pending
}

/// Wait until `rows` open rows exist (the held calls have written them).
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

/// The open row of `originator`'s key among `rows`.
fn row_of(rows: &[PendingRow], originator: &str) -> i64 {
    rows.iter()
        .find(|r| r.originator == originator)
        .unwrap()
        .pending_id
}

/// The owner's answer through the store, as the CLI writes it.
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

/// A page's listOutputs written whole on a raw HTTP/1.1 connection, which
/// the test closes.
async fn raw_listing(addr: SocketAddr, origin: &str, name: &str) -> tokio::net::TcpStream {
    let mut stream = tokio::net::TcpStream::connect(addr).await.unwrap();
    let body = basket(name).to_string();
    let request = format!(
        "POST /listOutputs HTTP/1.1\r\nHost: {addr}\r\nOrigin: {origin}\r\n\
         Content-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}",
        body.len()
    );
    stream.write_all(request.as_bytes()).await.unwrap();
    stream
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
    let mut leaving = raw_listing(addr, PAGE, "one").await;
    until_open(&wallet, 1).await;
    let staying = tokio::spawn(page_call(
        addr,
        PAGE,
        "/createSignature",
        signature("two two", 2),
    ));
    until_open(&wallet, 2).await;
    let other = tokio::spawn(page_call(addr, OTHER, "/listOutputs", basket("one")));
    let other_id = row_of(&until_open(&wallet, 3).await, "other.example");
    assert_eq!(held.rows_of("page.example").len(), 2);

    // The client closes its connection fully, after the request is written.
    leaving.shutdown().await.unwrap();
    drop(leaving);

    let staying = staying.await.unwrap();
    let denied = (
        400,
        Some("ERR_PERMISSION_DENIED"),
        Some("Permission denied."),
    );
    assert_eq!(refusal(&staying), denied);
    assert_eq!(
        staying.1.as_deref(),
        Some(PAGE),
        "the page reads its denial"
    );
    let open: Vec<i64> = open_rows(&wallet)
        .await
        .iter()
        .map(|r| r.pending_id)
        .collect();
    assert_eq!(
        open,
        [other_id],
        "page.example's rows deleted, other.example's kept"
    );
    let all = grants::listing(wallet.storage(), true).await.unwrap();
    assert!(
        all.pending.iter().all(|r| r.originator != "page.example"),
        "no row kept"
    );
    assert!(all.grants.is_empty(), "a disconnect writes no grant");
    assert!(!other.is_finished(), "other.example's call is still held");

    // A call of the same page after the close opens a new prompt and is
    // held: the snapshot bounded the withdrawal.
    let again = tokio::spawn(page_call(addr, PAGE, "/listOutputs", basket("one")));
    let again_id = row_of(&until_open(&wallet, 2).await, "page.example");
    tokio::time::sleep(Duration::from_millis(600)).await;
    assert_eq!(open_rows(&wallet).await.len(), 2, "the new row stays open");
    assert!(!again.is_finished());

    answer(&wallet, other_id, Answer::Deny).await;
    answer(&wallet, again_id, Answer::Grant).await;
    assert_eq!(other.await.unwrap().0, 400);
    assert_eq!(again.await.unwrap().0, 200);
    assert_eq!(held.counts("page.example"), (0, 0));
}

/// The window of #552: a page's connection closed after its row's commit
/// and before its guard is entered (a planted pause there) leaves no row
/// the owner can grant: the row is withdrawn, the owner's grant at its id
/// refused, no grant written.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_disconnect_during_the_opens_commit_leaves_no_grantable_row() {
    let (_dir, wallet) = wallet().await;
    let (addr, held) = served(&wallet, &[]).await;
    let pause = held.pause_after_open();
    let mut leaving = raw_listing(addr, PAGE, "one").await;
    pause.reached.notified().await;
    let id = row_of(&until_open(&wallet, 1).await, "page.example");
    assert!(
        held.rows_of("page.example").is_empty(),
        "the window: the row committed, no guard entered"
    );

    leaving.shutdown().await.unwrap();
    drop(leaving);
    // The server drops the call's future on the close; then the pause ends.
    tokio::time::sleep(Duration::from_millis(500)).await;
    pause.release.notify_one();
    for _ in 0..100 {
        if open_rows(&wallet).await.is_empty() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }

    let granting = json!({"pendingId": id, "answer": "grant"});
    let status = owner_post(addr, "/origins/answer", granting).await;
    assert_eq!(
        status, 400,
        "the owner's grant of the left page's row #{id}"
    );
    let all = grants::listing(wallet.storage(), true).await.unwrap();
    assert!(all.pending.is_empty(), "no row kept: {:?}", all.pending);
    assert!(all.grants.is_empty(), "no grant written: {:?}", all.grants);
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

    let third = page_call(addr, PAGE, "/listOutputs", basket("two")).await;
    let limit = (
        429,
        Some("ERR_PERMISSION_PENDING_LIMIT"),
        Some(powers::PENDING_LIMIT),
    );
    assert_eq!(refusal(&third), limit);
    assert_eq!(
        (third.1.as_deref(), third.2.as_deref()),
        (Some(PAGE), Some("5"))
    );
    assert_eq!(
        open_rows(&wallet).await.len(),
        1,
        "the refused call wrote no row"
    );

    let others = [OTHER, "http://third.example"]
        .map(|origin| tokio::spawn(page_call(addr, origin, "/listOutputs", basket("one"))));
    let open = until_open(&wallet, 3).await;
    assert_eq!(held.counts("other.example"), (1, 4));
    let fifth = page_call(addr, "http://fifth.example", "/listOutputs", basket("one")).await;
    assert_eq!(fifth.0, 429, "the fifth held call in all");

    // The owner's answer frees the page's two places at once.
    answer(&wallet, row_of(&open, "page.example"), Answer::Deny).await;
    assert_eq!(
        (first.await.unwrap().0, joined.await.unwrap().0),
        (400, 400)
    );
    let fifth = tokio::spawn(page_call(
        addr,
        "http://fifth.example",
        "/listOutputs",
        basket("one"),
    ));
    for row in until_open(&wallet, 3).await {
        answer(&wallet, row.pending_id, Answer::Deny).await;
    }
    for call in others.into_iter().chain([fifth]) {
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
    let todo = basket("todo items");
    let held = tokio::spawn(page_call(addr, PAGE, "/listOutputs", todo.clone()));
    let row = until_open(&wallet, 1).await.remove(0);
    let asked = (row.kind.as_str(), row.key.as_str(), row.usage.as_deref());
    assert_eq!(asked, ("basket", r#"["todo items"]"#, Some("listing")));
    let granting = json!({"pendingId": row.pending_id, "answer": "grant"});
    assert_eq!(owner_post(addr, "/origins/answer", granting).await, 200);
    let (status, echo, _, body) = held.await.unwrap();
    assert_eq!((status, echo.as_deref()), (200, Some(PAGE)), "{body}");

    let (_, _, _, listed) = get_from(addr, None, "/origins").await;
    assert_eq!(listed["grants"][0]["key"], r#"["todo items"]"#);
    assert_eq!(listed["grants"][0]["expiry"], 0);
    assert_eq!(listed["pending"][0]["answer"], "grant");
    assert_eq!(
        page_call(addr, PAGE, "/listOutputs", todo.clone()).await.0,
        200
    );

    let revoking =
        json!({"originator": "page.example", "kind": "basket", "key": r#"["todo items"]"#});
    assert_eq!(owner_post(addr, "/origins/revoke", revoking).await, 200);
    for origin in [PAGE, "http://page.example:8080"] {
        let refused = page_call(addr, origin, "/listOutputs", todo.clone()).await;
        let denied = (
            400,
            Some("ERR_PERMISSION_DENIED"),
            Some(origin_gate::DENIED),
        );
        assert_eq!(
            (refusal(&refused), refused.1.as_deref()),
            (denied, Some(origin))
        );
    }
    assert!(
        open_rows(&wallet).await.is_empty(),
        "a revoked power writes no row"
    );
    // Another power of the page is asked as before.
    let other = tokio::spawn(page_call(addr, PAGE, "/listOutputs", basket("other")));
    answer(
        &wallet,
        until_open(&wallet, 1).await[0].pending_id,
        Answer::Deny,
    )
    .await;
    let other = other.await.unwrap();
    assert_eq!(
        (other.0, other.3["message"].as_str()),
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
    let body = signature("renewed gate", 2);
    let call = tokio::spawn(page_call(addr, PAGE, "/createSignature", body));
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
    let answered = call.await.unwrap();
    let shut = (
        503,
        Some("WALLET_BRIDGE_UNAVAILABLE"),
        Some(powers::SHUTTING_DOWN),
    );
    assert_eq!(refusal(&answered), shut);
    assert_eq!(answered.1.as_deref(), Some(PAGE));
    let all = grants::listing(wallet.storage(), true).await.unwrap();
    assert!(all.pending.is_empty(), "the held call's row is withdrawn");
    assert_eq!(
        page_call(addr, PAGE, "/listOutputs", basket("two")).await.0,
        503
    );
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
    let seeded = "http://seeded.example";
    let listing = page_call(addr, seeded, "/listOutputs", basket("default")).await;
    assert_eq!(
        listing.0, 200,
        "the seed is not refused an admin-reserved basket"
    );
    let tx = format!("/tx/{}", "ab".repeat(32));
    for path in ["/permission-audit", "/chain-feed", tx.as_str()] {
        let by_seed = get_from(addr, Some("http://seeded.example:8080"), path).await;
        assert_ne!(by_seed.0, 400, "{path}: the seed reads a host route");
        assert_ne!(
            get_from(addr, None, path).await.0,
            400,
            "{path}: the owner reads it"
        );
        let page = get_from(addr, Some(PAGE), path).await;
        let denied = (
            400,
            Some("ERR_PERMISSION_DENIED"),
            Some(origin_gate::DENIED),
        );
        assert_eq!(
            (refusal(&page), page.1.as_deref()),
            (denied, Some(PAGE)),
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
        let open = get_from(addr, Some(PAGE), path).await;
        assert_eq!((open.0, open.1.as_deref()), (200, Some(PAGE)), "{path}");
    }
    let level_zero = page_call(addr, PAGE, "/createSignature", signature("open gate", 0)).await;
    assert_eq!((level_zero.0, level_zero.1.as_deref()), (200, Some(PAGE)));
    let refused = page_call(addr, PAGE, "/listOutputs", basket(" Default")).await;
    let words = "Basket \u{201c}default\u{201d} is admin-only.";
    assert_eq!(
        refusal(&refused),
        (400, Some("ERR_PERMISSION_DENIED"), Some(words))
    );
    let protocol = page_call(addr, PAGE, "/createSignature", signature("admin gate", 2)).await;
    let words = "Protocol \u{201c}admin gate\u{201d} is admin-only.";
    assert_eq!(
        (protocol.0, protocol.3["message"].as_str()),
        (400, Some(words))
    );
    let disagreeing = reqwest::Client::new()
        .post(at(addr, "/listOutputs"))
        .header("origin", "http://evil.example")
        .header("originator", "page.example")
        .json(&basket("one"));
    assert_eq!(read(disagreeing.send().await.unwrap()).await.0, 400);
    let all = grants::listing(wallet.storage(), true).await.unwrap();
    assert!(
        all.pending.is_empty(),
        "no row: open doors, level 0, admin names, disagreements"
    );
}
