//! `gift-claim` and a deposit mined before Genesis (#634, the port of the
//! stable line's 0.18.9): the command runs through its own code path
//! (`gift_claim::run`) over the toolbox's mock services, whose broadcaster
//! takes every post and whose header service proves the deposit at the
//! height asked.

use super::tests::{spendable, storage, Claim};
use super::*;
use crate::context::WalletContext;
use crate::test_support::txid_of;
use bsv_wallet_toolbox::services::mock::{MockResponse, MockWalletServices};
use bsv_wallet_toolbox::services::{GetMerklePathResult, GetRawTxResult};
use bsv_wallet_toolbox::Chain;

/// Services that hand out `deposit`'s bytes and prove it at `height`.
pub(super) fn proving(deposit: &[u8], height: u32) -> MockWalletServices {
    MockWalletServices::builder()
        .get_raw_tx_response(MockResponse::Success(GetRawTxResult {
            name: "Services".to_string(),
            txid: txid_of(deposit),
            raw_tx: Some(deposit.to_vec()),
            error: None,
            could_not_look: false,
        }))
        .get_merkle_path_response(MockResponse::Success(GetMerklePathResult {
            name: Some("Services".to_string()),
            merkle_path: Some(
                MerklePath::from_coinbase_txid(&txid_of(deposit), height)
                    .to_hex()
                    .unwrap(),
            ),
            header: None,
            error: None,
            notes: vec![],
        }))
        .build()
}

/// A context over `storage` and `services` as `WalletContext::load_with`
/// builds one (`Wallet::with_options`), `chain` its `--testnet` reading.
pub(super) async fn context(
    root: &bsv_sdk::primitives::PrivateKey,
    storage: StorageSqlx,
    services: MockWalletServices,
    chain: Chain,
) -> WalletContext<MockWalletServices> {
    let wallet = bsv_wallet_toolbox::Wallet::with_options(
        Some(root.clone()),
        storage,
        services,
        bsv_wallet_toolbox::WalletOptions::default(),
    )
    .await
    .unwrap();
    WalletContext {
        wallet,
        identity_key: root.public_key().to_hex(),
        root_key: root.clone(),
        chain,
        json_output: true,
        db_path: ":memory:".into(),
        config: std::sync::Arc::default(),
    }
}

/// THE TEST THAT DECIDES (#634), gift-claim: the gift's deposit is proven
/// at 620,537, one block below mainnet's Genesis. The claim must not be
/// posted and then meet the door's refusal of its record: it is refused
/// before the post, the broadcaster saw nothing, the tracker holds no row
/// and nothing is in the wallet.
#[tokio::test]
async fn a_claim_of_a_deposit_mined_before_genesis_is_not_posted() {
    let gift = Claim::new();
    let claim_txid = txid_of(&gift.claim);
    let ctx = context(
        &gift.root,
        storage().await,
        proving(&gift.deposit, 620_537),
        Chain::Main,
    )
    .await;

    let answer = crate::commands::gift_claim::run(&ctx, &txid_of(&gift.deposit), false).await;
    let posts = ctx.wallet.services().call_count("post_beef");
    let word = tracker_host::stored_word(ctx.wallet.storage(), &claim_txid)
        .await
        .unwrap();
    let held = spendable(ctx.wallet.storage()).await;
    let seen = format!("answer: {answer:?}; posts: {posts}; word: {word:?}; spendable: {held}");
    assert!(answer.is_err(), "{seen}");
    assert_eq!(posts, 0, "{seen}");
    assert_eq!(word, None, "{seen}");
    assert_eq!(held, 0, "{seen}");
}
