//! The owner's door over the served router (#414, the design's L3 exit
//! test): each route refused with any `Origin`, with a `Host` beyond
//! loopback, with no bearer under `AUTH_TOKEN`, and as a preflight; the
//! admitted call answering the listing's shape; the answer-once race. The
//! doors around it keep their gate.

use super::*;
use crate::store::origin_grants::{self as grants, Asking, Scope};
use crate::test_support::{response_answer, test_wallet};
use bsv_sdk::primitives::PrivateKey;
use tower::ServiceExt;

/// The four routes, each with a well-formed body.
fn calls() -> [(&'static str, &'static str, serde_json::Value); 4] {
    [
        ("GET", "/origins", json!(null)),
        (
            "POST",
            "/origins/answer",
            json!({"pendingId": 1, "answer": "deny"}),
        ),
        (
            "POST",
            "/origins/grant",
            json!({"originator": "app.example", "scope": "wallet"}),
        ),
        (
            "POST",
            "/origins/revoke",
            json!({"originator": "app.example"}),
        ),
    ]
}

async fn wallet() -> (tempfile::TempDir, WalletState) {
    let dir = tempfile::TempDir::new().unwrap();
    let file = dir.path().join("owner.db");
    let storage = StorageSqlx::open(file.to_str().unwrap()).await.unwrap();
    let key = PrivateKey::from_hex(&"02".repeat(32)).unwrap();
    (dir, test_wallet(storage, "owner", key).await)
}

fn router(wallet: &WalletState, token: Option<&str>) -> Router {
    let config = ServerConfig {
        auth_token: token.map(str::to_string),
        ..ServerConfig::admitting(["app.example"]).unwrap()
    };
    make_router(wallet.clone(), config)
}

/// A call from the owner's side: loopback `Host`, no `Origin`, `headers`
/// added.
fn call(method: &str, path: &str, body: &serde_json::Value, headers: &[(&str, &str)]) -> Request {
    let mut builder = Request::builder()
        .method(method)
        .uri(path)
        .header("host", "127.0.0.1:3322");
    for (k, v) in headers {
        builder = builder.header(*k, *v);
    }
    if body.is_null() {
        return builder.body(axum::body::Body::empty()).unwrap();
    }
    builder
        .header("content-type", "application/json")
        .body(axum::body::Body::from(body.to_string()))
        .unwrap()
}

async fn answered(app: &Router, request: Request) -> (StatusCode, HeaderMap, serde_json::Value) {
    let response = app.clone().oneshot(request).await.unwrap();
    let headers = response.headers().clone();
    let (status, _, body) = response_answer(response).await;
    (status, headers, body)
}

fn refused(body: &serde_json::Value) -> (&str, &str) {
    (
        body["code"].as_str().unwrap_or_default(),
        body["message"].as_str().unwrap_or_default(),
    )
}

#[tokio::test]
async fn a_call_with_any_origin_is_refused_on_every_route() {
    let (_dir, wallet) = wallet().await;
    let app = router(&wallet, None);
    // A listed origin is a page all the same; so is one never listed.
    for origin in ["http://app.example", "http://unlisted.test", "null"] {
        for (method, path, body) in calls() {
            let (status, headers, body) =
                answered(&app, call(method, path, &body, &[("origin", origin)])).await;
            assert_eq!(
                status,
                StatusCode::FORBIDDEN,
                "{method} {path} from {origin}"
            );
            assert_eq!(refused(&body), ("ACCESS_DENIED", owner_door::PAGE));
            assert!(
                !headers.contains_key("access-control-allow-origin"),
                "{path} answered CORS"
            );
        }
    }
    let rows = grants::listing(wallet.storage(), true).await.unwrap();
    assert!(rows.grants.is_empty(), "a refused call wrote a grant");
}

#[tokio::test]
async fn a_host_beyond_loopback_is_refused_on_every_route() {
    let (_dir, wallet) = wallet().await;
    let app = router(&wallet, None);
    for host in [
        "evil.example",
        "evil.example:3322",
        "10.0.0.2:3322",
        "x@127.0.0.1",
    ] {
        for (method, path, body) in calls() {
            let mut request = call(method, path, &body, &[]);
            request.headers_mut().insert("host", host.parse().unwrap());
            let (status, _, body) = answered(&app, request).await;
            assert_eq!(status, StatusCode::FORBIDDEN, "{method} {path} at {host}");
            let want = owner_door::not_loopback(Some(host));
            assert_eq!(refused(&body), ("ACCESS_DENIED", want.as_str()));
        }
    }
    // No Host at all.
    let mut request = call("GET", "/origins", &json!(null), &[]);
    request.headers_mut().remove("host");
    let (status, _, body) = answered(&app, request).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert_eq!(refused(&body).1, owner_door::not_loopback(None));
    // Every loopback name is the owner's.
    for host in ["localhost:3322", "127.0.0.1", "[::1]:3322", "127.0.0.2"] {
        let mut request = call("GET", "/origins", &json!(null), &[]);
        request.headers_mut().insert("host", host.parse().unwrap());
        assert_eq!(answered(&app, request).await.0, StatusCode::OK, "{host}");
    }
}

