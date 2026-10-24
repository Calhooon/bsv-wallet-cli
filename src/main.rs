mod arc_ingest;
mod atomic_beef;
mod brc29;
mod broadcast_reconcile;
mod broadcast_verify;
mod cli;
mod commands;
mod context;
mod explorers;
mod server;
#[cfg(test)]
mod test_support;
#[cfg(test)]
mod tracker_words_witnesses;

use anyhow::Result;
// The configuration, read once at start: the crate's one reader of the
// environment.
use bsv_wallet_cli::config;
// The services' options too: built from the configuration's settings.
use bsv_wallet_cli::services_env;
// The chain-event feed is the library's: the daemon holds it, the server
// reads its status.
use bsv_wallet_cli::chain_feed;
// The tracker host is the library's, one copy: its spend guard and its
// reads are public API the binary does not call itself.
use bsv_wallet_cli::tracker_host;
// The retire guard is the library's too: every retire path asks it.
use bsv_wallet_cli::retire_guard;
// The replacement act uses the same host implementation as its harness.
use bsv_wallet_cli::respend;
// The spend guard too: every spend of the binary runs it first.
use bsv_wallet_cli::spend_guard;
// The spend gate: the guard on the toolbox's coin selection (0.10.0).
use bsv_wallet_cli::backstop_log::{BackstopFilter, BackstopLayer};
use bsv_wallet_cli::spend_gate;
// The store's two doors and the typed busy.
use bsv_wallet_cli::store;
use bsv_wallet_cli::toolbox_words;
use clap::{CommandFactory, FromArgMatches};
use cli::{Cli, Commands};
use std::sync::Arc;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;
use tracing_subscriber::{EnvFilter, Layer};

