//! Shared ingestion of ARC/Arcade callback payloads.
//!
//! Both proof-delivery push paths converge here:
//! - the daemon's own `POST /arc-callback` route (direct webhook), and
//! - the `bsv-wallet-relay` poller (store-and-forward webhook).
//!
//! Payload shape (ARC webhook convention, Arcade V2 verified):
//! `{ "txid", "txStatus", "blockHash"?, "blockHeight"?, "merklePath"?, ... }`
//! — `merklePath` (BUMP hex) is present on `MINED`.
//!
//! With a merkle path present the proof is validated and stored via the
//! toolbox's `StorageSqlx::ingest_merkle_proof` (ChainTracker validation →
//! `proven_txs` insert → complete `proven_tx_reqs`/`transactions`). Status-only
//! payloads map through the same status transitions as the Monitor's SSE task.

use anyhow::{anyhow, Result};
use bsv_wallet_toolbox::monitor::ArcadeEventsTask;
use bsv_wallet_toolbox::{MonitorStorage, ProofIngestOutcome, StorageSqlx};
use std::sync::atomic::AtomicBool;

/// What ingesting a callback payload did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IngestAction {
    /// A merkle proof was validated and stored; records completed.
    ProofIngested,
    /// The proof was rejected (invalid root / unparseable) — NOT stored.
    ProofRejected(String),
    /// A status-only update was applied to storage.
    StatusApplied,
    /// A status-only update matched no records (unknown txid or already final).
    StatusIgnored,
}

/// Parse and apply one ARC/Arcade callback payload against wallet storage.
pub async fn ingest_arc_payload(
    storage: &StorageSqlx,
    payload: &serde_json::Value,
) -> Result<IngestAction> {
    let txid = payload
        .get("txid")
        .and_then(|v| v.as_str())
        .ok_or_else(|| anyhow!("payload missing txid"))?;
    if txid.len() != 64 || !txid.chars().all(|c| c.is_ascii_hexdigit()) {
        return Err(anyhow!("invalid txid"));
    }
    let tx_status = payload
        .get("txStatus")
        .and_then(|v| v.as_str())
        .unwrap_or("");

    let merkle_path_hex = payload
        .get("merklePath")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty());

    if let Some(mp_hex) = merkle_path_hex {
        let merkle_path = hex::decode(mp_hex).map_err(|e| anyhow!("merklePath not hex: {}", e))?;
        let block_height = payload
            .get("blockHeight")
            .and_then(|v| v.as_u64())
            .ok_or_else(|| anyhow!("merklePath payload missing blockHeight"))?
            as u32;
        let block_hash = payload
            .get("blockHash")
            .and_then(|v| v.as_str())
            .unwrap_or_default();

        // Ensure spendability transition happened even if we never saw
        // SEEN_ON_NETWORK (webhook may be the only delivery path).
        let _ = storage.mark_transaction_seen_on_network(txid).await;

        match storage
            .ingest_merkle_proof(txid, &merkle_path, block_height, block_hash, None)
            .await
            .map_err(|e| anyhow!("ingest_merkle_proof: {}", e))?
        {
            ProofIngestOutcome::Ingested(status) => {
                tracing::info!(
                    txid = %txid,
                    block_height = ?status.block_height,
                    "arc-callback: merkle proof ingested"
                );
                Ok(IngestAction::ProofIngested)
            }
            ProofIngestOutcome::InvalidMerkleRoot { computed_root } => {
                tracing::warn!(txid = %txid, computed_root = %computed_root, "arc-callback: proof rejected (invalid merkle root)");
                Ok(IngestAction::ProofRejected("invalid merkle root".into()))
            }
            ProofIngestOutcome::InvalidProof(e) => {
                tracing::warn!(txid = %txid, error = %e, "arc-callback: proof rejected (unparseable)");
                Ok(IngestAction::ProofRejected(e))
            }
            ProofIngestOutcome::TrackerError(e) => {
                tracing::warn!(txid = %txid, error = %e, "arc-callback: proof deferred (ChainTracker error) — polling sync will retry");
                Ok(IngestAction::ProofRejected(format!("tracker error: {}", e)))
            }
        }
    } else {
        // Status-only payload — identical mapping to the Monitor's SSE task.
        // Since arcade v0.10.1 (#259) MINED webhooks DO carry merklePath and
        // take the proof branch above; a status-only MINED still lands here
        // when upstream enrichment best-effort-misses (BUMP-cache eviction /
        // reorg race), and the proof is then fetched by the SSE task's
        // trigger / the polling backstop.
        let trigger = AtomicBool::new(false);
        let updated =
            ArcadeEventsTask::<StorageSqlx>::apply_status_event(storage, txid, tx_status, &trigger)
                .await
                .map_err(|e| anyhow!("apply_status_event: {}", e))?;
        tracing::info!(
            txid = %txid,
            status = %tx_status,
            updated,
            "arc-callback: status webhook received"
        );
        if updated {
            Ok(IngestAction::StatusApplied)
        } else {
            Ok(IngestAction::StatusIgnored)
        }
    }
}
