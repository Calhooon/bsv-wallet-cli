//! Host responses as functions of their individual store reads. Tests can
//! reach a later read while all earlier reads succeed.

use std::future::Future;

use anyhow::Result;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use bsv_wallet_toolbox::StorageSqlx;
use serde_json::{json, Value};

use crate::chain_feed::FeedStatus;

/// The stored tracker word, mined height and pending re-ask.
type StoredWord = (String, Option<bsv_tracker::Height>, Option<String>);
/// The stored proof's Merkle root and height.
type StoredProof = (String, i64);

/// The three reads that determine a chain-feed response.
pub(super) trait ChainFeedReads: Sync {
    /// The feed's last status, if it has run here.
    fn status(&self) -> impl Future<Output = Result<Option<FeedStatus>>> + Send;
    /// The last envelope delivered, or zero at birth.
    fn cursor(&self) -> impl Future<Output = Result<u64>> + Send;
    /// The number of frozen outpoints.
    fn frozen(&self) -> impl Future<Output = Result<u64>> + Send;
}

/// The three reads that determine a transaction-word response.
pub(super) trait TxReads: Sync {
    /// The tracker's word, if it holds one for this transaction.
    fn word(&self, txid: &str) -> impl Future<Output = Result<Option<StoredWord>>> + Send;
    /// The proof used as evidence for a mined word.
    fn proof(&self, txid: &str) -> impl Future<Output = Result<Option<StoredProof>>> + Send;
    /// The hint tier of a stale word, if present.
    fn tier(&self, txid: &str) -> impl Future<Output = Result<Option<&'static str>>> + Send;
}

/// Production reads from the wallet's existing storage handle.
pub(super) struct StoreReads<'a> {
    /// The same reader pool the host routes used before extracting the reads.
    pub(super) storage: &'a StorageSqlx,
}

impl ChainFeedReads for StoreReads<'_> {
    async fn status(&self) -> Result<Option<FeedStatus>> {
        crate::chain_feed::read_status(self.storage).await
    }

    async fn cursor(&self) -> Result<u64> {
        crate::tracker_host::feed_cursor(self.storage).await
    }

    async fn frozen(&self) -> Result<u64> {
        crate::tracker_host::frozen_count(self.storage).await
    }
}

impl TxReads for StoreReads<'_> {
    async fn word(&self, txid: &str) -> Result<Option<StoredWord>> {
        crate::tracker_host::stored_word(self.storage, txid).await
    }

    async fn proof(&self, txid: &str) -> Result<Option<StoredProof>> {
        crate::store::stored_proof(self.storage.pool(), txid).await
    }

    async fn tier(&self, txid: &str) -> Result<Option<&'static str>> {
        crate::tracker_host::stored_tier(self.storage, txid).await
    }
}

/// Read the feed fields in their original order, preserving every error type.
async fn read_feed(reads: &impl ChainFeedReads) -> Result<Value> {
    let status = reads.status().await?;
    let cursor = reads.cursor().await?;
    let frozen = reads.frozen().await?;
    Ok(json!({
        "status": status,
        "cursor": cursor.to_string(),
        "frozenCount": frozen,
    }))
}

/// Answer a chain-feed request through its one store-fault site.
pub(super) async fn answer_chain_feed(reads: &impl ChainFeedReads) -> Response {
    match read_feed(reads).await {
        Ok(body) => Json(body).into_response(),
        Err(e) => super::host_route_fault(&e, "ERR_CHAIN_FEED"),
    }
}

/// Read the word and its evidence, preserving the absent-word short circuit.
async fn read_word(reads: &impl TxReads, txid: &str) -> Result<Option<Value>> {
    let Some((word, height, reask)) = reads.word(txid).await? else {
        return Ok(None);
    };
    let evidence = if word == "mined" {
        reads
            .proof(txid)
            .await?
            .map(|(root, height)| json!({ "root": root, "height": height }))
    } else {
        None
    };
    let tier = reads.tier(txid).await?;
    let mut body = json!({
        "txid": txid,
        "word": word,
        "height": height,
        "reask": reask,
        "evidence": evidence,
    });
    if let Some(tier) = tier {
        body["tier"] = json!(tier);
    }
    Ok(Some(body))
}

/// Validate a transaction id and answer through the one store-fault site.
pub(super) async fn answer_tx_word(reads: &impl TxReads, txid: &str) -> Response {
    let txid = txid.trim().to_ascii_lowercase();
    if txid.len() != 64 || !txid.bytes().all(|b| b.is_ascii_hexdigit()) {
        return (
            StatusCode::BAD_REQUEST,
            Json(
                json!({ "code": "VALIDATION_ERROR", "message": "txid must be 64 hex characters" }),
            ),
        )
            .into_response();
    }
    match read_word(reads, &txid).await {
        Ok(Some(body)) => Json(body).into_response(),
        Ok(None) => (
            StatusCode::NOT_FOUND,
            Json(json!({ "code": "NOT_FOUND", "message": format!("no word stored for {txid}") })),
        )
            .into_response(),
        Err(e) => super::host_route_fault(&e, "ERR_TX_WORD"),
    }
}

#[cfg(test)]
mod tests;
