//! The hermetic e2e suite as a cargo test target (#393), so the Nightly's
//! mutation oracle can judge a mutant by it: `E2E_HERMETIC=1 node
//! tests/e2e/run.js` (docs/E2E-HERMETIC.md) driven against the binary cargo
//! built for this tree, `CARGO_BIN_EXE_bsv-wallet`, never a stale
//! `target/release`.
//!
//! Gated by `E2E_HERMETIC_ORACLE=1`. Unset, the test passes at once and says
//! why, so `cargo test`, CI's PR gate and the Nightly's cargo shards are
//! unchanged. Set, it needs Node (`E2E_NODE`, else `node` on `PATH`), the
//! harness's `npm ci` (in `tests/e2e`, or `NODE_PATH` naming that
//! `node_modules` when the tree is a copy without it), Playwright's Chromium
//! (rule 13), `sqlite3` on `PATH` and `BSV_WALLET_MCP_BIN` naming a built
//! `bsv-wallet-mcp` (the mcp package is not mutated; the Nightly builds it
//! once per shard). The suite holds 3370 to 3373, 3380 and the inspector's
//! 9321: one suite at a time per host.
//!
//! The test fails on the suite's non-zero exit, on a walled attempt at a host
//! the harness does not declare, on an unserved route and on a missing stat
//! line, as the `e2e-hermetic` job does. A fault in the environment rather
//! than in the binary (a held port, a missing MCP binary or Node, a binary
//! that is not this build's) prints `E2E_HERMETIC_UNJUDGED`: cargo-mutants
//! would read the failure as a caught mutant, so the Nightly's hermetic shard
//! refuses itself when any mutant's log carries the word.

use std::io::{BufRead, BufReader, Write};
use std::net::{SocketAddr, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant, SystemTime};

const GATE: &str = "E2E_HERMETIC_ORACLE";
const UNJUDGED: &str = "E2E_HERMETIC_UNJUDGED";
/// The suite's ports: wallets A, B, C, the funder, the stub, the inspector.
const PORTS: [u16; 6] = [3370, 3371, 3372, 3373, 3380, 9321];

/// Written past libtest's capture, so `cargo test` shows it without
/// `--nocapture` and cargo-mutants' log keeps it.
fn say(line: &str) {
    let mut err = std::io::stderr();
    let _ = writeln!(err, "{line}");
    let _ = err.flush();
}

fn unjudged(why: &str) -> ! {
    say(&format!("{UNJUDGED}: {why}"));
    panic!("{UNJUDGED}: {why}");
}

fn manifest_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

/// The newest modification time of the sources the `bsv-wallet` binary is
/// built from (`src/`, without `src/bin/`, whose other binaries it does not
/// link).
fn newest_source(dir: &Path, newest: &mut SystemTime) {
    for entry in std::fs::read_dir(dir).expect("read src/") {
        let entry = entry.expect("read an entry of src/");
        let path = entry.path();
        if path.is_dir() {
            if path != manifest_dir().join("src").join("bin") {
                newest_source(&path, newest);
            }
        } else if path.extension().is_some_and(|e| e == "rs") {
            let modified = entry
                .metadata()
                .and_then(|m| m.modified())
                .expect("a source's mtime");
            *newest = (*newest).max(modified);
        }
    }
}

/// The binary the suite drives is the one cargo built for this tree: the
/// `BSV_WALLET_BIN` of the environment, if any, is that binary; it lies in the
/// profile directory of this test's own executable (the current build's
/// target directory); and it is no older than the newest source it is built
/// from, so a binary built before a mutation was written is refused.
fn this_build() -> PathBuf {
    let built = PathBuf::from(env!("CARGO_BIN_EXE_bsv-wallet"));
    let built = built
        .canonicalize()
        .unwrap_or_else(|e| unjudged(&format!("{}: {e}", built.display())));
    if let Some(named) = std::env::var_os("BSV_WALLET_BIN") {
        let named = PathBuf::from(named);
        let same = named.canonicalize().is_ok_and(|n| n == built);
        if !same {
            unjudged(&format!(
                "BSV_WALLET_BIN is {}, not the binary cargo built for this tree ({})",
                named.display(),
                built.display()
            ));
        }
    }
    let exe = std::env::current_exe().expect("the test's own path");
    let exe = exe.canonicalize().unwrap_or(exe);
    // <target>/<profile>/deps/e2e_hermetic-<hash>
    let profile = exe.parent().and_then(Path::parent);
    if built.parent() != profile {
        unjudged(&format!(
            "{} is not in this build's profile directory ({})",
            built.display(),
            profile.map_or("none".into(), |p| p.display().to_string())
        ));
    }
    let mut newest = SystemTime::UNIX_EPOCH;
    newest_source(&manifest_dir().join("src"), &mut newest);
    let modified = std::fs::metadata(&built)
        .and_then(|m| m.modified())
        .expect("the binary's mtime");
    if modified < newest {
        unjudged(&format!(
            "{} is older than the newest source under src/: a stale build",
            built.display()
        ));
    }
    built
}

