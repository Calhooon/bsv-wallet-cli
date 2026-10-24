//! `serve-fleet` — ONE process serving MANY wallets (bsv-low plan row D2,
//! runner item 0d, 2026-09-01).
//!
//! The fleet box ran 32 separate `bsv-wallet serve` processes (one per seat),
//! each paying the full runtime overhead on a memory-bound 8c/16G shape. This
//! command consolidates them: each `--wallet <seat-dir>:<port>` serves
//! `<seat-dir>/wallet.db` on its OWN port with the ROOT_KEY read from
//! `<seat-dir>/.env` — the per-seat `http://127.0.0.1:<port>` contract the
//! harness pins is UNCHANGED (process consolidation only, zero harness edits).
//!
//! Key handling: per-wallet secrets cannot ride process env when one process
//! holds many wallets, so each seat's `.env` is read by path (dotenvy iterator
//! — never loaded into the process environment; keys stay scoped to their
//! context, exactly the isolation the per-process shape had). Global env
//! (ARC_URL, CHAINTRACKS_URL, AUTH_TOKEN, BIND_ADDR, TLS_*) still applies to
//! every tenant, matching the fleet's identical-services reality.
//!
//! Failure semantics: the process is ONE supervision unit — if any tenant's
//! server exits (bind failure, fatal error), the whole process exits nonzero
//! and the runner restarts the fleet. No tenant is silently missing: a fleet
//! where seat 7 quietly died is exactly the "unknown reads as fine" shape the
//! harness doctrine bans.
//!
//! `--daemon` runs each tenant's FULL daemon composition (its own monitor,
//! auto-reconcile task, relay poller when configured) via
//! `daemon_tenant::start` — the mode the bsv-low fleet box needs: bare
//! HTTP-only tenants never prove 0-conf ancestry and the deep-ancestry
//! createAction-502 class returns (the serveWallet lesson). HTTP-only mode
//! stays for wallets something else keeps proven.

use std::future::Future;
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{bail, Context, Result};

use crate::config::Config;
use crate::context::WalletContext;
use crate::server::{self, ServerConfig};

/// One `--wallet` spec, parsed.
#[derive(Debug, PartialEq, Eq)]
pub struct FleetEntry {
    pub dir: PathBuf,
    pub port: u16,
}

/// Parse `<seat-dir>:<port>`. The LAST colon splits (a path may not carry a
/// colon on the platforms we run, but being deliberate costs nothing).
pub fn parse_entry(spec: &str) -> Result<FleetEntry> {
    let (dir, port) = spec
        .rsplit_once(':')
        .with_context(|| format!("--wallet '{spec}': expected <seat-dir>:<port>"))?;
    if dir.is_empty() {
        bail!("--wallet '{spec}': empty seat dir");
    }
    let port: u16 = port
        .parse()
        .with_context(|| format!("--wallet '{spec}': '{port}' is not a port"))?;
    if port == 0 {
        bail!("--wallet '{spec}': port 0 is not servable");
    }
    Ok(FleetEntry {
        dir: PathBuf::from(dir),
        port,
    })
}

/// Parse + validate the whole fleet: at least one wallet, no duplicate port,
/// no duplicate dir (two tenants on one SQLite db is a corruption invitation).
pub fn parse_fleet(specs: &[String]) -> Result<Vec<FleetEntry>> {
    if specs.is_empty() {
        bail!("serve-fleet needs at least one --wallet <seat-dir>:<port>");
    }
    let entries: Vec<FleetEntry> = specs
        .iter()
        .map(|s| parse_entry(s))
        .collect::<Result<_>>()?;
    for (i, a) in entries.iter().enumerate() {
        for b in &entries[i + 1..] {
            if a.port == b.port {
                bail!(
                    "duplicate port {} ({} and {})",
                    a.port,
                    a.dir.display(),
                    b.dir.display()
                );
            }
            if a.dir == b.dir {
                bail!(
                    "duplicate seat dir {} (ports {} and {})",
                    a.dir.display(),
                    a.port,
                    b.port
                );
            }
        }
    }
    Ok(entries)
}

