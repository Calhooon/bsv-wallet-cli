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
//! The configuration stays GLOBAL (services config, TLS, AUTH_TOKEN,
//! BIND_ADDR, monitor knobs: the process's one `Config`): the fleet is uniform by design, and `serve_fleet` REFUSES a seat
//! whose own .env diverges on a service-relevant key before this module ever
//! runs (fail-fast beats a silently misconfigured tenant).

use anyhow::{Context, Result};
use bsv_sdk::primitives::PrivateKey;
use bsv_sdk::wallet::{ListOutputsArgs, WalletInterface};
use bsv_wallet_toolbox::{
    ArcadeMonitorConfig, Chain, Monitor, MonitorOptions, Services, StorageSqlx, Wallet,
};
use std::sync::Arc;

use crate::config::Config;
use crate::server::{self, ServerConfig};
use crate::services_env;

/// A running tenant: the monitor (stop it on shutdown) + the server task.
pub struct TenantHandle {
    pub port: u16,
    pub monitor: Monitor<StorageSqlx, Services>,
    pub server: tokio::task::JoinHandle<Result<()>>,
}

/// Start one wallet's FULL daemon composition (monitor + reconcile task +
/// relay poller when configured + HTTP server) on `port`. Mirrors
/// `daemon::run` exactly, parameterized by (db, root key, port), under the
/// process's one configuration.
pub async fn start(
    db: &str,
    root_key_hex: &str,
    chain: Chain,
    port: u16,
    config: &Arc<Config>,
) -> Result<TenantHandle> {
    let root_key = PrivateKey::from_hex(root_key_hex)?;

    // One store over the file (`store::open_shared`): the wallet's handle
    // and the monitor's share one writer pool and one reservation set.
    let (storage, storage_arc) = crate::store::open_shared(db).await?;

    let db_path = db.to_string();
    let make_services = |chain: Chain| -> anyhow::Result<Services> {
        let opts = services_env::services_options(&config.services, chain, &db_path)?;
        Ok(Services::with_options(chain, opts)?)
    };

    let arcade_rt = services_env::arcade_runtime(&config.services, db)?;
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
    let services_arc = Arc::new(services);

    let mut monitor_opts = MonitorOptions::default();
    if let Some(ref rt) = arcade_rt {
        monitor_opts.arcade = Some(ArcadeMonitorConfig {
            url: rt.url.clone(),
            callback_token: rt.callback_token.clone(),
        });
    }
    if let Some(secs) = config.daemon.check_proofs_interval_secs {
        monitor_opts.tasks.check_for_proofs.interval = std::time::Duration::from_secs(secs);
        monitor_opts.tasks.check_for_proofs.start_immediately = true;
    }
    // The monitor's reorg and review tasks demote a proof whose root moved
    // and re-ask its courier once (toolbox 0.10.0 `reask_lost_proof`): with
    // either running, a `stale` episode's follow-up is theirs, never the
    // served pass's too (`TickOptions::monitor_runs`).
    let monitor_runs =
        monitor_opts.tasks.reorg.enabled || monitor_opts.tasks.review_proven_txs.enabled;
    let monitor = Monitor::with_options(storage_arc.clone(), services_arc.clone(), monitor_opts);
    monitor
        .start()
        .await
        .map_err(|e| anyhow::anyhow!("{}", e))?;
    eprintln!("[{port}] Monitor started");

    // The header service's chain-event feed: one stream, one stored
    // cursor, every envelope to the tracker (`chain_feed`). `off` has no
    // header service and so no feed.
    let feed_url = config.services.header_service()?;
    let feed_held = feed_url.is_some();
    if let Some(feed_url) = feed_url {
        let feed_storage = storage_arc.clone();
        let feed_services = services_arc.clone();
        let feed_tracker = crate::tracker_host::TickOptions::from_config(config);
        eprintln!("[{port}] Chain-event feed: {feed_url}/events/stream");
        tokio::spawn(async move {
            crate::chain_feed::run(
                &feed_url,
                &feed_storage,
                &*feed_services,
                &crate::tracker_host::SystemClock,
                &feed_tracker,
                &crate::chain_feed::FeedOptions::default(),
            )
            .await
        });
    }

    // The wallet takes its store and services by value: the store's other
    // handle (its chain tracker, set above, is every handle's) and its own
    // `Services`.
    let services2 = make_services(chain)?;
    let wallet = Wallet::new(Some(root_key), storage, services2)
        .await
        .map_err(|e| anyhow::anyhow!("{}", e))?;
    let wallet_state = server::make_wallet_state(wallet);

    let arcade_token = arcade_rt.as_ref().map(|rt| rt.callback_token.clone());
    let server_config = ServerConfig::from_config(config, chain, arcade_token);

    // The served pass, as `serve` runs it (`broadcast_reconcile`): the
    // tracker's pass (the wallet's creators tracked, a proven creator
    // adopted from its stored proof, the re-asks that are due), the sweep
    // and the spend guard, every `BROADCAST_RECONCILE_INTERVAL_SECS`
    // (default 60; `BROADCAST_RECONCILE=0` turns it off). No SSE drain
    // here: the monitor holds Arcade's stream. With the feed held, the pass
    // keeps no ring of tips: a reorg reaches the mined words through the
    // feed and the crate's own recheck, and the pass asks a proof only for
    // a word whose root moved (ruling (5)). A `stale` word's follow-up at
    // the broadcasters is the monitor's (`monitor_runs`). The task lives as
    // long as the process.
    let _served = crate::broadcast_reconcile::spawn_served_loop(
        wallet_state.clone(),
        config,
        None,
        feed_held,
        monitor_runs,
    );

    // Periodic UTXO count check + auto-reconcile abandoned txs (#18, THE
    // RELEASE RULE 2026-08-29) — per tenant, same knobs as daemon.
    let min_utxos = config.daemon.min_utxos;
    let check_wallet = wallet_state.clone();
    let reconcile_chain = chain;
    let reconcile_config = config.clone();
    let reconcile_enabled = config.daemon.reconcile_abandoned;
    let reconcile_min_age_secs = config.daemon.reconcile_abandoned_min_age_secs;
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
                    check_wallet.storage(),
                    check_wallet.services(),
                    reconcile_chain,
                    &reconcile_config,
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
                    reconcile_config.reconcile.absence_minutes,
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
    if let Some(relay_url) = &config.daemon.relay_url {
        if let Some(relay_token) = server_config.callback_token.clone() {
            let relay_wallet = wallet_state.clone();
            let poll_secs = config.daemon.relay_poll_secs;
            let relay_base = relay_url.trim_end_matches('/').to_string();
            eprintln!(
                "[{port}] Relay polling enabled: {} (every {poll_secs}s)",
                crate::config::shown_url(&relay_base)
            );
            tokio::spawn(async move {
                let client = reqwest::Client::new();
                let mut last_id: i64 = 0;
                let mut interval = tokio::time::interval(std::time::Duration::from_secs(poll_secs));
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
                                    let (cursor, store_busy) =
                                        ingest_relay_items(relay_wallet.storage(), &items, last_id)
                                            .await;
                                    last_id = cursor;
                                    if store_busy {
                                        // One extra RELAY_POLL_SECS tick backs off after
                                        // the store's five-second busy ceiling: no spin.
                                        interval.tick().await;
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

    let server = tokio::spawn(server::run(wallet_state, port, server_config));
    Ok(TenantHandle {
        port,
        monitor,
        server,
    })
}

/// Consume the relay's id-ordered batch, returning the next cursor and whether
/// a typed store busy stopped it. Later items wait behind that unacked item.
async fn ingest_relay_items(
    storage: &StorageSqlx,
    items: &[serde_json::Value],
    mut cursor: i64,
) -> (i64, bool) {
    for item in items {
        let id = item.get("id").and_then(|v| v.as_i64()).unwrap_or(cursor);
        if let Some(payload) = item.get("payload") {
            match crate::arc_ingest::ingest_arc_payload(storage, payload).await {
                Ok(action) => log_relay_action(&action),
                Err(e) if crate::store::is_store_busy(&e) => {
                    tracing::warn!(
                        id,
                        error = %e,
                        "relay: store busy; the cursor stays, the item is read again next poll"
                    );
                    return (cursor, true);
                }
                Err(e) => tracing::warn!(error = %e, "relay: payload ingest failed"),
            }
        }
        cursor = cursor.max(id);
    }
    (cursor, false)
}

/// Log a refused proof or word as a warning, keeping successful actions distinct.
fn log_relay_action(action: &crate::arc_ingest::IngestAction) {
    use crate::arc_ingest::IngestAction;

    match action {
        IngestAction::ProofRejected(_) | IngestAction::Refused(_) => {
            tracing::warn!(?action, "relay: payload ingested, proof or word refused");
        }
        _ => tracing::info!(?action, "relay: payload ingested"),
    }
}

/// Identity for a root key hex — used by callers for boot logging.
pub fn identity_of(root_key_hex: &str) -> Result<String> {
    let key = PrivateKey::from_hex(root_key_hex).context("bad ROOT_KEY hex")?;
    Ok(key.public_key().to_hex())
}

#[cfg(test)]
mod tests {
    use super::ingest_relay_items;
    use crate::arc_ingest::{ingest_arc_payload, IngestAction};
    use crate::test_support::{
        busy_read_wallet, response_answer, seed_arc_requests, set_read_lock,
    };
    use bsv_wallet_toolbox::storage::{entities::ProvenTxReqStatus, FindProvenTxReqsArgs};
    use bsv_wallet_toolbox::WalletStorageReader;
    use serde_json::{json, Value};

    struct RelayFixture {
        _dir: tempfile::TempDir,
        wallet: crate::server::handlers::WalletState,
        holder: sqlx::SqliteConnection,
        items: Vec<Value>,
    }

    impl RelayFixture {
        async fn new() -> Self {
            let (dir, wallet, mut holder) = busy_read_wallet().await;
            set_read_lock(&mut holder, false).await;
            let txids = ["31".repeat(32), "42".repeat(32)];
            seed_arc_requests(wallet.storage(), &txids).await;
            let items = [
                (40, &txids[0], "RECEIVED"),
                (41, &txids[0], "SEEN_ON_NETWORK"),
                (42, &txids[1], "RECEIVED"),
                (43, &txids[1], "SEEN_ON_NETWORK"),
            ]
            .map(|(id, txid, status)| {
                json!({"id": id, "payload": {"txid": txid, "txStatus": status}})
            })
            .to_vec();
            Self {
                _dir: dir,
                wallet,
                holder,
                items,
            }
        }

        async fn assert_statuses(&self, expected: ProvenTxReqStatus) {
            let requests = self
                .wallet
                .storage()
                .find_proven_tx_reqs(FindProvenTxReqsArgs {
                    status: Some(vec![expected]),
                    ..Default::default()
                })
                .await
                .unwrap();
            // The toolbox decodes unsent/unmined as Pending; filter the
            // stored status itself so that lossy decoding cannot pass this.
            assert_eq!(requests.len(), 2, "stored status must be {expected:?}");
        }
    }

    #[tokio::test]
    async fn relay_busy_cursor_retries_after_release_in_order() {
        let mut fixture = RelayFixture::new().await;
        set_read_lock(&mut fixture.holder, true).await;
        let (cursor, busy) = ingest_relay_items(fixture.wallet.storage(), &fixture.items, 39).await;
        assert_eq!(
            cursor, 40,
            "the busy item and later items must stay unacked"
        );
        assert!(busy);
        set_read_lock(&mut fixture.holder, false).await;
        fixture.assert_statuses(ProvenTxReqStatus::Unsent).await;
        assert_eq!(
            ingest_relay_items(fixture.wallet.storage(), &fixture.items, cursor).await,
            (43, false)
        );
        fixture.assert_statuses(ProvenTxReqStatus::Unmined).await;
    }

    /// Beta has no batch function. Reproduce its inline cursor rule to show
    /// the next pull acknowledges failed deliveries and can never retry them.
    #[tokio::test]
    async fn relay_beta_inline_logic_loses_busy_deliveries() {
        let mut fixture = RelayFixture::new().await;
        set_read_lock(&mut fixture.holder, true).await;
        let mut cursor = 39;
        for item in &fixture.items {
            let id = item.get("id").and_then(Value::as_i64).unwrap_or(cursor);
            if let Some(payload) = item.get("payload") {
                let _ = ingest_arc_payload(fixture.wallet.storage(), payload).await;
            }
            cursor = cursor.max(id);
        }
        assert_eq!(cursor, 43, "beta acknowledges both busy status deliveries");
        set_read_lock(&mut fixture.holder, false).await;
        let next_pull: Vec<_> = fixture
            .items
            .into_iter()
            .filter(|item| item["id"].as_i64().unwrap() > cursor)
            .collect();
        fixture.items = next_pull;
        assert_eq!(
            ingest_relay_items(fixture.wallet.storage(), &fixture.items, cursor).await,
            (43, false)
        );
        fixture.assert_statuses(ProvenTxReqStatus::Unsent).await;
    }

    #[tokio::test]
    async fn relay_malformed_payload_advances_cursor() {
        let fixture = RelayFixture::new().await;
        let mut items = fixture.items;
        items[1]["payload"]["blockHeight"] = json!("Store busy: database is locked (code: 517)");
        assert_eq!(
            ingest_relay_items(fixture.wallet.storage(), &items, 39).await,
            (43, false)
        );
        let requests = fixture
            .wallet
            .storage()
            .find_proven_tx_reqs(FindProvenTxReqsArgs {
                status: Some(vec![ProvenTxReqStatus::Unsent]),
                ..Default::default()
            })
            .await
            .unwrap();
        assert_eq!(requests.len(), 1, "the valid later status was ingested");
    }

    #[tokio::test]
    async fn relay_refused_proof_advances_cursor() {
        let fixture = RelayFixture::new().await;
        let mut item = fixture.items[1].clone();
        let payload = &mut item["payload"];
        payload["txStatus"] = json!("MINED");
        payload["merklePath"] = json!("deadbeef");
        payload["blockHeight"] = json!(850_000);
        payload["blockHash"] = json!("ab".repeat(32));
        assert!(matches!(
            ingest_arc_payload(fixture.wallet.storage(), payload).await,
            Ok(IngestAction::ProofRejected(_))
        ));
        assert_eq!(
            ingest_relay_items(fixture.wallet.storage(), &[item], 40).await,
            (41, false)
        );
    }

    #[tokio::test]
    async fn relay_ok_items_advance_cursor_and_store_status() {
        let fixture = RelayFixture::new().await;
        assert_eq!(
            ingest_relay_items(fixture.wallet.storage(), &fixture.items, 39).await,
            (43, false)
        );
        fixture.assert_statuses(ProvenTxReqStatus::Unmined).await;
    }

    async fn relay_response(app: &axum::Router, uri: &str, payload: Option<Value>) -> Value {
        use axum::{body::Body, http::Request};
        use tower::ServiceExt;

        let request = Request::builder()
            .method(if payload.is_some() { "POST" } else { "GET" })
            .uri(uri)
            .header("content-type", "application/json")
            .header("x-callbacktoken", "relay-test")
            .body(payload.map_or_else(Body::empty, |value| Body::from(value.to_string())))
            .unwrap();
        let (status, _, body) = response_answer(app.clone().oneshot(request).await.unwrap()).await;
        assert_eq!(status, axum::http::StatusCode::OK);
        body
    }

    #[tokio::test]
    async fn relay_returns_unacked_items_and_deletes_acked_items() {
        use bsv_wallet_cli::relay::{make_relay_router, open_relay_db, RelayState};

        let dir = tempfile::tempdir().unwrap();
        let pool = open_relay_db(dir.path().join("relay.db").to_str().unwrap())
            .await
            .unwrap();
        let app = make_relay_router(RelayState {
            pool,
            admin_token: None,
            require_register: false,
        });
        let pull = "/pull?token=relay-test&after=0&ack=0";
        assert_eq!(relay_response(&app, pull, None).await, json!([]));
        relay_response(
            &app,
            "/arc-callback",
            Some(json!({"txid": "31".repeat(32), "txStatus": "SEEN_ON_NETWORK"})),
        )
        .await;
        let items = relay_response(&app, pull, None).await;
        assert_eq!(items.as_array().unwrap().len(), 1);
        assert_eq!(relay_response(&app, pull, None).await, items);
        let cursor = items[0]["id"].as_i64().unwrap();
        let ack = format!("/pull?token=relay-test&after={cursor}&ack={cursor}");
        assert_eq!(relay_response(&app, &ack, None).await, json!([]));
        assert_eq!(relay_response(&app, pull, None).await, json!([]));
    }
}
