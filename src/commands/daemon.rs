use std::future::Future;
use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use tokio::task::JoinHandle;

use crate::config::Config;

/// How long a daemon whose server stopped gives its monitor to stop before
/// it returns the server's error anyway. The other tasks (the served pass,
/// the reconcile loop, the relay poll, the feed) end with the runtime when
/// `main` returns the error.
const SHUTDOWN_GRACE: Duration = Duration::from_secs(5);

/// Monitor + HTTP server for ONE wallet — the composition itself lives in
/// `daemon_tenant::start` so `serve-fleet --daemon` can run it N times in one
/// process (bsv-low plan row D2). The configured ROOT_KEY; ctrl-c stops the
/// monitor then the server; a server that stops on its own (a bind error, a
/// ctrl-c listener fault, a panic) stops the monitor and is the daemon's
/// error, as it is `serve`'s (#120).
pub async fn run(config: &Arc<Config>) -> Result<()> {
    let root_key_hex = config.wallet.root_key()?;
    tracing::info!("config: {}", config.log_line());

    let tenant = super::daemon_tenant::start(
        &config.wallet.db,
        root_key_hex,
        config.chain(),
        config.wallet.port,
        config,
    )
    .await?;

    let monitor = tenant.monitor;
    join(
        tenant.server,
        tokio::signal::ctrl_c(),
        || async move { monitor.stop().await.map_err(|e| anyhow::anyhow!("{}", e)) },
        SHUTDOWN_GRACE,
    )
    .await
}

/// The daemon's join of its server task with ctrl-c. Whichever ends first,
/// the monitor is stopped (`stop_monitor`) before the daemon returns.
///
/// - ctrl-c: the monitor stops, then the server, which shuts down on its own
///   signal handler; its result is not the daemon's (the unchanged clean exit).
/// - the ctrl-c listener fails to register (its future is ready at once with
///   the error): handled as the server's fault below, with the context
///   "failed to listen for ctrl-c".
/// - the server ends cleanly: it saw ctrl-c first (the only way `server::run`
///   returns `Ok`); the monitor stops and the daemon exits as on ctrl-c.
/// - the server ends with an error or panics: logged at error level with its
///   words, the monitor stopped within `grace`, the error returned
///   (`main` exits non-zero).
async fn join<C, S, F>(
    mut server: JoinHandle<Result<()>>,
    ctrl_c: C,
    stop_monitor: S,
    grace: Duration,
) -> Result<()>
where
    C: Future<Output = std::io::Result<()>>,
    S: FnOnce() -> F,
    F: Future<Output = Result<()>>,
{
    tokio::select! {
        signal = ctrl_c => {
            if let Err(e) = signal {
                return Err(stop_on_fault(
                    anyhow::Error::new(e),
                    "failed to listen for ctrl-c",
                    stop_monitor,
                    grace,
                )
                .await);
            }
            stop_cleanly(stop_monitor).await?;
            // The server task shuts down via its own signal handler.
            let _ = server.await;
            Ok(())
        }
        ended = &mut server => match ended {
            Ok(Ok(())) => stop_cleanly(stop_monitor).await,
            Ok(Err(e)) => Err(stop_on_fault(e, SERVER_STOPPED, stop_monitor, grace).await),
            Err(join) => Err(stop_on_fault(
                anyhow::anyhow!("server task panicked: {join}"),
                SERVER_STOPPED,
                stop_monitor,
                grace,
            )
            .await),
        },
    }
}

async fn stop_cleanly<S, F>(stop_monitor: S) -> Result<()>
where
    S: FnOnce() -> F,
    F: Future<Output = Result<()>>,
{
    eprintln!("\nStopping monitor...");
    stop_monitor().await?;
    eprintln!("Monitor stopped");
    Ok(())
}

/// The context of a server that stopped on its own.
const SERVER_STOPPED: &str = "the daemon's HTTP server stopped";

