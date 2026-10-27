#[cfg(test)]
mod arc_callback_tests;
pub mod audit;
pub mod broadcast_follow_up;
pub mod error;
pub mod handlers;
mod host_reads;
mod page_bounds;
pub mod post_word;
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
use std::time::Duration;
use tower_http::cors::CorsLayer;
use tower_http::trace::TraceLayer;

use crate::config::{Config, ServerSettings};
use handlers::WalletState;

// One 5 s store busy ceiling fits; two do not. Leave time to deliver the
// answer before Arcade's 10 s POST timeout (arcade@1ae1208e64b77b64818044d606638939d9f1834a,
// config/config.go:1291, webhook.http_timeout_ms):
// https://github.com/bsv-blockchain/arcade/blob/1ae1208e64b77b64818044d606638939d9f1834a/config/config.go#L1291
const ARC_CALLBACK_DEADLINE: Duration = Duration::from_secs(8);

/// Serializes createAction; non-spending endpoints (encrypt, getPublicKey,
/// listOutputs, etc.) remain unaffected.
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
    /// Optional bearer token. When set, every request but an `OPTIONS` (CORS
    /// answers it) needs `Authorization: Bearer <token>`. None: auth is off.
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
    /// The configuration read at start: the spend gate's settings and the
    /// post-broadcast verifier are built from it once, in [`make_router`].
    pub settings: Arc<Config>,
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            auth_token: None,
            tls: None,
            chain: Chain::Main,
            bind_addr: std::net::IpAddr::from([127, 0, 0, 1]),
            callback_token: None,
            settings: Arc::new(Config::default()),
        }
    }
}

impl ServerConfig {
    /// The served wallet's server settings from the configuration
    /// (`BIND_ADDR`, `AUTH_TOKEN`, `TLS_CERT_PATH` with `TLS_KEY_PATH`), on
    /// `chain`. `/arc-callback` is enabled when a callback token exists: the
    /// Arcade runtime's (`arcade_token`), else an explicit `CALLBACK_TOKEN`.
    pub fn from_config(settings: &Arc<Config>, chain: Chain, arcade_token: Option<String>) -> Self {
        let (server, services) = (&settings.server, &settings.services);
        let owned = |t: &crate::config::Secret| t.expose().to_string();
        let tls = (server.tls_cert_path.clone()).zip(server.tls_key_path.clone());
        Self {
            auth_token: server.auth_token.as_ref().map(owned),
            tls: tls.map(|(cert_path, key_path)| TlsConfig {
                cert_path,
                key_path,
            }),
            chain,
            bind_addr: server.bind_addr,
            callback_token: arcade_token.or_else(|| services.callback_token.as_ref().map(owned)),
            settings: settings.clone(),
        }
    }
}

/// The bind guard (0.7.1). With no bearer token a served wallet answers any
/// caller that can reach it, and its two BEEF doors take a body of any size
/// (0.7.0), so an address beyond loopback needs a token or the operator's
/// explicit `allow_no_token`. An empty token counts as none. Loopback is
/// unchanged. Run at startup, before the wallet is opened or a socket bound.
pub fn refuse_open_bind(
    bind_addr: std::net::IpAddr,
    auth_token: Option<&str>,
    allow_no_token: bool,
) -> Result<()> {
    let has_token = auth_token.is_some_and(|t| !t.is_empty());
    if has_token || allow_no_token || bind_addr.to_canonical().is_loopback() {
        return Ok(());
    }
    anyhow::bail!(
        "refusing to bind {bind_addr}: no bearer token is configured, and a wallet served \
         beyond loopback without one answers anyone who can reach it. Set AUTH_TOKEN, or pass \
         --allow-no-token to bind it open deliberately."
    )
}

