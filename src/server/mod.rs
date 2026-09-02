pub mod audit;
pub mod broadcast_follow_up;
pub mod handlers;
pub mod types;

use anyhow::Result;
use axum::extract::{DefaultBodyLimit, Request};
use axum::http::{HeaderMap, StatusCode};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use bsv_wallet_toolbox::{Chain, Services, StorageSqlx, Wallet};
use serde_json::json;
use std::net::SocketAddr;
use std::sync::Arc;
use tower_http::cors::CorsLayer;
use tower_http::trace::TraceLayer;

use handlers::WalletState;

/// Lock that serializes spending operations (createAction).
/// Non-spending endpoints (encrypt, getPublicKey, listOutputs, etc.) are unaffected.
///
/// Why: With limited UTXOs, concurrent createAction calls race on SQLite's write lock
/// and the loser gets SQLITE_BUSY_SNAPSHOT instead of waiting for the winner's change
/// output. This lock queues them so each spending request sees the previous one's change.
pub type SpendingLock = Arc<tokio::sync::Mutex<()>>;

/// TLS configuration (cert + key paths).
#[derive(Clone)]
#[allow(dead_code)]
pub struct TlsConfig {
    pub cert_path: String,
    pub key_path: String,
}

/// Server configuration (auth, TLS, etc.)
#[derive(Clone)]
pub struct ServerConfig {
    /// Optional bearer token. When set, all requests must include
    /// `Authorization: Bearer <token>`. When None, auth is disabled.
    pub auth_token: Option<String>,
    /// Optional TLS config. Requires `--features tls` at build time.
    pub tls: Option<TlsConfig>,
    /// Network the served wallet is on. Drives post-broadcast verification
    /// (which ARC / WoC endpoints to probe). Defaults to `Main`.
    pub chain: Chain,
    /// Address to bind (default `127.0.0.1`). Set `BIND_ADDR=0.0.0.0` for the
    /// tunnel/public-webhook case (`POST /arc-callback` behind cloudflared /
    /// tailscale funnel / direct TLS).
    pub bind_addr: std::net::IpAddr,
    /// Per-wallet ARC/Arcade callback token. When set, `POST /arc-callback`
    /// is enabled, authenticated by THIS token (`Authorization: Bearer` or
    /// `X-CallbackToken`) and EXEMPT from the wallet bearer auth above.
    pub callback_token: Option<String>,
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            auth_token: None,
            tls: None,
            chain: Chain::Main,
            bind_addr: std::net::IpAddr::from([127, 0, 0, 1]),
            callback_token: None,
        }
    }
}

/// Auth middleware — checks Bearer token if configured.
async fn auth_middleware(headers: HeaderMap, request: Request, next: Next) -> Response {
    // /arc-callback is EXEMPT from wallet bearer auth: it is authenticated by
    // the per-wallet callback token inside its own handler (ARC/Arcade call it
    // with `Authorization: Bearer <callback-token>`, not the wallet token).
    if request.uri().path() == "/arc-callback" {
        return next.run(request).await;
    }

    // Extract config from request extensions
    let token = request
        .extensions()
        .get::<ServerConfig>()
        .and_then(|c| c.auth_token.clone());

    if let Some(expected) = token {
        let provided = headers
            .get("authorization")
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.strip_prefix("Bearer "));

        match provided {
            Some(t) if t == expected => {}
            _ => {
                return (
                    StatusCode::UNAUTHORIZED,
                    Json(json!({"code": "UNAUTHORIZED", "message": "Invalid or missing bearer token"})),
                )
                    .into_response();
            }
        }
    }

    next.run(request).await
}

