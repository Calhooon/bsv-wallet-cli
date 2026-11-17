//! Services with no chain tracker, as a wallet built with
//! `CHAINTRACKS_URL=off` has: the toolbox's `MockWalletServices` for every
//! answer and every call count, except that `get_chain_tracker` is the
//! error `Services` gives with no `chaintracks_url` (the toolbox's
//! not-configured text, which its doors read as no header source). A
//! wallet whose storage has no tracker and whose services have none has
//! every `internalizeAction` refused (the toolbox's door holds with no
//! header source, #509); a test of that mode reads what the mock was asked
//! (nothing posted, nothing fetched).

use std::sync::Arc;

use async_trait::async_trait;
use bsv_sdk::transaction::ChainTracker;
use bsv_wallet_toolbox::services::broadcast_memory::BroadcastMemory;
use bsv_wallet_toolbox::services::mock::MockWalletServices;
use bsv_wallet_toolbox::services::{
    BlockHeader, BoundSpender, FiatCurrency, GetBeefResult, GetMerklePathResult, GetRawTxResult,
    GetStatusForTxidsResult, GetUtxoStatusOutputFormat, GetUtxoStatusResult, NLockTimeInput,
    PostBeefResult, ServicesCallHistory, UtxoLeads, UtxoVerdict, WalletServices,
};
use bsv_wallet_toolbox::{Error, Result};

/// [`MockWalletServices`] with no chain tracker.
pub struct NoTracker(pub MockWalletServices);

impl NoTracker {
    pub fn new() -> Self {
        Self(MockWalletServices::new())
    }

    /// Every call the services were asked, `get_chain_tracker` included.
    pub fn total_calls(&self) -> usize {
        self.0.total_calls()
    }

    pub fn call_count(&self, method: &str) -> usize {
        self.0.call_count(method)
    }
}

impl Default for NoTracker {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl WalletServices for NoTracker {
    async fn get_chain_tracker(&self) -> Result<&dyn ChainTracker> {
        Err(Error::ServiceError(
            "ChainTracker not configured in Services (no chaintracks_url)".to_string(),
        ))
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

    async fn post_beef(&self, beef: &[u8], txids: &[String]) -> Result<Vec<PostBeefResult>> {
        self.0.post_beef(beef, txids).await
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
        self.0.set_broadcast_memory(memory);
    }

    fn broadcast_memory(&self) -> Option<Arc<dyn BroadcastMemory>> {
        self.0.broadcast_memory()
    }

    fn get_services_call_history(&self, reset: bool) -> ServicesCallHistory {
        self.0.get_services_call_history(reset)
    }
}
