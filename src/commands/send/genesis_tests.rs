//! `send` to the wallet's own address (#38) and a coin mined before Genesis
//! (#634): the self-payment posts and then records its own output through
//! `internalizeAction`, the door that does not judge a spend of such a
//! coin. The command runs through its own code path (`send::run`) over the
//! toolbox's mock services, the post-broadcast verifier off (no explorer is
//! asked). The wallets are `split`'s tests' (`wallet_with_proven`).

use super::*;
use crate::commands::split::genesis_tests::{
    assert_held_alone, assert_named_held, ledger, refusal_of, spender, wallet_with_proven,
};
use crate::context::WalletContext;
use crate::spend_gate::{AllowPending, GenesisHolder};
use bsv_wallet_toolbox::services::mock::MockWalletServices;
use bsv_wallet_toolbox::Chain;

type Ctx = WalletContext<MockWalletServices>;

/// A mainnet wallet of `root` holding `coins` (`(height, satoshis)`, each
/// proven at its height), the post-broadcast verifier off
/// (`BSV_WALLET_SKIP_BROADCAST_VERIFY`), so `send` asks no explorer after
/// the mock's post; and the coins' txids.
pub(crate) async fn wallet(root: &PrivateKey, coins: &[(u32, u64)]) -> (Ctx, Vec<String>) {
    let (mut ctx, txids) = wallet_with_proven(root, coins, Chain::Main).await;
    let mut config = crate::config::Config::default();
    config.verify.skip = true;
    ctx.config = std::sync::Arc::new(config);
    (ctx, txids)
}

/// A wallet of a fresh root holding one coin below Genesis (600,000, the
/// larger) and one above it (700,000): the root, the wallet, the txids.
async fn old_and_new() -> (PrivateKey, Ctx, Vec<String>) {
    let root = PrivateKey::random();
    let (ctx, txids) = wallet(&root, &[(600_000, 50_000), (700_000, 30_000)]).await;
    (root, ctx, txids)
}

/// `send::run` of 1,000 sats from `ctx` to `root`'s own deposit address,
/// `from` its named coins.
async fn self_send(ctx: &Ctx, root: &PrivateKey, from: &[(String, u32)]) -> Result<()> {
    let own = brc29::deposit_address(root, Chain::Main).unwrap();
    run(ctx, &own, 1_000, AllowPending::Off, from).await
}

/// The mock broadcaster's posts.
fn posts(ctx: &Ctx) -> usize {
    ctx.wallet.services().call_count("post_beef")
}

/// The self-send's reason for a coin proven at `height` (mainnet).
fn held_for(height: u32) -> String {
    format!(
        "mined before Genesis: coin height {height} is before Genesis height 620538; a send to this wallet's own address does not offer it"
    )
}

/// The payment of 1,000 sats kept by the door: the spendable default-basket
/// coins of 1,000 of the action that spent `txid`.0.
async fn kept(ctx: &Ctx, txid: &str) -> i64 {
    sqlx::query_scalar(
        "SELECT COUNT(*) FROM outputs o JOIN output_baskets b ON o.basket_id = b.basket_id \
         AND b.name = 'default' WHERE o.transaction_id = \
         (SELECT spent_by FROM outputs WHERE txid = ? AND vout = 0) \
         AND o.spendable = 1 AND o.satoshis = 1000",
    )
    .bind(txid)
    .fetch_one(ctx.wallet.storage().pool())
    .await
    .unwrap()
}

/// THE TEST THAT DECIDES (#634), `send` to the wallet's own address: its
/// only coin is proven at 600,000, below mainnet's Genesis (620,538). The
/// self-send must not be posted and then meet the door's refusal of its
/// record: it is refused before anything is posted, the broadcaster saw
/// nothing, the ledger is unmoved.
#[tokio::test]
async fn a_self_send_of_a_coin_mined_before_genesis_is_not_posted() {
    let root = PrivateKey::random();
    let (ctx, txids) = wallet(&root, &[(600_000, 50_000)]).await;
    let before = ledger(&ctx).await;
    assert_eq!(before.1, 50_000);

    let answer = self_send(&ctx, &root, &[]).await;
    let seen = format!(
        "answer: {:?}; posts: {}; spent: {:?}",
        answer.as_ref().map_err(|e| format!("{e:#}")),
        posts(&ctx),
        spender(&ctx, &txids[0]).await
    );
    assert!(answer.is_err(), "{seen}");
    assert_eq!(posts(&ctx), 0, "{seen}");
    assert_eq!(ledger(&ctx).await, before, "{seen}");
    assert_eq!(spender(&ctx, &txids[0]).await, None, "{seen}");
    assert_held_alone(
        refusal_of(answer.as_ref().unwrap_err()),
        &format!("{}.0", txids[0]),
        GenesisHolder::SelfSend,
        &held_for(600_000),
    );
}

