//! #330: a tracker's typed transport fault stays a fault through internalize.
use bsv_sdk::transaction::{beef_stream::ChainTip, ChainTracker, ChainTrackerError};
#[derive(Debug)]
pub(crate) struct Unavailable;
fn fault() -> ChainTrackerError {
    ChainTrackerError::TransportFault {
        fault: bsv_sdk::TransportFault::MayHaveBeenReceived,
        status: Some(503),
        detail: "header fixture unavailable".into(),
    }
}
#[async_trait::async_trait]
impl ChainTracker for Unavailable {
    async fn is_valid_root_for_height(&self, _: &str, _: u32) -> Result<bool, ChainTrackerError> {
        Err(fault())
    }
    async fn current_height(&self) -> Result<u32, ChainTrackerError> {
        Err(fault())
    }
    async fn mining_tip(&self) -> Result<Option<ChainTip>, ChainTrackerError> {
        Err(fault())
    }
}
