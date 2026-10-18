//! The five host-route tools against an in-process stub of the wallet's
//! and the relay's routes: their method, path, query, body and bearer as
//! `src/server/mod.rs` and `src/relay.rs` define them. Nothing reaches the
//! network.

use super::*;
use axum::extract::{Path, Query};
use axum::http::{HeaderMap, StatusCode};
use axum::routing::{get, post};
use axum::{Json, Router};
use std::collections::HashMap;

const TXID: &str = "abababababababababababababababababababababababababababababababab";

async fn stub() -> String {
    let app = Router::new()
        .route(
            "/chain-feed",
            get(|| async { Json(json!({"status": {"state": "following"}, "cursor": "42"})) }),
        )
        .route(
            "/tx/{txid}",
            get(|Path(txid): Path<String>| async move {
                Json(json!({"txid": txid, "word": "mined", "height": 900}))
            }),
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
            txid: TXID.to_uppercase(),
        }))
        .await
        .unwrap(),
    );
    assert_eq!(
        (word["txid"].as_str(), word["word"].as_str()),
        (Some(TXID), Some("mined"))
    );
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
