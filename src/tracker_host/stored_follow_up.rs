//! Read persisted re-announcement arguments without migrating stored rows.
use bsv_sdk::wallet::InternalizeActionArgs;
use serde::{Deserialize, Deserializer};

#[derive(Deserialize)]
#[serde(transparent)]
struct StoredArgs(#[serde(deserialize_with = "deserialize_args")] InternalizeActionArgs);

/// Decode either persisted transaction encoding, without changing the row.
pub(super) fn read(text: &str) -> serde_json::Result<InternalizeActionArgs> {
    serde_json::from_str::<StoredArgs>(text).map(|args| args.0)
}

fn deserialize_args<'de, D: Deserializer<'de>>(de: D) -> Result<InternalizeActionArgs, D::Error> {
    let mut value = serde_json::Value::deserialize(de)?;
    // bsv-rs beta.2 types.rs:1224 removed hex_bytes from InternalizeActionArgs.tx.
    // Beta.1 persisted hex strings; new rows carry byte arrays. Normalize only
    // the old field while reading, then let the pinned type validate both.
    if let Some(serde_json::Value::String(tx)) = value.get("tx") {
        let bytes = hex::decode(tx).map_err(serde::de::Error::custom)?;
        value["tx"] = serde_json::json!(bytes);
    }
    serde_json::from_value(value).map_err(serde::de::Error::custom)
}

#[cfg(test)]
mod tests {
    use super::super::*;
    use crate::test_support::{p2pkh, raw_tx, txid_of, TestChainTracker};
    use bsv_sdk::primitives::PrivateKey;
    use bsv_sdk::transaction::{Beef, MerklePath};
    use bsv_wallet_toolbox::services::mock::{
        success_post_beef_result, MockResponse, MockWalletServices,
    };
    use bsv_wallet_toolbox::{MonitorStorage, Wallet, WalletStorageWriter};
    use serde_json::json;

    async fn seeded() -> (Wallet<StorageSqlx, MockWalletServices>, String, Vec<u8>) {
        let raw = raw_tx(&[(&"11".repeat(32), 0)], &[(1000, p2pkh([7; 20]))]);
        let txid = txid_of(&raw);
        let mut beef = Beef::new();
        let bump = beef.merge_bump(MerklePath::from_coinbase_txid(&txid, 900_000));
        beef.merge_raw_tx(raw, Some(bump));
        let bytes = beef.to_binary_atomic(&txid).unwrap();
        let root = PrivateKey::from_hex(&"01".repeat(32)).unwrap();
        let storage = StorageSqlx::in_memory().await.unwrap();
        storage
            .migrate("stored-follow-up", &root.public_key().to_hex())
            .await
            .unwrap();
        storage.make_available().await.unwrap();
        MonitorStorage::set_max_acceptable_proof_height(&storage, u32::MAX)
            .await
            .unwrap();
        let mut tracker = TestChainTracker::new(900_001);
        tracker.add_root(900_000, txid.clone());
        storage
            .set_chain_tracker(std::sync::Arc::new(tracker))
            .await;
        let wallet = Wallet::new(Some(root), storage, MockWalletServices::builder().build())
            .await
            .unwrap();
        (wallet, txid, bytes)
    }

    async fn accepted(
        wallet: &Wallet<StorageSqlx, MockWalletServices>,
        txid: &str,
        bytes: &[u8],
        old_hex: bool,
    ) -> TickReport {
        let now = 2_000_000_000;
        let options = tests::opts();
        heard(
            wallet.storage(),
            &tests::FixedClock(now - 1),
            &options,
            txid,
            Hint::new(
                "Arcade|rejected",
                HintStatus::Rejected {
                    reason: "476 non-final".into(),
                },
                now - 1,
            ),
            Some(now),
            Some(Reannounce {
                beef: bytes.to_vec(),
                at: now,
                on_accept: None,
            }),
        )
        .await
        .unwrap();
        let mut args = json!({"tx": bytes, "description": "accepted stored follow-up",
        "outputs": [{"outputIndex": 0, "protocol": "basket insertion",
            "insertionRemittance": {"basket": "follow-up"}}]});
        if old_hex {
            args["tx"] = json!(hex::encode(bytes));
        }
        let planted = args.to_string();
        sqlx::query("UPDATE tracker_states SET on_accept = ? WHERE txid = ?")
            .bind(&planted)
            .bind(txid)
            .execute(wallet.storage().writer())
            .await
            .unwrap();
        let services = MockWalletServices::builder()
            .post_beef_response(MockResponse::Success(vec![success_post_beef_result(
                "Arcade",
                &[txid],
            )]))
            .build();
        let report = tick(
            wallet.storage(),
            &services,
            &tests::FixedClock(now),
            &options,
        )
        .await
        .unwrap();
        assert_eq!(report.reannounced, vec![txid.to_string()]);
        assert_eq!(report.on_accept, vec![(txid.to_string(), planted)]);
        report
    }

    /// Both stored shapes re-announce and leave a transaction and output in storage.
    #[tokio::test]
    async fn stored_beta1_follow_up_records_the_accepted_transaction() {
        for old_hex in [true, false] {
            let (wallet, txid, bytes) = seeded().await;
            let mut report = accepted(&wallet, &txid, &bytes, old_hex).await;
            record_accepted(&wallet, &mut report).await;
            assert_eq!(
                report.stored,
                vec![format!("{txid}: recorded in the wallet")]
            );
            assert!(report.on_accept.is_empty());
            let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM transactions WHERE txid = ?")
                .bind(&txid)
                .fetch_one(wallet.storage().pool())
                .await
                .unwrap();
            assert_eq!(count, 1);
            let output: (i64, i64) =
                sqlx::query_as("SELECT vout, satoshis FROM outputs WHERE txid = ?")
                    .bind(&txid)
                    .fetch_one(wallet.storage().pool())
                    .await
                    .unwrap();
            assert_eq!(output, (0, 1000));
        }
    }
}
