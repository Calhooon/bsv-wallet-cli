mod arc_ingest;
mod atomic_beef;
mod brc29;
mod broadcast_verify;
mod cli;
mod commands;
mod context;
mod server;
mod services_env;

use anyhow::Result;
use clap::Parser;
use cli::{Cli, Commands};
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() -> Result<()> {
    dotenvy::dotenv().ok();

    let cli = Cli::parse();

    // Init tracing
    let filter = if cli.verbose {
        EnvFilter::new("debug")
    } else {
        EnvFilter::try_from_default_env()
            .unwrap_or_else(|_| EnvFilter::new("bsv_wallet=info,tower_http=info"))
    };
    // Logs go to stderr so stdout stays clean machine-parseable output
    // (notably `--json`): tooling/agents can read stdout without log pollution.
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(std::io::stderr)
        .init();

    match &cli.command {
        Commands::Init { key, force } => {
            commands::init::run(&cli.db, key.as_deref(), *force).await?;
        }
        Commands::Identity => {
            let ctx = context::WalletContext::load(&cli).await?;
            commands::identity::run(&ctx).await?;
        }
        Commands::Balance => {
            let ctx = context::WalletContext::load(&cli).await?;
            commands::balance::run(&ctx).await?;
        }
        Commands::Address => {
            let ctx = context::WalletContext::load(&cli).await?;
            commands::address::run(&ctx).await?;
        }
        Commands::Send { address, satoshis } => {
            let ctx = context::WalletContext::load(&cli).await?;
            commands::send::run(&ctx, address, *satoshis).await?;
        }
        Commands::GiftSend {
            recipient,
            satoshis,
            unlock,
            fee_utxo,
        } => {
            let ctx = context::WalletContext::load(&cli).await?;
            commands::gift_send::run(&ctx, recipient, *satoshis, unlock, *fee_utxo).await?;
        }
        Commands::GiftInspect { txid } => {
            let ctx = context::WalletContext::load(&cli).await?;
            commands::gift_inspect::run(&ctx, txid).await?;
        }
        Commands::GiftClaim { txid, force } => {
            let ctx = context::WalletContext::load(&cli).await?;
            commands::gift_claim::run(&ctx, txid, *force).await?;
        }
        Commands::Drain { address } => {
            let ctx = context::WalletContext::load(&cli).await?;
            commands::drain::run(&ctx, address).await?;
        }
        Commands::Fund { beef_hex, vout } => {
            let ctx = context::WalletContext::load(&cli).await?;
            commands::fund::run(&ctx, beef_hex, *vout).await?;
        }
        Commands::Receive { txid, vout } => {
            let ctx = context::WalletContext::load(&cli).await?;
            commands::receive::run(&ctx, txid, *vout).await?;
        }
        Commands::Outputs { basket, tag } => {
            let ctx = context::WalletContext::load(&cli).await?;
            commands::outputs::run(&ctx, basket.as_deref(), tag.as_deref()).await?;
        }
        Commands::Actions { label } => {
            let ctx = context::WalletContext::load(&cli).await?;
            commands::actions::run(&ctx, label.as_deref()).await?;
        }
        Commands::Sync => {
            let ctx = context::WalletContext::load(&cli).await?;
            commands::sync::run(&ctx).await?;
        }
        Commands::Tick => {
            commands::tick::run(&cli).await?;
        }
        Commands::Daemon => {
            commands::daemon::run(&cli).await?;
        }
        Commands::Serve => {
            let ctx = context::WalletContext::load(&cli).await?;
            commands::serve::run(ctx, cli.port).await?;
        }
        Commands::Split { count } => {
            let ctx = context::WalletContext::load(&cli).await?;
            commands::split::run(&ctx, *count).await?;
        }
        Commands::Services => {
            commands::services::run(&cli).await?;
        }
        Commands::Ui { ui_port } => {
            commands::ui::run(&cli.db, *ui_port).await?;
        }
        Commands::Compact => {
            let ctx = context::WalletContext::load(&cli).await?;
            commands::compact::run(&ctx).await?;
        }
        Commands::Backup { to } => {
            commands::backup::run(&cli.db, to.clone()).await?;
        }
        Commands::ExportBeefs { to } => {
            let ctx = context::WalletContext::load(&cli).await?;
            commands::export_beefs::run(&ctx, to.clone()).await?;
        }
        Commands::CleanupAbandoned { execute } => {
            let ctx = context::WalletContext::load(&cli).await?;
            commands::cleanup_abandoned::run(&ctx, &cli.db, *execute).await?;
        }
    }

    Ok(())
}
