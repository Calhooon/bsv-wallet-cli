//! P0-1b (bsv-stack-lean #35): `CHAINTRACKS_URL=off` runs the wallet with no
//! chain tracker, and the toolbox then refuses every merkle proof. A command
//! whose job is to prove must say so and exit non-zero, before it reaches the
//! network. The binary is run as a user runs it: a throwaway wallet made by
//! `init` in a temp dir (no network), then `tick`.

use std::process::Command;
use tempfile::TempDir;

fn wallet(dir: &TempDir) -> Command {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_bsv-wallet"));
    cmd.current_dir(dir.path())
        .env_remove("ROOT_KEY")
        .env_remove("ARC_MODE")
        .env_remove("ARCADE")
        .env_remove("ARC_URL")
        .env("RUST_LOG", "warn");
    cmd
}

#[test]
fn tick_with_chaintracks_off_exits_non_zero_and_says_proofs_are_refused() {
    let dir = TempDir::new().expect("temp dir");
    let init = wallet(&dir)
        .args(["--db", "wallet.db", "init"])
        .output()
        .expect("run init");
    assert!(
        init.status.success(),
        "init: {}",
        String::from_utf8_lossy(&init.stderr)
    );

    let tick = wallet(&dir)
        .env("CHAINTRACKS_URL", "off")
        .args(["--db", "wallet.db", "tick"])
        .output()
        .expect("run tick");
    let stderr = String::from_utf8_lossy(&tick.stderr);
    let stdout = String::from_utf8_lossy(&tick.stdout);
    assert!(!tick.status.success(), "tick must fail: {stdout}{stderr}");
    assert!(stderr.contains("tick refused"), "{stderr}");
    assert!(
        stderr.contains("every merkle proof would be refused"),
        "{stderr}"
    );
    assert!(
        stderr.contains("every merkle proof will be refused and nothing marked proven"),
        "the startup warning is the true sentence: {stderr}"
    );
    assert!(
        !stderr.contains("stored without header validation"),
        "the old, false sentence is gone: {stderr}"
    );
    assert!(
        !stdout.contains("total:"),
        "no monitor pass ran (no network reached): {stdout}"
    );
}

/// P0-1c (bsv-stack-lean #48): an operator upgrading with no
/// `CHAINTRACKS_URL` set (the old default, the public Babbage chaintracks
/// with WhatsOnChain behind it) is told to set one, and nothing runs.
#[test]
fn an_unset_header_source_stops_the_command_and_names_the_setting() {
    let dir = TempDir::new().expect("temp dir");
    let init = wallet(&dir)
        .args(["--db", "wallet.db", "init"])
        .output()
        .expect("run init");
    assert!(
        init.status.success(),
        "init: {}",
        String::from_utf8_lossy(&init.stderr)
    );

    for command in ["tick", "compact"] {
        let out = wallet(&dir)
            .env_remove("CHAINTRACKS_URL")
            .args(["--db", "wallet.db", command])
            .output()
            .expect("run the command");
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert!(!out.status.success(), "{command} must fail: {stderr}");
        assert!(
            stderr.contains("CHAINTRACKS_URL is not set"),
            "{command}: {stderr}"
        );
        assert!(!stderr.contains("babbage"), "{command}: {stderr}");
    }
}
