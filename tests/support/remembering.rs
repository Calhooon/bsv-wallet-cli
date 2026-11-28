//! The toolbox's `MockWalletServices` with the broadcast memory the
//! concrete `Services` keeps: every answer and every call count is the
//! mock's, and a `post_beef` that a broadcaster accepts records that
//! broadcaster's acceptance (`provider`, `accepted`) for the posted txids in
//! the memory the storage attached (`StorageSqlx::set_services` attaches
//! its own), as `Services::post_beef` records it after each accepting
//! answer (bsv-wallet-toolbox-rs 0.14.0-beta.10, `services/services.rs`,
//! `remember_acceptance`). The plain mock keeps no memory, so a test of
//! what a broadcaster's acceptance leaves in the store needs this one. It
//! reaches nothing: every answer is the mock's.

use std::sync::{Arc, RwLock};

use async_trait::async_trait;
use bsv_sdk::transaction::ChainTracker;
use bsv_wallet_toolbox::services::broadcast_memory::{BroadcastMemory, BROADCAST_STATUS_ACCEPTED};
use bsv_wallet_toolbox::services::mock::MockWalletServices;
use bsv_wallet_toolbox::services::{
    BlockHeader, BoundSpender, BroadcastStatus, FiatCurrency, GetBeefResult, GetMerklePathResult,
    GetRawTxResult, GetStatusForTxidsResult, GetUtxoStatusOutputFormat, GetUtxoStatusResult,
    NLockTimeInput, PostBeefResult, ServicesCallHistory, UtxoLeads, UtxoVerdict, WalletServices,
};
use bsv_wallet_toolbox::Result;

/// [`MockWalletServices`] that keeps the broadcast memory it is handed.
pub struct Remembering(
    pub MockWalletServices,
    RwLock<Option<Arc<dyn BroadcastMemory>>>,
);

impl Remembering {
    pub fn new(mock: MockWalletServices) -> Self {
        Self(mock, RwLock::new(None))
    }

    pub fn call_count(&self, method: &str) -> usize {
        self.0.call_count(method)
    }
}

#[async_trait]
impl WalletServices for Remembering {
    async fn get_chain_tracker(&self) -> Result<&dyn ChainTracker> {
        self.0.get_chain_tracker().await
    }

    async fn get_height(&self) -> Result<u32> {
        self.0.get_height().await
    }

    async fn get_header_for_height(&self, height: u32) -> Result<Vec<u8>> {
        self.0.get_header_for_height(height).await
    }

    async fn get_chain_tip_header(&self) -> Result<BlockHeader> {
        self.0.get_chain_tip_header().await
    }

    async fn hash_to_header(&self, hash: &str) -> Result<BlockHeader> {
        self.0.hash_to_header(hash).await
    }

    async fn get_raw_tx(&self, txid: &str, use_next: bool) -> Result<GetRawTxResult> {
        self.0.get_raw_tx(txid, use_next).await
    }

    async fn get_merkle_path(&self, txid: &str, use_next: bool) -> Result<GetMerklePathResult> {
        self.0.get_merkle_path(txid, use_next).await
    }

    /// The mock's answer, each accepting broadcaster's acceptance recorded
    /// in the attached memory as `Services::post_beef` records it.
    async fn post_beef(&self, beef: &[u8], txids: &[String]) -> Result<Vec<PostBeefResult>> {
        let results = self.0.post_beef(beef, txids).await?;
        if let Some(memory) = self.broadcast_memory() {
            for result in results.iter().filter(|r| r.is_success()) {
                memory
                    .record_broadcast_seen_many(&result.name, BROADCAST_STATUS_ACCEPTED, txids)
                    .await
                    .expect("the acceptance recorded");
            }
        }
        Ok(results)
    }

    async fn get_utxo_status(
        &self,
        output: &str,
        output_format: Option<GetUtxoStatusOutputFormat>,
        outpoint: Option<&str>,
        use_next: bool,
    ) -> Result<GetUtxoStatusResult> {
        self.0
            .get_utxo_status(output, output_format, outpoint, use_next)
            .await
    }

    async fn get_status_for_txids(
        &self,
        txids: &[String],
        use_next: bool,
    ) -> Result<GetStatusForTxidsResult> {
        self.0.get_status_for_txids(txids, use_next).await
    }

    async fn get_bsv_exchange_rate(&self) -> Result<f64> {
        self.0.get_bsv_exchange_rate().await
    }

    async fn get_fiat_exchange_rate(
        &self,
        currency: FiatCurrency,
        base: Option<FiatCurrency>,
    ) -> Result<f64> {
        self.0.get_fiat_exchange_rate(currency, base).await
    }

    fn hash_output_script(&self, script: &[u8]) -> String {
        self.0.hash_output_script(script)
    }

    async fn is_utxo(&self, txid: &str, vout: u32, locking_script: &[u8]) -> UtxoVerdict {
        self.0.is_utxo(txid, vout, locking_script).await
    }

    async fn is_utxo_with_spender(
        &self,
        txid: &str,
        vout: u32,
        locking_script: &[u8],
    ) -> (UtxoVerdict, Option<BoundSpender>) {
        self.0
            .is_utxo_with_spender(txid, vout, locking_script)
            .await
    }

    async fn get_utxo_leads(
        &self,
        script_hash: &str,
        output_format: Option<GetUtxoStatusOutputFormat>,
    ) -> Result<UtxoLeads> {
        self.0.get_utxo_leads(script_hash, output_format).await
    }

    async fn n_lock_time_is_final(&self, n_lock_time: u32) -> Result<bool> {
        self.0.n_lock_time_is_final(n_lock_time).await
    }

    async fn n_lock_time_is_final_for_tx(&self, input: NLockTimeInput) -> Result<bool> {
        self.0.n_lock_time_is_final_for_tx(input).await
    }

    async fn get_beef(&self, txid: &str, known_txids: &[String]) -> Result<GetBeefResult> {
        self.0.get_beef(txid, known_txids).await
    }

    fn set_broadcast_memory(&self, memory: Arc<dyn BroadcastMemory>) {
        *self.1.write().unwrap() = Some(memory);
    }

    fn broadcast_memory(&self) -> Option<Arc<dyn BroadcastMemory>> {
        self.1.read().unwrap().clone()
    }

    fn get_services_call_history(&self, reset: bool) -> ServicesCallHistory {
        self.0.get_services_call_history(reset)
    }

    async fn get_broadcaster_statuses(&self, txid: &str) -> Vec<(String, BroadcastStatus)> {
        self.0.get_broadcaster_statuses(txid).await
    }
}