#[tokio::test]
async fn no_bearer_under_auth_token_is_refused_on_every_route() {
    let (_dir, wallet) = wallet().await;
    let app = router(&wallet, Some("owner-token"));
    for (method, path, body) in calls() {
        for headers in [&[][..], &[("authorization", "Bearer wrong")][..]] {
            let (status, _, body) = answered(&app, call(method, path, &body, headers)).await;
            assert_eq!(status, StatusCode::UNAUTHORIZED, "{method} {path}");
            assert_eq!(
                refused(&body),
                ("UNAUTHORIZED", "Invalid or missing bearer token")
            );
        }
    }
    let bearer = [("authorization", "Bearer owner-token")];
    let (status, _, body) = answered(&app, call("GET", "/origins", &json!(null), &bearer)).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body, json!({"grants": [], "pending": []}));
    // The admission runs before the bearer: a page with the bearer is
    // still a page.
    let page = [
        ("authorization", "Bearer owner-token"),
        ("origin", "http://app.example"),
    ];
    let (status, _, body) = answered(&app, call("GET", "/origins", &json!(null), &page)).await;
    assert_eq!(
        (status, refused(&body).1),
        (StatusCode::FORBIDDEN, owner_door::PAGE)
    );
}

#[tokio::test]
async fn a_preflight_is_refused_on_every_route() {
    let (_dir, wallet) = wallet().await;
    let app = router(&wallet, None);
    for (_, path, _) in calls() {
        for headers in [
            &[
                ("origin", "http://app.example"),
                ("access-control-request-method", "POST"),
            ][..],
            &[][..],
        ] {
            let (status, cors, body) =
                answered(&app, call("OPTIONS", path, &json!(null), headers)).await;
            assert_eq!(status, StatusCode::FORBIDDEN, "OPTIONS {path}");
            assert_eq!(refused(&body), ("ACCESS_DENIED", owner_door::PREFLIGHT));
            assert!(!cors.contains_key("access-control-allow-origin"));
        }
    }
}

/// A held call's pending row for `originator`'s `wallet` scope, as the
/// gate will write it (L5), opened at `now`; its id.
async fn pending(wallet: &WalletState, originator: &str, now: i64) -> i64 {
    let asking = Asking {
        originator,
        scope: Scope::Wallet,
        call: "/listOutputs",
        amount: None,
        renewal: false,
    };
    let opened = grants::open_or_join(wallet.storage(), &asking, now).await;
    opened.unwrap().row.pending_id
}

fn post(path: &str, body: &serde_json::Value) -> Request {
    call("POST", path, body, &[])
}

fn listing() -> Request {
    call("GET", "/origins", &json!(null), &[])
}

#[tokio::test]
async fn the_admitted_grant_and_listing_answer_the_shape() {
    let (_dir, wallet) = wallet().await;
    let app = router(&wallet, None);
    let grant = json!({"originator": "App.Example:8443", "scope": "spending", "amount": 60000});
    in_order(
        &raw(&app, post("/origins/grant", &grant)).await,
        &GRANT_KEYS,
    );
    let (status, headers, body) = answered(&app, post("/origins/grant", &grant)).await;
    assert_eq!(status, StatusCode::OK);
    assert!(!headers.contains_key("access-control-allow-origin"));
    let row = &body["grants"][0];
    let named = (&row["originator"], &row["rawOriginator"], &row["amount"]);
    assert_eq!(
        named,
        (
            &json!("app.example"),
            &json!("app.example:8443"),
            &json!(60000)
        )
    );
    let spent = (&row["spentThisMonth"], &row["source"]);
    assert_eq!(spent, (&json!(0), &json!("owner")));

    let id = pending(&wallet, "new.example:5173", unix_now()).await;
    let text = raw(&app, listing()).await;
    in_order(&text, &["\"grants\"", "\"pending\""]);
    in_order(&text, &PENDING_KEYS);
    let (_, _, listed) = answered(&app, listing()).await;
    assert_eq!(listed["grants"].as_array().unwrap().len(), 1);
    assert_eq!(listed["pending"][0]["pendingId"], json!(id));
    assert_eq!(listed["pending"][0]["rawOriginator"], "new.example:5173");
}

#[tokio::test]
async fn the_admitted_answer_grants_and_the_revoke_stamps() {
    let (_dir, wallet) = wallet().await;
    let app = router(&wallet, None);
    let now = unix_now();
    let id = pending(&wallet, "new.example:5173", now).await;
    let answer = json!({"pendingId": id, "answer": "grant", "expiry": now + 3600});
    let (status, _, body) = answered(&app, post("/origins/answer", &answer)).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["pending"][0]["answer"], "grant");
    assert_eq!(body["grants"][0]["originator"], "new.example");
    assert_eq!(body["grants"][0]["expiry"], json!(now + 3600));

    // No scope: every live row of the host (its wallet row) is stamped.
    let revoke = || post("/origins/revoke", &json!({"originator": "new.example"}));
    let (status, _, body) = answered(&app, revoke()).await;
    assert_eq!(status, StatusCode::OK);
    assert!(body["grants"][0]["revokedAt"].is_number());
    let (status, _, body) = answered(&app, revoke()).await;
    assert_eq!(status, StatusCode::CONFLICT);
    let why = "new.example holds no live grant row: nothing to revoke";
    assert_eq!(refused(&body).1, why);
}

