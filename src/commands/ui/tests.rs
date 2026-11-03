//! The inspector's tests: its admission (#411), the origins view's write
//! routes admitting its own page alone (#414, the design's L4 exit test),
//! and `/api/query` on the read-only pool (#440).

use super::*;
use axum::body::Body;
use bsv_wallet_cli::store::origin_grants::{self as grants, Asking, Granting, Power};
use std::fmt::Write as _;
use tower::ServiceExt;

fn headers(pairs: &[(header::HeaderName, &str)]) -> HeaderMap {
    let mut h = HeaderMap::new();
    for (k, v) in pairs {
        h.insert(k.clone(), v.parse().unwrap());
    }
    h
}

/// #411: the inspector's own page and the origins whose originator is
/// listed (any port of a listed host) are admitted;
/// a GET without an Origin is its own page's read; anything else, or a
/// Host that is not loopback, is refused.
#[test]
fn the_inspector_admits_its_own_origin_and_the_listed() {
    let listed = AllowedOrigins::new(["app.example"]).expect("an originator");
    let host = (header::HOST, "localhost:9321");
    let ok = |m: Method, h: HeaderMap| inspector_word(&m, &h, &listed).is_ok();
    assert!(ok(Method::GET, headers(std::slice::from_ref(&host))));
    assert!(ok(
        Method::HEAD,
        headers(&[(header::HOST, "127.0.0.1:9321")])
    ));
    assert!(ok(
        Method::POST,
        headers(&[host.clone(), (header::ORIGIN, "http://localhost:9321")])
    ));
    assert!(ok(
        Method::POST,
        headers(&[host.clone(), (header::ORIGIN, "https://app.example:5173")])
    ));
    assert!(!ok(Method::POST, headers(std::slice::from_ref(&host))));
    assert!(!ok(
        Method::POST,
        headers(&[host.clone(), (header::ORIGIN, "http://evil.example")])
    ));
    assert!(!ok(
        Method::GET,
        headers(&[host.clone(), (header::ORIGIN, "http://127.0.0.1:9321")])
    ));
    assert!(!ok(
        Method::GET,
        headers(&[(header::HOST, "evil.example:9321")])
    ));
    assert!(!ok(Method::GET, headers(&[])));
}

/// The inspector's own origin, as its page names it.
const OWN: &str = "http://localhost:9321";

/// An inspector over a fresh store holding one open pending row
/// (`new.example:5173` listing the basket `todo items`) and a grant row
/// of that basket for `granted.example`, and a probe row (#539:
/// `new.example`'s `getVersion`); its routes admit `app.example` by
/// `ALLOWED_ORIGINS`.
async fn inspector() -> (tempfile::TempDir, Arc<AppState>, Router, i64) {
    let dir = tempfile::TempDir::new().unwrap();
    let file = dir.path().join("inspector.db");
    let path = file.to_str().unwrap().to_string();
    let seed = StorageSqlx::open(&path).await.unwrap();
    let power = Power::basket("todo items").unwrap();
    let asking = Asking {
        originator: "new.example:5173",
        power: &power,
        usage: Some("listing"),
        call: "/listOutputs",
        reason: None,
        amount: None,
        renewal: false,
    };
    let pending = grants::open_or_join(&seed, &asking, 1_000).await.unwrap();
    let granting = Granting {
        originator: "granted.example",
        power: &power,
        amount: None,
        expiry: 0,
    };
    grants::grant(&seed, &granting, 1_000).await.unwrap();
    let probe = grants::Probe {
        originator: "new.example:5173".into(),
        door: "getVersion".into(),
        power: None,
        at: 1_000,
    };
    grants::probes::record(&seed, &[probe]).await.unwrap();
    drop(seed);
    let state = Arc::new(open(&path).await.unwrap());
    let listed = AllowedOrigins::new(["app.example"]).unwrap();
    let app = router(Arc::clone(&state), listed);
    (dir, state, app, pending.row.pending_id)
}

fn call(method: &str, path: &str, origin: Option<&str>, body: &serde_json::Value) -> Request {
    let mut b = Request::builder()
        .method(method)
        .uri(path)
        .header("host", "localhost:9321");
    if let Some(o) = origin {
        b = b.header("origin", o);
    }
    if body.is_null() {
        return b.body(Body::empty()).unwrap();
    }
    b.header("content-type", "application/json")
        .body(Body::from(body.to_string()))
        .unwrap()
}

/// The router's answer to `request`: its status and its JSON (`null`
/// when the body is none).
async fn answered(app: &Router, request: Request) -> (StatusCode, serde_json::Value) {
    let (parts, body) = app.clone().oneshot(request).await.unwrap().into_parts();
    let read = axum::body::to_bytes(body, 1 << 20).await.unwrap();
    (
        parts.status,
        serde_json::from_slice(&read).unwrap_or_default(),
    )
}