/// Read ROOT_KEY from a seat's `.env` WITHOUT touching process env.
pub fn root_key_from_env_file(env_path: &Path) -> Result<String> {
    let iter = dotenvy::from_path_iter(env_path)
        .with_context(|| format!("reading {}", env_path.display()))?;
    for item in iter {
        let (k, v) = item.with_context(|| format!("parsing {}", env_path.display()))?;
        if k == "ROOT_KEY" && !v.is_empty() {
            return Ok(v);
        }
    }
    bail!("no ROOT_KEY in {}", env_path.display())
}

/// Service-relevant env keys the process's configuration reads (`Config`).
/// A per-seat `.env` line for one of these would have applied under the
/// per-process shape but is INVISIBLE to a shared process — so a divergent
/// value must refuse the fleet rather than silently misconfigure one tenant.
/// (ROOT_KEY is deliberately absent: it is read per-tenant by design.)
pub const SHARED_ENV_KEYS: &[&str] = &[
    "ARC_MODE",
    "ARCADE",
    "ARC_URL",
    "PUBLIC_CALLBACK_URL",
    "CALLBACK_TOKEN",
    "CHAINTRACKS_URL",
    "BREAK_GLASS_EXPLORER_HEADERS",
    "TAAL_API_KEY",
    "MAIN_TAAL_API_KEY",
    "AUTH_TOKEN",
    "BIND_ADDR",
    "RELAY_URL",
];

/// Refuse a seat whose .env sets a SHARED key to a value differing from the
/// process env as the configuration read it at start (values never printed
/// — these are secrets).
pub fn assert_env_uniform(env_path: &Path, config: &Config) -> Result<()> {
    let iter = dotenvy::from_path_iter(env_path)
        .with_context(|| format!("reading {}", env_path.display()))?;
    for item in iter {
        let (k, v) = item.with_context(|| format!("parsing {}", env_path.display()))?;
        if SHARED_ENV_KEYS.contains(&k.as_str()) {
            match config.raw(&k) {
                Some(procv) if procv == v => {}
                _ => bail!(
                    "fleet env divergence: {} sets {} to a value the shared process env does not carry — \
                     under per-process daemons that line applied; in one process it would be silently ignored. \
                     Export it globally (identical fleet-wide) or drop the per-seat line.",
                    env_path.display(),
                    k
                ),
            }
        }
    }
    Ok(())
}

/// Build one tenant's ServerConfig — the same shape `serve` builds, with
/// the callback token resolved per-db (Arcade runtime is db-anchored).
fn tenant_config(ctx: &WalletContext) -> Result<ServerConfig> {
    let arcade_token = crate::services_env::arcade_runtime(&ctx.config.services, &ctx.db_path)?
        .map(|rt| rt.callback_token);
    Ok(ServerConfig::from_config(
        &ctx.config,
        ctx.chain,
        arcade_token,
    ))
}

/// The daemon mode's join: the first tenant's server to end decides the
/// fleet's exit (as it always has); then every tenant's monitor is stopped,
/// all at once, each under `grace` (`daemon::stop_within_grace`), so the
/// fleet's exit is bounded by one grace whatever the number of tenants. A
/// monitor whose stop fails or outlasts the grace is logged at error level
/// (marker `daemon_fault`) under its tenant's name and never holds the exit
/// (#182).
async fn join_daemons<S, F>(
    mut ended: tokio::sync::mpsc::Receiver<(u16, Result<()>)>,
    monitors: Vec<(String, S)>,
    grace: Duration,
) -> Result<()>
where
    S: FnOnce() -> F,
    F: Future<Output = Result<()>>,
{
    let outcome = ended.recv().await;
    // Every stop polled on this task at once (no spawn: rule A6), each one
    // bounded by its own grace.
    let mut stops: Vec<_> = monitors
        .into_iter()
        .map(|(who, stop)| {
            Box::pin(async move { super::daemon::stop_within_grace(&who, stop, grace).await })
        })
        .collect();
    std::future::poll_fn(|cx| {
        stops.retain_mut(|stop| stop.as_mut().poll(cx).is_pending());
        if stops.is_empty() {
            std::task::Poll::Ready(())
        } else {
            std::task::Poll::Pending
        }
    })
    .await;
    fleet_outcome(outcome)
}

