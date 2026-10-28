//! The beta.2 certificate bounds and the host's explicit, read-only reverify.
use bsv_sdk::auth::certificates::Certificate;
use bsv_sdk::primitives::PrivateKey;
use bsv_sdk::wallet::{AcquireCertificateArgs, WalletInterface};
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
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("wallet.db");
    let root = PrivateKey::from_hex(&"01".repeat(32)).unwrap();
    let issuer = PrivateKey::from_hex(&"02".repeat(32)).unwrap();
    let storage = StorageSqlx::open(path.to_str().unwrap()).await.unwrap();
    storage
        .migrate("certificate-pin", &root.public_key().to_hex())
        .await
        .unwrap();
    storage.make_available().await.unwrap();
    let wallet = Wallet::new(
        Some(root.clone()),
        storage,
        MockWalletServices::builder().build(),
    )
    .await
    .unwrap();
    for n in [7, 8] {
        let mut cert = Certificate::new([6; 32], [n; 32], root.public_key(), issuer.public_key());
        cert.set_field("name", "d2lyZQ==");
        cert.sign(&issuer).unwrap();
        let mut args = serde_json::to_value(&cert).unwrap();
        args["acquisitionProtocol"] = json!("direct");
        args["keyringRevealer"] = json!("certifier");
        args["keyringForSubject"] = json!({});
        args["revocationOutpoint"] = json!(format!("{}.0", "00".repeat(32)));
        if n == 8 {
            args["signature"] = json!("3006020101020101");
        }
        wallet
            .acquire_certificate(
                serde_json::from_value::<AcquireCertificateArgs>(args).unwrap(),
                Some("test.local"),
            )
            .await
            .unwrap();
    }
    let snapshot = || async {
        sqlx::query_as::<_, (String, String)>(
            "SELECT serial_number, signature FROM certificates ORDER BY serial_number",
        )
        .fetch_all(wallet.storage().pool())
        .await
        .unwrap()
    };
    let before = snapshot().await;
    for _ in 0..2 {
        let output = Command::new(env!("CARGO_BIN_EXE_bsv-wallet"))
            .current_dir(dir.path())
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
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let result: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(result, json!({"checked": 2, "failed": 1}));
    }
    assert_eq!(snapshot().await, before);
}
