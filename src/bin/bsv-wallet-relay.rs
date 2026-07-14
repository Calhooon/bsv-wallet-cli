//! bsv-wallet-relay — ONE public ARC/Arcade callback receiver for a fleet of
//! wallet daemons. Stores callback payloads per token; wallets drain their
//! queue OUTBOUND via `GET /pull` (connect/disconnect at will).
//!
//! Configuration (env):
//! - `RELAY_DB` — sqlite queue path (default `relay.db`)
//! - `RELAY_PORT` — listen port (default `3390`)
//! - `BIND_ADDR` — bind address (default `0.0.0.0` — this IS the public
//!   receiver; front it with TLS/tunnel)
//! - `RELAY_ADMIN_TOKEN` — enables `POST /register` with this bearer
//! - `RELAY_REQUIRE_REGISTER=1` — disable auto-registration on first /pull
//!
//! See `bsv_wallet_cli::relay` for the endpoint contract.

use anyhow::Result;
use bsv_wallet_cli::relay::{make_relay_router, open_relay_db, RelayState};
use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() -> Result<()> {
    dotenvy::dotenv().ok();
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .with_writer(std::io::stderr)
        .init();

    let db_path = std::env::var("RELAY_DB").unwrap_or_else(|_| "relay.db".to_string());
    let port: u16 = std::env::var("RELAY_PORT")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(3390);
    let bind_addr: IpAddr = std::env::var("BIND_ADDR")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or_else(|| "0.0.0.0".parse().unwrap());
    let admin_token = std::env::var("RELAY_ADMIN_TOKEN")
        .ok()
        .filter(|s| !s.is_empty())
        .map(Arc::new);
    let require_register = std::env::var("RELAY_REQUIRE_REGISTER")
        .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
        .unwrap_or(false);

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
