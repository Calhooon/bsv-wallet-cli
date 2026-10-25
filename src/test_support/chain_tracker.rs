//! A shared test tracker with the mining context required by toolbox #103.

#![allow(dead_code)]

use async_trait::async_trait;
use bsv_sdk::transaction::beef_stream::ChainTip;
use bsv_sdk::transaction::{
    AlwaysValidChainTracker, ChainTracker, ChainTrackerError, MockChainTracker,
};

/// Keep a mock's root and height answers while supplying a synthetic mining tip.
/// Toolbox #103's verifier holds an unproven payment when the tracker has no
/// tip: a missing tip is could-not-look, never a verdict. The context values
/// match the toolbox's mock; the height always comes from the wrapped tracker.
#[derive(Debug, Clone)]
pub struct TestChainTracker<T = MockChainTracker>(T);

impl<T> From<T> for TestChainTracker<T> {
    fn from(tracker: T) -> Self {
        Self(tracker)
    }
}

impl TestChainTracker {
    /// A root-aware SDK mock at `height` with mining context.
    #[must_use]
    pub fn new(height: u32) -> Self {
        Self(MockChainTracker::new(height))
    }

    /// Register the same root the wrapped SDK mock would accept.
    pub fn add_root(&mut self, height: u32, root: String) {
        self.0.add_root(height, root);
    }

    /// The SDK's permissive root answers at `height`, with mining context.
    #[must_use]
    pub fn always_valid(height: u32) -> TestChainTracker<AlwaysValidChainTracker> {
        TestChainTracker(MockChainTracker::always_valid(height))
    }
}

#[async_trait]
impl<T: ChainTracker> ChainTracker for TestChainTracker<T> {
    async fn is_valid_root_for_height(
        &self,
        merkle_root: &str,
        block_height: u32,
    ) -> Result<bool, ChainTrackerError> {
        self.0
            .is_valid_root_for_height(merkle_root, block_height)
            .await
    }

    async fn current_height(&self) -> Result<u32, ChainTrackerError> {
        self.0.current_height().await
    }

    async fn mining_tip(&self) -> Result<Option<ChainTip>, ChainTrackerError> {
        Ok(Some(ChainTip {
            height: self.0.current_height().await?,
            median_time_past: 1_700_000_000,
            genesis_height: 620_538,
            csv_height: 419_328,
            next_block_time: None,
        }))
    }
}