/// The store's origin rows, as the module lists them.
async fn rows(state: &AppState) -> String {
    format!("{:?}", grants::listing(&state.store, true).await.unwrap())
}

/// The L4 exit test: an origin `ALLOWED_ORIGINS` lists, and an origin
/// the store grants, cannot answer or revoke (403 naming the rule, the
/// rows unchanged).
#[tokio::test]
async fn a_listed_or_granted_origin_cannot_answer_or_revoke() {
    let (_dir, state, app, id) = inspector().await;
    let before = rows(&state).await;
    let answer = serde_json::json!({"pendingId": id, "answer": "grant"});
    let revoke = serde_json::json!({"originator": "granted.example"});
    // Every origin but the own page's is refused 403. A listed one passes
    // the inspector's admission and meets the write routes' rule by its
    // text; the others (a granted origin no list names, until the switch
    // lets the admission read the rows; another loopback name; none) are
    // refused by the admission ahead of it, and the write rule refuses
    // them too ([`origins::write_refusal`], below).
    for (origin, listed) in [
        (Some("https://app.example"), true),
        (Some("http://app.example:8080"), true),
        (Some("http://granted.example"), false),
        (Some("http://127.0.0.1:9321"), false),
        (None, false),
    ] {
        for (path, body) in [
            ("/api/origins/answer", &answer),
            ("/api/origins/revoke", &revoke),
        ] {
            let (status, body) = answered(&app, call("POST", path, origin, body)).await;
            assert_eq!(status, StatusCode::FORBIDDEN, "{origin:?} {path}: {body}");
            let why = body["error"].as_str().unwrap_or_default();
            assert_eq!(
                why.contains("a granted page must not write a grant"),
                listed,
                "{origin:?} {path}: {why}"
            );
        }
        let mut h = headers(&[(header::HOST, "localhost:9321")]);
        if let Some(o) = origin {
            h.insert(header::ORIGIN, o.parse().unwrap());
        }
        let why = origins::write_refusal(&h).expect("refused");
        assert!(
            why.contains("a granted page must not write a grant"),
            "{why}"
        );
    }
    let own = headers(&[(header::HOST, "localhost:9321"), (header::ORIGIN, OWN)]);
    assert_eq!(origins::write_refusal(&own), None);
    assert_eq!(rows(&state).await, before, "a refused write wrote nothing");
}