/// [`refuse_open_bind`] on the settings the served commands bind with
/// (`BIND_ADDR`, `AUTH_TOKEN`).
pub fn refuse_open_bind_for(settings: &ServerSettings, allow_no_token: bool) -> Result<()> {
    let token = settings.auth_token.as_ref().map(|t| t.expose());
    refuse_open_bind(settings.bind_addr, token, allow_no_token)
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
/// The start-time heal of a 0.15 process's holds is still owed: it failed
/// at start (the store busy, say), so the next served request tries it
/// again, until one succeeds.
#[derive(Clone, Default)]
pub struct HealOwed(Arc<std::sync::atomic::AtomicBool>);

impl HealOwed {
    pub fn owed() -> Self {
        Self(Arc::new(std::sync::atomic::AtomicBool::new(true)))
    }

    #[allow(dead_code)] // read by the tests
    pub fn is_owed(&self) -> bool {
        self.0.load(std::sync::atomic::Ordering::SeqCst)
    }
}

/// The heal of a 0.15 process's holds, at start or owed by a start that
/// failed: logged either way, never a request's error.
async fn heal_at_start(wallet: &WalletState) -> bool {
    match crate::spend_gate::heal_if_held(wallet.storage()).await {
        Ok(0) => true,
        Ok(n) => {
            tracing::info!(
                coins = n,
                "spend gate: healed the holds a 0.15 process left"
            );
            true
        }
        Err(e) => {
            tracing::warn!(error = %e, "spend gate: the holds a 0.15 process left were not healed; the next request tries again");
            false
        }
    }
}

/// A request after a start whose heal failed tries the heal once more,
/// before the request, after auth. Its own answer is unchanged by it.
async fn heal_owed(
    axum::extract::State(wallet): axum::extract::State<WalletState>,
    axum::Extension(owed): axum::Extension<HealOwed>,
    request: Request,
    next: middleware::Next,
) -> Response {
    if owed.0.swap(false, std::sync::atomic::Ordering::SeqCst) && !heal_at_start(&wallet).await {
        owed.0.store(true, std::sync::atomic::Ordering::SeqCst);
    }
    next.run(request).await
}

#[allow(dead_code)] // the binary serves through `run`; the tests build it
pub fn make_router(wallet: WalletState, config: ServerConfig) -> Router {
    make_router_owing(wallet, config, HealOwed::default())
}

/// [`make_router`], with the start-time heal owed when `owed` says so.
pub fn make_router_owing(wallet: WalletState, config: ServerConfig, owed: HealOwed) -> Router {
    // A browser caller reads `Retry-After` on a 503 STORE_BUSY (#44).
    let cors = CorsLayer::very_permissive().expose_headers([axum::http::header::RETRY_AFTER]);
    let cfg = config.clone();
    let spending_lock: SpendingLock = Arc::new(tokio::sync::Mutex::new(()));
    // Post-broadcast verifier — lets `/createAction` fail loudly when ARC
    // silently dropped an immediate broadcast (e.g. 465 fee-too-low on a deep
    // unconfirmed BEEF) instead of returning a phantom txid.
    let verifier = crate::broadcast_verify::BroadcastVerifier::new(config.chain, &config.settings);
    // The spend gate's settings, once: a request reads these, never the
    // environment.
    let gate = crate::spend_gate::GateOptions::from_config(&config.settings);

    Router::new()
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
        // CHAIN FEED (not BRC-100): the daemon's chain-event feed status,
        // its stored cursor and, when it stopped, the fault it stopped on.
        .route("/chain-feed", get(chain_feed_status))
        // THE TRACKER'S WORD (not BRC-100): one transaction's stored word,
        // its height, its pending re-ask and, for `mined`, the stored
        // proof's root and height. Behind the same auth as `/chain-feed`.
        .route("/tx/{txid}", get(tx_word))
        .route("/createSignature", post(handlers::create_signature))
        // The two doors that carry a caller's BEEF (`inputBEEF`, `tx`): no
        // body cap, a valid BEEF is never refused for its size (see
        // `BEEF_DOORS`).
        .route(
            "/createAction",
            post(handlers::create_action).layer(DefaultBodyLimit::disable()),
        )
        .route(
            "/internalizeAction",
            post(handlers::internalize_action).layer(DefaultBodyLimit::disable()),
        )
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
        // The last layer added runs first: body limit, trace, extensions, CORS
        // (it answers each OPTIONS, a preflight carries no bearer, and rides a
        // 401 so a browser reads it: #138), auth BEFORE the body is read (an
        // uncapped BEEF door is open only to the wallet's own caller), the body.
        .layer(middleware::from_fn_with_state(wallet.clone(), heal_owed))
        .layer(middleware::from_fn(lenient_json_body))
        .layer(middleware::from_fn(auth_middleware))
        .layer(cors)
        .layer(axum::Extension(cfg))
        .layer(axum::Extension(spending_lock))
        .layer(axum::Extension(verifier))
        .layer(axum::Extension(gate))
        .layer(axum::Extension(owed))
        .layer(TraceLayer::new_for_http())
        .layer(DefaultBodyLimit::max(BODY_CAP))
        .with_state(wallet)
}

/// `POST /arc-callback` — ARC/Arcade status webhook receiving push status
/// updates and (on MINED) the merkle path, straight into wallet storage.
///
/// Authenticated by the per-wallet callback token: ARC-convention
/// `Authorization: Bearer <callback-token>` or an `X-CallbackToken` header
/// (both accepted — broadcaster implementations vary). Returns 404 when no
/// callback token is configured (route effectively disabled).
///
/// Ingestion has an 8 s deadline, below Arcade's 10 s POST timeout:
/// <https://github.com/bsv-blockchain/arcade/blob/1ae1208e64b77b64818044d606638939d9f1834a/config/config.go#L1291>
/// Expiration is 503 `STORE_BUSY` with `Retry-After: 1`; an earlier typed
/// busy keeps its existing message.
async fn arc_callback(state: axum::extract::State<WalletState>, request: Request) -> Response {
    arc_callback_with_deadline(state, request, ARC_CALLBACK_DEADLINE).await
}

async fn arc_callback_with_deadline(
    axum::extract::State(wallet): axum::extract::State<WalletState>,
    request: Request,
    deadline: Duration,
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

    ingest_arc_before_deadline(wallet.storage(), &payload, deadline).await
}

async fn ingest_arc_before_deadline(
    storage: &StorageSqlx,
    payload: &serde_json::Value,
    deadline: Duration,
) -> Response {
    // The deadline covers the store and the header service's inline MINED root
    // check; it cannot identify which waited. timeout drops the ingest future.
    // Toolbox 0.13.0's Seen/InBlock/Mined paths have two separate writes:
    // broadcast memory's BEGIN IMMEDIATE (its busy is swallowed by
    // record_broadcast_status_quiet), then the request UPDATE, and no transaction
    // wraps both (toolbox #205 commits the status and the hint in one). sqlx rolls
    // back an unacknowledged BEGIN, but a running UPDATE can land after the 503:
    // request unmined, transaction sending, memory empty. A retry converges: the
    // status updates are conditional, the memory ladder upserts, ingest_merkle_proof
    // commits proof and completion links together, reusing a matching anchor. The
    // tests cover cancellation before any write on all three paths and after the
    // statuses land on MINED: one retry equals a clean delivery, a second is a no-op.
    match tokio::time::timeout(
        deadline,
        crate::arc_ingest::ingest_arc_payload(storage, payload),
    )
    .await
    {
        Ok(Ok(action)) => (
            StatusCode::OK,
            Json(json!({"ok": true, "action": format!("{:?}", action)})),
        )
            .into_response(),
        Ok(Err(e)) => {
            if let Some(busy) = error::AppError::store_busy(&e) {
                return busy.into_response();
            }
            (
                StatusCode::BAD_REQUEST,
                Json(json!({"code": "BAD_PAYLOAD", "message": e.to_string()})),
            )
                .into_response()
        }
        Err(_) => error::AppError::new(
            error::Kind::StoreBusy,
            format!("the arc-callback deadline of {deadline:?} elapsed before the toolbox answered (the store or the header service)"),
        )
        .into_response(),
    }
}

/// Browser wallets accept JSON whose strings carry lone UTF-16 surrogate escapes
/// (`"\ud83d"` without its pair); serde does not, so a page whose keyID or data
/// happened to contain one got `400 Failed to parse the request body as JSON:
/// keyID: lone leading surrogate` from this wallet while MetaNet Desktop served
/// the same call (beta soak, 2026-09-02, two logins lost). A browser's encoder
/// turns such a code unit into U+FFFD when it hashes the string, so the same
/// substitution here yields the same bytes the JS wallet would derive from.
pub fn sanitize_lone_surrogates(input: &[u8]) -> std::borrow::Cow<'_, [u8]> {
    if !input.windows(2).any(|w| w == b"\\u") {
        return std::borrow::Cow::Borrowed(input);
    }
    let hex4 = |b: &[u8]| -> Option<u32> {
        if b.len() < 4 {
            return None;
        }
        std::str::from_utf8(&b[..4])
            .ok()
            .and_then(|h| u32::from_str_radix(h, 16).ok())
    };
    let mut out = Vec::with_capacity(input.len());
    let mut i = 0;
    let mut changed = false;
    while i < input.len() {
        if input[i] == b'\\' && i + 1 < input.len() && input[i + 1] == b'u' {
            if let Some(cu) = hex4(&input[i + 2..]) {
                let is_lead = (0xD800..=0xDBFF).contains(&cu);
                let is_trail = (0xDC00..=0xDFFF).contains(&cu);
                if is_lead {
                    let next = &input[i + 6..];
                    let paired = next.len() >= 6
                        && next[0] == b'\\'
                        && next[1] == b'u'
                        && hex4(&next[2..]).is_some_and(|t| (0xDC00..=0xDFFF).contains(&t));
                    if paired {
                        out.extend_from_slice(&input[i..i + 12]);
                        i += 12;
                        continue;
                    }
                    out.extend_from_slice(b"\\ufffd");
                    changed = true;
                    i += 6;
                    continue;
                }
                if is_trail {
                    out.extend_from_slice(b"\\ufffd");
                    changed = true;
                    i += 6;
                    continue;
                }
                out.extend_from_slice(&input[i..i + 6]);
                i += 6;
                continue;
            }
        }
        out.push(input[i]);
        i += 1;
    }
    if changed {
        std::borrow::Cow::Owned(out)
    } else {
        std::borrow::Cow::Borrowed(input)
    }
}

