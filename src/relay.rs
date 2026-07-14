//! bsv-wallet-relay — store-and-forward ARC/Arcade callback receiver.
//!
//! ONE public callback receiver on a box; multiple wallet daemons (which may
//! be localhost-only) drain their own queue OUTBOUND by callback token:
//!
//! ```text
//! Arcade ──POST /arc-callback (Bearer <token>)──▶ relay (sqlite queue, per token)
//! wallet ──GET  /pull?token=<token>&after=<id>──▶ relay (returns queued payloads)
//! wallet ──GET  /pull?token=..&after=N&ack=N   ─▶ relay (deletes acked rows)
//! ```
//!
//! # Token registration (simpler-but-safe design, documented trade-off)
//!
//! A token is auto-registered the first time a wallet pulls with it — wallets
//! start polling at daemon boot, before any submit, so the token is always
//! registered before Arcade can call back. Callbacks with unknown tokens are
//! rejected (401) so the relay never stores unsolicited data. Tokens are
//! high-entropy (32 hex), so pulling with a guessed token yields nothing
//! (an empty, freshly-registered queue) and cannot read another wallet's
//! payloads. Optionally, `POST /register {"token": "..."}` with
//! `Authorization: Bearer <RELAY_ADMIN_TOKEN>` pre-registers tokens and
//! `RELAY_REQUIRE_REGISTER=1` disables auto-registration entirely.
//!
//! # Endpoints
//!
//! | Route | Auth | Behavior |
//! |-------|------|----------|
//! | `GET /health` | none | `{"healthy":true}` |
//! | `POST /arc-callback` | callback token (Bearer or `X-CallbackToken`) | queue payload for that token |
//! | `GET /pull?token=&after=&ack=` | token in query | return payloads with `id > after` (max 100); delete `id <= ack` first |
//! | `POST /register` | `RELAY_ADMIN_TOKEN` bearer | register `{"token": "..."}` |

use axum::extract::{Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::json;
use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};
use sqlx::SqlitePool;
use std::sync::Arc;

/// Shared relay state.
#[derive(Clone)]
pub struct RelayState {
    /// Payload queue database.
    pub pool: SqlitePool,
    /// Admin bearer for `POST /register` (None disables the route).
    pub admin_token: Option<Arc<String>>,
    /// When false, `/pull` with an unknown token auto-registers it.
    pub require_register: bool,
}

/// Open (and migrate) the relay sqlite database.
pub async fn open_relay_db(path: &str) -> anyhow::Result<SqlitePool> {
    let opts = SqliteConnectOptions::new()
        .filename(path)
        .create_if_missing(true);
    let pool = SqlitePoolOptions::new()
        .max_connections(5)
        .connect_with(opts)
        .await?;

    sqlx::query(
        r#"
        CREATE TABLE IF NOT EXISTS tokens (
            token       TEXT PRIMARY KEY,
            created_at  TEXT NOT NULL
        )
        "#,
    )
    .execute(&pool)
    .await?;

    sqlx::query(
        r#"
        CREATE TABLE IF NOT EXISTS payloads (
            id          INTEGER PRIMARY KEY AUTOINCREMENT,
            token       TEXT NOT NULL,
            received_at TEXT NOT NULL,
            body        TEXT NOT NULL
        )
        "#,
    )
    .execute(&pool)
    .await?;

    sqlx::query("CREATE INDEX IF NOT EXISTS idx_payloads_token_id ON payloads (token, id)")
        .execute(&pool)
        .await?;

    Ok(pool)
}

/// Build the relay router.
pub fn make_relay_router(state: RelayState) -> Router {
    Router::new()
        .route("/health", get(health))
        .route("/arc-callback", post(arc_callback))
        .route("/pull", get(pull))
        .route("/register", post(register))
        .with_state(state)
}

async fn health() -> Json<serde_json::Value> {
    Json(json!({"healthy": true, "service": "bsv-wallet-relay"}))
}

/// Extract the callback token from `Authorization: Bearer <tok>` (the ARC
/// webhook convention) or an `X-CallbackToken` header (accepted for
/// compatibility — implementations vary).
fn extract_callback_token(headers: &HeaderMap) -> Option<String> {
    if let Some(auth) = headers.get("authorization").and_then(|v| v.to_str().ok()) {
        if let Some(tok) = auth.strip_prefix("Bearer ") {
            if !tok.is_empty() {
                return Some(tok.to_string());
            }
        }
    }
    headers
        .get("x-callbacktoken")
        .and_then(|v| v.to_str().ok())
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string())
}

async fn token_registered(pool: &SqlitePool, token: &str) -> Result<bool, sqlx::Error> {
    let row: Option<(String,)> = sqlx::query_as("SELECT token FROM tokens WHERE token = ?")
        .bind(token)
        .fetch_optional(pool)
        .await?;
    Ok(row.is_some())
}

