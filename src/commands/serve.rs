use anyhow::Result;

use crate::context::WalletContext;
use crate::server::{self, ServerConfig};

pub async fn run(ctx: WalletContext, port: u16) -> Result<()> {
    tracing::info!("config: {}", ctx.config.log_line());

    // /arc-callback is enabled when a callback token exists (Arcade mode, or
    // explicit CALLBACK_TOKEN). No monitor runs under `serve`, so push proofs
    // arrive via the webhook only.
    let arcade_token = crate::services_env::arcade_runtime(&ctx.config.services, &ctx.db_path)?
        .map(|rt| rt.callback_token);
    let config = ServerConfig::from_config(&ctx.config, ctx.chain, arcade_token);
    let settings = ctx.config.clone();
    let db_path = ctx.db_path.clone();
    let wallet_state = server::make_wallet_state(ctx.wallet);
    // No monitor under `serve`: the served loop drains Arcade's SSE, runs
    // the tracker's pass (a proof asked for one named transaction when its
    // re-ask is due, never a chain index) and sweeps the verdicts in
    // storage, every 60 s.
    let reconcile =
        crate::broadcast_reconcile::spawn_serve_loop(wallet_state.clone(), &settings, &db_path);
    let result = server::run(wallet_state, port, config).await;
    if let Some(handle) = reconcile {
        handle.abort();
    }
    result?;
    Ok(())
}
