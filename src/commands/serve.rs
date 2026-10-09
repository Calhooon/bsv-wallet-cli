use anyhow::Result;

use crate::context::WalletContext;
use crate::server::{self, ServerConfig, TlsConfig};

pub async fn run(ctx: WalletContext, port: u16) -> Result<()> {
    let tls = match (
        std::env::var("TLS_CERT_PATH").ok(),
        std::env::var("TLS_KEY_PATH").ok(),
    ) {
        (Some(cert_path), Some(key_path)) => Some(TlsConfig {
            cert_path,
            key_path,
        }),
        _ => None,
    };

    // /arc-callback is enabled when a callback token exists (Arcade mode, or
    // explicit CALLBACK_TOKEN). No monitor runs under `serve`, so push proofs
    // arrive via the webhook only.
    let callback_token = crate::services_env::arcade_runtime(&ctx.db_path)?
        .map(|rt| rt.callback_token)
        .or_else(|| {
            std::env::var("CALLBACK_TOKEN")
                .ok()
                .filter(|s| !s.is_empty())
        });

    let bind_addr = server::bind_addr_from_env();

    let config = ServerConfig {
        auth_token: std::env::var("AUTH_TOKEN").ok(),
        tls,
        chain: ctx.chain,
        bind_addr,
        callback_token,
    };
    let db_path = ctx.db_path.clone();
    let wallet_state = server::make_wallet_state(ctx.wallet);
    // No monitor under `serve`: the served loop drains Arcade's SSE, runs
    // the tracker's pass (a proof asked for one named transaction when its
    // re-ask is due, never a chain index) and sweeps the verdicts in
    // storage, every 60 s.
    let reconcile = crate::broadcast_reconcile::spawn_serve_loop(wallet_state.clone(), &db_path);
    let result = server::run(wallet_state, port, config).await;
    if let Some(handle) = reconcile {
        handle.abort();
    }
    result?;
    Ok(())
}