/// Every port of the suite is free, waiting up to a minute for a previous
/// run's children to go.
fn ports_free() {
    let deadline = Instant::now() + Duration::from_secs(60);
    loop {
        let held: Vec<u16> = PORTS
            .iter()
            .copied()
            .filter(|&p| {
                let addr = SocketAddr::from(([127, 0, 0, 1], p));
                TcpStream::connect_timeout(&addr, Duration::from_millis(200)).is_ok()
            })
            .collect();
        if held.is_empty() {
            return;
        }
        if Instant::now() >= deadline {
            unjudged(&format!(
                "port(s) {held:?} answer before the suite starts; one suite at a time per host"
            ));
        }
        std::thread::sleep(Duration::from_secs(1));
    }
}

/// The value after `label` on the summary's line that starts with it.
fn stat<'a>(out: &'a str, label: &str) -> &'a str {
    out.lines()
        .find_map(|l| l.trim_start().strip_prefix(label))
        .map(str::trim)
        .unwrap_or_else(|| panic!("the suite printed no `{label}` line: its stats are missing"))
}

#[test]
fn hermetic_suite_judges_this_build() {
    if std::env::var(GATE).as_deref() != Ok("1") {
        say(&format!(
            "e2e_hermetic: {GATE} is not 1, so the hermetic suite does not run (the Nightly's \
             Mutants-hermetic shards set it; docs/E2E-HERMETIC.md)"
        ));
        return;
    }
    let bin = this_build();
    let mcp = std::env::var_os("BSV_WALLET_MCP_BIN")
        .map(PathBuf::from)
        .filter(|p| p.is_file())
        .unwrap_or_else(|| unjudged("BSV_WALLET_MCP_BIN must name a built bsv-wallet-mcp"));
    let node = std::env::var_os("E2E_NODE").unwrap_or_else(|| "node".into());
    ports_free();
    let scenarios = std::fs::read_dir(manifest_dir().join("tests/e2e/scenarios"))
        .expect("read tests/e2e/scenarios")
        .filter(|e| {
            e.as_ref()
                .is_ok_and(|e| e.path().extension().is_some_and(|x| x == "js"))
        })
        .count();

    say(&format!(
        "e2e_hermetic: {scenarios} scenarios against {}",
        bin.display()
    ));
    let started = Instant::now();
    let mut child = Command::new(&node)
        .arg("tests/e2e/run.js")
        .current_dir(manifest_dir())
        .env("E2E_HERMETIC", "1")
        .env("NODE_USE_ENV_PROXY", "1")
        .env("BSV_WALLET_BIN", &bin)
        .env("BSV_WALLET_MCP_BIN", &mcp)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()
        .unwrap_or_else(|e| unjudged(&format!("start {}: {e}", node.to_string_lossy())));
    let mut out = String::new();
    for line in BufReader::new(child.stdout.take().expect("the suite's stdout")).lines() {
        let line = line.expect("read the suite's output");
        say(&line);
        out.push_str(&line);
        out.push('\n');
    }
    let status = child.wait().expect("wait for the suite");
    say(&format!(
        "e2e_hermetic: the suite ended {status} after {:.0} s",
        started.elapsed().as_secs_f64()
    ));

    assert!(status.success(), "the hermetic suite failed: {status}");
    let passed = stat(&out, "Passed:");
    let skipped: usize = stat(&out, "Skipped:")
        .split_whitespace()
        .next()
        .and_then(|n| n.parse().ok())
        .expect("the Skipped line's count");
    let (ran, total) = passed.split_once('/').expect("Passed: N/M");
    let (ran, total): (usize, usize) =
        (ran.parse().expect("passed"), total.parse().expect("total"));
    assert_eq!(ran, total, "Passed: {passed}");
    assert_eq!(
        total + skipped,
        scenarios,
        "{total} run and {skipped} skipped of {scenarios} scenarios"
    );
    let walled = stat(&out, "Walled:");
    assert!(
        walled.ends_with(", 0 new"),
        "a walled attempt at an undeclared host: {walled}"
    );
    let unserved = stat(&out, "Unserved:");
    assert!(
        unserved.starts_with("0 request(s)"),
        "an unserved route: {unserved}"
    );
}
