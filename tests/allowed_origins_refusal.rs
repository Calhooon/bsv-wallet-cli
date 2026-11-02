//! #433: the browser gate's constructors return the rule an entry breaks
//! instead of panicking, and the binary's startup refusal of a bad
//! `ALLOWED_ORIGINS` entry is unchanged: one line on stderr naming the
//! variable, the entry and the rule, exit 1, before the wallet is opened or
//! any socket is bound. The text below is the one `serve`, `daemon` and
//! `ui` printed at beta 0aeb7f1, before the change, byte for byte. The
//! binary is run as a user runs it, on a throwaway wallet made by `init`
//! with `CHAINTRACKS_URL=off`; any request it would make goes to a closed
//! local port.

use std::process::{Command, Stdio};
use std::time::{Duration, Instant};
use tempfile::TempDir;

fn wallet(dir: &TempDir) -> Command {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_bsv-wallet"));
    cmd.current_dir(dir.path())
        .env_remove("ROOT_KEY")
        .env_remove("AUTH_TOKEN")
        .env_remove("BIND_ADDR")
        .env_remove("ALLOWED_ORIGINS")
        .env_remove("ARC_MODE")
        .env_remove("ARCADE")
        .env_remove("ARC_URL")
        .env("CHAINTRACKS_URL", "off")
        .env("BROADCAST_RECONCILE", "0")
        .env("HTTPS_PROXY", "http://127.0.0.1:9")
        .env("HTTP_PROXY", "http://127.0.0.1:9")
        .env("RUST_LOG", "warn");
    cmd
}

/// Run to its exit, bounded: a command that does not refuse would serve,
/// so it is killed and the test fails.
fn refusal(dir: &TempDir, allowed: &str, args: &[&str]) -> (Option<i32>, String) {
    let mut child = wallet(dir)
        .env("ALLOWED_ORIGINS", allowed)
        .args(["--db", "wallet.db"])
        .args(args)
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn");
    let deadline = Instant::now() + Duration::from_secs(60);
    while child.try_wait().expect("wait").is_none() {
        if Instant::now() > deadline {
            let _ = child.kill();
            let _ = child.wait();
            panic!("{args:?} with ALLOWED_ORIGINS={allowed} did not refuse at start");
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    let out = child.wait_with_output().expect("output");
    (
        out.status.code(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

const SCHEME: &str = "Error: ALLOWED_ORIGINS is \"http://localhost\", which is not an \
    originator: a domain name with an optional port, without a URL scheme (`localhost`, \
    `app.example:8443`; the reference's prepareOriginator); this entry carries a scheme: \
    set it so, or unset it for the default\n";

const PATH: &str = "Error: ALLOWED_ORIGINS is \"localhost/app\", which is not an \
    originator: a domain name with an optional port, without credentials, a path, a query \
    parameter or a fragment (the reference's prepareOriginator): set it so, or unset it \
    for the default\n";

#[test]
fn a_bad_allowed_origins_entry_refuses_the_start_by_its_rule_exit_1() {
    let dir = TempDir::new().expect("tempdir");
    let out = wallet(&dir)
        .args(["--db", "wallet.db", "init"])
        .output()
        .expect("run init");
    assert!(
        out.status.success(),
        "init: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    for command in [
        &["serve", "--port", "9"][..],
        &["daemon", "--port", "9"],
        &["ui", "--ui-port", "9"],
    ] {
        for (allowed, shown) in [
            ("localhost,http://localhost", SCHEME),
            ("localhost/app", PATH),
        ] {
            let (code, stderr) = refusal(&dir, allowed, command);
            assert_eq!(code, Some(1), "{command:?} {allowed}: {stderr}");
            assert_eq!(stderr, shown, "{command:?} {allowed}");
        }
    }
}
