//! The five host-route tools against an in-process stub of the wallet's
//! and the relay's routes: their method, path, query, body and bearer as
//! `src/server/mod.rs` and `src/relay.rs` define them; the owner's three
//! (#414) against a stub of `src/server/owner_door.rs`'s routes that
//! echoes each call's method, path, `Origin` and bearer. Nothing reaches
//! the network.

use super::*;
use axum::extract::Query;
use axum::http::{HeaderMap, StatusCode};
use axum::routing::{get, post};
use axum::{Json, Router};
use std::collections::HashMap;

/// `app` on a loopback port of its own: its base URL.
async fn serve(app: Router) -> String {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.ok();
    });
    format!("http://{addr}")
}

async fn stub() -> String {
    let app = Router::new()
        .route(
            "/chain-feed",
            get(|| async { Json(json!({"status": {"state": "following"}, "cursor": "42"})) }),
        )
        .route(
            "/tx/{txid}",
            get(
                |axum::extract::Path(txid): axum::extract::Path<String>| async move {
                    if txid == "aa".repeat(32) {
                        (
                            StatusCode::OK,
                            Json(json!({"txid": txid, "word": "mined", "height": 900, "reask": null,
                                        "evidence": {"root": "bb".repeat(32), "height": 900}})),
                        )
                    } else {
                        (
                            StatusCode::NOT_FOUND,
                            Json(json!({"code": "NOT_FOUND", "message": "no word stored"})),
                        )
                    }
                },
            ),
        )
        .route(
            "/health",
            get(|| async { Json(json!({"healthy": true, "service": "bsv-wallet-relay"})) }),
        )
        .route(
            "/pull",
            get(|Query(q): Query<HashMap<String, String>>| async move {
                Json(json!({"token": q.get("token"), "after": q.get("after"), "ack": q.get("ack"), "payloads": []}))
            }),
        )
        .route(
            "/register",
            post(|headers: HeaderMap, Json(body): Json<Value>| async move {
                if headers.get("authorization").and_then(|v| v.to_str().ok())
                    != Some("Bearer admin")
                {
                    return (
                        StatusCode::UNAUTHORIZED,
                        Json(json!({"error": "invalid admin token"})),
                    );
                }
                (StatusCode::OK, Json(json!({"registered": body["token"]})))
            }),
        );
    serve(app).await
}

fn text(result: CallToolResult) -> Value {
    let ContentBlock::Text(t) = &result.content[0] else {
        panic!("not text");
    };
    serde_json::from_str(&t.text).unwrap()
}

#[tokio::test]
async fn the_five_host_tools_reach_their_routes() {
    let base = stub().await;
    let mcp = WalletMcp::new(base.clone(), "http://localhost".into())
        .with_relay(Some(base.clone()), Some("admin".into()));

    let feed = text(mcp.chain_feed_status().await.unwrap());
    assert_eq!(feed["cursor"], "42");

    let word = text(
        mcp.tx_word(Parameters(TxWordParams {
            txid: "AA".repeat(32),
        }))
        .await
        .unwrap(),
    );
    assert_eq!(
        (&word["word"], &word["evidence"]["height"]),
        (&json!("mined"), &json!(900))
    );
    let unknown = mcp
        .tx_word(Parameters(TxWordParams {
            txid: "cc".repeat(32),
        }))
        .await
        .unwrap_err();
    assert!(unknown.message.contains("HTTP 404"), "{}", unknown.message);
    assert!(mcp
        .tx_word(Parameters(TxWordParams { txid: "zz".into() }))
        .await
        .is_err());

    assert_eq!(text(mcp.health().await.unwrap())["healthy"], true);

    let pulled = text(
        mcp.pull(Parameters(PullParams {
            token: "t1".into(),
            after: Some(3),
            ack: Some(2),
        }))
        .await
        .unwrap(),
    );
    assert_eq!(
        (&pulled["token"], &pulled["after"], &pulled["ack"]),
        (&json!("t1"), &json!("3"), &json!("2"))
    );

    let registered = text(
        mcp.register(Parameters(RegisterParams { token: "t1".into() }))
            .await
            .unwrap(),
    );
    assert_eq!(registered["registered"], "t1");

    // Without the admin bearer the relay refuses, and the tool says so.
    let bare = WalletMcp::new(base.clone(), "http://localhost".into()).with_relay(Some(base), None);
    let err = bare
        .register(Parameters(RegisterParams { token: "t1".into() }))
        .await
        .unwrap_err();
    assert!(err.message.contains("HTTP 401"), "{}", err.message);

    // No relay named: the relay tools say so, and ask nothing.
    let none = WalletMcp::new("http://127.0.0.1:9".into(), "http://localhost".into());
    assert!(none
        .health()
        .await
        .unwrap_err()
        .message
        .contains("RELAY_URL"));
}

