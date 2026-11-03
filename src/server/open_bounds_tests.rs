//! The open doors' bound over the served router (#541): the 429 a page
//! reads past each class's bound (its code, the door and the window named,
//! `Retry-After` exposed through CORS), the window's reset, per-origin
//! isolation, nothing written to the store (no grant, no pending row, and
//! no probe row: only an answered call is counted, #539), and the owner's
//! routes, the host routes' owner and the seed outside the bound.

use std::net::SocketAddr;
use std::time::Duration;

use super::owner_door_tests::wallet;
use super::*;
use crate::store::origin_grants as grants;
use crate::test_support::serve_router;

const PAGE: &str = "http://page.example";
const OTHER: &str = "http://other.example";

/// A call's answer as a page reads it: the status, the CORS echo, the
/// exposed headers, `Retry-After` and the body.
struct Read {
    status: u16,
    echo: Option<String>,
    exposed: Option<String>,
    retry: Option<String>,
    body: serde_json::Value,
}

async fn served(wallet: &WalletState, seeded: &[&str]) -> (SocketAddr, probes::Probes) {
    let config = ServerConfig::admitting(seeded.iter().copied()).unwrap();
    let probes = config.probes.clone();
    let (url, _task) = serve_router(make_router(wallet.clone(), config)).await;
    (url.trim_start_matches("http://").parse().unwrap(), probes)
}

/// The router's URL for `path`.
fn at(addr: SocketAddr, path: &str) -> String {
    format!("http://{addr}{path}")
}

async fn read(response: reqwest::Response) -> Read {
    let header = |name: &str| {
        response
            .headers()
            .get(name)
            .map(|v| v.to_str().unwrap().to_string())
    };
    let (echo, exposed, retry) = (
        header("access-control-allow-origin"),
        header("access-control-expose-headers"),
        header("retry-after"),
    );
    let status = response.status().as_u16();
    let body = response.json().await.unwrap_or(serde_json::Value::Null);
    Read {
        status,
        echo,
        exposed,
        retry,
        body,
    }
}

/// A GET from `origin`, or from the owner's side with none.
async fn get(addr: SocketAddr, origin: Option<&str>, path: &str) -> Read {
    let mut request = reqwest::Client::new().get(at(addr, path));
    if let Some(origin) = origin {
        request = request.header("origin", origin);
    }
    read(request.send().await.unwrap()).await
}

async fn post(addr: SocketAddr, origin: &str, path: &str, body: serde_json::Value) -> Read {
    let request = reqwest::Client::new()
        .post(at(addr, path))
        .header("origin", origin)
        .json(&body);
    read(request.send().await.unwrap()).await
}

fn level_zero() -> serde_json::Value {
    json!({"protocolID": [0, "open gate"], "keyID": "1", "counterparty": "self",
           "data": [1, 2, 3]})
}

/// The 429 a page reads: the code, the door and the window in the
/// message, `Retry-After` and the echo with `retry-after` exposed.
fn assert_limited(read: &Read, origin: &str, door: &str, window: &str, retry: &str) {
    assert_eq!(read.status, 429, "{}", read.body);
    assert_eq!(read.body["code"], "ERR_RATE_LIMITED");
    let message = read.body["message"].as_str().unwrap();
    assert!(
        message.contains(door) && message.contains(window),
        "{message}"
    );
    assert!(
        message.ends_with(&format!("retry after {retry} s.")),
        "{message}"
    );
    assert_eq!(read.retry.as_deref(), Some(retry));
    assert_eq!(read.echo.as_deref(), Some(origin));
    assert_eq!(read.exposed.as_deref(), Some("retry-after"));
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
        let counted: i64 = all
            .probes
            .iter()
            .filter(|r| r.originator == *host)
            .map(|r| r.count)
            .sum();
        assert_eq!(counted, *n, "{host}: only the answered calls are counted");
    }
}