#[tokio::test]
async fn a_malformed_body_is_refused_by_name() {
    let (_dir, wallet) = wallet().await;
    let app = router(&wallet, None);
    for (path, body, words) in [
        (
            "/origins/answer",
            json!({"pendingId": 1, "answer": "maybe"}),
            "unknown variant `maybe`",
        ),
        (
            "/origins/grant",
            json!({"originator": "a.example", "scope": "wallet", "expires": 5}),
            "unknown field `expires`",
        ),
        (
            "/origins/grant",
            json!({"originator": "https://a.example", "scope": "wallet"}),
            "is not",
        ),
        (
            "/origins/grant",
            json!({"originator": "a.example", "scope": "spending"}),
            "names its amount",
        ),
        (
            "/origins/answer",
            json!({"pendingId": 99, "answer": "deny"}),
            "no pending request #99",
        ),
    ] {
        let (status, _, answer) = answered(&app, call("POST", path, &body, &[])).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{path} {body}");
        let (code, message) = refused(&answer);
        assert_eq!(code, "VALIDATION_ERROR");
        assert!(message.contains(words), "{path}: {message}");
    }
}

#[tokio::test]
async fn two_answers_race_to_one_winner_and_the_loser_is_told() {
    let (_dir, wallet) = wallet().await;
    let app = router(&wallet, None);
    let id = pending(&wallet, "race.example", unix_now()).await;
    let say = |word: &str| post("/origins/answer", &json!({"pendingId": id, "answer": word}));
    let (a, b) = tokio::join!(answered(&app, say("grant")), answered(&app, say("deny")));
    let mut statuses = [a.0, b.0];
    statuses.sort();
    assert_eq!(statuses, [StatusCode::OK, StatusCode::CONFLICT]);
    let (winner, loser) = if a.0 == StatusCode::OK {
        (a, b)
    } else {
        (b, a)
    };
    let word = winner.2["pending"][0]["answer"]
        .as_str()
        .unwrap()
        .to_string();
    let (code, message) = refused(&loser.2);
    assert_eq!(code, "CONFLICT");
    let told = format!("pending request #{id} was already answered {word} at ");
    assert!(message.starts_with(&told), "{message}");
    // One answer stands, and one grant at most.
    let rows = grants::listing(wallet.storage(), true).await.unwrap();
    assert_eq!(
        rows.pending[0].answer.map(grants::Answer::as_str),
        Some(word.as_str())
    );
    assert_eq!(rows.grants.len(), usize::from(word == "grant"));
}

#[tokio::test]
async fn the_doors_keep_their_gate_beside_the_owners_routes() {
    let (_dir, wallet) = wallet().await;
    let app = router(&wallet, None);
    // A door without an Origin is still the client's refusal.
    let (status, _, body) = answered(&app, call("GET", "/getVersion", &json!(null), &[])).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(refused(&body).1, origin_gate::NO_ORIGIN);
    // A listed page reaches its doors, with the echo.
    let page = [("origin", "http://app.example")];
    let (status, headers, _) =
        answered(&app, call("GET", "/getVersion", &json!(null), &page)).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(headers["access-control-allow-origin"], "http://app.example");
    // A path near the owner's is a door's: gated, then unknown.
    let (status, _, body) = answered(&app, call("GET", "/origins/", &json!(null), &[])).await;
    assert_eq!(
        (status, refused(&body).1),
        (StatusCode::BAD_REQUEST, origin_gate::NO_ORIGIN)
    );
}

/// Section (4)'s keys of a grant row, in its order.
const GRANT_KEYS: [&str; 9] = [
    "\"originator\"",
    "\"rawOriginator\"",
    "\"scope\"",
    "\"amount\"",
    "\"spentThisMonth\"",
    "\"grantedAt\"",
    "\"expiry\"",
    "\"revokedAt\"",
    "\"source\"",
];

/// Section (4)'s keys of a pending row, in its order.
const PENDING_KEYS: [&str; 10] = [
    "\"pendingId\"",
    "\"originator\"",
    "\"rawOriginator\"",
    "\"scope\"",
    "\"call\"",
    "\"amount\"",
    "\"renewal\"",
    "\"firstSeen\"",
    "\"answeredAt\"",
    "\"answer\"",
];

/// The body's text as served.
async fn raw(app: &Router, request: Request) -> String {
    let response = app.clone().oneshot(request).await.unwrap();
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    String::from_utf8(bytes.to_vec()).unwrap()
}

/// Each of `keys` appears in `text`, the first of each after the one
/// before it.
fn in_order(text: &str, keys: &[&str]) {
    let mut at = 0;
    for key in keys {
        let found = text[at..]
            .find(key)
            .unwrap_or_else(|| panic!("{key} after {at} in {text}"));
        at += found + key.len();
    }
}

pub(super) fn unix_now() -> i64 {
    let since = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap();
    i64::try_from(since.as_secs()).unwrap()
}
