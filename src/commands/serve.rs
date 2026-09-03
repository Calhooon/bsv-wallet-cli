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

    let bind_addr: std::net::IpAddr = std::env::var("BIND_ADDR")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or_else(|| std::net::IpAddr::from([127, 0, 0, 1]));

    let config = ServerConfig {
        auth_token: std::env::var("AUTH_TOKEN").ok(),
        tls,
        chain: ctx.chain,
        bind_addr,
        callback_token,
    };
    let chain = ctx.chain;
    let db_path = ctx.db_path.clone();
    let wallet_state = server::make_wallet_state(ctx.wallet);
    // No monitor under `serve`: the broadcast reconciler is what re-examines
    // accepted-but-never-propagated transactions (every 60 s, bounded).
    let reconcile =
        crate::broadcast_reconcile::spawn_serve_loop(wallet_state.clone(), chain, &db_path);
    let result = server::run(wallet_state, port, config).await;
    if let Some(handle) = reconcile {
        handle.abort();
    }
    result?;
    Ok(())
}
