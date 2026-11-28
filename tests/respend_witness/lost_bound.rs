//! The settle's binding (#660, R9): `settle_proven` moves our coins to the
//! replacement only on the replacement's own bytes (they hash to its txid),
//! on the outpoints those bytes spend, under a root computed from that
//! txid that our headers carry. Each plant below is one missing binding
//! (or a fault); the expected answer is that no coin moves and the link
//! stays unsettled. Every service is `MockWalletServices` and every
//! tracker `TestChainTracker` or the fault tracker here: no network.

use std::sync::Arc;

use async_trait::async_trait;
use bsv_sdk::script::{LockingScript, UnlockingScript};
use bsv_sdk::transaction::{
    ChainTracker, ChainTrackerError, Transaction, TransactionInput, TransactionOutput,
};
use bsv_wallet_cli::respend::settle_proven;
use bsv_wallet_toolbox::services::mock::MockWalletServices;
use bsv_wallet_toolbox::{MonitorStorage, WalletServices, WalletStorageProvider};

use crate::world::{
    coin_state, fixture, headers_with, proof, proof_services, seen_once_absent_now, words, Fixture,
};

/// A tracker that cannot look: every root question is a fault, its text.
#[derive(Debug)]
struct FaultTracker(&'static str);

#[async_trait]
impl ChainTracker for FaultTracker {
    async fn is_valid_root_for_height(
        &self,
        root: &str,
        height: u32,
    ) -> Result<bool, ChainTrackerError> {
        Err(ChainTrackerError::NetworkError(format!(
            "{}: {root} at {height}",
            self.0
        )))
    }

    async fn current_height(&self) -> Result<u32, ChainTrackerError> {
        Err(ChainTrackerError::NetworkError(self.0.to_string()))
    }
}

/// The world one step before the settle: the replacement built and
/// posted, its competitor proof written `doubleSpend` on the phantom, its
/// own proof ingested. `settle_proven` has not run.
struct Settling {
    f: Fixture,
    replacement: String,
    replacement_row: i64,
    services: Arc<MockWalletServices>,
}

async fn settling() -> Settling {
    let f = fixture().await;
    let s = f.wallet.storage();
    let absent = seen_once_absent_now(&f).await;
    let replacement = crate::build_and_post(s, &absent, &f.root, &f.phantom, &["mock"])
        .await
        .expect("the respend builds and posts");
    let raw: Vec<u8> = sqlx::query_scalar("SELECT raw_tx FROM transactions WHERE txid=?")
        .bind(&replacement)
        .fetch_one(s.pool())
        .await
        .unwrap();
    let services = proof_services(&replacement, raw);
    WalletStorageProvider::set_services(s, services.clone() as Arc<dyn WalletServices>);
    s.set_max_acceptable_proof_height(871_000).await.unwrap();
    s.set_chain_tracker(headers_with(&replacement)).await;
    s.synchronize_transaction_statuses().await.unwrap();
    assert_eq!(words(&f).await, ("failed".into(), "doubleSpend".into()));
    s.ingest_merkle_proof(
        &replacement,
        &proof(&replacement),
        870_900,
        &"aa".repeat(32),
        None,
    )
    .await
    .unwrap();
    let replacement_row =
        sqlx::query_scalar("SELECT transaction_id FROM transactions WHERE txid=?")
            .bind(&replacement)
            .fetch_one(s.pool())
            .await
            .unwrap();
    Settling {
        f,
        replacement,
        replacement_row,
        services,
    }
}

async fn settled(w: &Settling) -> i64 {
    sqlx::query_scalar("SELECT settled FROM respend_links WHERE replacement_id=?")
        .bind(w.replacement_row)
        .fetch_one(w.f.wallet.storage().pool())
        .await
        .unwrap()
}

/// The phantom's change output: a free coin of ours the replacement does
/// not spend, as `(txid, vout)`.
async fn free_coin(f: &Fixture) -> (String, u32) {
    let vout: i64 = sqlx::query_scalar("SELECT vout FROM outputs WHERE output_id=?")
        .bind(f.change)
        .fetch_one(f.wallet.storage().pool())
        .await
        .unwrap();
    (f.phantom.clone(), u32::try_from(vout).unwrap())
}

/// After a plant: nothing moved, nothing settled.
async fn nothing_moved(w: &Settling, free_before: (bool, Option<i64>)) {
    let f = &w.f;
    assert_eq!(
        coin_state(f, f.coin).await,
        (false, Some(f.phantom_row)),
        "the phantom's input stays where it was"
    );
    assert_eq!(
        coin_state(f, f.change).await,
        free_before,
        "a coin the replacement does not spend never moves"
    );
    assert_eq!(settled(w).await, 0, "the link stays unsettled");
}

/// Settle under `tracker`; nothing moves.
async fn settle_moves_nothing(w: &Settling, tracker: Arc<dyn ChainTracker>) {
    let s = w.f.wallet.storage();
    let free_before = coin_state(&w.f, w.f.change).await;
    s.set_chain_tracker(tracker).await;
    settle_proven(s, w.services.as_ref()).await.ok();
    nothing_moved(w, free_before).await;
}

/// (i) Bytes stored under the replacement's txid that are not its
/// transaction (they spend the phantom's change, a free coin of ours):
/// no coin moves.
#[tokio::test]
async fn settle_bytes_that_do_not_hash_to_the_replacement_move_no_coin() {
    let w = settling().await;
    let s = w.f.wallet.storage();
    let free_before = coin_state(&w.f, w.f.change).await;
    assert_eq!(free_before.1, None, "the change is a free coin");
    let (src, vout) = free_coin(&w.f).await;
    let mut planted = Transaction::new();
    let mut input = TransactionInput::new(src, vout);
    input.unlocking_script = Some(UnlockingScript::from_hex("00").unwrap());
    planted.inputs_mut().push(input);
    planted.outputs_mut().push(TransactionOutput {
        satoshis: Some(1),
        locking_script: LockingScript::from_hex("51").unwrap(),
        change: false,
    });
    assert_ne!(planted.id(), w.replacement);
    let n = sqlx::query("UPDATE proven_txs SET raw_tx=? WHERE txid=?")
        .bind(planted.to_binary())
        .bind(&w.replacement)
        .execute(s.writer())
        .await
        .unwrap()
        .rows_affected();
    assert_eq!(n, 1, "the plant sits under the replacement's proof");
    settle_proven(s, w.services.as_ref()).await.ok();
    nothing_moved(&w, free_before).await;
}

/// (iii) The stored path's leaf is another transaction of the block: the
/// root computed from the replacement's txid does not exist, no coin moves.
#[tokio::test]
async fn settle_a_path_for_another_transaction_of_the_block_moves_no_coin() {
    let w = settling().await;
    let other = "ab".repeat(32);
    sqlx::query("UPDATE proven_txs SET merkle_path=? WHERE txid=?")
        .bind(proof(&other))
        .bind(&w.replacement)
        .execute(w.f.wallet.storage().writer())
        .await
        .unwrap();
    // The other transaction's root is one our headers carry.
    settle_moves_nothing(&w, headers_with(&other)).await;
}

/// (iv) The root computed from the replacement's txid is not one our
/// headers carry: no coin moves.
#[tokio::test]
async fn settle_a_root_our_headers_do_not_carry_moves_no_coin() {
    let w = settling().await;
    settle_moves_nothing(&w, headers_with(&"cd".repeat(32))).await;
}

/// (v) The header service cannot look: a fault is no answer, no coin moves.
#[tokio::test]
async fn settle_a_header_fault_moves_no_coin() {
    let w = settling().await;
    settle_moves_nothing(&w, Arc::new(FaultTracker("planted fault"))).await;
}

/// The honest case: the replacement's own bytes, the outpoints they spend,
/// the root from its txid under our headers. The phantom's input moves to
/// the replacement; a coin the replacement does not spend stays. (Case
/// (ii), bytes that hash right but spend another outpoint, cannot be
/// planted here: a txid commits to its inputs, so the settle's bytes that
/// hash right spend exactly the replacement's outpoints, which this case
/// pins.)
#[tokio::test]
async fn settle_the_replacements_own_bytes_and_root_move_its_input() {
    let w = settling().await;
    let s = w.f.wallet.storage();
    let free_before = coin_state(&w.f, w.f.change).await;
    settle_proven(s, w.services.as_ref()).await.unwrap();
    assert_eq!(
        coin_state(&w.f, w.f.coin).await,
        (false, Some(w.replacement_row))
    );
    assert_eq!(coin_state(&w.f, w.f.change).await, free_before);
    assert_eq!(settled(&w).await, 1);
}
