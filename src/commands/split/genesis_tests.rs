//! `split` and a coin mined before Genesis (#634, the port of the stable
//! line's 0.18.9): the wallet's coin is taken in as a proven payment, which
//! the door accepts, and `split` runs through its own code path with the
//! toolbox's mock broadcaster.

use super::*;
use crate::test_support::Markers;
use bsv_sdk::primitives::PrivateKey;
use bsv_sdk::transaction::MerklePath;
use bsv_wallet_toolbox::services::mock::MockWalletServices;
use bsv_wallet_toolbox::services::BlockHeader;
use bsv_wallet_toolbox::storage::sqlx::StorageSqlxOptions;
use bsv_wallet_toolbox::{Chain, StorageSqlx, Wallet, WalletStorageWriter};

/// The header service's tip, and the storage's tracker's height.
const TIP: u32 = 900_000;

/// A header at `height` whose merkle root is `root` (a one-transaction
/// block), its hash computed when `hashed`.
pub(super) fn header(height: u32, root: &str, hashed: bool) -> BlockHeader {
    let mut h = BlockHeader {
        version: 1,
        previous_hash: "00".repeat(32),
        merkle_root: root.to_string(),
        time: 1_700_000_000,
        bits: 0x1d00_ffff,
        nonce: 0,
        hash: String::new(),
        height,
    };
    if hashed {
        let mut hash = bsv_sdk::primitives::hash::sha256d(&h.to_binary()).to_vec();
        hash.reverse();
        h.hash = hex::encode(hash);
    }
    h
}

/// A wallet of `root` whose store is migrated on `store_chain` (as `init`
/// migrates it from `--testnet`), its context's `chain` the same, the wallet
/// built as `WalletContext::load_with` builds it (`Wallet::with_options`,
/// the toolbox's default chain), holding one coin per `(height, satoshis)`:
/// a payment to its deposit script whose BEEF proves it at `height`, taken
/// in through the toolbox's own `internalizeAction` as `fund` takes one.
/// The mock's header service carries each block and the tip; the storage's
/// tracker answers every root. The coins' txids.
pub(super) async fn wallet_with_proven(
    root: &PrivateKey,
    coins: &[(u32, u64)],
    store_chain: Chain,
) -> (WalletContext<MockWalletServices>, Vec<String>) {
    let storage = StorageSqlx::open_with_options(
        ":memory:",
        StorageSqlxOptions {
            chain: store_chain,
            ..Default::default()
        },
    )
    .await
    .unwrap();
    storage
        .migrate("split", &root.public_key().to_hex())
        .await
        .unwrap();
    storage.make_available().await.unwrap();
    // The proof gate raised as the monitor raises it, so the payment's
    // checked proof is stored with it (`proven_txs`).
    bsv_wallet_toolbox::MonitorStorage::set_max_acceptable_proof_height(&storage, TIP)
        .await
        .unwrap();
    storage
        .set_chain_tracker(std::sync::Arc::new(
            crate::test_support::TestChainTracker::always_valid(TIP),
        ))
        .await;
    let services = MockWalletServices::builder().build();
    services.set_tip_header(header(TIP, &"0e".repeat(32), true));
    let wallet = Wallet::with_options(
        Some(root.clone()),
        storage,
        services,
        bsv_wallet_toolbox::WalletOptions::default(),
    )
    .await
    .unwrap();
    let script = crate::brc29::deposit_script(root).unwrap();
    let anyone = KeyDeriver::anyone_key().1.to_hex();
    let mut txids = Vec::new();
    for (i, (height, satoshis)) in coins.iter().enumerate() {
        let raw = crate::test_support::raw_tx(
            &[(&format!("{:02x}", 0x51 + i).repeat(32), 0)],
            &[(*satoshis, script.clone())],
        );
        let txid = crate::test_support::txid_of(&raw);
        let mut beef = Beef::new();
        let bump = beef.merge_bump(MerklePath::from_coinbase_txid(&txid, *height));
        beef.merge_raw_tx(raw, Some(bump)).unwrap();
        let accepted = wallet
            .internalize_action(
                InternalizeActionArgs {
                    tx: beef.to_binary_atomic(&txid).unwrap(),
                    outputs: vec![InternalizeOutput {
                        output_index: 0,
                        protocol: "wallet payment".to_string(),
                        payment_remittance: Some(WalletPayment {
                            derivation_prefix: DEFAULT_DERIVATION_PREFIX.to_string(),
                            derivation_suffix: DEFAULT_DERIVATION_SUFFIX.to_string(),
                            sender_identity_key: anyone.clone(),
                        }),
                        insertion_remittance: None,
                    }],
                    description: "a mined payment".to_string(),
                    labels: None,
                    seek_permission: None,
                },
                Some("split.local"),
            )
            .await
            .expect("the door takes a proven payment");
        assert!(accepted.accepted);
        wallet
            .services()
            .set_header_for_height(header(*height, &txid, false));
        txids.push(txid);
    }
    let ctx = WalletContext {
        wallet,
        identity_key: root.public_key().to_hex(),
        root_key: root.clone(),
        chain: store_chain,
        json_output: false,
        db_path: ":memory:".into(),
        config: std::sync::Arc::default(),
    };
    (ctx, txids)
}

/// (actions, unspent satoshis of the default basket).
pub(super) async fn ledger(ctx: &WalletContext<MockWalletServices>) -> (i64, i64) {
    sqlx::query_as(
        "SELECT (SELECT COUNT(*) FROM transactions), \
                (SELECT COALESCE(SUM(o.satoshis), 0) FROM outputs o \
                 JOIN output_baskets b ON o.basket_id = b.basket_id AND b.name = 'default' \
                 WHERE o.spendable = 1 AND o.spent_by IS NULL)",
    )
    .fetch_one(ctx.wallet.storage().pool())
    .await
    .unwrap()
}

/// THE TEST THAT DECIDES (#634), split: the wallet's only coin is proven
/// at 600,000, below mainnet's Genesis (620,538). The split must not be
/// posted and then meet the door's refusal of its record: it is refused
/// before anything is posted, the broadcaster saw nothing, the ledger is
/// unmoved.
#[tokio::test]
async fn a_split_of_a_coin_mined_before_genesis_is_not_posted() {
    let (markers, _guard) = Markers::capture();
    let root = PrivateKey::random();
    let (ctx, txids) = wallet_with_proven(&root, &[(600_000, 50_000)], Chain::Main).await;
    let proven: Option<i64> = sqlx::query_scalar("SELECT height FROM proven_txs WHERE txid = ?")
        .bind(&txids[0])
        .fetch_optional(ctx.wallet.storage().pool())
        .await
        .unwrap();
    assert_eq!(proven, Some(600_000), "the coin's proof record");
    let before = ledger(&ctx).await;
    assert_eq!(before.1, 50_000);

    let answer = run(&ctx, 2, crate::spend_gate::AllowPending::Off, &[]).await;
    let posts = ctx.wallet.services().call_count("post_beef");
    let refused = markers.named("post_broadcast_split_internalize_failed");
    let seen = format!(
        "answer: {answer:?}; posts: {posts}; the door's refusals of the record: {:?}",
        refused.iter().map(|m| m.error.clone()).collect::<Vec<_>>()
    );
    assert!(answer.is_err(), "{seen}");
    assert_eq!(posts, 0, "{seen}");
    assert_eq!(ledger(&ctx).await, before, "{seen}");
}
