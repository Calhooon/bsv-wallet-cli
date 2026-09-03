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

use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};

use crate::context::WalletContext;
use crate::server::{self, ServerConfig, TlsConfig};

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

/// Service-relevant env keys `services_env`/the server read from PROCESS env.
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
    "TAAL_API_KEY",
    "MAIN_TAAL_API_KEY",
    "AUTH_TOKEN",
    "BIND_ADDR",
    "RELAY_URL",
];

/// Refuse a seat whose .env sets a SHARED key to a value differing from the
/// process env (values never printed — these are secrets).
pub fn assert_env_uniform(env_path: &Path) -> Result<()> {
    let iter = dotenvy::from_path_iter(env_path)
        .with_context(|| format!("reading {}", env_path.display()))?;
    for item in iter {
        let (k, v) = item.with_context(|| format!("parsing {}", env_path.display()))?;
        if SHARED_ENV_KEYS.contains(&k.as_str()) {
            match std::env::var(&k) {
                Ok(procv) if procv == v => {}
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

/// Build one tenant's ServerConfig — the same env-driven shape `serve` builds,
/// with the callback token resolved per-db (Arcade runtime is db-anchored).
fn tenant_config(ctx: &WalletContext) -> Result<ServerConfig> {
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
    let callback_token = crate::services_env::arcade_runtime(&ctx.db_path)?
        .map(|rt| rt.callback_token)
        .or_else(|| {
            std::env::var("CALLBACK_TOKEN")
                .ok()
                .filter(|s| !s.is_empty())
        });
    let bind_addr: std::net::IpAddr = std::env::var("BIND_ADDR")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or_else(|| std::net::IpAddr::from([127, 0, 0, 1]));
    Ok(ServerConfig {
        auth_token: std::env::var("AUTH_TOKEN").ok(),
        tls,
        chain: ctx.chain,
        bind_addr,
        callback_token,
    })
}

pub async fn run(specs: &[String], testnet: bool, daemon: bool) -> Result<()> {
    let entries = parse_fleet(specs)?;
    let n = entries.len();
    let chain = if testnet {
        bsv_wallet_toolbox::Chain::Test
    } else {
        bsv_wallet_toolbox::Chain::Main
    };

    // Validate EVERY seat (env uniformity + key present) BEFORE anything binds:
    // a fleet that cannot fully start must not half-start (the runner would
    // read partial as healthy).
    let mut keys = Vec::with_capacity(n);
    for e in &entries {
        let env_path = e.dir.join(".env");
        assert_env_uniform(&env_path)?;
        keys.push(root_key_from_env_file(&env_path)?);
    }

    if daemon {
        // FULL daemon composition per tenant (monitor + reconcile + server).
        let mut monitors = Vec::with_capacity(n);
        let (tx, mut rx) = tokio::sync::mpsc::channel::<(u16, Result<()>)>(n.max(1));
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
            let tenant = super::daemon_tenant::start(db_str, key, chain, e.port)
                .await
                .with_context(|| format!("starting tenant {}", e.dir.display()))?;
            let port = tenant.port;
            let server = tenant.server;
            monitors.push(tenant.monitor);
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
        let outcome = rx.recv().await;
        // Whatever ended first, stop every monitor before exiting.
        for m in monitors {
            let _ = m.stop().await;
        }
        return match outcome {
            Some((port, Ok(()))) => {
                eprintln!("[serve-fleet] port {port} exited cleanly — shutting the fleet down");
                Ok(())
            }
            Some((port, Err(e))) => bail!("tenant on port {port} failed: {e:#}"),
            None => bail!("serve-fleet: no tenants ran"),
        };
    }

    // HTTP-only mode.
    let mut tenants = Vec::with_capacity(n);
    for (e, key) in entries.iter().zip(&keys) {
        let db = e.dir.join("wallet.db");
        let db_str = db
            .to_str()
            .with_context(|| format!("non-UTF8 path {}", db.display()))?;
        let ctx = WalletContext::load_with(db_str, key, testnet, false)
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
        let config = tenant_config(&ctx)?;
        let chain = ctx.chain;
        let db_path = ctx.db_path.clone();
        let state = server::make_wallet_state(ctx.wallet);
        // Bare serve has no monitor: each tenant gets its own broadcast
        // reconciler (every 60 s, bounded).
        let reconcile =
            crate::broadcast_reconcile::spawn_serve_loop(state.clone(), chain, &db_path);
        let tx = tx.clone();
        tokio::spawn(async move {
            let res = server::run(state, port, config).await;
            if let Some(handle) = reconcile {
                handle.abort();
            }
            let _ = tx.send((port, res)).await;
        });
    }
    drop(tx);
    eprintln!("[serve-fleet] {n} wallet(s) serving");

    match rx.recv().await {
        Some((port, Ok(()))) => {
            // A clean exit (graceful shutdown signal) — end the whole fleet.
            eprintln!("[serve-fleet] port {port} exited cleanly — shutting the fleet down");
            Ok(())
        }
        Some((port, Err(e))) => bail!("tenant on port {port} failed: {e:#}"),
        None => bail!("serve-fleet: no tenants ran"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

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
        std::env::remove_var("TAAL_API_KEY");
        let err = assert_env_uniform(&env_path).unwrap_err().to_string();
        assert!(err.contains("fleet env divergence"), "{err}");
        assert!(err.contains("TAAL_API_KEY"), "{err}");
        assert!(
            !err.contains("seat-specific-secret"),
            "secret leaked: {err}"
        );
        // matching value passes; ROOT_KEY alone always passes
        std::env::set_var("TAAL_API_KEY", "seat-specific-secret");
        assert!(assert_env_uniform(&env_path).is_ok());
        std::env::remove_var("TAAL_API_KEY");
        std::fs::write(&env_path, "ROOT_KEY=aa\n").unwrap();
        assert!(assert_env_uniform(&env_path).is_ok());
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
}