#[test]
fn the_tools_are_the_29_the_five_and_the_owners_three() {
    let tools = WalletMcp::tool_router().list_all();
    assert_eq!(tools.len(), 37);
    let described = |name: &str| {
        let tool = tools.iter().find(|t| t.name == name).expect(name);
        tool.description.as_deref().unwrap_or_default().to_string()
    };
    for name in ["chain_feed_status", "tx_word", "health", "pull", "register"] {
        let description = described(name);
        assert!(
            [
                "proof",
                "hint",
                "nothing about the chain",
                "checked against the headers"
            ]
            .iter()
            .any(|w| description.contains(w)),
            "{name}: {description}"
        );
    }
    for (name, route) in [
        ("list_origins", "GET /origins"),
        ("answer_origin", "POST /origins/answer"),
        ("revoke_origin", "POST /origins/revoke"),
    ] {
        let description = described(name);
        assert!(description.contains(route), "{name}: {description}");
    }
    let answer = tools.iter().find(|t| t.name == "answer_origin").unwrap();
    let schema = serde_json::to_value(&*answer.input_schema).unwrap();
    let required = schema["required"].as_array().unwrap();
    assert_eq!(required, &[json!("pendingId"), json!("answer")]);
    let text = schema.to_string();
    for word in [r#""grant""#, r#""once""#, r#""deny""#] {
        assert!(text.contains(word), "{word} in {text}");
    }
}

/// The owner's three routes, each answering the call as it came: its
/// method, path, `Origin` (or null), `Authorization` (or null) and body.
async fn owner_stub() -> String {
    async fn echo(request: axum::extract::Request) -> Json<Value> {
        let (parts, body) = request.into_parts();
        let header = |name: &str| {
            parts
                .headers
                .get(name)
                .map(|v| v.to_str().unwrap().to_string())
        };
        let bytes = axum::body::to_bytes(body, usize::MAX).await.unwrap();
        let body: Value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
        Json(json!({
            "method": parts.method.as_str(),
            "path": parts.uri.path(),
            "origin": header("origin"),
            "authorization": header("authorization"),
            "body": body,
        }))
    }
    let app = Router::new()
        .route("/origins", get(echo))
        .route("/origins/answer", post(echo))
        .route("/origins/revoke", post(echo));
    serve(app).await
}

/// The call the owner's stub echoes: `method` and `path`, no `Origin`,
/// `bearer` (or none) and `body`.
fn echoed(method: &str, path: &str, bearer: Option<&str>, body: &Value) -> Value {
    let authorization = bearer.map(|t| format!("Bearer {t}"));
    json!({"method": method, "path": path, "origin": null,
           "authorization": authorization, "body": body})
}

/// `revoke_origin` of `app.example`: `kind` and `key`.
fn revoking(kind: Option<OriginKind>, key: Option<&str>) -> Parameters<RevokeOriginParams> {
    Parameters(RevokeOriginParams {
        originator: "app.example".into(),
        kind,
        key: key.map(str::to_string),
    })
}

