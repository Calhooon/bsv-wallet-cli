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

/// Services that hand out `deposit`'s bytes and prove it at `height`; the
/// mock's tip (the door's header source) above it.
pub(super) fn proving(deposit: &[u8], height: u32) -> MockWalletServices {
    MockWalletServices::builder()
        .height(height.max(880_000) + 10)
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
    assert_eq!(
        answer.unwrap_err().to_string(),
        format!(
            "gift-claim refused, nothing posted: the claim spends the gift's deposit {}, which its proof places at height 620537, before Genesis height 620538, and internalizeAction refuses a payment spending a coin mined before Genesis as not judged, so the wallet could not record the claim.",
            txid_of(&gift.deposit)
        )
    );
}

/// The store a context was built on, on `chain` (as `init` migrates it
/// from `--testnet`).
async fn store_on(chain: Chain) -> StorageSqlx {
    use bsv_wallet_toolbox::WalletStorageWriter;
    let storage = StorageSqlx::open_with_options(
        ":memory:",
        bsv_wallet_toolbox::storage::sqlx::StorageSqlxOptions {
            chain,
            ..Default::default()
        },
    )
    .await
    .unwrap();
    storage
        .migrate("gift-tests", &("02".to_string() + &"ab".repeat(32)))
        .await
        .unwrap();
    storage.make_available().await.unwrap();
    storage
}

/// The pure check: a claim spending a deposit its BEEF proves one block
/// below Genesis is named; at the Genesis height, and with the deposit
/// carried unproven, nothing is named.
#[test]
fn the_claims_beef_is_asked_the_genesis_rule_alone() {
    let gift = Claim::new();
    let deposit_txid = txid_of(&gift.deposit);
    let with = |height: Option<u32>| {
        let mut beef = Beef::new();
        let bump =
            height.map(|h| beef.merge_bump(MerklePath::from_coinbase_txid(&deposit_txid, h)));
        beef.merge_raw_tx(gift.deposit.clone(), bump).unwrap();
        beef.merge_raw_tx(gift.claim.clone(), None).unwrap();
        beef.to_binary().unwrap()
    };
    assert_eq!(
        claim_not_recordable(&with(Some(620_537)), 620_538).as_deref(),
        Some(format!(
            "gift-claim refused, nothing posted: the claim spends the gift's deposit {deposit_txid}, which its proof places at height 620537, before Genesis height 620538, and internalizeAction refuses a payment spending a coin mined before Genesis as not judged, so the wallet could not record the claim."
        ).as_str())
    );
    assert_eq!(claim_not_recordable(&with(Some(620_538)), 620_538), None);
    assert_eq!(claim_not_recordable(&with(None), 620_538), None);
}

/// The same gift with its deposit proven at the Genesis height (620,538):
/// posted once, and the wallet records its claim through the door.
#[tokio::test]
async fn a_claim_of_a_deposit_mined_at_genesis_is_posted_and_recorded() {
    let gift = Claim::new();
    let ctx = context(
        &gift.root,
        storage().await,
        proving(&gift.deposit, 620_538),
        Chain::Main,
    )
    .await;
    crate::commands::gift_claim::run(&ctx, &txid_of(&gift.deposit), false)
        .await
        .expect("posted");
    // The command's post, then the door's own share of the claim it
    // records (the toolbox's `internalizeAction` broadcasts inside its
    // storage step).
    assert_eq!(ctx.wallet.services().call_count("post_beef"), 2);
    assert_eq!(
        spendable(ctx.wallet.storage()).await as u64,
        gift.plan.covenant.amount + gift.plan.change,
        "the claim recorded through the door"
    );
}

/// A context built as `--testnet` builds it: its store on the testnet
/// chain, which the door judges by, the `Wallet`'s own chain mainnet. A
/// deposit proven at 700,000 is refused by name before the post (testnet's
/// Genesis, 1,344,302); one proven at 1,344,302 is posted and recorded.
/// Host and door agree.
#[tokio::test]
async fn a_testnet_contexts_claim_is_asked_the_chain_its_door_judges_by() {
    let gift = Claim::new();
    let ctx = context(
        &gift.root,
        store_on(Chain::Test).await,
        proving(&gift.deposit, 700_000),
        Chain::Test,
    )
    .await;
    assert_eq!(ctx.wallet.chain(), Chain::Main, "the Wallet's own chain");
    let err = crate::commands::gift_claim::run(&ctx, &txid_of(&gift.deposit), false)
        .await
        .expect_err("refused before the post");
    assert!(
        err.to_string()
            .contains("at height 700000, before Genesis height 1344302"),
        "{err}"
    );
    assert_eq!(ctx.wallet.services().call_count("post_beef"), 0);

    let ctx = context(
        &gift.root,
        store_on(Chain::Test).await,
        proving(&gift.deposit, 1_344_302),
        Chain::Test,
    )
    .await;
    crate::commands::gift_claim::run(&ctx, &txid_of(&gift.deposit), false)
        .await
        .expect("posted");
    // The command's post, then the door's own share of the claim it
    // records (the toolbox's `internalizeAction` broadcasts inside its
    // storage step).
    assert_eq!(ctx.wallet.services().call_count("post_beef"), 2);
    assert_eq!(
        spendable(ctx.wallet.storage()).await as u64,
        gift.plan.covenant.amount + gift.plan.change,
        "the claim recorded through the door"
    );
}