#[tokio::main]
async fn main() -> Result<()> {
    dotenvy::dotenv().ok();

    let matches = Cli::command().get_matches();
    let cli = Cli::from_arg_matches(&matches).unwrap_or_else(|e| e.exit());
    let config = Arc::new(config::Config::load(&flags(&cli, &matches))?);

    // Init tracing
    let filter = if cli.verbose {
        EnvFilter::new("debug")
    } else {
        EnvFilter::try_from_default_env()
            .unwrap_or_else(|_| EnvFilter::new("bsv_wallet=info,tower_http=info"))
    };
    // Logs go to stderr so stdout stays clean machine-parseable output
    // (notably `--json`): tooling/agents can read stdout without log pollution.
    // Beside the filtered log, one layer reads the toolbox's two lines
    // behind a proof its monitor's backstop stored and says which provider
    // served the merkle path, at info (`backstop_log`).
    tracing_subscriber::registry()
        .with(
            tracing_subscriber::fmt::layer()
                .with_writer(std::io::stderr)
                .with_filter(filter),
        )
        .with(BackstopLayer::logging().with_filter(BackstopFilter))
        .init();

    // The wallet the configuration names, for the commands that open one.
    let open = || context::WalletContext::load(&config, cli.json);
    match &cli.command {
        Commands::Init { key, force } => {
            commands::init::run(&cli.db, key.as_deref(), *force).await?;
        }
        Commands::Identity => {
            let ctx = open().await?;
            commands::identity::run(&ctx).await?;
        }
        Commands::Balance => {
            let ctx = open().await?;
            commands::balance::run(&ctx).await?;
        }
        Commands::Address => {
            let ctx = open().await?;
            commands::address::run(&ctx).await?;
        }
        Commands::Send {
            address,
            satoshis,
            allow_pending,
            coins,
        } => {
            let allow_pending = word(allow_pending, coins)?;
            let ctx = open().await?;
            commands::send::run(&ctx, address, *satoshis, allow_pending).await?;
        }
        Commands::Respend { txid } => {
            let ctx = open().await?;
            commands::respend::run(&ctx, txid).await?;
        }
        Commands::GiftSend {
            recipient,
            satoshis,
            unlock,
            fee_utxo,
            allow_pending,
            coins,
        } => {
            let allow_pending = word(allow_pending, coins)?;
            let ctx = open().await?;
            commands::gift_send::run(&ctx, recipient, *satoshis, unlock, *fee_utxo, allow_pending)
                .await?;
        }
        Commands::GiftInspect { txid } => {
            let ctx = open().await?;
            commands::gift_inspect::run(&ctx, txid).await?;
        }
        Commands::GiftClaim { txid, force } => {
            let ctx = open().await?;
            commands::gift_claim::run(&ctx, txid, *force).await?;
        }
        Commands::Drain {
            address,
            allow_pending,
            coins,
        } => {
            let allow_pending = word(allow_pending, coins)?;
            let ctx = open().await?;
            commands::drain::run(&ctx, address, allow_pending).await?;
        }
        Commands::Fund { beef_hex, vout } => {
            let ctx = open().await?;
            commands::fund::run(&ctx, beef_hex, *vout).await?;
        }
        Commands::Receive { txid, vout } => {
            let ctx = open().await?;
            commands::receive::run(&ctx, txid, *vout).await?;
        }
        Commands::Outputs { basket, tag } => {
            let ctx = open().await?;
            commands::outputs::run(&ctx, basket.as_deref(), tag.as_deref()).await?;
        }
        Commands::Actions { label } => {
            let ctx = open().await?;
            commands::actions::run(&ctx, label.as_deref()).await?;
        }
        Commands::Sync { reconcile_spent } => {
            let ctx = open().await?;
            commands::sync::run(&ctx, *reconcile_spent).await?;
        }
        Commands::Tick => {
            commands::tick::run(&config, cli.json).await?;
        }
        Commands::TrackerTick => {
            let ctx = open().await?;
            commands::tracker_tick::run(&ctx).await?;
        }
        Commands::Daemon { allow_no_token } => {
            server::refuse_open_bind_from_env(*allow_no_token)?;
            commands::daemon::run(&config).await?;
        }
        Commands::Serve { allow_no_token } => {
            server::refuse_open_bind_from_env(*allow_no_token)?;
            let ctx = open().await?;
            commands::serve::run(ctx, cli.port).await?;
        }
        Commands::ServeFleet {
            wallet,
            daemon,
            allow_no_token,
        } => {
            server::refuse_open_bind_from_env(*allow_no_token)?;
            commands::serve_fleet::run(wallet, *daemon, &config).await?;
        }
        Commands::Split {
            count,
            allow_pending,
            coins,
        } => {
            let allow_pending = word(allow_pending, coins)?;
            let ctx = open().await?;
            commands::split::run(&ctx, *count, allow_pending).await?;
        }
        Commands::Services => {
            commands::services::run(&config, cli.json).await?;
        }
        Commands::Ui { ui_port } => {
            commands::ui::run(&cli.db, *ui_port).await?;
        }
        Commands::Compact => {
            let ctx = open().await?;
            commands::compact::run(&ctx).await?;
        }
        Commands::Backup { to } => {
            commands::backup::run(&cli.db, to.clone()).await?;
        }
        Commands::ExportBeefs { to } => {
            let ctx = open().await?;
            commands::export_beefs::run(&ctx, to.clone()).await?;
        }
        Commands::CleanupAbandoned { execute } => {
            let ctx = open().await?;
            commands::cleanup_abandoned::run(&ctx, *execute).await?;
        }
        Commands::ReconcileBroadcasts {
            execute,
            max_probes,
        } => {
            let ctx = open().await?;
            commands::reconcile_broadcasts::run(&ctx, *execute, *max_probes).await?;
        }
        Commands::ReconcileOutputs {
            execute,
            max_chain_checks,
        } => {
            let ctx = open().await?;
            commands::reconcile_outputs::run(&ctx, *execute, *max_chain_checks).await?;
        }
        Commands::Reproof {
            since_height,
            all,
            execute,
        } => {
            let ctx = open().await?;
            commands::reproof::run(&ctx, *since_height, *all, *execute).await?;
        }
    }

    Ok(())
}

/// The global flags as the configuration takes them, with the ones given on
/// the command line (the rest are clap's defaults).
fn flags(cli: &Cli, matches: &clap::ArgMatches) -> config::Flags {
    let given = [("db", "--db"), ("testnet", "--testnet"), ("port", "--port")]
        .into_iter()
        .filter(|(id, _)| {
            matches!(
                given_in(matches, id),
                Some(clap::parser::ValueSource::CommandLine)
            )
        })
        .map(|(_, flag)| flag)
        .collect();
    config::Flags {
        db: cli.db.clone(),
        testnet: cli.testnet,
        port: cli.port,
        given,
    }
}

/// Where a global argument's value came from, read at the deepest
/// subcommand (a global given after the subcommand is recorded there).
fn given_in(matches: &clap::ArgMatches, id: &str) -> Option<clap::parser::ValueSource> {
    let mut m = matches;
    while let Some((_, sub)) = m.subcommand() {
        m = sub;
    }
    m.value_source(id)
}

/// The `--allow-pending` flag, with its `--coin`s, as the gate's word for
/// this one action (R1 (d); the scope, #63). Read before the wallet is
/// opened: a coin that does not parse is the caller's error, not the
/// wallet's.
fn word(allow_pending: &Option<String>, coins: &[String]) -> Result<spend_gate::AllowPending> {
    spend_gate::AllowPending::from_cli(allow_pending.as_deref(), coins)
        .map_err(|e| anyhow::anyhow!("Invalid argument: {e}"))
}
