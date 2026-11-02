//! Certificate bounds and the host's explicit, read-only legacy reverify.
use bsv_sdk::auth::certificates::Certificate;
use bsv_sdk::primitives::{to_base64, PrivateKey};
use bsv_sdk::wallet::Outpoint;
use bsv_wallet_toolbox::{
    services::mock::MockWalletServices, StorageSqlx, Wallet, WalletStorageWriter,
};
use serde_json::json;
use std::process::Command;

#[test]
fn certificate_writer_bounds_and_boolean_verifier() {
    let key = PrivateKey::from_hex(&"01".repeat(32)).unwrap();
    let mut c = Certificate::new([6; 32], [7; 32], key.public_key(), key.public_key());
    c.set_field("name", "A".repeat(1_048_576));
    assert!(c.to_binary(false).is_ok());
    c.set_field("name", "A".repeat(1_048_577));
    assert!(c
        .to_binary(false)
        .unwrap_err()
        .to_string()
        .contains("1048576"));
    assert!(!c.verify().unwrap());
    c.fields.clear();
    c.signature = Some(vec![1; 73]);
    assert!(c.to_binary(true).is_err());
    assert!(!c.verify().unwrap());
    c.signature = None;
    for n in 0..16 {
        c.set_field(format!("field{n}"), "A".repeat(1_048_576));
    }
    assert!(c
        .to_binary(false)
        .unwrap_err()
        .to_string()
        .contains("16777216"));
    assert!(!c.verify().unwrap());
}

#[tokio::test]
async fn reverify_command_counts_good_and_bad_signatures_without_changing_certificates() {
    for output in audit(&[(7, true), (8, false)], 2).await {
        assert_eq!(output.status.code(), Some(1));
        let result: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(result, json!({"checked": 2, "failed": 1}));
    }
}

// #430: the failing certificate is stored first, so a reverify that stopped
// at the first failure would answer one checked. Every row is read, the
// command exits non-zero and nothing changes. Naming the failing one waits
// on the toolbox's per-certificate verdicts (toolbox #585).
#[tokio::test]
async fn reverify_checks_every_certificate_past_a_failure_and_exits_non_zero() {
    let output = audit(&[(9, false), (10, true), (11, true)], 1)
        .await
        .remove(0);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert_eq!(output.status.code(), Some(1), "{stdout}");
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&stdout).unwrap(),
        json!({"checked": 3, "failed": 1})
    );
    assert!(String::from_utf8_lossy(&output.stderr)
        .contains("1 of 3 stored certificates failed signature verification"));
}

type TestWallet = Wallet<StorageSqlx, MockWalletServices>;

fn key() -> PrivateKey {
    PrivateKey::from_hex(&"01".repeat(32)).unwrap()
}

/// Runs the reverify `runs` times over a scratch store of `rows` and
/// asserts the certificates are unchanged after.
async fn audit(rows: &[(u8, bool)], runs: usize) -> Vec<std::process::Output> {
    let (dir, wallet) = store(rows).await;
    let path = dir.path().join("wallet.db");
    let before = snapshot(&wallet).await;
    let outputs = (0..runs).map(|_| reverify_output(&path, &key())).collect();
    assert_eq!(snapshot(&wallet).await, before);
    outputs
}

/// A scratch store holding one certificate per `(serial byte, good)` row.
async fn store(rows: &[(u8, bool)]) -> (tempfile::TempDir, TestWallet) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("wallet.db");
    let root = key();
    let identity = root.public_key().to_hex();
    let storage = StorageSqlx::open(path.to_str().unwrap()).await.unwrap();
    storage.migrate("certificate-pin", &identity).await.unwrap();
    storage.make_available().await.unwrap();
    let wallet = Wallet::new(
        Some(root.clone()),
        storage,
        MockWalletServices::builder().build(),
    )
    .await
    .unwrap();
    seed(&wallet, &root, rows).await;
    (dir, wallet)
}

async fn seed(wallet: &TestWallet, root: &PrivateKey, rows: &[(u8, bool)]) {
    let issuer = PrivateKey::from_hex(&"02".repeat(32)).unwrap();
    for &(n, good) in rows {
        let mut cert = Certificate::new([6; 32], [n; 32], root.public_key(), issuer.public_key())
            .with_revocation_outpoint(Some(Outpoint::new([0; 32], 0)));
        cert.set_field("name", "d2lyZQ==");
        cert.sign(&issuer).unwrap();
        let signature = if good {
            hex::encode(cert.signature.unwrap())
        } else {
            "3006020101020101".into()
        };
        // The audit reads legacy rows. Beta.3's acquire door correctly
        // refuses the invalid row, so seed the earlier stored state directly.
        let now = chrono::Utc::now();
        let id = sqlx::query("INSERT INTO certificates (user_id, type, serial_number, certifier, subject, revocation_outpoint, signature, created_at, updated_at) SELECT user_id, ?, ?, ?, ?, ?, ?, ?, ? FROM users WHERE identity_key = ?")
            .bind(to_base64(&[6; 32])).bind(to_base64(&[n; 32]))
            .bind(issuer.public_key().to_hex()).bind(root.public_key().to_hex())
            .bind(format!("{}.0", "00".repeat(32))).bind(signature)
            .bind(now).bind(now).bind(root.public_key().to_hex())
            .execute(wallet.storage().writer()).await.unwrap().last_insert_rowid();
        sqlx::query("INSERT INTO certificate_fields (certificate_id, user_id, field_name, field_value, master_key, created_at, updated_at) SELECT certificate_id, user_id, 'name', 'd2lyZQ==', '', ?, ? FROM certificates WHERE certificate_id = ?")
            .bind(now).bind(now).bind(id).execute(wallet.storage().writer())
            .await
            .unwrap();
    }
}

async fn snapshot(wallet: &TestWallet) -> Vec<(String, String)> {
    sqlx::query_as::<_, (String, String)>(
        "SELECT serial_number, signature FROM certificates ORDER BY serial_number",
    )
    .fetch_all(wallet.storage().pool())
    .await
    .unwrap()
}

fn reverify_output(path: &std::path::Path, root: &PrivateKey) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_bsv-wallet"))
        .current_dir(path.parent().unwrap())
        .env_remove("RUST_LOG")
        .env("ROOT_KEY", root.to_hex())
        .env("CHAINTRACKS_URL", "off")
        .env("ARC_MODE", "classic")
        .env("ARC_URL", "http://127.0.0.1:9")
        .args([
            "--db",
            path.to_str().unwrap(),
            "--json",
            "certificates",
            "--reverify",
        ])
        .output()
        .unwrap()
}
