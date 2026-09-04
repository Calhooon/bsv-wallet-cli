//! The per-wallet daemon composition, extracted from `daemon.rs` so ONE
//! process can run it for MANY wallets (`serve-fleet --daemon`, bsv-low plan
//! row D2 follow-up 2026-09-01).
//!
//! WHY the fleet needs DAEMON semantics, not bare `serve`: the harness's
//! serveWallet comment (bsv-low app/e2e/lib/cli.ts) — bare serve never proves
//! 0-conf ancestry, so back-to-back hands pile deep "unproven ancestor" BEEF
//! chains until createAction 502s mid-hand. Every tenant here gets its OWN
//! monitor (proof fetches, Arcade SSE when configured), its own auto-reconcile
//! task, and its own HTTP server — the same composition `daemon` runs for one
//! wallet, N times on one runtime.
//!
//! Env stays GLOBAL (services config, TLS, AUTH_TOKEN, BIND_ADDR, monitor
//! knobs): the fleet is uniform by design, and `serve_fleet` REFUSES a seat
//! whose own .env diverges on a service-relevant key before this module ever
//! runs (fail-fast beats a silently misconfigured tenant).

use anyhow::{Context, Result};
use bsv_sdk::primitives::PrivateKey;
use bsv_sdk::wallet::{ListOutputsArgs, WalletInterface};
use bsv_wallet_toolbox::{
    ArcadeMonitorConfig, Chain, Monitor, MonitorOptions, Services, StorageSqlx, Wallet,
    WalletStorageWriter,
};
use std::sync::Arc;

use crate::server::{self, ServerConfig, TlsConfig};
use crate::services_env;

/// A running tenant: the monitor (stop it on shutdown) + the server task.
pub struct TenantHandle {
    pub port: u16,
    pub monitor: Monitor<StorageSqlx, Services>,
    pub server: tokio::task::JoinHandle<Result<()>>,
}

