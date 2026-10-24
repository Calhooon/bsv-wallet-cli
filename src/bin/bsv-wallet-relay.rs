//! bsv-wallet-relay — ONE public ARC/Arcade callback receiver for a fleet of
//! wallet daemons. Stores callback payloads per token; wallets drain their
//! queue OUTBOUND via `GET /pull` (connect/disconnect at will).
//!
//! Configuration (env, read once at start by
//! `bsv_wallet_cli::config::RelaySettings`):
//! - `RELAY_DB` — sqlite queue path (default `relay.db`)
//! - `RELAY_PORT` — listen port (default `3390`)
//! - `BIND_ADDR` — bind address (default `0.0.0.0` — this IS the public
//!   receiver; front it with TLS/tunnel)
//! - `RELAY_ADMIN_TOKEN` — enables `POST /register` with this bearer
//! - `RELAY_REQUIRE_REGISTER=1` — disable auto-registration on first /pull
//!
//! See `bsv_wallet_cli::relay` for the endpoint contract.

use anyhow::Result;
use bsv_wallet_cli::config::RelaySettings;
use bsv_wallet_cli::relay::{make_relay_router, open_relay_db, RelayState};
use std::net::SocketAddr;
use std::sync::Arc;
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() -> Result<()> {
    dotenvy::dotenv().ok();
    // Read once, before anything binds (`bsv_wallet_cli::config`): a
    // malformed value is refused here, naming the variable.
    let settings = RelaySettings::load()?;
    tracing_subscriber::fmt()
        .with_env_filter(
            settings
                .rust_log
                .as_deref()
                .and_then(|directives| EnvFilter::try_new(directives).ok())
                .unwrap_or_else(|| EnvFilter::new("info")),
        )
        .with_writer(std::io::stderr)
        .init();
    let db_path = settings.db;
    let port = settings.port;
    let bind_addr = settings.bind_addr;
    let admin_token = settings
        .admin_token
        .map(|t| Arc::new(t.expose().to_string()));
    let require_register = settings.require_register;

    let pool = open_relay_db(&db_path).await?;
    let state = RelayState {
        pool,
        admin_token,
        require_register,
    };
    let app = make_relay_router(state);

    let addr = SocketAddr::from((bind_addr, port));
    tracing::info!(%addr, db = %db_path, require_register, "bsv-wallet-relay listening");
    eprintln!("bsv-wallet-relay listening on {addr} (db: {db_path})");

    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, app)
        .with_graceful_shutdown(async {
            tokio::signal::ctrl_c().await.ok();
            eprintln!("\nShutting down relay...");
        })
        .await?;

    Ok(())
}
