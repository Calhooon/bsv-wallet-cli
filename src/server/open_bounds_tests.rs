//! The open doors' bound over the served router (#541): the 429 a page
//! reads past each class's bound (its code, the door and the window named,
//! `Retry-After` exposed through CORS), the window's reset, per-origin
//! isolation, nothing written to the store (no grant, no pending row, and
//! no probe row: only an answered call is counted, #539), and the owner's
//! routes, the host routes' owner and the seed outside the bound.

use std::net::SocketAddr;
use std::time::Duration;

use serde_json::json;

use crate::server::owner_door_tests::wallet;
use crate::server::powers_tests::{at, get_from, page_call, read, serve_at, signature};
use crate::server::powers_tests::{Read, OTHER, PAGE};
use crate::server::{probes, ServerConfig, WalletState};
use crate::store::origin_grants as grants;

/// A wallet served with the seed `seeded`: its directory, the wallet, the
/// router's address and its probe queue.
async fn served(seeded: &[&str]) -> (tempfile::TempDir, WalletState, SocketAddr, probes::Probes) {
    let (dir, wallet) = wallet().await;
    let config = ServerConfig::admitting(seeded.iter().copied()).unwrap();
    let probes = config.probes.clone();
    (dir, wallet.clone(), serve_at(&wallet, config).await, probes)
}

/// The 429 a page reads: the code, the door and the window in the
/// message, `Retry-After` and the echo.
fn assert_limited(read: &Read, door: &str, window: &str, retry: &str) {
    let (status, echo, retry_after, body) = read;
    assert_eq!(*status, 429, "{body}");
    assert_eq!(body["code"], "ERR_RATE_LIMITED");
    let message = body["message"].as_str().unwrap();
    let tail = format!("retry after {retry} s.");
    assert!(
        message.contains(door) && message.contains(window) && message.ends_with(&tail),
        "{message}"
    );
    assert_eq!(retry_after.as_deref(), Some(retry));
    assert_eq!(echo.as_deref(), Some(PAGE));
}

/// No grant and no pending row, and the probe record counts the answered
/// calls alone: `answered` per originator's hostname.
async fn no_rows(wallet: &WalletState, probes: &probes::Probes, answered: &[(&str, i64)]) {
    probes.settled().await;
    let all = grants::listing(wallet.storage(), true).await.unwrap();
    assert!(
        all.pending.is_empty() && all.grants.is_empty(),
        "a bounded call writes no row"
    );
    for (host, n) in answered {
        let rows = all.probes.iter().filter(|r| r.originator == *host);
        let counted: i64 = rows.map(|r| r.count).sum();
        assert_eq!(counted, *n, "{host}: only the answered calls are counted");
    }
}

/// The chain doors at the edge, served: ten calls at once (the four doors
/// one bound) are answered, the eleventh (`getHeight`) is 429 with
/// `Retry-After: 1`, which CORS exposes to the page; every port of the host
/// is the one originator; another origin is untouched; after the window a
/// call is answered again; no row is written.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_page_past_the_chain_burst_reads_429_then_200_after_the_window() {
    let (_dir, wallet, addr, probes) = served(&[]).await;
    for n in 0..10 {
        let door = ["/getNetwork", "/getVersion"][n % 2];
        let answered = get_from(addr, Some(PAGE), door).await;
        assert_eq!(
            (answered.0, answered.1.as_deref()),
            (200, Some(PAGE)),
            "{n}"
        );
    }
    let response = reqwest::Client::new()
        .get(at(addr, "/getHeight"))
        .header("origin", PAGE)
        .send()
        .await
        .unwrap();
    let exposed = response.headers().get("access-control-expose-headers");
    assert_eq!(exposed.map(|v| v.to_str().unwrap()), Some("retry-after"));
    let window = "the chain doors (getHeight, getHeaderForHeight, getNetwork, getVersion) \
                  answer 60 a minute per site, 10 at once";
    assert_limited(&read(response).await, "calls to /getHeight", window, "1");
    let port = get_from(addr, Some("http://page.example:8080"), "/getVersion").await;
    assert_eq!(port.0, 429, "no fresh bound by port");
    let other = get_from(addr, Some(OTHER), "/getVersion").await;
    assert_eq!((other.0, other.1.as_deref()), (200, Some(OTHER)));
    tokio::time::sleep(Duration::from_millis(1100)).await;
    assert_eq!(get_from(addr, Some(PAGE), "/getVersion").await.0, 200);
    let answered = [("page.example", 11), ("other.example", 1)];
    no_rows(&wallet, &probes, &answered).await;
}

/// The presence doors, served: each answers a page once a minute, the
/// second call 429 with `Retry-After: 60`; the other presence door answers
/// its own once; another origin is untouched.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_presence_door_answers_a_page_once() {
    let (_dir, wallet, addr, probes) = served(&[]).await;
    let opening = get_from(addr, Some(PAGE), "/isAuthenticated").await;
    assert_eq!(
        (opening.0, opening.3["authenticated"].as_bool()),
        (200, Some(true))
    );
    let window = "a presence door answers once a minute per site";
    let again = get_from(addr, Some(PAGE), "/isAuthenticated").await;
    assert_limited(&again, "calls to /isAuthenticated", window, "60");
    assert_eq!(
        get_from(addr, Some(PAGE), "/waitForAuthentication").await.0,
        200
    );
    let wait = get_from(addr, Some(PAGE), "/waitForAuthentication").await;
    assert_limited(&wait, "calls to /waitForAuthentication", window, "60");
    assert_eq!(get_from(addr, Some(OTHER), "/isAuthenticated").await.0, 200);
    let answered = [("other.example", 1), ("page.example", 2)];
    no_rows(&wallet, &probes, &answered).await;
}

