//! `split` and a coin mined before Genesis (#634, the port of the stable
//! line's 0.18.9): the wallet's coin is taken in as a proven payment, which
//! the door accepts, and `split` runs through its own code path with the
//! toolbox's mock broadcaster.

use super::*;
use crate::spend_gate::{GateError, GenesisHolder, PRE_GENESIS_WHY};
use crate::test_support::Markers;
use bsv_sdk::primitives::PrivateKey;
use bsv_sdk::transaction::MerklePath;
use bsv_wallet_toolbox::services::mock::MockWalletServices;
use bsv_wallet_toolbox::services::BlockHeader;
use bsv_wallet_toolbox::storage::sqlx::StorageSqlxOptions;
use bsv_wallet_toolbox::{Chain, StorageSqlx, Wallet, WalletStorageWriter};

/// The header service's tip, and the storage's tracker's height, at least
/// (ten above the highest coin when that is higher).
const TIP: u32 = 900_000;

/// A header at `height` whose merkle root is `root` (a one-transaction
/// block), its hash computed when `hashed`.
pub(super) fn header(height: u32, root: &str, hashed: bool) -> BlockHeader {
    let mut h = BlockHeader {
        height,
        merkle_root: root.to_string(),
        previous_hash: "00".repeat(32),
        version: 1,
        bits: 0x1d00_ffff,
        time: 1_700_000_000,
        hash: String::new(),
        nonce: 0,
    };
    if hashed {
        let digest = bsv_sdk::primitives::hash::sha256d(&h.to_binary());
        h.hash = hex::encode(digest.iter().rev().copied().collect::<Vec<u8>>());
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
pub(crate) async fn wallet_with_proven(
    root: &PrivateKey,
    coins: &[(u32, u64)],
    store_chain: Chain,
) -> (WalletContext<MockWalletServices>, Vec<String>) {
    let tip = coins
        .iter()
        .map(|(h, _)| *h + 10)
        .max()
        .unwrap_or(0)
        .max(TIP);
    let services = MockWalletServices::builder().build();
    services.set_tip_header(header(tip, &"0e".repeat(32), true));
    let wallet = Wallet::with_options(
        Some(root.clone()),
        store(root, store_chain, tip).await,
        services,
        bsv_wallet_toolbox::WalletOptions::default(),
    )
    .await
    .unwrap();
    let mut txids = Vec::new();
    for (i, coin) in coins.iter().enumerate() {
        txids.push(take_in(&wallet, root, i, *coin).await);
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

/// A store of `root` migrated on `chain`, its proof gate and its tracker
/// at `tip`.
async fn store(root: &PrivateKey, chain: Chain, tip: u32) -> StorageSqlx {
    let options = StorageSqlxOptions {
        chain,
        ..Default::default()
    };
    let storage = StorageSqlx::open_with_options(":memory:", options)
        .await
        .unwrap();
    let identity = root.public_key().to_hex();
    storage.migrate("split-genesis", &identity).await.unwrap();
    storage.make_available().await.unwrap();
    // The proof gate raised as the monitor raises it, so the payment's
    // checked proof is stored with it (`proven_txs`).
    bsv_wallet_toolbox::MonitorStorage::set_max_acceptable_proof_height(&storage, tip)
        .await
        .unwrap();
    storage
        .set_chain_tracker(std::sync::Arc::new(
            crate::test_support::TestChainTracker::always_valid(tip),
        ))
        .await;
    storage
}

/// The `i`th coin, `(height, satoshis)`: a payment to `root`'s deposit
/// script whose BEEF proves it at `height`, taken in through the toolbox's
/// own `internalizeAction` as `fund` takes one; the header service then
/// carries its block. Its txid.
async fn take_in(
    wallet: &Wallet<StorageSqlx, MockWalletServices>,
    root: &PrivateKey,
    i: usize,
    (height, satoshis): (u32, u64),
) -> String {
    let script = crate::brc29::deposit_script(root).unwrap();
    let anyone = KeyDeriver::anyone_key().1.to_hex();
    let raw = crate::test_support::raw_tx(
        &[(&format!("{:02x}", 0x51 + i).repeat(32), 0)],
        &[(satoshis, script)],
    );
    let txid = crate::test_support::txid_of(&raw);
    let path = MerklePath::from_coinbase_txid(&txid, height);
    let mut beef = Beef::new();
    let index = beef.merge_bump(path);
    beef.merge_raw_tx(raw, Some(index)).unwrap();
    let accepted = wallet
        .internalize_action(
            InternalizeActionArgs {
                tx: beef.to_binary_atomic(&txid).unwrap(),
                outputs: vec![InternalizeOutput {
                    insertion_remittance: None,
                    payment_remittance: Some(WalletPayment {
                        sender_identity_key: anyone,
                        derivation_prefix: DEFAULT_DERIVATION_PREFIX.to_string(),
                        derivation_suffix: DEFAULT_DERIVATION_SUFFIX.to_string(),
                    }),
                    protocol: "wallet payment".to_string(),
                    output_index: 0,
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
        .set_header_for_height(header(height, &txid, false));
    txid
}

/// `split` of two outputs, refused by the gate: its refusal.
async fn refused_split(ctx: &WalletContext<MockWalletServices>) -> crate::spend_gate::Refusal {
    let err = run(ctx, 2, crate::spend_gate::AllowPending::Off, &[])
        .await
        .expect_err("refused");
    refusal_of(&err).clone()
}

/// The gate's refusal `err` carries.
pub(crate) fn refusal_of(err: &anyhow::Error) -> &crate::spend_gate::Refusal {
    match err.downcast_ref::<GateError>() {
        Some(GateError::Refused(refusal)) => refusal,
        _ => panic!("not the gate's refusal: {err}"),
    }
}

/// (actions, unspent satoshis of the default basket).
pub(crate) async fn ledger(ctx: &WalletContext<MockWalletServices>) -> (i64, i64) {
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
    assert_held_alone(
        refusal_of(answer.as_ref().unwrap_err()),
        &format!("{}.0", txids[0]),
        GenesisHolder::Split,
        "mined before Genesis: coin height 600000 is before Genesis height 620538; split does not offer it",
    );
    assert_eq!(spender(&ctx, &txids[0]).await, None);
}

/// `refusal` is the gate's `INSUFFICIENT_FUNDS` with nothing offered and
/// one coin held, `outpoint`, by `holder` for `reason`; its message ends in
/// the holder's sentence and its JSON carries the reason.
pub(crate) fn assert_held_alone(
    refusal: &crate::spend_gate::Refusal,
    outpoint: &str,
    holder: GenesisHolder,
    reason: &str,
) {
    assert_eq!(refusal.code(), "INSUFFICIENT_FUNDS");
    assert_eq!(refusal.offered_satoshis, 0);
    assert_eq!(refusal.held.len(), 1, "{refusal}");
    assert_eq!(refusal.held[0].outpoint, outpoint);
    assert_eq!(refusal.held[0].before_genesis, Some(holder));
    assert_eq!(refusal.held[0].reason, reason);
    let message = refusal.to_string();
    assert!(message.starts_with("Insufficient funds: "), "{message}");
    assert!(
        message.ends_with(&format!("\n{}", holder.why())),
        "{message}"
    );
    assert_eq!(
        refusal.to_json()["held"][0]["reason"],
        refusal.held[0].reason
    );
}

/// The action that spent the coin `txid`.0, if any.
pub(crate) async fn spender(ctx: &WalletContext<MockWalletServices>, txid: &str) -> Option<i64> {
    sqlx::query_scalar("SELECT spent_by FROM outputs WHERE txid = ? AND vout = 0")
        .bind(txid)
        .fetch_one(ctx.wallet.storage().pool())
        .await
        .unwrap()
}

/// The split's outputs of `per` sats recorded through the door: those of
/// the action that spent `txid`.0.
async fn recorded(ctx: &WalletContext<MockWalletServices>, txid: &str, per: i64) -> i64 {
    sqlx::query_scalar(
        "SELECT COUNT(*) FROM outputs WHERE transaction_id = \
         (SELECT spent_by FROM outputs WHERE txid = ? AND vout = 0) \
         AND spendable = 1 AND satoshis = ?",
    )
    .bind(txid)
    .bind(per)
    .fetch_one(ctx.wallet.storage().pool())
    .await
    .unwrap()
}

/// One coin one block below Genesis (620,537, the larger) and one at it
/// (620,538): the split spends the coin at the Genesis height alone, its
/// two outputs are recorded through the door, the old coin is left unspent.
#[tokio::test]
async fn a_split_spends_the_coin_at_genesis_and_leaves_the_one_below() {
    let root = PrivateKey::random();
    let (ctx, txids) =
        wallet_with_proven(&root, &[(620_537, 50_000), (620_538, 30_000)], Chain::Main).await;
    let coins = crate::spend_gate::UserCoins::from_wallet(&ctx.wallet)
        .await
        .unwrap();
    assert_eq!(
        crate::spend_gate::pre_genesis_coins(coins, 620_538)
            .await
            .unwrap()
            .into_iter()
            .map(|held| (held.outpoint, held.satoshis))
            .collect::<Vec<_>>(),
        vec![(format!("{}.0", txids[0]), 50_000)]
    );

    run(&ctx, 2, crate::spend_gate::AllowPending::Off, &[])
        .await
        .expect("the split spends the coin at Genesis");
    assert_eq!(ctx.wallet.services().call_count("post_beef"), 1);
    assert_eq!(spender(&ctx, &txids[0]).await, None, "the old coin left");
    assert!(
        spender(&ctx, &txids[1]).await.is_some(),
        "the coin at Genesis spent"
    );
    assert_eq!(
        recorded(&ctx, &txids[1], 14_750).await,
        2,
        "the split's two outputs recorded through the door"
    );
}

/// `split --from` naming the coin mined before Genesis: the gate's refusal
/// of a named input (`INPUT_NOT_OFFERED`), the coin under `held` with its
/// reason, nothing posted.
#[tokio::test]
async fn a_split_from_the_coin_mined_before_genesis_is_refused_by_name() {
    let root = PrivateKey::random();
    let (ctx, txids) =
        wallet_with_proven(&root, &[(600_000, 50_000), (620_538, 30_000)], Chain::Main).await;
    let before = ledger(&ctx).await;
    let err = run(
        &ctx,
        2,
        crate::spend_gate::AllowPending::Off,
        &[(txids[0].clone(), 0)],
    )
    .await
    .expect_err("refused");
    assert_named_held(refusal_of(&err), GenesisHolder::Split);
    assert_eq!(ctx.wallet.services().call_count("post_beef"), 0);
    assert_eq!(ledger(&ctx).await, before);
}

/// `refusal` is the gate's refusal of a named input (`INPUT_NOT_OFFERED`),
/// one coin under `held` by `holder`, its message ending in the holder's
/// sentence.
pub(crate) fn assert_named_held(refusal: &crate::spend_gate::Refusal, holder: GenesisHolder) {
    assert_eq!(refusal.code(), "INPUT_NOT_OFFERED", "{refusal}");
    assert_eq!(refusal.held.len(), 1, "{refusal}");
    assert_eq!(refusal.held[0].before_genesis, Some(holder));
    assert!(refusal.to_string().ends_with(holder.why()));
}

/// A send of 1,000 sats to a stranger, signed; posted unless `no_send`.
fn send_args(no_send: bool) -> CreateActionArgs {
    CreateActionArgs {
        description: "a send".to_string(),
        input_beef: None,
        inputs: None,
        outputs: Some(vec![CreateActionOutput {
            tags: None,
            basket: None,
            custom_instructions: None,
            output_description: "to someone".to_string(),
            satoshis: 1_000,
            locking_script: crate::test_support::p2pkh([9; 20]),
        }]),
        labels: None,
        lock_time: None,
        version: None,
        options: Some(CreateActionOptions {
            sign_and_process: Some(true),
            no_send: Some(no_send),
            ..Default::default()
        }),
    }
}

/// The hold is `split`'s alone: the gate every other command uses (a
/// `send`'s, signed, not sent) offers the coin mined before Genesis and
/// the spend signs.
#[tokio::test]
async fn a_send_of_the_coin_mined_before_genesis_is_not_held() {
    let root = PrivateKey::random();
    let (ctx, txids) = wallet_with_proven(&root, &[(600_000, 50_000)], Chain::Main).await;
    let made =
        crate::spend_gate::create_action(&ctx.wallet, send_args(true), "split.local", &ctx.gate())
            .await
            .expect("a send is not held for this reason");
    assert!(made.tx.is_some(), "signed");
    assert!(spender(&ctx, &txids[0]).await.is_some());
    assert_eq!(ctx.wallet.services().call_count("post_beef"), 0, "not sent");
}

/// The change of a posted `send` of the coin mined before Genesis, not
/// mined yet: a split of it would carry the send unproven, spending a coin
/// the send's parent's proof record places at 600,000, so every change coin
/// of the send is held with that reason and the split is refused with
/// nothing more posted.
#[tokio::test]
async fn a_split_of_the_unmined_change_of_a_send_of_such_a_coin_is_refused_by_name() {
    let root = PrivateKey::random();
    let (ctx, _) = wallet_with_proven(&root, &[(600_000, 50_000)], Chain::Main).await;
    let sent =
        crate::spend_gate::create_action(&ctx.wallet, send_args(false), "split.local", &ctx.gate())
            .await
            .expect("the send is posted");
    let send_txid = to_hex(&sent.txid.expect("signed"));
    assert_eq!(ctx.wallet.services().call_count("post_beef"), 1);
    let before = ledger(&ctx).await;

    let refusal = refused_split(&ctx).await;
    assert_eq!(refusal.code(), "INSUFFICIENT_FUNDS");
    let change: Vec<_> = refusal
        .held
        .iter()
        .filter(|c| c.outpoint.starts_with(&send_txid))
        .collect();
    assert!(!change.is_empty(), "{refusal}");
    for coin in &change {
        assert_eq!(coin.before_genesis, Some(GenesisHolder::Split));
        assert_eq!(
            coin.reason,
            format!(
                "mined before Genesis: {send_txid} is not mined yet and spends a coin of height 600000, before Genesis height 620538; split does not offer it"
            )
        );
    }
    assert!(refusal.to_string().ends_with(PRE_GENESIS_WHY));
    assert_eq!(
        ctx.wallet.services().call_count("post_beef"),
        1,
        "nothing more posted"
    );
    assert_eq!(ledger(&ctx).await, before);
}

/// A context built as `--testnet` builds it: its store migrated on the
/// testnet chain (the word `test`, which the door judges by), the
/// `Wallet`'s own chain mainnet. A coin proven at 700,000 (after mainnet's
/// Genesis, before testnet's 1,344,302) is held and the split refused,
/// nothing posted; a coin proven at 1,344,302 is offered, and the split is
/// posted and recorded through the door. Host and door agree; asked the
/// `Wallet`'s chain the hold would offer the coin at 700,000 and the door
/// would refuse its record.
#[tokio::test]
async fn a_testnet_context_holds_by_the_chain_its_door_judges_by() {
    let root = PrivateKey::random();
    let (ctx, txids) = wallet_with_proven(&root, &[(700_000, 30_000)], Chain::Test).await;
    assert_eq!(ctx.chain, Chain::Test);
    assert_eq!(ctx.wallet.chain(), Chain::Main, "the Wallet's own chain");
    assert_eq!(
        bsv_wallet_cli::genesis::door_network(ctx.wallet.storage()),
        "test",
        "the door's chain"
    );
    let refusal = refused_split(&ctx).await;
    assert_eq!(
        refusal.held[0].reason,
        "mined before Genesis: coin height 700000 is before Genesis height 1344302; split does not offer it"
    );
    assert_eq!(ctx.wallet.services().call_count("post_beef"), 0);
    assert_eq!(spender(&ctx, &txids[0]).await, None);

    let (ctx, txids) = wallet_with_proven(&root, &[(1_344_302, 30_000)], Chain::Test).await;
    run(&ctx, 2, crate::spend_gate::AllowPending::Off, &[])
        .await
        .expect("the split spends the coin at testnet's Genesis");
    assert_eq!(ctx.wallet.services().call_count("post_beef"), 1);
    assert_eq!(
        recorded(&ctx, &txids[0], 14_750).await,
        2,
        "the split's two outputs recorded through the door"
    );
}