async fn register_token(pool: &SqlitePool, token: &str) -> Result<(), sqlx::Error> {
    sqlx::query("INSERT OR IGNORE INTO tokens (token, created_at) VALUES (?, ?)")
        .bind(token)
        .bind(chrono::Utc::now().to_rfc3339())
        .execute(pool)
        .await?;
    Ok(())
}

fn err(status: StatusCode, code: &str, msg: &str) -> Response {
    (status, Json(json!({"code": code, "message": msg}))).into_response()
}

/// `POST /arc-callback` — queue a callback payload for its token.
async fn arc_callback(
    State(state): State<RelayState>,
    headers: HeaderMap,
    Json(payload): Json<serde_json::Value>,
) -> Response {
    let Some(token) = extract_callback_token(&headers) else {
        return err(
            StatusCode::UNAUTHORIZED,
            "NO_TOKEN",
            "missing callback token",
        );
    };

    match token_registered(&state.pool, &token).await {
        Ok(true) => {}
        Ok(false) => {
            // Unknown token: never store unsolicited data.
            return err(
                StatusCode::UNAUTHORIZED,
                "UNKNOWN_TOKEN",
                "token not registered",
            );
        }
        Err(e) => {
            tracing::error!(error = %e, "relay: token lookup failed");
            return err(
                StatusCode::INTERNAL_SERVER_ERROR,
                "DB_ERROR",
                "storage error",
            );
        }
    }

    let body = payload.to_string();
    if body.len() > 1_000_000 {
        return err(
            StatusCode::PAYLOAD_TOO_LARGE,
            "TOO_LARGE",
            "payload too large",
        );
    }

    match sqlx::query("INSERT INTO payloads (token, received_at, body) VALUES (?, ?, ?)")
        .bind(&token)
        .bind(chrono::Utc::now().to_rfc3339())
        .bind(&body)
        .execute(&state.pool)
        .await
    {
        Ok(_) => {
            let txid = payload.get("txid").and_then(|v| v.as_str()).unwrap_or("?");
            let status = payload
                .get("txStatus")
                .and_then(|v| v.as_str())
                .unwrap_or("?");
            tracing::info!(txid = %txid, tx_status = %status, "relay: payload queued");
            (StatusCode::OK, Json(json!({"ok": true}))).into_response()
        }
        Err(e) => {
            tracing::error!(error = %e, "relay: payload insert failed");
            err(
                StatusCode::INTERNAL_SERVER_ERROR,
                "DB_ERROR",
                "storage error",
            )
        }
    }
}

#[derive(Debug, Deserialize)]
struct PullQuery {
    token: String,
    /// Return payloads with `id > after` (default 0).
    #[serde(default)]
    after: i64,
    /// Delete payloads with `id <= ack` before returning (optional).
    ack: Option<i64>,
}

/// `GET /pull?token=<tok>&after=<id>[&ack=<id>]` — drain the queue.
async fn pull(State(state): State<RelayState>, Query(q): Query<PullQuery>) -> Response {
    if q.token.is_empty() {
        return err(StatusCode::BAD_REQUEST, "NO_TOKEN", "token required");
    }

    // Auto-register on first pull (unless RELAY_REQUIRE_REGISTER).
    match token_registered(&state.pool, &q.token).await {
        Ok(true) => {}
        Ok(false) if !state.require_register => {
            if let Err(e) = register_token(&state.pool, &q.token).await {
                tracing::error!(error = %e, "relay: auto-register failed");
                return err(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "DB_ERROR",
                    "storage error",
                );
            }
            tracing::info!("relay: auto-registered new token on first pull");
        }
        Ok(false) => {
            return err(
                StatusCode::UNAUTHORIZED,
                "UNKNOWN_TOKEN",
                "token not registered",
            );
        }
        Err(e) => {
            tracing::error!(error = %e, "relay: token lookup failed");
            return err(
                StatusCode::INTERNAL_SERVER_ERROR,
                "DB_ERROR",
                "storage error",
            );
        }
    }

    if let Some(ack) = q.ack {
        if let Err(e) = sqlx::query("DELETE FROM payloads WHERE token = ? AND id <= ?")
            .bind(&q.token)
            .bind(ack)
            .execute(&state.pool)
            .await
        {
            tracing::error!(error = %e, "relay: ack delete failed");
            return err(
                StatusCode::INTERNAL_SERVER_ERROR,
                "DB_ERROR",
                "storage error",
            );
        }
    }

    let rows: Result<Vec<(i64, String, String)>, sqlx::Error> = sqlx::query_as(
        "SELECT id, received_at, body FROM payloads WHERE token = ? AND id > ? ORDER BY id LIMIT 100",
    )
    .bind(&q.token)
    .bind(q.after)
    .fetch_all(&state.pool)
    .await;

    match rows {
        Ok(rows) => {
            let items: Vec<serde_json::Value> = rows
                .into_iter()
                .map(|(id, received_at, body)| {
                    let payload: serde_json::Value =
                        serde_json::from_str(&body).unwrap_or(serde_json::Value::Null);
                    json!({"id": id, "receivedAt": received_at, "payload": payload})
                })
                .collect();
            (StatusCode::OK, Json(json!(items))).into_response()
        }
        Err(e) => {
            tracing::error!(error = %e, "relay: pull query failed");
            err(
                StatusCode::INTERNAL_SERVER_ERROR,
                "DB_ERROR",
                "storage error",
            )
        }
    }
}