/// Build the axum Router with all 28 WalletInterface endpoints.
pub fn make_router(wallet: WalletState, config: ServerConfig) -> Router {
    let cors = CorsLayer::very_permissive();
    let cfg = config.clone();
    let spending_lock: SpendingLock = Arc::new(tokio::sync::Mutex::new(()));
    // Post-broadcast verifier — lets `/createAction` fail loudly when ARC
    // silently dropped an immediate broadcast (e.g. 465 fee-too-low on a deep
    // unconfirmed BEEF) instead of returning a phantom txid.
    let verifier = crate::broadcast_verify::BroadcastVerifier::from_env(config.chain);

    Router::new()
        // Existing 5 endpoints
        // Status endpoints accept BOTH GET and POST: @bsv/sdk's HTTPWalletJSON substrate
        // POSTs every method (incl. these), while curl/CLI clients GET them. Supporting
        // both makes the server wire-compatible with browser WalletClient connections.
        .route(
            "/isAuthenticated",
            get(handlers::is_authenticated).post(handlers::is_authenticated),
        )
        .route("/getPublicKey", post(handlers::get_public_key))
        // PERMISSION AUDIT (not BRC-100): what a wallet with a human behind it
        // would have prompted for. See `audit.rs` — this daemon grants everything
        // silently, which is what makes it headless AND blind.
        .route("/permission-audit", get(permission_audit))
        .route("/permission-audit/reset", post(permission_audit_reset))
        .route("/createSignature", post(handlers::create_signature))
        .route("/createAction", post(handlers::create_action))
        .route("/internalizeAction", post(handlers::internalize_action))
        // Batch 1: Status (GET + POST for HTTPWalletJSON compat)
        .route(
            "/getHeight",
            get(handlers::get_height).post(handlers::get_height),
        )
        .route(
            "/getNetwork",
            get(handlers::get_network).post(handlers::get_network),
        )
        .route(
            "/getVersion",
            get(handlers::get_version).post(handlers::get_version),
        )
        .route(
            "/waitForAuthentication",
            get(handlers::wait_for_authentication).post(handlers::wait_for_authentication),
        )
        // Batch 2: Header
        .route("/getHeaderForHeight", post(handlers::get_header_for_height))
        // Batch 3: Crypto
        .route("/verifySignature", post(handlers::verify_signature))
        .route("/encrypt", post(handlers::encrypt))
        .route("/decrypt", post(handlers::decrypt))
        .route("/createHmac", post(handlers::create_hmac))
        .route("/verifyHmac", post(handlers::verify_hmac))
        // Batch 4: Transaction workflow
        .route("/signAction", post(handlers::sign_action))
        .route("/abortAction", post(handlers::abort_action))
        .route("/listActions", post(handlers::list_actions))
        .route("/listOutputs", post(handlers::list_outputs))
        .route("/relinquishOutput", post(handlers::relinquish_output))
        // Batch 5: Certificates
        .route("/acquireCertificate", post(handlers::acquire_certificate))
        .route("/listCertificates", post(handlers::list_certificates))
        .route("/proveCertificate", post(handlers::prove_certificate))
        .route(
            "/relinquishCertificate",
            post(handlers::relinquish_certificate),
        )
        // Batch 6: Discovery + Key linkage
        .route(
            "/discoverByIdentityKey",
            post(handlers::discover_by_identity_key),
        )
        .route(
            "/discoverByAttributes",
            post(handlers::discover_by_attributes),
        )
        .route(
            "/revealCounterpartyKeyLinkage",
            post(handlers::reveal_counterparty_key_linkage),
        )
        .route(
            "/revealSpecificKeyLinkage",
            post(handlers::reveal_specific_key_linkage),
        )
        // ARC/Arcade proof-delivery webhook (callback-token auth, exempt from
        // wallet bearer auth — see auth_middleware).
        .route("/arc-callback", post(arc_callback))
        // Layer ordering: CORS (outermost) → auth → trace → body limit
        .layer(cors)
        .layer(middleware::from_fn(auth_middleware))
        .layer(axum::Extension(cfg))
        .layer(axum::Extension(spending_lock))
        .layer(axum::Extension(verifier))
        .layer(TraceLayer::new_for_http())
        .layer(DefaultBodyLimit::max(50 * 1024 * 1024))
        .with_state(wallet)
}

