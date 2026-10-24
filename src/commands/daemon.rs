use std::sync::Arc;

use anyhow::Result;

use crate::config::Config;

/// Monitor + HTTP server for ONE wallet — the composition itself lives in
/// `daemon_tenant::start` so `serve-fleet --daemon` can run it N times in one
/// process (bsv-low plan row D2). Behavior here is unchanged: the configured
/// ROOT_KEY, ctrl-c stops the monitor then the server.
pub async fn run(config: &Arc<Config>) -> Result<()> {
    let root_key_hex = config.wallet.root_key()?;

    let tenant = super::daemon_tenant::start(
        &config.wallet.db,
        root_key_hex,
        config.chain(),
        config.wallet.port,
        config,
    )
    .await?;

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