#[derive(Debug, Deserialize)]
struct RegisterBody {
    token: String,
}

/// `POST /register {"token": "..."}` with the admin bearer.
async fn register(
    State(state): State<RelayState>,
    headers: HeaderMap,
    Json(body): Json<RegisterBody>,
) -> Response {
    let Some(admin) = state.admin_token.as_ref() else {
        return err(
            StatusCode::NOT_FOUND,
            "REGISTER_DISABLED",
            "set RELAY_ADMIN_TOKEN to enable /register",
        );
    };

    let provided = headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "));
    if provided != Some(admin.as_str()) {
        return err(StatusCode::UNAUTHORIZED, "UNAUTHORIZED", "bad admin token");
    }

    if body.token.len() < 16 {
        return err(
            StatusCode::BAD_REQUEST,
            "WEAK_TOKEN",
            "token too short (min 16 chars)",
        );
    }

    match register_token(&state.pool, &body.token).await {
        Ok(()) => (StatusCode::OK, Json(json!({"ok": true}))).into_response(),
        Err(e) => {
            tracing::error!(error = %e, "relay: register failed");
            err(
                StatusCode::INTERNAL_SERVER_ERROR,
                "DB_ERROR",
                "storage error",
            )
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::Request;

    async fn test_state(
        require_register: bool,
        admin: Option<&str>,
    ) -> (RelayState, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("relay.db");
        let pool = open_relay_db(db.to_str().unwrap()).await.unwrap();
        (
            RelayState {
                pool,
                admin_token: admin.map(|s| Arc::new(s.to_string())),
                require_register,
            },
            dir,
        )
    }

    async fn send(router: &Router, req: Request<Body>) -> (StatusCode, serde_json::Value) {
        use tower::ServiceExt;
        let resp = router.clone().oneshot(req).await.unwrap();
        let status = resp.status();
        let bytes = axum::body::to_bytes(resp.into_body(), 1_000_000)
            .await
            .unwrap();
        let value: serde_json::Value =
            serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null);
        (status, value)
    }

    fn callback_req(token: &str, payload: &serde_json::Value) -> Request<Body> {
        Request::builder()
            .method("POST")
            .uri("/arc-callback")
            .header("authorization", format!("Bearer {}", token))
            .header("content-type", "application/json")
            .body(Body::from(payload.to_string()))
            .unwrap()
    }

    fn pull_req(token: &str, after: i64, ack: Option<i64>) -> Request<Body> {
        let mut uri = format!("/pull?token={}&after={}", token, after);
        if let Some(a) = ack {
            uri.push_str(&format!("&ack={}", a));
        }
        Request::builder()
            .method("GET")
            .uri(uri)
            .body(Body::empty())
            .unwrap()
    }

    #[tokio::test]
    async fn health_ok() {
        let (state, _dir) = test_state(false, None).await;
        let router = make_relay_router(state);
        let (status, body) = send(
            &router,
            Request::builder()
                .method("GET")
                .uri("/health")
                .body(Body::empty())
                .unwrap(),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["healthy"], true);
    }

    #[tokio::test]
    async fn queue_round_trip_with_ack() {
        let (state, _dir) = test_state(false, None).await;
        let router = make_relay_router(state);
        let tok = "a1b2c3d4e5f60718293a4b5c6d7e8f90";

        // 1. Wallet registers by pulling first (auto-register) — empty queue.
        let (status, body) = send(&router, pull_req(tok, 0, None)).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body.as_array().unwrap().len(), 0);

        // 2. Arcade posts two callbacks.
        let p1 = serde_json::json!({"txid": "aa".repeat(32), "txStatus": "SEEN_ON_NETWORK"});
        let p2 = serde_json::json!({"txid": "aa".repeat(32), "txStatus": "MINED",
            "blockHeight": 850000, "blockHash": "bb".repeat(32), "merklePath": "deadbeef"});
        let (s1, _) = send(&router, callback_req(tok, &p1)).await;
        let (s2, _) = send(&router, callback_req(tok, &p2)).await;
        assert_eq!(s1, StatusCode::OK);
        assert_eq!(s2, StatusCode::OK);

        // 3. Wallet drains the queue.
        let (status, body) = send(&router, pull_req(tok, 0, None)).await;
        assert_eq!(status, StatusCode::OK);
        let items = body.as_array().unwrap().clone();
        assert_eq!(items.len(), 2);
        assert_eq!(items[0]["payload"]["txStatus"], "SEEN_ON_NETWORK");
        assert_eq!(items[1]["payload"]["txStatus"], "MINED");
        let last_id = items[1]["id"].as_i64().unwrap();

        // 4. Ack deletes; nothing left after the acked id.
        let (status, body) = send(&router, pull_req(tok, last_id, Some(last_id))).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body.as_array().unwrap().len(), 0);

        // 5. Even pulling from 0 again returns nothing — rows deleted.
        let (_, body) = send(&router, pull_req(tok, 0, None)).await;
        assert_eq!(body.as_array().unwrap().len(), 0);
    }

    #[tokio::test]
    async fn callback_with_unknown_token_rejected() {
        let (state, _dir) = test_state(false, None).await;
        let router = make_relay_router(state);

        let p = serde_json::json!({"txid": "aa".repeat(32), "txStatus": "MINED"});
        let (status, body) = send(&router, callback_req("never-registered-token", &p)).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
        assert_eq!(body["code"], "UNKNOWN_TOKEN");
    }

    #[tokio::test]
    async fn callback_without_token_rejected() {
        let (state, _dir) = test_state(false, None).await;
        let router = make_relay_router(state);

        let req = Request::builder()
            .method("POST")
            .uri("/arc-callback")
            .header("content-type", "application/json")
            .body(Body::from("{}"))
            .unwrap();
        let (status, _) = send(&router, req).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn x_callback_token_header_accepted() {
        let (state, _dir) = test_state(false, None).await;
        let router = make_relay_router(state);
        let tok = "ffeeddccbbaa99887766554433221100";

        // register via pull
        let _ = send(&router, pull_req(tok, 0, None)).await;

        let p = serde_json::json!({"txid": "cc".repeat(32), "txStatus": "SEEN_ON_NETWORK"});
        let req = Request::builder()
            .method("POST")
            .uri("/arc-callback")
            .header("x-callbacktoken", tok)
            .header("content-type", "application/json")
            .body(Body::from(p.to_string()))
            .unwrap();
        let (status, _) = send(&router, req).await;
        assert_eq!(status, StatusCode::OK);

        let (_, body) = send(&router, pull_req(tok, 0, None)).await;
        assert_eq!(body.as_array().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn tokens_are_isolated() {
        let (state, _dir) = test_state(false, None).await;
        let router = make_relay_router(state);
        let tok_a = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
        let tok_b = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
        let _ = send(&router, pull_req(tok_a, 0, None)).await;
        let _ = send(&router, pull_req(tok_b, 0, None)).await;

        let p = serde_json::json!({"txid": "dd".repeat(32), "txStatus": "MINED"});
        let _ = send(&router, callback_req(tok_a, &p)).await;

        let (_, body_b) = send(&router, pull_req(tok_b, 0, None)).await;
        assert_eq!(
            body_b.as_array().unwrap().len(),
            0,
            "token B must not see token A's queue"
        );
        let (_, body_a) = send(&router, pull_req(tok_a, 0, None)).await;
        assert_eq!(body_a.as_array().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn require_register_blocks_unknown_pull() {
        let (state, _dir) = test_state(true, Some("admin-secret")).await;
        let router = make_relay_router(state);
        let tok = "1234567890abcdef1234567890abcdef";

        let (status, _) = send(&router, pull_req(tok, 0, None)).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);

        // Admin registers the token.
        let req = Request::builder()
            .method("POST")
            .uri("/register")
            .header("authorization", "Bearer admin-secret")
            .header("content-type", "application/json")
            .body(Body::from(serde_json::json!({"token": tok}).to_string()))
            .unwrap();
        let (status, _) = send(&router, req).await;
        assert_eq!(status, StatusCode::OK);

        let (status, _) = send(&router, pull_req(tok, 0, None)).await;
        assert_eq!(status, StatusCode::OK);
    }

    #[tokio::test]
    async fn register_requires_admin_bearer() {
        let (state, _dir) = test_state(true, Some("admin-secret")).await;
        let router = make_relay_router(state);

        let req = Request::builder()
            .method("POST")
            .uri("/register")
            .header("authorization", "Bearer wrong")
            .header("content-type", "application/json")
            .body(Body::from(
                serde_json::json!({"token": "1234567890abcdef"}).to_string(),
            ))
            .unwrap();
        let (status, _) = send(&router, req).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
    }
}
