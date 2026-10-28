//! #330: a tracker's typed transport fault stays a fault through internalize.
use bsv_sdk::transaction::{beef_stream::ChainTip, ChainTracker, ChainTrackerError};
#[derive(Debug)]
pub(crate) struct Unavailable {
    status: u16,
}
impl Unavailable {
    pub(crate) async fn install(storage: &bsv_wallet_toolbox::StorageSqlx) {
        storage
            .set_chain_tracker(std::sync::Arc::new(Self { status: 503 }))
            .await;
    }

    fn fault(&self) -> ChainTrackerError {
        ChainTrackerError::TransportFault {
            fault: bsv_sdk::TransportFault::MayHaveBeenReceived,
            status: Some(self.status),
            detail: "header fixture unavailable".into(),
        }
    }
}
#[async_trait::async_trait]
impl ChainTracker for Unavailable {
    async fn is_valid_root_for_height(&self, _: &str, _: u32) -> Result<bool, ChainTrackerError> {
        Err(self.fault())
    }
    async fn current_height(&self) -> Result<u32, ChainTrackerError> {
        Err(self.fault())
    }
    async fn mining_tip(&self) -> Result<Option<ChainTip>, ChainTrackerError> {
        Err(self.fault())
    }
}