/// Start one wallet's FULL daemon composition (monitor + reconcile task +
/// relay poller when configured + HTTP server) on `port`. Mirrors
/// `daemon::run` exactly, parameterized by (db, root key, port).
pub async fn start(db: &str, root_key_hex: &str, chain: Chain, port: u16) -> Result<TenantHandle> {
    let root_key = PrivateKey::from_hex(root_key_hex)?;

    let storage = StorageSqlx::open(db).await?;
    storage.make_available().await?;
    // busy_timeout: concurrent writes wait instead of SQLITE_BUSY (error 517).
    sqlx::query("PRAGMA busy_timeout = 500")
        .execute(storage.pool())
        .await
        .ok();

    let db_path = db.to_string();
    let make_services = |chain: Chain| -> anyhow::Result<Services> {
        let opts = services_env::services_options_from_env(chain, &db_path)?;
        Ok(Services::with_options(chain, opts)?)
    };

    let arcade_rt = services_env::arcade_runtime(db)?;
    if let Some(ref rt) = arcade_rt {
        eprintln!(
            "[{port}] Arcade V2 mode: {} (SSE status stream enabled)",
            rt.url
        );
        if let Some(ref cb) = rt.public_callback_url {
            eprintln!("[{port}] Arcade webhook: {cb} (X-CallbackUrl on submits)");
        }
    }

    let services = make_services(chain)?;
    if let Some(ref ct) = services.chaintracks {
        storage.set_chain_tracker(ct.clone()).await;
    }
    let storage_arc = Arc::new(storage);
    let services_arc = Arc::new(services);

    let mut monitor_opts = MonitorOptions::default();
    if let Some(ref rt) = arcade_rt {
        monitor_opts.arcade = Some(ArcadeMonitorConfig {
            url: rt.url.clone(),
            callback_token: rt.callback_token.clone(),
        });
    }
    if let Some(secs) = std::env::var("MONITOR_CHECK_PROOFS_INTERVAL_SECS")
        .ok()
        .and_then(|v| v.parse::<u64>().ok())
        .filter(|s| *s > 0)
    {
        monitor_opts.tasks.check_for_proofs.interval = std::time::Duration::from_secs(secs);
        monitor_opts.tasks.check_for_proofs.start_immediately = true;
    }
    let monitor = Monitor::with_options(storage_arc.clone(), services_arc.clone(), monitor_opts);
    monitor
        .start()
        .await
        .map_err(|e| anyhow::anyhow!("{}", e))?;
    eprintln!("[{port}] Monitor started");

    // The wallet needs its own storage/services (Wallet::new takes owned).
    let storage2 = StorageSqlx::open(db).await?;
    storage2.make_available().await?;
    sqlx::query("PRAGMA busy_timeout = 500")
        .execute(storage2.pool())
        .await
        .ok();
    let services2 = make_services(chain)?;
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
    let callback_token = arcade_rt
        .as_ref()
        .map(|rt| rt.callback_token.clone())
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
        chain,
        bind_addr,
        callback_token,
    };

    // Periodic UTXO count check + auto-reconcile abandoned txs (#18, THE
    // RELEASE RULE 2026-08-29) — per tenant, same knobs as daemon.
    let min_utxos: usize = std::env::var("MIN_UTXOS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(3);
    let check_wallet = wallet_state.clone();
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
                            port = port,
                            "Low UTXO count! Run `bsv-wallet split --count {}` to create more.",
                            min_utxos
                        );
                    }
                }
                Err(e) => {
                    tracing::debug!("UTXO check failed: {}", e);
                }
            }
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
                            absent = r.abandoned.len(),
                            absent_past_threshold = r.absent_past_threshold.len(),
                            conflicted = r.conflicted.len(),
                            reqs_retired = r.reqs_retired,
                            stale_reqs_retired = r.stale_reqs_retired.len(),
                            restored_count = r.restored_count,
                            restored_sats = r.restored_sats,
                            phantom_count = r.phantom_count,
                            phantom_sats = r.phantom_sats,
                            port = port,
                            "auto-reconciled abandoned tx(s): marked {} failed, retired {} proof request(s), restored {} input(s) ({} sats), invalidated {} phantom output(s) ({} sats)",
                            r.failed, r.reqs_retired, r.restored_count, r.restored_sats, r.phantom_count, r.phantom_sats
                        );
                    }
                    Ok(r) => {
                        if !r.absent_on_clock.is_empty() || !r.stale_reqs_known.is_empty() {
                            tracing::info!(
                                on_clock = ?r.absent_on_clock,
                                stale_reqs_known = ?r.stale_reqs_known,
                                port = port,
                                "abandoned-tx reconcile: absence clock running, or a failed transaction the chain index knows"
                            );
                        }
                    }
                    Err(e) => {
                        tracing::debug!("abandoned-tx reconcile skipped: {}", e);
                    }
                }
                // Poisoned chains: whatever a verdict (SSE, webhook, the
                // sweep above) failed, its unproven descendants go with it.
                match crate::broadcast_reconcile::run_sweep(
                    check_wallet.storage(),
                    check_wallet.services(),
                    true,
                )
                .await
                {
                    Ok(sweep) => {
                        if sweep.locked.restored > 0 || sweep.locked.spent > 0 {
                            tracing::warn!(
                                restored = sweep.locked.restored,
                                restored_sats = sweep.locked.restored_sats,
                                spent = sweep.locked.spent,
                                unknown = sweep.locked.unknown,
                                port = port,
                                "locked inputs re-checked"
                            );
                        }
                        let reports = sweep.poison;
                        for r in reports
                            .iter()
                            .filter(|r| r.outcome == bsv_wallet_toolbox::PoisonOutcome::Retired)
                        {
                            tracing::warn!(
                                root = %r.root,
                                txs = r.retirable_txids().len(),
                                restored = r.restored,
                                invalidated = r.invalidated,
                                internalized = r.internalized.len(),
                                port = port,
                                "auto-retired a poisoned chain"
                            );
                        }
                    }
                    Err(e) => tracing::debug!("poisoned-chain sweep skipped: {}", e),
                }
            }
        }
    });

    // Proof-delivery ladder rung (c): drain a bsv-wallet-relay queue.
    if let Ok(relay_url) = std::env::var("RELAY_URL") {
        if !relay_url.is_empty() {
            if let Some(relay_token) = config.callback_token.clone() {
                let relay_wallet = wallet_state.clone();
                let poll_secs: u64 = std::env::var("RELAY_POLL_SECS")
                    .ok()
                    .and_then(|v| v.parse().ok())
                    .unwrap_or(10);
                let relay_base = relay_url.trim_end_matches('/').to_string();
                eprintln!("[{port}] Relay polling enabled: {relay_base} (every {poll_secs}s)");
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

    let server = tokio::spawn(server::run(wallet_state, port, config));
    Ok(TenantHandle {
        port,
        monitor,
        server,
    })
}

/// Identity for a root key hex — used by callers for boot logging.
pub fn identity_of(root_key_hex: &str) -> Result<String> {
    let key = PrivateKey::from_hex(root_key_hex).context("bad ROOT_KEY hex")?;
    Ok(key.public_key().to_hex())
}