/// A level-0 power, served: ten signatures at once are answered, the
/// eleventh (at another door, the class one bound) is 429; the chain doors
/// are untouched by it.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_level_zero_power_is_bounded_on_its_own() {
    let (_dir, wallet, addr, probes) = served(&[]).await;
    for n in 0..10 {
        let signed = page_call(addr, PAGE, "/createSignature", signature("open gate", 0)).await;
        assert_eq!(signed.0, 200, "{n}: {}", signed.3);
    }
    let hmac = json!({"protocolID": [0, "open gate"], "keyID": "1", "data": [1, 2, 3]});
    let limited = page_call(addr, PAGE, "/createHmac", hmac).await;
    let window = "the level-0 protocol powers answer 60 a minute per site, 10 at once";
    assert_limited(&limited, "level-0 calls to /createHmac", window, "1");
    assert_eq!(get_from(addr, Some(PAGE), "/getVersion").await.0, 200);
    let other = page_call(addr, OTHER, "/createSignature", signature("open gate", 0)).await;
    assert_eq!(other.0, 200);
    let answered = [("other.example", 1), ("page.example", 11)];
    no_rows(&wallet, &probes, &answered).await;
}

/// Outside the bound: the owner's routes and the host routes' owner (no
/// `Origin`, loopback), and a seed-listed originator (the allow-list is the
/// gate, ahead of the bound).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_owner_and_the_seed_are_outside_the_bound() {
    let seeded = "http://seeded.example";
    let (_dir, _wallet, addr, _) = served(&["seeded.example"]).await;
    for n in 0..30 {
        assert_eq!(get_from(addr, None, "/origins").await.0, 200, "owner {n}");
        assert_eq!(
            get_from(addr, None, "/permission-audit").await.0,
            200,
            "host {n}"
        );
        assert_eq!(
            get_from(addr, Some(seeded), "/getVersion").await.0,
            200,
            "seed {n}"
        );
        let present = get_from(addr, Some(seeded), "/isAuthenticated").await;
        assert_eq!(present.0, 200, "seed presence {n}");
        let signed = page_call(addr, seeded, "/createSignature", signature("open gate", 0)).await;
        assert_eq!(signed.0, 200, "seed level 0 {n}");
    }
}

/// A call by `method` from `origin` at `path`, no body: as a page reads it.
async fn by_method(addr: SocketAddr, method: &str, origin: &str, path: &str) -> Read {
    let method = reqwest::Method::from_bytes(method.as_bytes()).unwrap();
    let request = reqwest::Client::new().request(method, at(addr, path));
    read(request.header("origin", origin).send().await.unwrap()).await
}

/// The methods a page might call an open door by, `OPTIONS` aside (the CORS
/// layer answers it as a preflight before the gate).
const METHODS: [&str; 7] = ["GET", "HEAD", "POST", "PUT", "DELETE", "PATCH", "TRACE"];

/// The gate's map of the open doors' methods is the router's (#556): from
/// a seed-listed originator (admitted before the bound, so the router
/// answers every call), each open door answers `405` by exactly the
/// methods the gate's map leaves out.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_open_doors_methods_are_the_routers() {
    let seeded = "http://seeded.example";
    let (_dir, _wallet, addr, _) = served(&["seeded.example"]).await;
    for (door, methods) in super::OPEN_DOORS {
        for method in METHODS {
            let answered = by_method(addr, method, seeded, door).await;
            let gate_says = methods.iter().any(|m| m.as_str() == method);
            assert_eq!(
                answered.0 != 405,
                gate_says,
                "{method} {door}: the router answered {}",
                answered.0
            );
        }
    }
}

/// A wrong method at an open door is the router's `405`, neither counted
/// nor charged (#556): a `GET` at the POST door `getHeaderForHeight` past
/// the burst, and a `PUT`, `DELETE` and `PATCH` at the GET-and-POST doors
/// (no open door answers `GET` alone), leave the chain and presence
/// counters and the probe record where they stood; the calls after them
/// are answered within the burst, and only they are counted.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_wrong_method_at_an_open_door_is_neither_counted_nor_charged() {
    let (_dir, wallet, addr, probes) = served(&[]).await;
    for n in 0..12 {
        let wrong = by_method(addr, "GET", PAGE, "/getHeaderForHeight").await;
        assert_eq!((wrong.0, wrong.1.as_deref()), (405, Some(PAGE)), "{n}");
    }
    for door in ["/getHeight", "/isAuthenticated", "/waitForAuthentication"] {
        for method in ["PUT", "DELETE", "PATCH"] {
            let wrong = by_method(addr, method, PAGE, door).await;
            assert_eq!(wrong.0, 405, "{method} {door}");
        }
    }
    no_rows(&wallet, &probes, &[("page.example", 0)]).await;
    let present = by_method(addr, "POST", PAGE, "/isAuthenticated").await;
    assert_eq!(present.0, 200, "the presence door's minute is whole");
    for n in 0..10 {
        let door = ["/getVersion", "/getNetwork"][n % 2];
        let answered = by_method(addr, "POST", PAGE, door).await;
        assert_eq!(answered.0, 200, "{n}: the chain burst is whole");
    }
    let past = by_method(addr, "POST", PAGE, "/getHeight").await;
    assert_eq!(past.0, 429, "the eleventh is past the burst");
    no_rows(&wallet, &probes, &[("page.example", 11)]).await;
}
