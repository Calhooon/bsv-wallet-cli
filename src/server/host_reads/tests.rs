//! Real locked-store errors fed at each host read, including later reads.

use std::future::ready;
use std::sync::Mutex;

use super::*;
use crate::test_support::{busy_read_wallet, response_answer};

/// A valid transaction id that allows the response body to reach its reads.
const TXID: &str = "1111111111111111111111111111111111111111111111111111111111111111";

/// Successful reads except for the selected position, which consumes a
/// real store error once. Calls record that earlier reads actually succeeded.
struct FaultReads {
    /// The one failing read.
    position: &'static str,
    /// An error returned by a helper under another connection's lock.
    fault: Mutex<Option<anyhow::Error>>,
    /// Reads reached by the response body, in order.
    calls: Mutex<Vec<&'static str>>,
}

impl FaultReads {
    /// Return a normal value unless this is the selected failure position.
    fn read<T>(&self, position: &'static str, value: T) -> Result<T> {
        self.calls.lock().unwrap().push(position);
        if position == self.position {
            return Err(self.fault.lock().unwrap().take().unwrap());
        }
        Ok(value)
    }
}

impl ChainFeedReads for FaultReads {
    fn status(&self) -> impl Future<Output = Result<Option<FeedStatus>>> + Send {
        ready(self.read("status", None))
    }

    fn cursor(&self) -> impl Future<Output = Result<u64>> + Send {
        ready(self.read("cursor", 17))
    }

    fn frozen(&self) -> impl Future<Output = Result<u64>> + Send {
        ready(self.read("frozen", 2))
    }
}

impl TxReads for FaultReads {
    fn word(&self, _: &str) -> impl Future<Output = Result<Option<StoredWord>>> + Send {
        ready(self.read("word", Some(("mined".to_string(), Some(42), None))))
    }

    fn proof(&self, _: &str) -> impl Future<Output = Result<Option<StoredProof>>> + Send {
        ready(self.read("proof", Some(("ab".repeat(32), 42))))
    }

    fn tier(&self, _: &str) -> impl Future<Output = Result<Option<&'static str>>> + Send {
        ready(self.read("tier", None))
    }
}

/// The host body under test, with its read order and plain-fault code.
#[derive(Clone, Copy)]
enum Route {
    /// GET /chain-feed.
    ChainFeed,
    /// GET /tx/{txid}, using a mined word to reach the proof read.
    Transaction,
}

impl Route {
    /// Every read that a complete successful response reaches.
    fn positions(self) -> &'static [&'static str] {
        match self {
            Self::ChainFeed => &["status", "cursor", "frozen"],
            Self::Transaction => &["word", "proof", "tier"],
        }
    }

    /// The route's existing code for faults that are not busy.
    fn code(self) -> &'static str {
        match self {
            Self::ChainFeed => "ERR_CHAIN_FEED",
            Self::Transaction => "ERR_TX_WORD",
        }
    }

    /// Call the same response body as the axum adapter.
    async fn answer(self, reads: &FaultReads) -> Response {
        match self {
            Self::ChainFeed => answer_chain_feed(reads).await,
            Self::Transaction => answer_tx_word(reads, TXID).await,
        }
    }
}

/// Read a helper under the shared fixture's lock; never manufacture busy.
async fn helper_fault(reads: &StoreReads<'_>, position: &str) -> anyhow::Error {
    let result = match position {
        "status" => reads.status().await.map(|_| ()),
        "word" => reads.word(TXID).await.map(|_| ()),
        "proof" => reads.proof(TXID).await.map(|_| ()),
        "tier" => reads.tier(TXID).await.map(|_| ()),
        "cursor" => reads.cursor().await.map(|_| ()),
        "frozen" => reads.frozen().await.map(|_| ()),
        _ => panic!("unknown store read: {position}"),
    };
    result.expect_err("another connection holds the store lock")
}

/// Check one position's complete error response and the reads it reached.
async fn assert_fault_answer(route: Route, position: usize, fault: anyhow::Error, busy: bool) {
    let reads = FaultReads {
        position: route.positions()[position],
        calls: Mutex::new(Vec::new()),
        fault: Mutex::new(Some(fault)),
    };
    let message = reads.fault.lock().unwrap().as_ref().unwrap().to_string();
    let expected = if busy {
        (
            StatusCode::SERVICE_UNAVAILABLE,
            Some("1".to_string()),
            "STORE_BUSY",
        )
    } else {
        (StatusCode::INTERNAL_SERVER_ERROR, None, route.code())
    };
    assert_eq!(
        response_answer(route.answer(&reads).await).await,
        (
            expected.0,
            expected.1,
            json!({"code": expected.2, "message": message})
        ),
        "{} at {}",
        route.code(),
        reads.position,
    );
    assert_eq!(*reads.calls.lock().unwrap(), route.positions()[..=position]);
}

/// One table per route. Each case acquires its real error once under the
/// same fixture's lock, then feeds it past successful earlier reads.
async fn assert_position_answers(route: Route) {
    use sqlx::Connection;

    let (_dir, wallet, holder) = busy_read_wallet().await;
    let reads = StoreReads {
        storage: wallet.storage(),
    };
    let mut busy_message = String::new();
    for (index, position) in route.positions().iter().enumerate() {
        let busy = helper_fault(&reads, position).await;
        assert!(crate::store::is_store_busy(&busy), "{position}: {busy}");
        busy_message = busy.to_string();
        assert_fault_answer(route, index, busy, true).await;
    }
    holder.close().await.unwrap();
    for plain in [
        anyhow::Error::new(sqlx::Error::RowNotFound),
        anyhow::anyhow!(busy_message),
    ] {
        assert_fault_answer(route, 2, plain, false).await;
    }
}

#[tokio::test]
async fn chain_feed_each_read_is_503_store_busy() {
    assert_position_answers(Route::ChainFeed).await;
}

#[tokio::test]
async fn tx_word_each_read_is_503_store_busy() {
    assert_position_answers(Route::Transaction).await;
}

/// Check the production adapter and its helper preserve a real busy type.
async fn assert_helper_busy(position: &str) {
    use sqlx::Connection;

    let (_dir, wallet, holder) = busy_read_wallet().await;
    let busy = helper_fault(
        &StoreReads {
            storage: wallet.storage(),
        },
        position,
    )
    .await;
    assert!(crate::store::is_store_busy(&busy), "{position}: {busy}");
    holder.close().await.unwrap();
}

#[tokio::test]
async fn read_status_preserves_busy_type() {
    assert_helper_busy("status").await;
}

#[tokio::test]
async fn stored_word_preserves_busy_type() {
    assert_helper_busy("word").await;
}

#[tokio::test]
async fn stored_tier_preserves_busy_type() {
    assert_helper_busy("tier").await;
}

#[tokio::test]
async fn feed_cursor_preserves_busy_type() {
    assert_helper_busy("cursor").await;
}

#[tokio::test]
async fn frozen_count_preserves_busy_type() {
    assert_helper_busy("frozen").await;
}

#[tokio::test]
async fn proof_query_preserves_busy_type() {
    assert_helper_busy("proof").await;
}