/// The routes whose JSON body carries a caller's BEEF. Their body is read
/// whole with no cap: the BRC-100 JSON wire carries the BEEF as one array, so
/// the body is held in memory (linear in the BEEF, about four bytes of body
/// per byte of BEEF) and the BEEF is then judged by the toolbox's streaming
/// reader for invalid bytes only. Every other route keeps
/// [`BODY_CAP`].
const BEEF_DOORS: [&str; 2] = ["/createAction", "/internalizeAction"];

/// The body cap of the routes that carry no BEEF.
const BODY_CAP: usize = 50 * 1024 * 1024;

/// Rewrite lone surrogate escapes in JSON request bodies before the `Json`
/// extractors see them (see [`sanitize_lone_surrogates`]).
async fn lenient_json_body(request: Request, next: Next) -> Response {
    let is_json = request
        .headers()
        .get(axum::http::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|ct| ct.to_ascii_lowercase().contains("json"));
    if !is_json {
        return next.run(request).await;
    }
    let (parts, body) = request.into_parts();
    let cap = if BEEF_DOORS.contains(&parts.uri.path()) {
        usize::MAX
    } else {
        BODY_CAP
    };
    let bytes = match axum::body::to_bytes(body, cap).await {
        Ok(b) => b,
        Err(_) => {
            return (
                StatusCode::PAYLOAD_TOO_LARGE,
                Json(json!({"code": "TOO_LARGE", "message": "payload too large"})),
            )
                .into_response();
        }
    };
    let body = match sanitize_lone_surrogates(&bytes) {
        std::borrow::Cow::Borrowed(_) => axum::body::Body::from(bytes),
        std::borrow::Cow::Owned(fixed) => {
            tracing::warn!(
                path = %parts.uri.path(),
                "json body carried a lone UTF-16 surrogate escape; substituted U+FFFD"
            );
            axum::body::Body::from(fixed)
        }
    };
    next.run(Request::from_parts(parts, body)).await
}