#[tokio::test]
async fn the_owners_tools_send_no_origin_and_the_wallet_bearer() {
    let base = owner_stub().await;
    let mcp = WalletMcp::new(base, "http://localhost".into())
        .with_wallet_bearer(Some("owner-token".into()));
    let bearer = Some("owner-token");

    let listed = text(mcp.list_origins().await.unwrap());
    assert_eq!(listed, echoed("GET", "/origins", bearer, &Value::Null));

    let answering = Parameters(AnswerOriginParams {
        pending_id: 7,
        answer: OriginAnswer::Grant,
        expiry: None,
        amount: Some(60_000),
    });
    let answered = text(mcp.answer_origin(answering).await.unwrap());
    let body = json!({"pendingId": 7, "answer": "grant", "amount": 60000});
    assert_eq!(answered, echoed("POST", "/origins/answer", bearer, &body));

    let one = revoking(Some(OriginKind::Basket), Some(r#"["todo items"]"#));
    let revoked = text(mcp.revoke_origin(one).await.unwrap());
    let body = json!({"originator": "app.example", "kind": "basket", "key": "[\"todo items\"]"});
    assert_eq!(revoked, echoed("POST", "/origins/revoke", bearer, &body));

    // Each kind is its word; a kind alone sends no key.
    for (kind, word) in [
        (OriginKind::Protocol, "protocol"),
        (OriginKind::Basket, "basket"),
        (OriginKind::Certificate, "certificate"),
        (OriginKind::Spending, "spending"),
    ] {
        let revoked = text(mcp.revoke_origin(revoking(Some(kind), None)).await.unwrap());
        let body = json!({"originator": "app.example", "kind": word});
        assert_eq!(revoked, echoed("POST", "/origins/revoke", bearer, &body));
    }
}

#[tokio::test]
async fn without_wallet_auth_token_the_owners_tools_send_no_bearer() {
    let base = owner_stub().await;
    // An empty token is none; still no Origin. No kind and no key are
    // absent from the body (the route revokes every row).
    let tokenless =
        WalletMcp::new(base, "http://localhost".into()).with_wallet_bearer(Some(String::new()));
    let revoked = text(tokenless.revoke_origin(revoking(None, None)).await.unwrap());
    let body = json!({"originator": "app.example"});
    assert_eq!(revoked, echoed("POST", "/origins/revoke", None, &body));
    let listed = text(tokenless.list_origins().await.unwrap());
    assert_eq!(listed, echoed("GET", "/origins", None, &Value::Null));
}

// ── The configuration (#171) ───────────────────────────────────

use config::{Source, DEFAULT_WALLET_ORIGIN, DEFAULT_WALLET_URL};

fn config(pairs: &[(&str, &str)]) -> Result<Config, config::Refused> {
    Config::from_vars(
        pairs
            .iter()
            .map(|(k, v)| ((*k).to_string(), (*v).to_string())),
    )
}

/// The `--config` row of `name`, split on whitespace.
fn row(table: &str, name: &str) -> Vec<String> {
    table
        .lines()
        .find(|l| l.split_whitespace().next() == Some(name))
        .unwrap_or_else(|| panic!("no {name} row in {table}"))
        .split_whitespace()
        .map(str::to_string)
        .collect()
}

#[test]
fn an_empty_environment_is_the_defaults() {
    let c = config(&[("PATH", "/bin"), ("ROOT_KEY", "not ours")]).unwrap();
    assert_eq!(c.wallet_url, DEFAULT_WALLET_URL);
    assert_eq!(c.wallet_url, "http://localhost:3322");
    assert_eq!(c.wallet_origin, DEFAULT_WALLET_ORIGIN);
    assert_eq!(c.wallet_origin, "http://localhost");
    assert_eq!(c.relay_url, None);
    assert_eq!(c.relay_admin_token, None);
    assert_eq!(c.rust_log, None);
    let table = c.table();
    assert_eq!(
        row(&table, "WALLET_URL"),
        ["WALLET_URL", "http://localhost:3322", "default"]
    );
    assert_eq!(
        row(&table, "WALLET_ORIGIN"),
        ["WALLET_ORIGIN", "http://localhost", "default"]
    );
    assert_eq!(row(&table, "RELAY_URL"), ["RELAY_URL", "-", "default"]);
    assert_eq!(
        row(&table, "WALLET_AUTH_TOKEN"),
        ["WALLET_AUTH_TOKEN", "-", "default"]
    );
    assert_eq!(
        row(&table, "RELAY_ADMIN_TOKEN"),
        ["RELAY_ADMIN_TOKEN", "-", "default"]
    );
    assert_eq!(
        row(&table, "RUST_LOG"),
        ["RUST_LOG", "error,bsv_wallet_mcp=info", "default"]
    );
    assert!(
        !table.contains("ROOT_KEY") && !table.contains("PATH"),
        "{table}"
    );
    assert_eq!(table.lines().count(), 1 + config::NAMES.len());
}

#[test]
fn each_variable_set_is_read_from_the_environment() {
    let c = config(&[
        ("WALLET_URL", "http://127.0.0.1:3370"),
        ("WALLET_ORIGIN", "http://e2e-test"),
        ("WALLET_AUTH_TOKEN", "wallet-bearer-bytes"),
        ("RELAY_URL", "http://127.0.0.1:3390/"),
        ("RELAY_ADMIN_TOKEN", "admin-token-bytes"),
        ("RUST_LOG", "debug"),
    ])
    .unwrap();
    assert_eq!(c.wallet_url, "http://127.0.0.1:3370");
    assert_eq!(c.wallet_origin, "http://e2e-test");
    assert_eq!(c.relay_url.as_deref(), Some("http://127.0.0.1:3390/"));
    assert_eq!(
        c.relay_admin_token.as_ref().map(config::Secret::expose),
        Some("admin-token-bytes")
    );
    assert_eq!(c.rust_log.as_deref(), Some("debug"));
    for name in config::NAMES {
        assert_eq!(c.source(name), Source::Env, "{name}");
    }
    let table = c.table();
    assert_eq!(
        row(&table, "RELAY_ADMIN_TOKEN"),
        ["RELAY_ADMIN_TOKEN", "<set>", "env"]
    );
    assert_eq!(
        row(&table, "RUST_LOG"),
        ["RUST_LOG", "debug,bsv_wallet_mcp=info", "env"]
    );

    // The server is built from it as `main` builds it: the relay without
    // its trailing slash, the bearer passed through.
    let mcp = WalletMcp::from_config(&c);
    assert_eq!(
        (mcp.url.as_str(), mcp.origin.as_str()),
        ("http://127.0.0.1:3370", "http://e2e-test")
    );
    assert_eq!(mcp.relay_url.as_deref(), Some("http://127.0.0.1:3390"));
    assert_eq!(mcp.relay_admin_token.as_deref(), Some("admin-token-bytes"));
    assert_eq!(
        mcp.wallet_auth_token.as_deref(),
        Some("wallet-bearer-bytes")
    );
    assert_eq!(
        row(&table, "WALLET_AUTH_TOKEN"),
        ["WALLET_AUTH_TOKEN", "<set>", "env"]
    );
}

#[test]
fn an_empty_relay_url_is_no_relay_and_an_empty_token_none() {
    let c = config(&[
        ("RELAY_URL", ""),
        ("RELAY_ADMIN_TOKEN", ""),
        ("WALLET_AUTH_TOKEN", ""),
    ])
    .unwrap();
    assert_eq!(c.relay_url, None);
    assert_eq!(c.relay_admin_token, None);
    assert_eq!(c.wallet_auth_token, None);
    assert_eq!(c.source("RELAY_URL"), Source::Env);
    let table = c.table();
    assert_eq!(row(&table, "RELAY_URL"), ["RELAY_URL", "-", "env"]);
    assert_eq!(
        row(&table, "RELAY_ADMIN_TOKEN"),
        ["RELAY_ADMIN_TOKEN", "empty", "env"]
    );
    assert_eq!(
        row(&table, "WALLET_AUTH_TOKEN"),
        ["WALLET_AUTH_TOKEN", "empty", "env"]
    );
    let mcp = WalletMcp::from_config(&c);
    assert_eq!(mcp.wallet_auth_token, None);
    assert_eq!((mcp.relay_url, mcp.relay_admin_token), (None, None));
}

#[test]
fn the_printed_configuration_never_holds_a_credential() {
    let token = "s3cr3t-admin-token-never-printed";
    let bearer = "s3cr3t-wallet-bearer-never-printed";
    let c = config(&[
        ("WALLET_AUTH_TOKEN", bearer),
        (
            "WALLET_URL",
            "http://user:pass-in-url@127.0.0.1:3370/?key=q-secret",
        ),
        ("RELAY_URL", "https://relay-user:relay-pass@relay.test/"),
        ("RELAY_ADMIN_TOKEN", token),
    ])
    .unwrap();
    let table = c.table();
    for bytes in [token, bearer, "pass-in-url", "q-secret", "relay-pass"] {
        assert!(!table.contains(bytes), "{bytes} in {table}");
    }
    assert!(!format!("{c:?}").contains(token));
    assert!(!format!("{c:?}").contains(bearer));
    assert_eq!(
        row(&table, "WALLET_URL"),
        ["WALLET_URL", "http://<set>@127.0.0.1:3370/?<set>", "env"]
    );
    assert_eq!(
        row(&table, "RELAY_URL"),
        ["RELAY_URL", "https://<set>@relay.test/", "env"]
    );
}

#[test]
fn a_malformed_value_is_refused_by_name_and_every_one_together() {
    let config::Refused { errors, config: c } = config(&[
        ("WALLET_URL", "localhost:3322"),
        ("WALLET_ORIGIN", " "),
        ("RELAY_URL", "ftp://user:pw@relay.test"),
        ("RELAY_ADMIN_TOKEN", "admin"),
    ])
    .unwrap_err();
    let c = *c;
    let names: Vec<_> = errors.iter().map(|e| e.variable).collect();
    assert_eq!(names, ["WALLET_URL", "WALLET_ORIGIN", "RELAY_URL"]);
    assert_eq!(
        errors[0].to_string(),
        "WALLET_URL is \"localhost:3322\", which is not an http or https URL: \
         set it so, or unset it for the default"
    );
    assert!(!errors[2].to_string().contains("pw"), "{}", errors[2]);
    // `--config` still prints, each refused setting at its default and
    // marked, the rest as read.
    assert_eq!(c.wallet_url, DEFAULT_WALLET_URL);
    assert_eq!(c.relay_url, None);
    assert_eq!(c.refused(), &errors[..]);
    let table = c.table();
    assert_eq!(
        row(&table, "RELAY_URL"),
        [
            "RELAY_URL",
            "ftp://<set>@relay.test",
            "env",
            "REFUSED:",
            "not",
            "an",
            "http",
            "or",
            "https",
            "URL"
        ]
    );
    assert_eq!(
        row(&table, "RELAY_ADMIN_TOKEN"),
        ["RELAY_ADMIN_TOKEN", "<set>", "env"]
    );
}

/// The six certificate tools carry the beta.2 JSON, including the keys
/// direct acquisition and selective proof need. Discovery is the daemon's
/// door (#331): the stub's answer passes through.
#[tokio::test]
async fn certificate_tools_carry_the_published_shape() {
    let (base, task) = certificate_stub().await;
    let mcp = WalletMcp::new(base, "http://localhost".into());
    let body = json!({"type": "BgYGBgYGBgYGBgYGBgYGBgYGBgYGBgYGBgYGBgYGBgY=", "certifier": "key",
        "serialNumber": "serial", "signature": "3006020101020101", "revocationOutpoint": "outpoint",
        "fields": {"name": "d2lyZQ=="}, "keyringRevealer": "certifier", "keyringForSubject": {"name": "a2V5"},
        "acquisitionProtocol": "direct"});
    let acquired = text(
        mcp.acquire_certificate(Parameters(serde_json::from_value(body.clone()).unwrap()))
            .await
            .unwrap(),
    );
    assert_eq!(acquired, body);
    let proof = json!({"certificate": body, "verifier": "key", "fieldsToReveal": ["name"]});
    assert_eq!(
        text(
            mcp.prove_certificate(Parameters(serde_json::from_value(proof.clone()).unwrap()))
                .await
                .unwrap()
        ),
        proof
    );
    let listed = text(
        mcp.list_certificates(Parameters(serde_json::from_value(json!({})).unwrap()))
            .await
            .unwrap(),
    );
    assert_eq!(listed["limit"], 10);
    let relinquished = text(
        mcp.relinquish_certificate(Parameters(
            serde_json::from_value(
                json!({"type": "type", "serialNumber": "serial", "certifier": "key"}),
            )
            .unwrap(),
        ))
        .await
        .unwrap(),
    );
    assert_eq!(relinquished["type"], "type");
    for found in [
        mcp.discover_by_identity_key(Parameters(
            serde_json::from_value(json!({"identityKey": "key"})).unwrap(),
        ))
        .await
        .unwrap(),
        mcp.discover_by_attributes(Parameters(
            serde_json::from_value(json!({"attributes": {"name": "wire"}})).unwrap(),
        ))
        .await
        .unwrap(),
    ] {
        assert_eq!(
            text(found),
            json!({"totalCertificates": 0, "certificates": []})
        );
    }
    task.abort();
}

async fn certificate_stub() -> (String, tokio::task::JoinHandle<()>) {
    let app = Router::new()
        .route(
            "/acquireCertificate",
            post(|Json(body): Json<Value>| async { Json(body) }),
        )
        .route(
            "/listCertificates",
            post(|Json(body): Json<Value>| async { Json(body) }),
        )
        .route(
            "/proveCertificate",
            post(|Json(body): Json<Value>| async { Json(body) }),
        )
        .route(
            "/relinquishCertificate",
            post(|Json(body): Json<Value>| async { Json(body) }),
        )
        .route(
            "/discoverByIdentityKey",
            post(|| async { Json(json!({"totalCertificates": 0, "certificates": []})) }),
        )
        .route(
            "/discoverByAttributes",
            post(|| async { Json(json!({"totalCertificates": 0, "certificates": []})) }),
        );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let task = tokio::spawn(async { axum::serve(listener, app).await.unwrap() });
    (base, task)
}