/// The chain doors at the edge, served: ten calls at once (the four doors
/// one bound) are answered, the eleventh (`getHeight`) is 429 with
/// `Retry-After: 1` read through CORS; another origin is untouched; after
/// the window a call is answered again; no row is written.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_page_past_the_chain_burst_reads_429_then_200_after_the_window() {
    let (_dir, wallet) = wallet().await;
    let (addr, probes) = served(&wallet, &[]).await;
    for n in 0..10 {
        let door = ["/getNetwork", "/getVersion"][n % 2];
        let answered = get(addr, Some(PAGE), door).await;
        assert_eq!(
            (answered.status, answered.echo.as_deref()),
            (200, Some(PAGE)),
            "call {n}"
        );
    }
    let limited = get(addr, Some(PAGE), "/getHeight").await;
    let window = "the chain doors (getHeight, getHeaderForHeight, getNetwork, getVersion) \
                  answer 60 a minute per site, 10 at once";
    assert_limited(&limited, PAGE, "calls to /getHeight", window, "1");
    // Every port of a host is one originator: no fresh bound by port.
    let port = get(addr, Some("http://page.example:8080"), "/getVersion").await;
    assert_eq!(port.status, 429);
    let other = get(addr, Some(OTHER), "/getVersion").await;
    assert_eq!((other.status, other.echo.as_deref()), (200, Some(OTHER)));
    tokio::time::sleep(Duration::from_millis(1100)).await;
    assert_eq!(get(addr, Some(PAGE), "/getVersion").await.status, 200);
    no_rows(
        &wallet,
        &probes,
        &[("page.example", 11), ("other.example", 1)],
    )
    .await;
}

/// The presence doors, served: each answers a page once a minute, the
/// second call 429 with `Retry-After: 60`; the other presence door answers
/// its own once; another origin is untouched.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_presence_door_answers_a_page_once() {
    let (_dir, wallet) = wallet().await;
    let (addr, probes) = served(&wallet, &[]).await;
    let opening = get(addr, Some(PAGE), "/isAuthenticated").await;
    assert_eq!(
        (opening.status, opening.body["authenticated"].as_bool()),
        (200, Some(true))
    );
    let again = get(addr, Some(PAGE), "/isAuthenticated").await;
    let window = "a presence door answers once a minute per site";
    assert_limited(&again, PAGE, "calls to /isAuthenticated", window, "60");
    assert_eq!(
        get(addr, Some(PAGE), "/waitForAuthentication").await.status,
        200
    );
    let wait = get(addr, Some(PAGE), "/waitForAuthentication").await;
    assert_limited(&wait, PAGE, "calls to /waitForAuthentication", window, "60");
    assert_eq!(get(addr, Some(OTHER), "/isAuthenticated").await.status, 200);
    no_rows(
        &wallet,
        &probes,
        &[("page.example", 2), ("other.example", 1)],
    )
    .await;
}

/// A level-0 power, served: ten signatures at once are answered, the
/// eleventh (at another door, the class one bound) is 429; the chain doors
/// are untouched by it.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_level_zero_power_is_bounded_on_its_own() {
    let (_dir, wallet) = wallet().await;
    let (addr, probes) = served(&wallet, &[]).await;
    for n in 0..10 {
        let signed = post(addr, PAGE, "/createSignature", level_zero()).await;
        assert_eq!(signed.status, 200, "call {n}: {}", signed.body);
    }
    let hmac = json!({"protocolID": [0, "open gate"], "keyID": "1", "data": [1, 2, 3]});
    let limited = post(addr, PAGE, "/createHmac", hmac).await;
    let window = "the level-0 protocol powers answer 60 a minute per site, 10 at once";
    assert_limited(&limited, PAGE, "level-0 calls to /createHmac", window, "1");
    assert_eq!(get(addr, Some(PAGE), "/getVersion").await.status, 200);
    assert_eq!(
        post(addr, OTHER, "/createSignature", level_zero())
            .await
            .status,
        200
    );
    no_rows(
        &wallet,
        &probes,
        &[("page.example", 11), ("other.example", 1)],
    )
    .await;
}

/// Outside the bound: the owner's routes and the host routes' owner (no
/// `Origin`, loopback), and a seed-listed originator (the allow-list is the
/// gate, ahead of the bound).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_owner_and_the_seed_are_outside_the_bound() {
    let (_dir, wallet) = wallet().await;
    let (addr, _) = served(&wallet, &["seeded.example"]).await;
    for n in 0..30 {
        assert_eq!(get(addr, None, "/origins").await.status, 200, "owner {n}");
        assert_eq!(
            get(addr, None, "/permission-audit").await.status,
            200,
            "host {n}"
        );
        let seeded = get(addr, Some("http://seeded.example"), "/getVersion").await;
        assert_eq!(seeded.status, 200, "seed {n}");
        let present = get(addr, Some("http://seeded.example"), "/isAuthenticated").await;
        assert_eq!(present.status, 200, "seed presence {n}");
        let signed = post(
            addr,
            "http://seeded.example",
            "/createSignature",
            level_zero(),
        )
        .await;
        assert_eq!(signed.status, 200, "seed level 0 {n}");
    }
}