pub async fn run(wallet: WalletState, port: u16, config: ServerConfig) -> Result<()> {
    // Once at start (daemon and serve both come here): the holds a 0.15
    // process left in `spend_gate_holds` are change again. The gate writes
    // no hold from 0.16: it offers (`create_action_offering`). A heal that
    // fails here is owed, and the next served request tries it again.
    let owed = if heal_at_start(&wallet).await {
        HealOwed::default()
    } else {
        HealOwed::owed()
    };
    let tls = config.tls.clone();
    let bind_addr = config.bind_addr;
    let app = make_router_owing(wallet, config, owed);
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
    let (signal_fault, mut signal_fault_rx) = tokio::sync::oneshot::channel();
    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal(signal_fault))
        .await?;
    // A listener that could not be installed shuts the server down and is
    // the caller's error, never a panic.
    if let Ok(e) = signal_fault_rx.try_recv() {
        return Err(anyhow::Error::new(e).context("failed to listen for ctrl-c"));
    }

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

async fn shutdown_signal(fault: tokio::sync::oneshot::Sender<std::io::Error>) {
    if let Err(e) = tokio::signal::ctrl_c().await {
        let _ = fault.send(e);
        return;
    }
    eprintln!("\nShutting down...");
}

/// GET /permission-audit — every permissioned request since start or reset.
async fn permission_audit() -> Json<serde_json::Value> {
    let entries = audit::snapshot();
    Json(json!({ "count": entries.len(), "entries": entries }))
}