/// `POST /arc-callback` — ARC/Arcade status webhook receiving push status
/// updates and (on MINED) the merkle path, straight into wallet storage.
///
/// Authenticated by the per-wallet callback token: ARC-convention
/// `Authorization: Bearer <callback-token>` or an `X-CallbackToken` header
/// (both accepted — broadcaster implementations vary). Returns 404 when no
/// callback token is configured (route effectively disabled).
async fn arc_callback(
    axum::extract::State(wallet): axum::extract::State<WalletState>,
    request: Request,
) -> Response {
    let expected = request
        .extensions()
        .get::<ServerConfig>()
        .and_then(|c| c.callback_token.clone());
    let Some(expected) = expected else {
        return (
            StatusCode::NOT_FOUND,
            Json(json!({"code": "CALLBACK_DISABLED", "message": "no callback token configured"})),
        )
            .into_response();
    };

    let headers = request.headers().clone();
    let provided = headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .map(str::to_string)
        .or_else(|| {
            headers
                .get("x-callbacktoken")
                .and_then(|v| v.to_str().ok())
                .map(str::to_string)
        });

    if provided.as_deref() != Some(expected.as_str()) {
        // Arcade authenticates its webhook POSTs as `Authorization: Bearer
        // <X-CallbackToken>` — a 401 here means a token mismatch and Arcade
        // will burn its (~10) retries, then permanently drop the submission.
        tracing::warn!(
            has_authorization = headers.contains_key("authorization"),
            has_x_callbacktoken = headers.contains_key("x-callbacktoken"),
            "arc-callback: rejected POST with invalid/missing callback token"
        );
        return (
            StatusCode::UNAUTHORIZED,
            Json(json!({"code": "UNAUTHORIZED", "message": "invalid or missing callback token"})),
        )
            .into_response();
    }

    let bytes = match axum::body::to_bytes(request.into_body(), 1_000_000).await {
        Ok(b) => b,
        Err(_) => {
            return (
                StatusCode::PAYLOAD_TOO_LARGE,
                Json(json!({"code": "TOO_LARGE", "message": "payload too large"})),
            )
                .into_response();
        }
    };
    let payload: serde_json::Value = match serde_json::from_slice(&bytes) {
        Ok(v) => v,
        Err(e) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({"code": "BAD_JSON", "message": e.to_string()})),
            )
                .into_response();
        }
    };

    match crate::arc_ingest::ingest_arc_payload(wallet.storage(), &payload).await {
        Ok(action) => (
            StatusCode::OK,
            Json(json!({"ok": true, "action": format!("{:?}", action)})),
        )
            .into_response(),
        Err(e) => (
            StatusCode::BAD_REQUEST,
            Json(json!({"code": "BAD_PAYLOAD", "message": e.to_string()})),
        )
            .into_response(),
    }
}

pub async fn run(wallet: WalletState, port: u16, config: ServerConfig) -> Result<()> {
    let tls = config.tls.clone();
    let bind_addr = config.bind_addr;
    let app = make_router(wallet, config);
    let addr = SocketAddr::from((bind_addr, port));

    #[cfg(feature = "tls")]
    if let Some(tls_cfg) = tls {
        use axum_server::tls_rustls::RustlsConfig;
        let rustls = RustlsConfig::from_pem_file(&tls_cfg.cert_path, &tls_cfg.key_path).await?;
        tracing::info!("HTTPS server listening on {addr}");
        eprintln!("HTTPS server listening on {addr}");
        axum_server::bind_rustls(addr, rustls)
            .serve(app.into_make_service())
            .await?;
        return Ok(());
    }

    #[cfg(not(feature = "tls"))]
    if tls.is_some() {
        anyhow::bail!("TLS requested but binary was built without `--features tls`");
    }

    tracing::info!("HTTP server listening on {addr}");
    eprintln!("HTTP server listening on {addr}");

    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await?;

    Ok(())
}

pub fn make_wallet_state(wallet: Wallet<StorageSqlx, Services>) -> WalletState {
    Arc::new(wallet)
}

/// Build a `WalletState` from an existing `Arc<Wallet>`.
///
/// Use this when the caller is already holding an `Arc<Wallet>` and wants the
/// HTTP server to share the *same* wallet instance — same identity key, same
/// balance, same UTXO set. Avoids opening a second `Wallet` handle against
/// the same SQLite DB (which would work via busy_timeout but adds write
/// contention and a second instance of any in-memory caches).
#[allow(dead_code)] // public API for external callers; not used inside this binary
pub fn make_wallet_state_from_arc(wallet: Arc<Wallet<StorageSqlx, Services>>) -> WalletState {
    wallet
}

async fn shutdown_signal() {
    tokio::signal::ctrl_c()
        .await
        .expect("failed to listen for ctrl-c");
    eprintln!("\nShutting down...");
}

/// GET /permission-audit — every permissioned request since start or reset.
async fn permission_audit() -> Json<serde_json::Value> {
    let entries = audit::snapshot();
    Json(json!({ "count": entries.len(), "entries": entries }))
}

/// POST /permission-audit/reset — call immediately before the flow you measure.
async fn permission_audit_reset() -> Json<serde_json::Value> {
    audit::reset();
    Json(json!({ "ok": true }))
}
