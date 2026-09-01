use anyhow::{Context, Result};
use bsv_wallet_toolbox::Chain;

use crate::cli::Cli;

/// Monitor + HTTP server for ONE wallet — the composition itself lives in
/// `daemon_tenant::start` so `serve-fleet --daemon` can run it N times in one
/// process (bsv-low plan row D2). Behavior here is unchanged: env-loaded
/// ROOT_KEY, ctrl-c stops the monitor then the server.
pub async fn run(cli: &Cli) -> Result<()> {
    let root_key_hex =
        std::env::var("ROOT_KEY").context("ROOT_KEY not set. Run `bsv-wallet init` first.")?;
    let chain = if cli.testnet {
        Chain::Test
    } else {
        Chain::Main
    };

    let tenant = super::daemon_tenant::start(&cli.db, &root_key_hex, chain, cli.port).await?;

    tokio::signal::ctrl_c().await?;
    eprintln!("\nStopping monitor...");
    tenant
        .monitor
        .stop()
        .await
        .map_err(|e| anyhow::anyhow!("{}", e))?;
    eprintln!("Monitor stopped");
    // The server task shuts down via its own signal handler.
    let _ = tenant.server.await;
    Ok(())
}
