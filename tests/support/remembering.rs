//! The toolbox's `MockWalletServices` with the broadcast memory the
//! concrete `Services` keeps: every answer and every call count is the
//! mock's, and a `post_beef` that a broadcaster accepts records that
//! broadcaster's acceptance (`provider`, `accepted`) for the posted txids in
//! the memory the storage attached (`StorageSqlx::set_services` attaches
//! its own), as `Services::post_beef` records it after each accepting
//! answer (bsv-wallet-toolbox-rs 0.14.0-beta.10, `services/services.rs`,
//! `remember_acceptance`). The plain mock keeps no memory, so a test of
//! what a broadcaster's acceptance leaves in the store needs this one. It
//! reaches nothing: every answer is the mock's. The methods it forwards
//! unchanged are `src/test_support/no_tracker.rs`'s macro
//! `forward_to_the_mock`, which the test target's root mounts before it.

use std::sync::{Arc, RwLock};

use bsv_sdk::transaction::ChainTracker;
use bsv_wallet_toolbox::services::broadcast_memory::{BroadcastMemory, BROADCAST_STATUS_ACCEPTED};
use bsv_wallet_toolbox::services::mock::MockWalletServices;
use bsv_wallet_toolbox::services::PostBeefResult;
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

forward_to_the_mock!(Remembering, {
    async fn get_chain_tracker(&self) -> Result<&dyn ChainTracker> {
        self.0.get_chain_tracker().await
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

    fn set_broadcast_memory(&self, memory: Arc<dyn BroadcastMemory>) {
        *self.1.write().unwrap() = Some(memory);
    }

    fn broadcast_memory(&self) -> Option<Arc<dyn BroadcastMemory>> {
        self.1.read().unwrap().clone()
    }
});