/// One coin below Genesis (600,000, the larger) and one above it
/// (700,000): the self-send spends the coin above alone, is posted once,
/// and its payment is recorded through the door; the old coin is left.
#[tokio::test]
async fn a_self_send_spends_the_coin_above_genesis_and_leaves_the_one_below() {
    let (root, ctx, txids) = old_and_new().await;
    self_send(&ctx, &root, &[])
        .await
        .expect("the self-send spends the coin above Genesis");
    let (old, new) = (
        spender(&ctx, &txids[0]).await,
        spender(&ctx, &txids[1]).await,
    );
    assert_eq!((posts(&ctx), old.is_none(), new.is_some()), (1, true, true));
    assert_eq!(kept(&ctx, &txids[1]).await, 1, "the payment recorded");
}

/// The boundary: a lone coin one block below Genesis (620,537) is held and
/// the self-send refused, nothing posted; a lone coin at Genesis (620,538)
/// is sent and its payment recorded.
#[tokio::test]
async fn a_self_send_holds_one_below_genesis_and_spends_one_at_it() {
    let root = PrivateKey::random();
    let (ctx, txids) = wallet(&root, &[(620_537, 30_000)]).await;
    let err = self_send(&ctx, &root, &[]).await.expect_err("refused");
    let outpoint = format!("{}.0", txids[0]);
    let reason = held_for(620_537);
    assert_held_alone(
        refusal_of(&err),
        &outpoint,
        GenesisHolder::SelfSend,
        &reason,
    );
    assert_eq!(posts(&ctx), 0);
    assert_eq!(spender(&ctx, &txids[0]).await, None);

    let (ctx, txids) = wallet(&root, &[(620_538, 30_000)]).await;
    self_send(&ctx, &root, &[])
        .await
        .expect("the self-send spends the coin at Genesis");
    assert_eq!(posts(&ctx), 1);
    assert_eq!(kept(&ctx, &txids[0]).await, 1);
}

/// A self-send `--from` the coin mined before Genesis: the gate's refusal
/// of a named input (`INPUT_NOT_OFFERED`), the coin under `held` with the
/// self-send's hold, nothing posted.
#[tokio::test]
async fn a_self_send_from_the_coin_mined_before_genesis_is_refused_by_name() {
    let (root, ctx, txids) = old_and_new().await;
    let before = ledger(&ctx).await;
    let err = self_send(&ctx, &root, &[(txids[0].clone(), 0)])
        .await
        .expect_err("refused");
    assert_named_held(refusal_of(&err), GenesisHolder::SelfSend);
    assert_eq!(posts(&ctx), 0);
    assert_eq!(ledger(&ctx).await, before);
}

/// A `send` to another party's address holds nothing for this reason:
/// `send::run` of the wallet's only coin, proven at 600,000, to a
/// stranger's address is posted.
#[tokio::test]
async fn a_send_of_the_coin_mined_before_genesis_to_another_party_is_posted() {
    let root = PrivateKey::random();
    let (ctx, txids) = wallet(&root, &[(600_000, 50_000)]).await;
    // An obviously-synthetic key (0x02 x32), never anyone's wallet.
    let stranger = PrivateKey::from_hex(&"02".repeat(32)).unwrap();
    let to = brc29::deposit_address(&stranger, Chain::Main).unwrap();
    run(&ctx, &to, 1_000, AllowPending::Off, &[])
        .await
        .expect("a send to another party is not held for this reason");
    assert_eq!(posts(&ctx), 1);
    assert!(
        spender(&ctx, &txids[0]).await.is_some(),
        "the old coin spent"
    );
}
