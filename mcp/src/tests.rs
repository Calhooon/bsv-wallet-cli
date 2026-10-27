//! The five host-route tools against an in-process stub of the wallet's
//! and the relay's routes: their method, path, query, body and bearer as
//! `src/server/mod.rs` and `src/relay.rs` define them. Nothing reaches the
//! network.

use super::*;
use axum::extract::Query;
use axum::http::{HeaderMap, StatusCode};
use axum::routing::{get, post};
use axum::{Json, Router};
use std::collections::HashMap;

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
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.ok();
    });
    format!("http://{addr}")
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
fn the_tools_are_the_29_and_the_five() {
    let tools = WalletMcp::tool_router().list_all();
    assert_eq!(tools.len(), 34);
    for name in ["chain_feed_status", "tx_word", "health", "pull", "register"] {
        let tool = tools.iter().find(|t| t.name == name).expect(name);
        let description = tool.description.as_deref().unwrap_or_default();
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
}

#[test]
fn an_empty_relay_url_is_no_relay_and_an_empty_token_none() {
    let c = config(&[("RELAY_URL", ""), ("RELAY_ADMIN_TOKEN", "")]).unwrap();
    assert_eq!(c.relay_url, None);
    assert_eq!(c.relay_admin_token, None);
    assert_eq!(c.source("RELAY_URL"), Source::Env);
    let table = c.table();
    assert_eq!(row(&table, "RELAY_URL"), ["RELAY_URL", "-", "env"]);
    assert_eq!(
        row(&table, "RELAY_ADMIN_TOKEN"),
        ["RELAY_ADMIN_TOKEN", "empty", "env"]
    );
    let mcp = WalletMcp::from_config(&c);
    assert_eq!((mcp.relay_url, mcp.relay_admin_token), (None, None));
}

#[test]
fn the_printed_configuration_never_holds_a_credential() {
    let token = "s3cr3t-admin-token-never-printed";
    let c = config(&[
        (
            "WALLET_URL",
            "http://user:pass-in-url@127.0.0.1:3370/?key=q-secret",
        ),
        ("RELAY_URL", "https://relay-user:relay-pass@relay.test/"),
        ("RELAY_ADMIN_TOKEN", token),
    ])
    .unwrap();
    let table = c.table();
    for bytes in [token, "pass-in-url", "q-secret", "relay-pass"] {
        assert!(!table.contains(bytes), "{bytes} in {table}");
    }
    assert!(!format!("{c:?}").contains(token));
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