/// GET /chain-feed — the feed's stored status and cursor (`chain_feed`).
/// `{"status": null}` where no feed ran (`serve`, or `CHAINTRACKS_URL=off`).
async fn chain_feed_status(
    axum::extract::State(wallet): axum::extract::State<WalletState>,
) -> Response {
    host_reads::answer_chain_feed(&host_reads::StoreReads {
        storage: wallet.storage(),
    })
    .await
}

/// Host-route store faults keep their code unless the store is busy by
/// type (#203, #227). The position tests feed a real busy error at each read
/// and require 503 with the shared body and retry header. Helper tests
/// check that store reads preserve its type, including the proof query.
fn host_route_fault(e: &anyhow::Error, code: &str) -> Response {
    if let Some(busy) = error::AppError::store_busy(e) {
        return busy.into_response();
    }
    (
        StatusCode::INTERNAL_SERVER_ERROR,
        Json(json!({ "code": code, "message": e.to_string() })),
    )
        .into_response()
}

/// GET /tx/{txid} — the tracker's stored word for one transaction
/// (`tracker_host::stored_word`): `{txid, word, height, reask, evidence}`.
/// `evidence` is, for a `mined` word, the stored proof's root and height
/// (the toolbox's `proven_txs` row the spend gate reads), else `null`. A
/// `stale` word adds `tier`, the word its hints alone give (`announced`,
/// `seen`: the status before its proof, read as "confirming"), and its
/// `reask` carries the re-ask the reorg left (`reorg@<forkHeight>`). Ruling 7b (b) of 2026-10-20 20:1xZ
/// (bsv-stack-lean `docs/DECISIONS-2026-10-19.md` section 8c). A view for
/// a person or an agent; nothing is decided from it. 404 when the tracker
/// holds no word for the txid.
async fn tx_word(
    axum::extract::State(wallet): axum::extract::State<WalletState>,
    axum::extract::Path(txid): axum::extract::Path<String>,
) -> Response {
    host_reads::answer_tx_word(
        &host_reads::StoreReads {
            storage: wallet.storage(),
        },
        &txid,
    )
    .await
}

/// POST /permission-audit/reset — call immediately before the flow you measure.
async fn permission_audit_reset() -> Json<serde_json::Value> {
    audit::reset();
    Json(json!({ "ok": true }))
}

#[cfg(test)]
mod lenient_json_tests {
    use super::sanitize_lone_surrogates;

    #[test]
    fn lone_lead_and_lone_trail_become_fffd_and_pairs_survive() {
        let lead = br#"{"keyID":"ab \ud83d cd"}"#;
        assert_eq!(
            &*sanitize_lone_surrogates(lead),
            br#"{"keyID":"ab \ufffd cd"}"#
        );
        let trail = br#"{"keyID":"\udc00"}"#;
        assert_eq!(&*sanitize_lone_surrogates(trail), br#"{"keyID":"\ufffd"}"#);
        let pair = br#"{"keyID":"\ud83d\ude00"}"#;
        assert!(matches!(
            sanitize_lone_surrogates(pair),
            std::borrow::Cow::Borrowed(_)
        ));
        let plain = br#"{"keyID":"plain \\u0041"}"#;
        assert!(matches!(
            sanitize_lone_surrogates(plain),
            std::borrow::Cow::Borrowed(_)
        ));
        let fixed = sanitize_lone_surrogates(lead);
        let v: serde_json::Value = serde_json::from_slice(&fixed).unwrap();
        assert_eq!(v["keyID"], "ab \u{fffd} cd");
    }
}

#[cfg(test)]
mod bind_guard_tests {
    use super::refuse_open_bind;
    use std::net::IpAddr;

    fn ip(s: &str) -> IpAddr {
        s.parse().unwrap()
    }

    #[test]
    fn beyond_loopback_needs_a_token_or_the_flag() {
        for open in ["0.0.0.0", "::", "192.168.1.10", "10.0.0.1"] {
            let err = refuse_open_bind(ip(open), None, false).unwrap_err();
            assert!(
                err.to_string().contains("--allow-no-token"),
                "{open}: {err}"
            );
            assert!(
                refuse_open_bind(ip(open), Some(""), false).is_err(),
                "{open}"
            );
            assert!(
                refuse_open_bind(ip(open), Some("t"), false).is_ok(),
                "{open}"
            );
            assert!(refuse_open_bind(ip(open), None, true).is_ok(), "{open}");
        }
    }

    #[test]
    fn loopback_is_unchanged() {
        for lo in ["127.0.0.1", "127.0.0.2", "::1", "::ffff:127.0.0.1"] {
            assert!(refuse_open_bind(ip(lo), None, false).is_ok(), "{lo}");
        }
    }
}
