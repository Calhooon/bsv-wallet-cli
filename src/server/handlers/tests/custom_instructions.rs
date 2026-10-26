//! The internalize-only UTF-8 byte bound (#157), through the served door.

use super::*;
use bsv_wallet_toolbox::WalletStorageWriter;

// #157: exercise the served door with valid, proven BEEF. A refused request
// must not store even the valid output preceding the oversized field.
async fn custom_instructions_wallet() -> (WalletState, axum::Router, Vec<u8>) {
    use crate::test_support::{p2pkh, raw_tx, txid_of, TestChainTracker};
    use bsv_sdk::transaction::{Beef, MerklePath};
    let height = 900_000;
    let raw = raw_tx(
        &[(&"11".repeat(32), 0)],
        &[(1000, p2pkh([0xdb; 20])), (2000, p2pkh([0xdc; 20]))],
    );
    let txid = txid_of(&raw);
    let mut beef = Beef::new();
    let bump = beef.merge_bump(MerklePath::from_coinbase_txid(&txid, height));
    beef.merge_raw_tx(raw, Some(bump));
    let bytes = beef.to_binary_atomic(&txid).unwrap();

    let storage = StorageSqlx::in_memory().await.unwrap();
    let key = bsv_sdk::primitives::PrivateKey::random();
    storage
        .migrate("custom-instructions", &key.public_key().to_hex())
        .await
        .unwrap();
    storage.make_available().await.unwrap();
    bsv_wallet_toolbox::MonitorStorage::set_max_acceptable_proof_height(&storage, u32::MAX)
        .await
        .unwrap();
    let mut tracker = TestChainTracker::new(height + 1);
    tracker.add_root(height, txid);
    storage.set_chain_tracker(Arc::new(tracker)).await;
    let services = Services::with_options(
        bsv_wallet_toolbox::Chain::Main,
        bsv_wallet_toolbox::ServicesOptions::mainnet(),
    )
    .unwrap();
    let wallet =
        crate::server::make_wallet_state(Wallet::new(Some(key), storage, services).await.unwrap());
    let router = crate::server::make_router(wallet.clone(), crate::server::ServerConfig::default());
    (wallet, router, bytes)
}

async fn custom_instructions_post(
    app: axum::Router,
    tx: Vec<u8>,
    instructions: &str,
    preceding_output: bool,
) -> (StatusCode, serde_json::Value) {
    use tower::ServiceExt;
    let output = |index, instructions: &str| {
        json!({
            "outputIndex": index,
            "protocol": "basket insertion",
            "insertionRemittance": { "basket": "instructions", "customInstructions": instructions }
        })
    };
    let outputs = if preceding_output {
        vec![output(0, "valid preceding output"), output(1, instructions)]
    } else {
        vec![output(0, instructions)]
    };
    let body = json!({ "tx": tx, "outputs": outputs, "description": "custom instructions byte bound", "labels": ["instructions-bound"] });
    let response = app
        .oneshot(
            axum::http::Request::post("/internalizeAction")
                .header("Origin", "http://test.local")
                .header("Content-Type", "application/json")
                .body(axum::body::Body::from(serde_json::to_vec(&body).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    (status, serde_json::from_slice(&bytes).unwrap())
}

async fn custom_instructions_stored_counts(wallet: &WalletState) -> (i64, i64, i64, i64, i64) {
    sqlx::query_as("SELECT (SELECT COUNT(*) FROM transactions), (SELECT COUNT(*) FROM outputs), (SELECT COUNT(*) FROM proven_txs), (SELECT COUNT(*) FROM proven_tx_reqs), (SELECT COUNT(*) FROM output_baskets)")
        .fetch_one(wallet.storage().pool()).await.unwrap()
}

#[tokio::test]
async fn internalize_custom_instructions_accepts_1000_bytes_and_reads_them_back() {
    let instructions = "a".repeat(1000);
    let (wallet, app, tx) = custom_instructions_wallet().await;
    let (status, body) = custom_instructions_post(app, tx, &instructions, false).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["accepted"], true);
    let stored: String =
        sqlx::query_scalar("SELECT custom_instructions FROM outputs WHERE vout = 0")
            .fetch_one(wallet.storage().pool())
            .await
            .unwrap();
    assert_eq!(stored, instructions);
}

#[tokio::test]
async fn internalize_custom_instructions_refuses_1001_bytes_before_any_output_is_stored() {
    let (wallet, app, tx) = custom_instructions_wallet().await;
    let before = custom_instructions_stored_counts(&wallet).await;
    let (status, body) = custom_instructions_post(app, tx, &"a".repeat(1001), true).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert_eq!(
        body,
        json!({
            "code": "VALIDATION_ERROR",
            "message": "Validation error: customInstructions: no more than 1000 length. (UTF-8 bytes: limit 1000, got 1001)"
        })
    );
    assert_eq!(custom_instructions_stored_counts(&wallet).await, before);
}

#[tokio::test]
async fn internalize_custom_instructions_counts_four_byte_characters_as_four_bytes() {
    for (instructions, accepted) in [
        ("🦀".repeat(250), true),
        (format!("{}a", "🦀".repeat(250)), false),
        ("🦀".repeat(251), false),
    ] {
        let (wallet, app, tx) = custom_instructions_wallet().await;
        let before = custom_instructions_stored_counts(&wallet).await;
        let (status, body) = custom_instructions_post(app, tx, &instructions, false).await;
        if accepted {
            assert_eq!(instructions.len(), 1000);
            assert_eq!(status, StatusCode::OK, "{body}");
            assert_eq!(body["accepted"], true);
            let stored: String =
                sqlx::query_scalar("SELECT custom_instructions FROM outputs WHERE vout = 0")
                    .fetch_one(wallet.storage().pool())
                    .await
                    .unwrap();
            assert_eq!(stored, instructions);
        } else {
            assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
            assert_eq!(
                body,
                json!({
                    "code": "VALIDATION_ERROR",
                    "message": format!("Validation error: customInstructions: no more than 1000 length. (UTF-8 bytes: limit 1000, got {})", instructions.len())
                })
            );
            assert_eq!(custom_instructions_stored_counts(&wallet).await, before);
        }
    }
}
