use anyhow::{Context, Result};
use bsv_sdk::primitives::PrivateKey;
use bsv_sdk::wallet::{ListOutputsArgs, WalletInterface};
use bsv_wallet_toolbox::{
    ArcadeMonitorConfig, Chain, Monitor, MonitorOptions, Services, StorageSqlx, Wallet,
    WalletStorageWriter,
};
use std::sync::Arc;

use crate::cli::Cli;
use crate::server::{self, ServerConfig, TlsConfig};
use crate::services_env;

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

    // Shared env-driven services config (CHAINTRACKS_URL, ARC_URL,
    // ARC_MODE=arcade, TAAL keys, callback token) — see services_env.rs.
    let db_path = cli.db.clone();
    let make_services = |chain: Chain| -> anyhow::Result<Services> {
        let opts = services_env::services_options_from_env(chain, &db_path)?;
        Ok(Services::with_options(chain, opts)?)
    };

    // Arcade V2 runtime (None in classic ARC mode).
    let arcade_rt = services_env::arcade_runtime(&cli.db)?;
    if let Some(ref rt) = arcade_rt {
        eprintln!("Arcade V2 mode: {} (SSE status stream enabled)", rt.url);
        if let Some(ref cb) = rt.public_callback_url {
            eprintln!("Arcade webhook: {} (X-CallbackUrl on submits)", cb);
        }
    }

    let services = make_services(chain)?;

    // Wire ChainTracker into storage for Layer 1 proof validation
    if let Some(ref ct) = services.chaintracks {
        storage.set_chain_tracker(ct.clone()).await;
    }

    let storage_arc = Arc::new(storage);
    let services_arc = Arc::new(services);

    // Start the monitor. In Arcade mode this additionally starts the
    // arcade_events task: outbound SSE subscription for push statuses
    // (SEEN_ON_NETWORK spendability gate) and event-driven proof fetch on
    // MINED — no schedule polling.
    let mut monitor_opts = MonitorOptions::default();
    if let Some(ref rt) = arcade_rt {
        monitor_opts.arcade = Some(ArcadeMonitorConfig {
            url: rt.url.clone(),
            callback_token: rt.callback_token.clone(),
        });
    }
    let monitor = Monitor::with_options(storage_arc.clone(), services_arc.clone(), monitor_opts);
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
    // /arc-callback is enabled when a callback token exists: always in Arcade
    // mode, or via explicit CALLBACK_TOKEN env with classic ARC.
    let callback_token = arcade_rt
        .as_ref()
        .map(|rt| rt.callback_token.clone())
        .or_else(|| {
            std::env::var("CALLBACK_TOKEN")
                .ok()
                .filter(|s| !s.is_empty())
        });

    // BIND_ADDR=0.0.0.0 for the tunnel/public-webhook case; default stays
    // loopback-only.
    let bind_addr: std::net::IpAddr = std::env::var("BIND_ADDR")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or_else(|| std::net::IpAddr::from([127, 0, 0, 1]));

    let config = ServerConfig {
        auth_token: std::env::var("AUTH_TOKEN").ok(),
        tls,
        chain,
        bind_addr,
        callback_token,
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

    // Proof-delivery ladder rung (c): drain a bsv-wallet-relay queue.
    // RELAY_URL points at a shared public callback receiver; we poll OUTBOUND
    // with our callback token and push each queued ARC/Arcade payload through
    // the same ingest path as the direct /arc-callback webhook.
    if let Ok(relay_url) = std::env::var("RELAY_URL") {
        if !relay_url.is_empty() {
            if let Some(relay_token) = config.callback_token.clone() {
                let relay_wallet = wallet_state.clone();
                let poll_secs: u64 = std::env::var("RELAY_POLL_SECS")
                    .ok()
                    .and_then(|v| v.parse().ok())
                    .unwrap_or(10);
                let relay_base = relay_url.trim_end_matches('/').to_string();
                eprintln!("Relay polling enabled: {relay_base} (every {poll_secs}s)");
                tokio::spawn(async move {
                    let client = reqwest::Client::new();
                    let mut last_id: i64 = 0;
                    let mut interval =
                        tokio::time::interval(std::time::Duration::from_secs(poll_secs));
                    loop {
                        interval.tick().await;
                        let url = format!(
                            "{}/pull?token={}&after={}&ack={}",
                            relay_base, relay_token, last_id, last_id
                        );
                        match client.get(&url).send().await {
                            Ok(resp) if resp.status().is_success() => {
                                match resp.json::<Vec<serde_json::Value>>().await {
                                    Ok(items) => {
                                        for item in items {
                                            let id = item
                                                .get("id")
                                                .and_then(|v| v.as_i64())
                                                .unwrap_or(last_id);
                                            if let Some(payload) = item.get("payload") {
                                                match crate::arc_ingest::ingest_arc_payload(
                                                    relay_wallet.storage(),
                                                    payload,
                                                )
                                                .await
                                                {
                                                    Ok(action) => tracing::info!(
                                                        ?action,
                                                        "relay: payload ingested"
                                                    ),
                                                    Err(e) => tracing::warn!(
                                                        error = %e,
                                                        "relay: payload ingest failed"
                                                    ),
                                                }
                                            }
                                            last_id = last_id.max(id);
                                        }
                                    }
                                    Err(e) => {
                                        tracing::debug!(error = %e, "relay: pull parse failed")
                                    }
                                }
                            }
                            Ok(resp) => {
                                tracing::debug!(status = %resp.status(), "relay: pull failed")
                            }
                            Err(e) => tracing::debug!(error = %e, "relay: pull failed"),
                        }
                    }
                });
            } else {
                tracing::warn!(
                    "RELAY_URL set but no callback token available — relay polling disabled"
                );
            }
        }
    }

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