/// A fault that ends the daemon (the server's, or the ctrl-c listener's),
/// logged with its words under `context`; the monitor stopped within `grace`
/// (a monitor that fails or hangs is logged, never the daemon's answer); the
/// fault, with `context`, is what the daemon returns.
async fn stop_on_fault<S, F>(
    e: anyhow::Error,
    context: &'static str,
    stop_monitor: S,
    grace: Duration,
) -> anyhow::Error
where
    S: FnOnce() -> F,
    F: Future<Output = Result<()>>,
{
    tracing::error!(
        error = %format!("{e:#}"),
        "daemon: {context}; stopping the monitor"
    );
    match tokio::time::timeout(grace, stop_monitor()).await {
        Ok(Ok(())) => {}
        Ok(Err(stop)) => tracing::error!(error = %stop, "daemon: the monitor did not stop cleanly"),
        Err(_) => tracing::error!(
            grace_ms = grace.as_millis() as u64,
            "daemon: the monitor did not stop within the grace"
        ),
    }
    e.context(context)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, Ordering};

    fn stopper(flag: &Arc<AtomicBool>) -> impl FnOnce() -> std::future::Ready<Result<()>> {
        let flag = flag.clone();
        move || {
            flag.store(true, Ordering::SeqCst);
            std::future::ready(Ok(()))
        }
    }

    #[tokio::test]
    async fn a_server_that_fails_ends_the_daemon_with_its_error_within_the_grace() {
        let stopped = Arc::new(AtomicBool::new(false));
        let server =
            tokio::spawn(async { Err(anyhow::anyhow!("Address already in use (os error 48)")) });
        let ended = tokio::time::timeout(
            SHUTDOWN_GRACE + Duration::from_secs(1),
            join(
                server,
                std::future::pending(),
                stopper(&stopped),
                SHUTDOWN_GRACE,
            ),
        )
        .await
        .expect("the daemon ends within its grace");
        let e = ended.expect_err("a failed server is the daemon's error");
        let words = format!("{e:#}");
        assert!(
            words.contains("the daemon's HTTP server stopped"),
            "{words}"
        );
        assert!(words.contains("Address already in use"), "{words}");
        assert!(stopped.load(Ordering::SeqCst), "the monitor was stopped");
    }

    #[tokio::test]
    async fn a_server_that_panics_ends_the_daemon_with_an_error() {
        let stopped = Arc::new(AtomicBool::new(false));
        let server: JoinHandle<Result<()>> = tokio::spawn(async { panic!("boom") });
        let e = tokio::time::timeout(
            SHUTDOWN_GRACE + Duration::from_secs(1),
            join(
                server,
                std::future::pending(),
                stopper(&stopped),
                SHUTDOWN_GRACE,
            ),
        )
        .await
        .expect("the daemon ends within its grace")
        .expect_err("a panicked server is the daemon's error");
        assert!(format!("{e:#}").contains("server task panicked"));
        assert!(stopped.load(Ordering::SeqCst));
    }

    #[tokio::test]
    async fn a_ctrl_c_listener_fault_stops_the_monitor_and_is_the_daemons_error() {
        let stopped = Arc::new(AtomicBool::new(false));
        let server = tokio::spawn(async {
            tokio::task::yield_now().await;
            Err(anyhow::anyhow!("the server erred after the listener"))
        });
        let ended = tokio::time::timeout(
            SHUTDOWN_GRACE + Duration::from_secs(1),
            join(
                server,
                std::future::ready(Err(std::io::Error::other("signal registration refused"))),
                stopper(&stopped),
                SHUTDOWN_GRACE,
            ),
        )
        .await
        .expect("the daemon ends within its grace");
        let e = ended.expect_err("a ctrl-c listener fault is the daemon's error");
        let words = format!("{e:#}");
        assert!(words.contains("failed to listen for ctrl-c"), "{words}");
        assert!(words.contains("signal registration refused"), "{words}");
        assert!(stopped.load(Ordering::SeqCst), "the monitor was stopped");
    }

    #[tokio::test]
    async fn a_monitor_that_hangs_does_not_hold_the_daemon_past_the_grace() {
        let grace = Duration::from_millis(50);
        let server = tokio::spawn(async { Err(anyhow::anyhow!("listener fault")) });
        let e = tokio::time::timeout(
            grace + Duration::from_secs(1),
            join(server, std::future::pending(), std::future::pending, grace),
        )
        .await
        .expect("the daemon ends past a hung monitor")
        .expect_err("the server's error is returned after the grace");
        assert!(format!("{e:#}").contains("listener fault"));
    }

    #[tokio::test]
    async fn ctrl_c_stops_the_monitor_then_the_server_and_exits_cleanly() {
        let stopped = Arc::new(AtomicBool::new(false));
        let (shut, shut_rx) = tokio::sync::oneshot::channel::<()>();
        // The server ends on its own signal; here, once the daemon has
        // stopped the monitor and awaits it.
        let server = tokio::spawn(async move {
            let _ = shut_rx.await;
            Ok(())
        });
        let flag = stopped.clone();
        let ended = join(
            server,
            std::future::ready(Ok(())),
            move || {
                flag.store(true, Ordering::SeqCst);
                let _ = shut.send(());
                std::future::ready(Ok(()))
            },
            SHUTDOWN_GRACE,
        )
        .await;
        assert!(ended.is_ok(), "{ended:?}");
        assert!(stopped.load(Ordering::SeqCst));
    }

    #[tokio::test]
    async fn a_server_that_saw_ctrl_c_first_is_the_clean_exit() {
        let stopped = Arc::new(AtomicBool::new(false));
        let server = tokio::spawn(async { Ok(()) });
        let ended = tokio::time::timeout(
            SHUTDOWN_GRACE + Duration::from_secs(1),
            join(
                server,
                std::future::pending(),
                stopper(&stopped),
                SHUTDOWN_GRACE,
            ),
        )
        .await
        .expect("the daemon ends when its server does");
        assert!(ended.is_ok(), "{ended:?}");
        assert!(stopped.load(Ordering::SeqCst));
    }
}