/// The read route keeps the inspector's admission (a listed origin's
/// read admitted, an unlisted one refused); the inspector's own page
/// answers and revokes, and a second answer is told the row was already
/// answered.
#[tokio::test]
async fn the_inspectors_own_page_answers_and_revokes() {
    let (_dir, _state, app, id) = inspector().await;
    let read = |origin| answered(&app, call("GET", "/api/origins", origin, &json_null()));
    let (status, listed) = read(None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(listed["pending"][0]["rawOriginator"], "new.example:5173");
    assert_eq!(listed["grants"][0]["originator"], "granted.example");
    assert_eq!(listed["grants"][0]["kind"], "basket");
    let probe = r#"{"count":1,"door":"getVersion","firstSeen":1000,"lastSeen":1000,"originator":"new.example","power":null,"rawOriginator":"new.example:5173"}"#;
    assert_eq!(listed["probes"].to_string(), format!("[{probe}]"));
    assert_eq!(
        listed["pending"][0]["asked"],
        "list the basket \"todo items\""
    );
    assert_eq!(read(Some("https://app.example")).await.0, StatusCode::OK);
    assert_eq!(
        read(Some("http://evil.example")).await.0,
        StatusCode::FORBIDDEN
    );

    // Each step: the route, the body, the status, and a word the answer
    // carries. The own page answers; the second answer loses and is told;
    // it revokes one power's row; a second revoke names the originator and
    // the power; a key without its kind is refused.
    let todo = r#"["todo items"]"#;
    let steps = [
        (
            "answer",
            serde_json::json!({"pendingId": id, "answer": "grant"}),
            200,
            "\"key\":\"[\\\"todo items\\\"]\"",
        ),
        (
            "answer",
            serde_json::json!({"pendingId": id, "answer": "deny"}),
            409,
            "already answered",
        ),
        (
            "revoke",
            serde_json::json!({"originator": "granted.example", "kind": "basket", "key": todo}),
            200,
            "\"revokedAt\":1",
        ),
        (
            "revoke",
            serde_json::json!({"originator": "granted.example", "kind": "basket", "key": todo}),
            409,
            "granted.example holds no live grant row for basket [\\\"todo items\\\"]",
        ),
        (
            "revoke",
            serde_json::json!({"originator": "granted.example", "key": todo}),
            400,
            "name the kind with the key",
        ),
    ];
    for (route, body, status, word) in steps {
        let path = format!("/api/origins/{route}");
        let (got, answer) = answered(&app, call("POST", &path, Some(OWN), &body)).await;
        assert_eq!(got.as_u16(), status, "{route} {body}: {answer}");
        assert!(
            answer.to_string().contains(word),
            "{route} {body}: {answer}"
        );
    }
}

fn json_null() -> serde_json::Value {
    serde_json::Value::Null
}

/// Everything the store holds that a write could move: `sqlite_master`,
/// the header's `user_version`, and every row of every table.
async fn dump(path: &str) -> String {
    let reads = read_only_pool(path).await.unwrap();
    let mut out = String::new();
    let version: i64 = sqlx::query_scalar("PRAGMA user_version")
        .fetch_one(&reads)
        .await
        .unwrap();
    writeln!(out, "user_version {version}").unwrap();
    let master: Vec<(String, String, Option<String>)> =
        sqlx::query_as("SELECT type, name, sql FROM sqlite_master ORDER BY type, name")
            .fetch_all(&reads)
            .await
            .unwrap();
    for (kind, name, sql) in &master {
        writeln!(out, "{kind} {name} {sql:?}").unwrap();
        if kind == "table" {
            let rows = sqlx::query(&format!("SELECT * FROM \"{name}\""))
                .fetch_all(&reads)
                .await
                .unwrap();
            for row in &rows {
                let cells: Vec<String> = row
                    .columns()
                    .iter()
                    .map(|c| sqlite_value_to_json(row, c).to_string())
                    .collect();
                writeln!(out, "  {}", cells.join(",")).unwrap();
            }
        }
    }
    out
}

/// #440: a write disguised as a read reaches `/api/query` past the
/// courtesy prefix and fails on the read-only pool (SQLite's `readonly`),
/// the store unchanged byte for byte in a before/after dump: a CTE that
/// inserts, `;` and a second statement, a `PRAGMA` that writes, and the
/// `query_only` switch a `PRAGMA query_only = ON` pool would have honoured.
#[tokio::test]
async fn a_write_disguised_as_a_read_fails_on_the_read_only_pool() {
    let (dir, _state, app, _) = inspector().await;
    let path = dir.path().join("inspector.db");
    let path = path.to_str().unwrap();
    let insert = "INSERT INTO origin_grants (originator, raw_originator, kind, key, granted_at) \
                  VALUES ('evil.example', 'evil.example', 'basket', '[\"x\"]', 1)";
    let before = dump(path).await;
    for sql in [
        format!("WITH x AS (SELECT 1) {insert}"),
        format!("SELECT 1; {insert}"),
        "SELECT 1; DELETE FROM origin_pending".to_string(),
        "PRAGMA user_version = 7".to_string(),
        format!("PRAGMA query_only = OFF; {insert}"),
        "SELECT 1; UPDATE origin_grants SET revoked_at = 1".to_string(),
    ] {
        let body = serde_json::json!({ "sql": sql });
        let (status, answer) = answered(&app, call("POST", "/api/query", Some(OWN), &body)).await;
        assert_eq!(status, StatusCode::OK, "{sql}: {answer}");
        let error = answer["error"].as_str().unwrap_or_default();
        assert!(error.contains("readonly"), "{sql}: {answer}");
    }
    // A read still reads.
    let body = serde_json::json!({ "sql": "SELECT COUNT(*) AS n FROM origin_grants" });
    let (_, answer) = answered(&app, call("POST", "/api/query", Some(OWN), &body)).await;
    assert_eq!(answer["rows"][0]["n"], 1, "{answer}");
    assert_eq!(dump(path).await, before, "the store is unchanged");
}

/// A missing wallet file is refused by name, never created.
#[tokio::test]
async fn the_inspector_opens_no_missing_file() {
    let dir = tempfile::TempDir::new().unwrap();
    let file = dir.path().join("absent.db");
    let Err(e) = open(file.to_str().unwrap()).await else {
        panic!("a missing file opened");
    };
    assert!(e.to_string().contains("no wallet database"), "{e}");
    assert!(!file.exists());
}

/// Why the open flag and not `PRAGMA query_only = ON`: the store's own
/// reader is a `query_only` pool, and one query's text turns it off and
/// writes, since sqlx runs every statement of the text.
#[tokio::test]
async fn a_query_only_pool_is_no_guard_against_a_query() {
    let (_dir, state, _app, _) = inspector().await;
    let sql = "PRAGMA query_only = OFF; DELETE FROM origin_pending";
    let mut reader = state.store.pool().acquire().await.unwrap();
    sqlx::query(sql).execute(&mut *reader).await.unwrap();
    let open: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM origin_pending")
        .fetch_one(&mut *reader)
        .await
        .unwrap();
    assert_eq!(open, 0, "the query_only reader wrote");
    let rows = sqlx::query(sql).execute(&state.pool).await;
    assert!(rows.unwrap_err().to_string().contains("readonly"));
}
