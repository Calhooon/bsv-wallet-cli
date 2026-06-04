pub mod handlers;
pub mod types;

use anyhow::Result;
use axum::extract::{DefaultBodyLimit, Request};
use axum::http::{HeaderMap, StatusCode};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use bsv_wallet_toolbox::{Services, StorageSqlx, Wallet};
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
#[derive(Clone, Default)]
pub struct ServerConfig {
    /// Optional bearer token. When set, all requests must include
    /// `Authorization: Bearer <token>`. When None, auth is disabled.
    pub auth_token: Option<String>,
    /// Optional TLS config. Requires `--features tls` at build time.
    pub tls: Option<TlsConfig>,
}

/// Auth middleware — checks Bearer token if configured.
async fn auth_middleware(headers: HeaderMap, request: Request, next: Next) -> Response {
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
        // Layer ordering: CORS (outermost) → auth → trace → body limit
        .layer(cors)
        .layer(middleware::from_fn(auth_middleware))
        .layer(axum::Extension(cfg))
        .layer(axum::Extension(spending_lock))
        .layer(TraceLayer::new_for_http())
        .layer(DefaultBodyLimit::max(50 * 1024 * 1024))
        .with_state(wallet)
}

pub async fn run(wallet: WalletState, port: u16, config: ServerConfig) -> Result<()> {
    let tls = config.tls.clone();
    let app = make_router(wallet, config);
    let addr = SocketAddr::from(([127, 0, 0, 1], port));

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
