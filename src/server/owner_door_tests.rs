//! The owner's door over the served router (#414, the design's L3 exit
//! test): each route refused with any `Origin`, with a `Host` beyond
//! loopback, with no bearer under `AUTH_TOKEN`, and as a preflight; the
//! admitted call answering the listing's shape; the answer-once race. The
//! doors around it keep their gate.

use super::*;
use crate::store::origin_grants::{self as grants, Asking, Power};
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
            json!({"originator": "app.example", "kind": "basket", "key": "[\"todo items\"]"}),
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

/// A held call's pending row for `originator`'s listing of the basket
/// `todo items`, as the gate will write it (L5b), opened at `now`; its id.
async fn pending(wallet: &WalletState, originator: &str, now: i64) -> i64 {
    let power = Power::basket("todo items").unwrap();
    let asking = Asking {
        originator,
        power: &power,
        usage: Some("listing"),
        call: "/listOutputs",
        reason: None,
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
    let grant = json!({"originator": "App.Example:8443", "kind": "spending", "amount": 60000});
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
    let spent = (&row["spentThisMonth"], &row["kind"], &row["key"]);
    assert_eq!(spent, (&json!(0), &json!("spending"), &json!("[]")));

    let id = pending(&wallet, "new.example:5173", unix_now()).await;
    let text = raw(&app, listing()).await;
    in_order(&text, &["\"grants\"", "\"pending\""]);
    in_order(&text, &PENDING_KEYS);
    let (_, _, listed) = answered(&app, listing()).await;
    assert_eq!(listed["grants"].as_array().unwrap().len(), 1);
    assert_eq!(listed["pending"][0]["pendingId"], json!(id));
    assert_eq!(listed["pending"][0]["rawOriginator"], "new.example:5173");
    assert_eq!(
        listed["pending"][0]["asked"],
        "list the basket \"todo items\""
    );
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
    assert_eq!(body["grants"][0]["key"], "[\"todo items\"]");
    assert_eq!(body["grants"][0]["expiry"], json!(now + 3600));

    // No kind: every live row of the host (its basket row) is stamped.
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
            json!({"originator": "a.example", "kind": "spending", "amount": 1, "expires": 5}),
            "unknown field `expires`",
        ),
        (
            "/origins/grant",
            json!({"originator": "a.example", "kind": "wallet"}),
            "unknown variant `wallet`",
        ),
        (
            "/origins/grant",
            json!({"originator": "a.example", "scope": "spending", "amount": 1}),
            "unknown field `scope`",
        ),
        (
            "/origins/grant",
            json!({"originator": "https://a.example", "kind": "spending", "amount": 1}),
            "is not",
        ),
        (
            "/origins/grant",
            json!({"originator": "a.example", "kind": "spending"}),
            "names its amount",
        ),
        (
            "/origins/grant",
            json!({"originator": "a.example", "kind": "protocol"}),
            "a protocol grant names its key",
        ),
        (
            "/origins/grant",
            json!({"originator": "a.example", "kind": "basket", "key": "[\"Todo\"]"}),
            "write it [\"todo\"]",
        ),
        (
            "/origins/grant",
            json!({"originator": "a.example", "kind": "basket", "key": "[\"default\"]"}),
            "Basket \u{201c}default\u{201d} is admin-only.",
        ),
        (
            "/origins/grant",
            json!({"originator": "a.example", "kind": "protocol", "key": "[false,0,\"todo list\",\"\"]"}),
            "security level 0",
        ),
        (
            "/origins/revoke",
            json!({"originator": "a.example", "key": "[\"todo\"]"}),
            "name the kind with the key",
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

/// A power of each kind as the routes name it: its kind, its key (none
/// for spending) and a grant's amount.
fn every_kind() -> [(&'static str, Option<String>, Option<i64>); 5] {
    let g = "0279be667ef9dcbbac55a06295ce870b07029bfcdb2dce28d959f2815b16f81798";
    let t = "AGfk/WrT1eBDXpz3mcw386Zww2HmqcIn3uY6x4Af1eo=";
    [
        (
            "protocol",
            Some(r#"[false,2,"todo list","self"]"#.into()),
            None,
        ),
        (
            "protocol",
            Some(r#"[false,1,"action label invoices",""]"#.into()),
            None,
        ),
        ("basket", Some(r#"["todo items"]"#.into()), None),
        (
            "certificate",
            Some(format!(r#"[false,"{t}","{g}",["email","name"]]"#)),
            None,
        ),
        ("spending", None, Some(60_000)),
    ]
}

#[tokio::test]
async fn the_routes_grant_list_and_revoke_a_power_of_each_kind() {
    let (_dir, wallet) = wallet().await;
    let app = router(&wallet, None);
    for (kind, key, amount) in every_kind() {
        let mut body = json!({"originator": "app.example:8443", "kind": kind});
        if let Some(key) = &key {
            body["key"] = json!(key);
        }
        if let Some(amount) = amount {
            body["amount"] = json!(amount);
        }
        let (status, _, granted) = answered(&app, post("/origins/grant", &body)).await;
        assert_eq!(status, StatusCode::OK, "{body}: {granted}");
        let row = &granted["grants"][0];
        assert_eq!(
            (&row["kind"], &row["key"]),
            (&json!(kind), &json!(key.clone().unwrap_or("[]".into())))
        );
    }
    let (_, _, listed) = answered(&app, listing()).await;
    assert_eq!(listed["grants"].as_array().unwrap().len(), 5);

    // One power's row, then a kind's rows, then every row.
    let one = json!({"originator": "app.example", "kind": "basket", "key": "[\"todo items\"]"});
    let (status, _, body) = answered(&app, post("/origins/revoke", &one)).await;
    assert_eq!(
        (status, body["grants"].as_array().unwrap().len()),
        (StatusCode::OK, 1)
    );
    let (status, _, body) = answered(&app, post("/origins/revoke", &one)).await;
    assert_eq!(status, StatusCode::CONFLICT);
    let why = "app.example holds no live grant row for basket [\"todo items\"]: nothing to revoke";
    assert_eq!(refused(&body).1, why);
    let protocols = json!({"originator": "app.example", "kind": "protocol"});
    let (_, _, body) = answered(&app, post("/origins/revoke", &protocols)).await;
    assert_eq!(
        body["grants"].as_array().unwrap().len(),
        2,
        "the protocol and the label"
    );
    let (_, _, body) = answered(&app, post("/origins/revoke", &protocols)).await;
    let why = "app.example holds no live grant row of the protocol kind: nothing to revoke";
    assert_eq!(refused(&body).1, why);
    let every = json!({"originator": "app.example"});
    let (_, _, body) = answered(&app, post("/origins/revoke", &every)).await;
    let kinds: Vec<&str> = body["grants"]
        .as_array()
        .unwrap()
        .iter()
        .map(|g| g["kind"].as_str().unwrap())
        .collect();
    assert_eq!(kinds, ["certificate", "spending"]);
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
    "\"kind\"",
    "\"key\"",
    "\"amount\"",
    "\"spentThisMonth\"",
    "\"grantedAt\"",
    "\"expiry\"",
    "\"revokedAt\"",
];

/// Section (4)'s keys of a pending row, in its order.
const PENDING_KEYS: [&str; 14] = [
    "\"pendingId\"",
    "\"originator\"",
    "\"rawOriginator\"",
    "\"kind\"",
    "\"key\"",
    "\"usage\"",
    "\"asked\"",
    "\"call\"",
    "\"reason\"",
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
