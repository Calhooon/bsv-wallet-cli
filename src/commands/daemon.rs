use anyhow::{Context, Result};
use bsv_sdk::primitives::PrivateKey;
use bsv_sdk::wallet::{ListOutputsArgs, WalletInterface};
use bsv_wallet_toolbox::{
    services::providers::ArcConfig, Chain, Monitor, Services, ServicesOptions, StorageSqlx, Wallet,
    WalletStorageWriter,
};
// Hardcoded TAAL API endpoint for clarity in the override below.
const TAAL_ARC_URL: &str = "https://arc.taal.com";
use std::sync::Arc;

use crate::cli::Cli;
use crate::server::{self, ServerConfig, TlsConfig};

pub async fn run(cli: &Cli) -> Result<()> {
    let root_key_hex =
        std::env::var("ROOT_KEY").context("ROOT_KEY not set. Run `bsv-wallet init` first.")?;
    let root_key = PrivateKey::from_hex(&root_key_hex)?;

    let chain = if cli.testnet {
        Chain::Test
    } else {
        Chain::Main
    };

    let storage = StorageSqlx::open(&cli.db).await?;
    storage.make_available().await?;
    // Set busy_timeout so concurrent writes wait instead of returning SQLITE_BUSY.
    // Without this, relinquishOutput racing with createAction gets error 517 instantly.
    sqlx::query("PRAGMA busy_timeout = 500")
        .execute(storage.pool())
        .await
        .ok();

    let make_services = |chain: Chain| -> anyhow::Result<Services> {
        let mut opts = match chain {
            Chain::Main => ServicesOptions::mainnet(),
            Chain::Test => ServicesOptions::testnet(),
        };
        if let Ok(url) = std::env::var("CHAINTRACKS_URL") {
            opts = opts.with_chaintracks_url(url);
        }
        // Optionally authenticate TAAL broadcasts via MAIN_TAAL_API_KEY env.
        // TAAL accepts `Authorization: <key>` WITHOUT the "Bearer " prefix
        // that the toolbox's ArcConfig.api_key would generate, so we pass
        // the header directly via additional_headers. TAAL is now the first
        // post_beef provider in bsv-wallet-toolbox-rs 0.3.37+, so setting
        // this key lets the first broadcast attempt use authenticated TAAL.
        if let Ok(api_key) = std::env::var("MAIN_TAAL_API_KEY") {
            if !api_key.is_empty() {
                let mut headers = std::collections::HashMap::new();
                headers.insert("Authorization".to_string(), api_key);
                opts = opts.with_arc(
                    TAAL_ARC_URL,
                    Some(ArcConfig {
                        headers: Some(headers),
                        ..Default::default()
                    }),
                );
            }
        }
        Ok(Services::with_options(chain, opts)?)
    };

    let services = make_services(chain)?;

    // Wire ChainTracker into storage for Layer 1 proof validation
    if let Some(ref ct) = services.chaintracks {
        storage.set_chain_tracker(ct.clone()).await;
    }

    let storage_arc = Arc::new(storage);
    let services_arc = Arc::new(services);

    // Start the monitor (12 background tasks)
    let monitor = Monitor::new(storage_arc.clone(), services_arc.clone());
    monitor
        .start()
        .await
        .map_err(|e| anyhow::anyhow!("{}", e))?;
    eprintln!("Monitor started");

    // Create wallet using cloned Arcs (Wallet::new takes owned values, not Arcs,
    // so we need to create a separate storage/services for the wallet)
    let storage2 = StorageSqlx::open(&cli.db).await?;
    storage2.make_available().await?;
    sqlx::query("PRAGMA busy_timeout = 500")
        .execute(storage2.pool())
        .await
        .ok();
    let services2 = make_services(chain)?;
    // Wire ChainTracker into wallet storage for Layer 4 BEEF validation
    // (must match storage1 — without this, create_action skips stored BEEF validation)
    if let Some(ref ct) = services2.chaintracks {
        storage2.set_chain_tracker(ct.clone()).await;
    }
    let wallet = Wallet::new(Some(root_key), storage2, services2)
        .await
        .map_err(|e| anyhow::anyhow!("{}", e))?;

    let wallet_state = server::make_wallet_state(wallet);
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
    let config = ServerConfig {
        auth_token: std::env::var("AUTH_TOKEN").ok(),
        tls,
    };

    // Periodic UTXO count check — warn if below threshold
    let min_utxos: usize = std::env::var("MIN_UTXOS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(3);
    let check_wallet = wallet_state.clone();
    // Auto-reconcile abandoned transactions (#18): a never-landed `unproven` tx's
    // change must not be selected to fund — and orphan — a new transaction. Each
    // tick we WoC-check unproven txs older than RECONCILE_ABANDONED_MIN_AGE_SECS
    // (default 1h, so an in-flight tx still propagating is never mis-classified)
    // and fail the ones missing on chain. Toggle off with RECONCILE_ABANDONED=0.
    let reconcile_chain = chain;
    let reconcile_enabled = std::env::var("RECONCILE_ABANDONED")
        .map(|v| v != "0" && !v.eq_ignore_ascii_case("false"))
        .unwrap_or(true);
    let reconcile_min_age_secs: i64 = std::env::var("RECONCILE_ABANDONED_MIN_AGE_SECS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(3600);
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(std::time::Duration::from_secs(300));
        loop {
            interval.tick().await;
            match check_wallet
                .list_outputs(
                    ListOutputsArgs {
                        basket: "default".to_string(),
                        tags: None,
                        tag_query_mode: None,
                        include: None,
                        include_custom_instructions: None,
                        include_tags: None,
                        include_labels: None,
                        limit: None,
                        offset: None,
                        seek_permission: None,
                    },
                    "bsv-wallet-daemon",
                )
                .await
            {
                Ok(result) => {
                    let count = result.outputs.len();
                    if count < min_utxos {
                        tracing::warn!(
                            utxo_count = count,
                            min_utxos = min_utxos,
                            "Low UTXO count! Run `bsv-wallet split --count {}` to create more.",
                            min_utxos
                        );
                    }
                }
                Err(e) => {
                    tracing::debug!("UTXO check failed: {}", e);
                }
            }

            // Auto-reconcile abandoned (never-landed) transactions (#18).
            if reconcile_enabled {
                match crate::commands::cleanup_abandoned::reconcile(
                    check_wallet.storage().pool(),
                    reconcile_chain,
                    reconcile_min_age_secs,
                    true,
                )
                .await
                {
                    Ok(r) if r.applied => {
                        tracing::warn!(
                            failed = r.failed,
                            restored_count = r.restored_count,
                            restored_sats = r.restored_sats,
                            phantom_count = r.phantom_count,
                            phantom_sats = r.phantom_sats,
                            "auto-reconciled abandoned tx(s): marked {} failed, restored {} input(s) ({} sats), invalidated {} phantom output(s) ({} sats)",
                            r.failed, r.restored_count, r.restored_sats, r.phantom_count, r.phantom_sats
                        );
                    }
                    Ok(_) => {}
                    Err(e) => {
                        tracing::debug!("abandoned-tx reconcile skipped: {}", e);
                    }
                }
            }
        }
    });

    // Run HTTP server (blocks until shutdown signal)
    let server_handle = tokio::spawn(server::run(wallet_state, cli.port, config));

    // Wait for ctrl-c
    tokio::signal::ctrl_c().await?;
    eprintln!("\nStopping monitor...");
    monitor.stop().await.map_err(|e| anyhow::anyhow!("{}", e))?;
    eprintln!("Monitor stopped");

    // The server task will also shut down via its own signal handler
    let _ = server_handle.await;

    Ok(())
}
