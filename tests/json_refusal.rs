//! F4 of the stack's coin read (bsv-stack-lean #164): under `--json` a
//! spend the gate refuses prints the refusal as one JSON object on stdout,
//! `{code, message, offeredSatoshis, held}`, and exits 1. The binary is run
//! as a user runs it, on a throwaway wallet made by `init` (no coin, so
//! nothing is built, signed or broadcast) with `CHAINTRACKS_URL=off`.
//! `drain` and `split` stop before the gate on an empty wallet; they print
//! through the same calls (`spend_gate::refused_as_json`,
//! `spend_gate::busy_as_json`). The store busy (#44) prints as the route's
//! 503 body, `{code: STORE_BUSY, message}`, and exits 1 the same way.

use std::process::Command;
use tempfile::TempDir;

fn wallet(dir: &TempDir) -> Command {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_bsv-wallet"));
    cmd.current_dir(dir.path())
        .env_remove("ROOT_KEY")
        .env_remove("ARC_MODE")
        .env_remove("ARCADE")
        .env_remove("ARC_URL")
        .env("CHAINTRACKS_URL", "off")
        // Any request the run would make goes to a closed local port.
        .env("HTTPS_PROXY", "http://127.0.0.1:9")
        .env("HTTP_PROXY", "http://127.0.0.1:9")
        .env("RUST_LOG", "warn");
    cmd
}

#[test]
fn a_refused_spend_under_json_prints_the_refusal_as_json_and_exits_1() {
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
    let g = "0279be667ef9dcbbac55a06295ce870b07029bfcdb2dce28d959f2815b16f81798";
    let runs: [&[&str]; 2] = [
        &["send", "12tRsyKopKkn5kpxhgNUZ7WaddmU8b8X3w", "30000"],
        &["gift-send", g, "30000", "--unlock", "2030-01-01"],
    ];
    for args in runs {
        let out = wallet(&dir)
            .args(["--db", "wallet.db", "--json"])
            .args(args)
            .output()
            .expect("run the command");
        let stdout = String::from_utf8_lossy(&out.stdout);
        let stderr = String::from_utf8_lossy(&out.stderr);
        println!("{args:?}: {stdout}");
        assert_eq!(out.status.code(), Some(1), "{args:?}: {stdout}{stderr}");
        let json: serde_json::Value = serde_json::from_str(stdout.trim())
            .unwrap_or_else(|e| panic!("{args:?}: not JSON ({e}): {stdout}{stderr}"));
        assert_eq!(json["code"], "INSUFFICIENT_FUNDS", "{json}");
        assert_eq!(json["offeredSatoshis"], 0, "{json}");
        assert_eq!(json["held"], serde_json::json!([]), "{json}");
        assert!(json["message"]
            .as_str()
            .unwrap()
            .contains("Insufficient funds"));
    }
}

/// The store busy under `--json` (#44): another process holds the wallet
/// file's write lock past the store's ceiling while `send` and `gift-send`
/// run; each prints `{code: STORE_BUSY, message}` and exits 1, nothing
/// built or posted.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_busy_store_under_json_prints_store_busy_and_exits_1() {
    use sqlx::{ConnectOptions, Connection};
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
    // A first command opens the store (its schema settled), then the lock.
    let warm = wallet(&dir)
        .args(["--db", "wallet.db", "--json", "balance"])
        .output()
        .expect("run balance");
    assert!(
        warm.status.success(),
        "balance: {}",
        String::from_utf8_lossy(&warm.stderr)
    );
    let mut other = sqlx::sqlite::SqliteConnectOptions::new()
        .filename(dir.path().join("wallet.db"))
        .connect()
        .await
        .unwrap();
    sqlx::query("BEGIN IMMEDIATE")
        .execute(&mut other)
        .await
        .unwrap();
    let g = "0279be667ef9dcbbac55a06295ce870b07029bfcdb2dce28d959f2815b16f81798";
    let runs: [&[&str]; 2] = [
        &["send", "12tRsyKopKkn5kpxhgNUZ7WaddmU8b8X3w", "30000"],
        &["gift-send", g, "30000", "--unlock", "2030-01-01"],
    ];
    for args in runs {
        let out = wallet(&dir)
            .args(["--db", "wallet.db", "--json"])
            .args(args)
            .output()
            .expect("run the command");
        let stdout = String::from_utf8_lossy(&out.stdout);
        let stderr = String::from_utf8_lossy(&out.stderr);
        println!("{args:?}: {stdout}");
        assert_eq!(out.status.code(), Some(1), "{args:?}: {stdout}{stderr}");
        let json: serde_json::Value = serde_json::from_str(stdout.trim())
            .unwrap_or_else(|e| panic!("{args:?}: not JSON ({e}): {stdout}{stderr}"));
        assert_eq!(json["code"], "STORE_BUSY", "{json}");
        assert_eq!(json.as_object().unwrap().len(), 2, "{json}");
        assert!(
            json["message"].as_str().is_some_and(|m| !m.is_empty()),
            "{json}"
        );
    }
    sqlx::query("ROLLBACK").execute(&mut other).await.unwrap();
    other.close().await.unwrap();
}
