//! 0.7.1 (bsv-stack-lean, the CLI 0.7.1 lane): a served wallet with no bearer
//! token answers any caller that can reach it, and since 0.7.0 its two BEEF
//! doors take a body of any size. So `serve`, `daemon` and `serve-fleet`
//! refuse at startup to bind an address beyond loopback when no token is
//! configured, before the wallet is opened or any socket is bound;
//! `--allow-no-token` permits the open bind deliberately; loopback without a
//! token is unchanged. The binary is run as a user runs it: a throwaway wallet
//! made by `init` in a temp dir, `CHAINTRACKS_URL=off` and the served loop off,
//! so nothing reaches the network.

use std::io::{BufRead, BufReader};
use std::net::{TcpListener, TcpStream};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc;
use std::time::Duration;
use tempfile::TempDir;

fn wallet(dir: &TempDir) -> Command {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_bsv-wallet"));
    cmd.current_dir(dir.path())
        .env_remove("ROOT_KEY")
        .env_remove("AUTH_TOKEN")
        .env_remove("BIND_ADDR")
        .env_remove("TLS_CERT_PATH")
        .env_remove("TLS_KEY_PATH")
        .env_remove("ARC_MODE")
        .env_remove("ARCADE")
        .env_remove("ARC_URL")
        .env("CHAINTRACKS_URL", "off")
        .env("BROADCAST_RECONCILE", "0")
        .env("RUST_LOG", "warn");
    cmd
}

fn init(dir: &TempDir) {
    let out = wallet(dir)
        .args(["--db", "wallet.db", "init"])
        .output()
        .expect("run init");
    assert!(
        out.status.success(),
        "init: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

fn free_port() -> u16 {
    TcpListener::bind("127.0.0.1:0")
        .expect("probe bind")
        .local_addr()
        .expect("probe addr")
        .port()
}

/// The child is killed by its own pid when the guard drops, pass or fail.
struct Running(Child);

impl Drop for Running {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

/// Spawn the command and wait (bounded) for it to exit or to print its
/// listening line. Returns the exit status if it exited, and the stderr read.
fn run_until_listening_or_exit(mut cmd: Command) -> (Option<std::process::ExitStatus>, String) {
    let mut child = cmd
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn");
    let stderr = child.stderr.take().expect("stderr");
    let (tx, rx) = mpsc::channel::<String>();
    std::thread::spawn(move || {
        for line in BufReader::new(stderr).lines().map_while(Result::ok) {
            if tx.send(line).is_err() {
                break;
            }
        }
    });
    let mut running = Running(child);
    let mut seen = String::new();
    let deadline = std::time::Instant::now() + Duration::from_secs(30);
    while std::time::Instant::now() < deadline {
        match rx.recv_timeout(Duration::from_millis(100)) {
            Ok(line) => {
                let listening = line.contains("server listening on");
                seen.push_str(&line);
                seen.push('\n');
                if listening {
                    return (None, seen);
                }
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                let status = running.0.wait().expect("wait");
                return (Some(status), seen);
            }
        }
        if let Some(status) = running.0.try_wait().expect("try_wait") {
            while let Ok(line) = rx.recv_timeout(Duration::from_millis(200)) {
                seen.push_str(&line);
                seen.push('\n');
            }
            return (Some(status), seen);
        }
    }
    panic!("neither exited nor listened within 30 s: {seen}");
}

#[test]
fn an_open_bind_without_a_token_is_refused_before_any_socket_is_opened() {
    for command in [
        vec!["serve"],
        vec!["daemon"],
        vec!["serve-fleet", "--wallet", "seat:3999"],
    ] {
        let dir = TempDir::new().expect("temp dir");
        init(&dir);
        let port = free_port();
        let mut cmd = wallet(&dir);
        cmd.env("BIND_ADDR", "0.0.0.0")
            .args(["--db", "wallet.db", "--port", &port.to_string()])
            .args(&command);
        let (status, stderr) = run_until_listening_or_exit(cmd);
        let status = status.unwrap_or_else(|| panic!("{command:?} bound open: {stderr}"));
        assert!(!status.success(), "{command:?} must fail: {stderr}");
        assert!(
            stderr.contains("no bearer token") && stderr.contains("--allow-no-token"),
            "{command:?}: the one plain error names the flag: {stderr}"
        );
        assert!(
            !stderr.contains("listening on"),
            "{command:?}: no socket was opened: {stderr}"
        );
        assert!(
            TcpStream::connect(("127.0.0.1", port)).is_err(),
            "{command:?}: nothing listens on {port}"
        );
    }
}

#[test]
fn the_flag_permits_the_open_bind() {
    let dir = TempDir::new().expect("temp dir");
    init(&dir);
    let port = free_port();
    let mut cmd = wallet(&dir);
    cmd.env("BIND_ADDR", "0.0.0.0").args([
        "--db",
        "wallet.db",
        "--port",
        &port.to_string(),
        "serve",
        "--allow-no-token",
    ]);
    let (status, stderr) = run_until_listening_or_exit(cmd);
    assert!(status.is_none(), "serve --allow-no-token exited: {stderr}");
    assert!(
        stderr.contains(&format!("listening on 0.0.0.0:{port}")),
        "{stderr}"
    );
}

#[test]
fn a_token_permits_the_open_bind() {
    let dir = TempDir::new().expect("temp dir");
    init(&dir);
    let port = free_port();
    let mut cmd = wallet(&dir);
    cmd.env("BIND_ADDR", "0.0.0.0")
        .env("AUTH_TOKEN", "witness-token")
        .args(["--db", "wallet.db", "--port", &port.to_string(), "serve"]);
    let (status, stderr) = run_until_listening_or_exit(cmd);
    assert!(status.is_none(), "serve with a token exited: {stderr}");
    assert!(
        stderr.contains(&format!("listening on 0.0.0.0:{port}")),
        "{stderr}"
    );
}

#[test]
fn loopback_without_a_token_binds_as_before() {
    for bind in [None, Some("127.0.0.1"), Some("::1")] {
        let dir = TempDir::new().expect("temp dir");
        init(&dir);
        let port = free_port();
        let mut cmd = wallet(&dir);
        if let Some(addr) = bind {
            cmd.env("BIND_ADDR", addr);
        }
        cmd.args(["--db", "wallet.db", "--port", &port.to_string(), "serve"]);
        let (status, stderr) = run_until_listening_or_exit(cmd);
        assert!(status.is_none(), "{bind:?}: serve exited: {stderr}");
        assert!(stderr.contains("listening on"), "{bind:?}: {stderr}");
    }
}