/// The fleet's word on the tenant exit that ended it, read the same in both
/// modes.
fn fleet_outcome(outcome: Option<(u16, Result<()>)>) -> Result<()> {
    match outcome {
        Some((port, Ok(()))) => {
            // A clean exit (the graceful shutdown signal) ends the whole fleet.
            eprintln!("[serve-fleet] port {port} exited cleanly — shutting the fleet down");
            Ok(())
        }
        Some((port, Err(e))) => bail!("tenant on port {port} failed: {e:#}"),
        None => bail!("serve-fleet: no tenants ran"),
    }
}

pub async fn run(specs: &[String], daemon: bool, config: &std::sync::Arc<Config>) -> Result<()> {
    let entries = parse_fleet(specs)?;
    let n = entries.len();
    let chain = config.chain();
    let testnet = chain == bsv_wallet_toolbox::Chain::Test;
    tracing::info!("config: {}", config.log_line());

    // Validate EVERY seat (env uniformity + key present) BEFORE anything binds:
    // a fleet that cannot fully start must not half-start (the runner would
    // read partial as healthy).
    let mut keys = Vec::with_capacity(n);
    for e in &entries {
        let env_path = e.dir.join(".env");
        assert_env_uniform(&env_path, config)?;
        keys.push(root_key_from_env_file(&env_path)?);
    }

    if daemon {
        // FULL daemon composition per tenant (monitor + reconcile + server).
        let mut monitors = Vec::with_capacity(n);
        let (tx, rx) = tokio::sync::mpsc::channel::<(u16, Result<()>)>(n.max(1));
        for (e, key) in entries.iter().zip(&keys) {
            let db = e.dir.join("wallet.db");
            let db_str = db
                .to_str()
                .with_context(|| format!("non-UTF8 path {}", db.display()))?;
            let ident = super::daemon_tenant::identity_of(key)?;
            eprintln!(
                "[serve-fleet] {} → port {} (identity {}…, daemon)",
                e.dir.display(),
                e.port,
                &ident[..12.min(ident.len())]
            );
            let tenant = super::daemon_tenant::start(db_str, key, chain, e.port, config)
                .await
                .with_context(|| format!("starting tenant {}", e.dir.display()))?;
            let port = tenant.port;
            let server = tenant.server;
            let monitor = tenant.monitor;
            monitors.push((
                format!("serve-fleet: tenant {} (port {port})", e.dir.display()),
                move || async move { monitor.stop().await.map_err(anyhow::Error::from) },
            ));
            let tx = tx.clone();
            tokio::spawn(async move {
                let res = match server.await {
                    Ok(r) => r,
                    Err(join) => Err(anyhow::anyhow!("server task panicked: {join}")),
                };
                let _ = tx.send((port, res)).await;
            });
        }
        drop(tx);
        eprintln!("[serve-fleet] {n} wallet(s) serving (daemon mode)");
        return join_daemons(rx, monitors, super::daemon::SHUTDOWN_GRACE).await;
    }

    // HTTP-only mode.
    let mut tenants = Vec::with_capacity(n);
    for (e, key) in entries.iter().zip(&keys) {
        let db = e.dir.join("wallet.db");
        let db_str = db
            .to_str()
            .with_context(|| format!("non-UTF8 path {}", db.display()))?;
        let ctx = WalletContext::load_with(db_str, key, testnet, false, config)
            .await
            .with_context(|| format!("loading wallet {}", e.dir.display()))?;
        eprintln!(
            "[serve-fleet] {} → port {} (identity {}…)",
            e.dir.display(),
            e.port,
            &ctx.identity_key[..12.min(ctx.identity_key.len())]
        );
        tenants.push((ctx, e.port));
    }

    // One exit ends the fleet (fail-fast supervision unit).
    let (tx, mut rx) = tokio::sync::mpsc::channel::<(u16, Result<()>)>(n.max(1));
    for (ctx, port) in tenants {
        let server_config = tenant_config(&ctx)?;
        let db_path = ctx.db_path.clone();
        let state = server::make_wallet_state(ctx.wallet);
        // Bare serve has no monitor: each tenant gets its own served loop
        // (the SSE drain, the tracker's pass, the sweep; every 60 s).
        let reconcile =
            crate::broadcast_reconcile::spawn_serve_loop(state.clone(), config, &db_path);
        let tx = tx.clone();
        tokio::spawn(async move {
            let res = server::run(state, port, server_config).await;
            if let Some(handle) = reconcile {
                handle.abort();
            }
            let _ = tx.send((port, res)).await;
        });
    }
    drop(tx);
    eprintln!("[serve-fleet] {n} wallet(s) serving");

    fleet_outcome(rx.recv().await)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    #[test]
    fn parses_dir_colon_port() {
        let e = parse_entry("e2e/wallets/.fleet/p7:3407").unwrap();
        assert_eq!(e.dir, PathBuf::from("e2e/wallets/.fleet/p7"));
        assert_eq!(e.port, 3407);
    }

    #[test]
    fn refuses_bad_specs() {
        assert!(parse_entry("no-port").is_err());
        assert!(parse_entry(":3407").is_err());
        assert!(parse_entry("dir:notaport").is_err());
        assert!(parse_entry("dir:0").is_err());
    }

    #[test]
    fn refuses_duplicate_port_and_dir() {
        let dup_port = ["a:3401".to_string(), "b:3401".to_string()];
        assert!(parse_fleet(&dup_port)
            .unwrap_err()
            .to_string()
            .contains("duplicate port"));
        let dup_dir = ["a:3401".to_string(), "a:3402".to_string()];
        assert!(parse_fleet(&dup_dir)
            .unwrap_err()
            .to_string()
            .contains("duplicate seat dir"));
        assert!(parse_fleet(&[]).is_err());
    }

    #[test]
    fn env_divergence_refused_by_name_secrets_never_printed() {
        let dir = std::env::temp_dir().join(format!("serve-fleet-env-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let env_path = dir.join(".env");
        std::fs::write(
            &env_path,
            "ROOT_KEY=aa\nTAAL_API_KEY=seat-specific-secret\n",
        )
        .unwrap();
        // process env does not carry TAAL_API_KEY (or carries another value):
        let none = Config::default();
        let err = assert_env_uniform(&env_path, &none)
            .unwrap_err()
            .to_string();
        assert!(err.contains("fleet env divergence"), "{err}");
        assert!(err.contains("TAAL_API_KEY"), "{err}");
        assert!(
            !err.contains("seat-specific-secret"),
            "secret leaked: {err}"
        );
        // matching value passes; ROOT_KEY alone always passes
        let same = Config::from_pairs(&[("TAAL_API_KEY", "seat-specific-secret")]).unwrap();
        assert!(assert_env_uniform(&env_path, &same).is_ok());
        std::fs::write(&env_path, "ROOT_KEY=aa\n").unwrap();
        assert!(assert_env_uniform(&env_path, &none).is_ok());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn reads_root_key_without_touching_process_env() {
        let dir = std::env::temp_dir().join(format!("serve-fleet-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let env_path = dir.join(".env");
        let mut f = std::fs::File::create(&env_path).unwrap();
        writeln!(f, "# seat p9").unwrap();
        writeln!(f, "SOMETHING_ELSE=x").unwrap();
        writeln!(f, "ROOT_KEY=deadbeef01").unwrap();
        drop(f);
        assert_eq!(root_key_from_env_file(&env_path).unwrap(), "deadbeef01");
        // the read must NOT leak into process env (per-tenant isolation)
        assert!(std::env::var("SOMETHING_ELSE").is_err());
        let missing = dir.join("missing.env");
        assert!(root_key_from_env_file(&missing).is_err());
        std::fs::remove_dir_all(&dir).ok();
    }

    type Stop =
        Box<dyn FnOnce() -> std::pin::Pin<Box<dyn Future<Output = Result<()>> + Send>> + Send>;

    /// A tenant's monitor that stops at once and says so.
    fn clean(name: &str, stopped: &Arc<AtomicUsize>) -> (String, Stop) {
        let stopped = stopped.clone();
        (
            name.to_string(),
            Box::new(move || {
                Box::pin(async move {
                    stopped.fetch_add(1, Ordering::SeqCst);
                    Ok(())
                })
            }),
        )
    }

    /// A tenant's monitor that never returns from its stop.
    fn hung(name: &str) -> (String, Stop) {
        (
            name.to_string(),
            Box::new(|| Box::pin(std::future::pending())),
        )
    }

    /// A tenant's monitor whose stop fails at once.
    fn refused(name: &str) -> (String, Stop) {
        (
            name.to_string(),
            Box::new(|| Box::pin(async { Err(anyhow::anyhow!("stop refused")) })),
        )
    }

    /// A tenant's monitor whose stop waits at `meet` for every other stop,
    /// counts itself in `passed`, then never returns. Stops run at once all
    /// pass `meet`; stops run one after another do not (each waits on peers
    /// not yet polled until its grace drops it, and only the last arrival
    /// passes, so `passed` is 1).
    fn hung_after_meeting(
        name: &str,
        meet: &Arc<tokio::sync::Barrier>,
        passed: &Arc<AtomicUsize>,
    ) -> (String, Stop) {
        let (meet, passed) = (meet.clone(), passed.clone());
        (
            name.to_string(),
            Box::new(move || {
                Box::pin(async move {
                    meet.wait().await;
                    passed.fetch_add(1, Ordering::SeqCst);
                    std::future::pending().await
                })
            }),
        )
    }

    /// The fleet's channel with the first tenant's end already in it.
    fn ended(port: u16, res: Result<()>) -> tokio::sync::mpsc::Receiver<(u16, Result<()>)> {
        let (tx, rx) = tokio::sync::mpsc::channel(1);
        tx.try_send((port, res)).unwrap();
        rx
    }

    /// The fleet's exit after `first` ended, held to `bound` of real clock.
    async fn fleet_exit(
        bound: Duration,
        first: (u16, Result<()>),
        monitors: Vec<(String, Stop)>,
        grace: Duration,
    ) -> Result<()> {
        tokio::time::timeout(
            bound,
            join_daemons(ended(first.0, first.1), monitors, grace),
        )
        .await
        .expect("the fleet exits within its bound")
    }

    #[tokio::test]
    async fn a_fleet_whose_monitor_hangs_exits_within_the_grace_naming_that_tenant() {
        let (markers, _guard) = crate::test_support::Markers::capture();
        let grace = Duration::from_millis(50);
        let stopped = Arc::new(AtomicUsize::new(0));
        let monitors = vec![
            clean("serve-fleet: tenant a (port 3401)", &stopped),
            hung("serve-fleet: tenant b (port 3402)"),
            clean("serve-fleet: tenant c (port 3403)", &stopped),
        ];
        let exit = fleet_exit(
            grace + Duration::from_secs(1),
            (3401, Ok(())),
            monitors,
            grace,
        )
        .await;
        assert!(exit.is_ok(), "the first end still decides: {exit:?}");
        assert_eq!(
            stopped.load(Ordering::SeqCst),
            2,
            "the clean monitors stopped"
        );
        assert_eq!(
            markers.fault_line().0,
            "serve-fleet: tenant b (port 3402): the monitor did not stop within the grace"
        );
    }

    #[tokio::test]
    async fn the_fleets_stops_run_at_once_so_its_exit_is_one_grace_not_n() {
        let (markers, _guard) = crate::test_support::Markers::capture();
        let grace = Duration::from_millis(300);
        let meet = Arc::new(tokio::sync::Barrier::new(3));
        let passed = Arc::new(AtomicUsize::new(0));
        let monitors = vec![
            hung_after_meeting("serve-fleet: tenant a (port 3401)", &meet, &passed),
            hung_after_meeting("serve-fleet: tenant b (port 3402)", &meet, &passed),
            hung_after_meeting("serve-fleet: tenant c (port 3403)", &meet, &passed),
        ];
        let started = tokio::time::Instant::now();
        // The clock bounds liveness only (never forever); that the stops run
        // at once is the rendezvous below, which no runner's load can move.
        let exit = fleet_exit(
            Duration::from_secs(30),
            (3402, Err(anyhow::anyhow!("Address already in use"))),
            monitors,
            grace,
        )
        .await;
        assert!(started.elapsed() >= grace);
        assert_eq!(
            passed.load(Ordering::SeqCst),
            3,
            "every stop was running at once, so the fleet waited one grace, not three"
        );
        let words = format!(
            "{:#}",
            exit.expect_err("the failed tenant is the fleet's error")
        );
        assert!(words.contains("tenant on port 3402 failed"), "{words}");
        assert!(words.contains("Address already in use"), "{words}");
        let mut named: Vec<String> = markers
            .named("daemon_fault")
            .into_iter()
            .filter_map(|m| m.message)
            .collect();
        named.sort();
        assert_eq!(
            named,
            ["a (port 3401)", "b (port 3402)", "c (port 3403)"].map(|t| format!(
                "serve-fleet: tenant {t}: the monitor did not stop within the grace"
            ))
        );
    }

    #[tokio::test]
    async fn a_fleet_whose_monitor_refuses_its_stop_exits_naming_that_tenant_and_the_refusal() {
        let stopped = Arc::new(AtomicUsize::new(0));
        let (markers, _guard) = crate::test_support::Markers::capture();
        let monitors = vec![
            refused("serve-fleet: tenant b (port 3402)"),
            clean("serve-fleet: tenant a (port 3401)", &stopped),
        ];
        let exit = fleet_exit(
            Duration::from_secs(30),
            (3401, Ok(())),
            monitors,
            super::super::daemon::SHUTDOWN_GRACE,
        )
        .await;
        assert!(
            exit.is_ok(),
            "the refused stop is not the fleet's answer: {exit:?}"
        );
        assert_eq!(
            stopped.load(Ordering::SeqCst),
            1,
            "the clean monitor stopped"
        );
        let (message, error) = markers.fault_line();
        assert_eq!(
            message,
            "serve-fleet: tenant b (port 3402): the monitor did not stop cleanly"
        );
        assert!(error.contains("stop refused"), "{error}");
    }

    #[tokio::test]
    async fn a_fleet_of_clean_monitors_exits_as_today() {
        let (markers, _guard) = crate::test_support::Markers::capture();
        let stopped = Arc::new(AtomicUsize::new(0));
        let monitors = vec![
            clean("serve-fleet: tenant a (port 3401)", &stopped),
            clean("serve-fleet: tenant b (port 3402)", &stopped),
        ];
        let exit = fleet_exit(
            Duration::from_secs(1),
            (3402, Ok(())),
            monitors,
            super::super::daemon::SHUTDOWN_GRACE,
        )
        .await;
        assert!(exit.is_ok(), "{exit:?}");
        assert_eq!(stopped.load(Ordering::SeqCst), 2);
        assert!(markers.named("daemon_fault").is_empty());

        let failed = join_daemons(
            ended(3401, Err(anyhow::anyhow!("listener fault"))),
            vec![clean("serve-fleet: tenant a (port 3401)", &stopped)],
            super::super::daemon::SHUTDOWN_GRACE,
        )
        .await
        .expect_err("a failed tenant is the fleet's error, as today");
        assert_eq!(
            format!("{failed:#}"),
            "tenant on port 3401 failed: listener fault"
        );
        assert_eq!(stopped.load(Ordering::SeqCst), 3);
    }
}
