//! `send` to the wallet's own address (#38) and a coin mined before Genesis
//! (#634): the self-payment posts and then records its own output through
//! `internalizeAction`, the door that does not judge a spend of such a
//! coin. The command runs through its own code path (`send::run`) over the
//! toolbox's mock services, the post-broadcast verifier off (no explorer is
//! asked).

use super::*;
use crate::commands::split::genesis_tests::{ledger, wallet_with_proven};
use crate::context::WalletContext;
use crate::spend_gate::{GateError, GenesisHolder, Refusal, SELF_SEND_PRE_GENESIS_WHY};
use bsv_wallet_toolbox::services::mock::MockWalletServices;
use bsv_wallet_toolbox::Chain;

/// `ctx` with the post-broadcast verifier off (`BSV_WALLET_SKIP_BROADCAST_VERIFY`),
/// so `send` asks no explorer after the mock's post.
fn quiet(mut ctx: WalletContext<MockWalletServices>) -> WalletContext<MockWalletServices> {
    let mut config = crate::config::Config::default();
    config.verify.skip = true;
    ctx.config = std::sync::Arc::new(config);
    ctx
}

/// The coin `txid`.0's spender, if any.
async fn spender(ctx: &WalletContext<MockWalletServices>, txid: &str) -> Option<i64> {
    sqlx::query_scalar("SELECT spent_by FROM outputs WHERE txid = ? AND vout = 0")
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
    let (ctx, txids) = wallet_with_proven(&root, &[(600_000, 50_000)], Chain::Main).await;
    let ctx = quiet(ctx);
    let own = brc29::deposit_address(&root, Chain::Main).unwrap();
    let before = ledger(&ctx).await;
    assert_eq!(before.1, 50_000);

    let answer = run(&ctx, &own, 1_000, crate::spend_gate::AllowPending::Off, &[]).await;
    let posts = ctx.wallet.services().call_count("post_beef");
    let seen = format!(
        "answer: {:?}; posts: {posts}; spent: {:?}",
        answer.as_ref().map_err(|e| format!("{e:#}")),
        spender(&ctx, &txids[0]).await
    );
    assert!(answer.is_err(), "{seen}");
    assert_eq!(posts, 0, "{seen}");
    assert_eq!(ledger(&ctx).await, before, "{seen}");
    assert_eq!(spender(&ctx, &txids[0]).await, None, "{seen}");
    let refusal = refusal_of(answer.as_ref().unwrap_err());
    assert_eq!(refusal.code(), "INSUFFICIENT_FUNDS");
    assert_eq!(refusal.offered_satoshis, 0);
    assert_eq!(refusal.held.len(), 1, "{refusal}");
    assert_eq!(refusal.held[0].outpoint, format!("{}.0", txids[0]));
    assert_eq!(
        refusal.held[0].before_genesis,
        Some(GenesisHolder::SelfSend)
    );
    assert_eq!(
        refusal.held[0].reason,
        "mined before Genesis: coin height 600000 is before Genesis height 620538; a send to this wallet's own address does not offer it"
    );
    let message = refusal.to_string();
    assert!(message.starts_with("Insufficient funds: "), "{message}");
    assert!(
        message.ends_with(&format!("\n{SELF_SEND_PRE_GENESIS_WHY}")),
        "{message}"
    );
    assert_eq!(
        refusal.to_json()["held"][0]["reason"],
        refusal.held[0].reason
    );
}

/// The gate's refusal `err` carries.
fn refusal_of(err: &anyhow::Error) -> &Refusal {
    match err.downcast_ref::<GateError>() {
        Some(GateError::Refused(refusal)) => refusal,
        _ => panic!("not the gate's refusal: {err:#}"),
    }
}

/// The payment of `satoshis` kept by the door: a spendable default-basket
/// coin of the action that spent `txid`.0.
async fn kept(ctx: &WalletContext<MockWalletServices>, txid: &str, satoshis: i64) -> i64 {
    sqlx::query_scalar(
        "SELECT COUNT(*) FROM outputs o JOIN output_baskets b ON o.basket_id = b.basket_id \
         AND b.name = 'default' WHERE o.transaction_id = \
         (SELECT spent_by FROM outputs WHERE txid = ? AND vout = 0) \
         AND o.spendable = 1 AND o.satoshis = ?",
    )
    .bind(txid)
    .bind(satoshis)
    .fetch_one(ctx.wallet.storage().pool())
    .await
    .unwrap()
}

/// One coin below Genesis (600,000, the larger) and one above it
/// (700,000): the self-send spends the coin above alone, is posted once,
/// and its payment is recorded through the door; the old coin is left.
#[tokio::test]
async fn a_self_send_spends_the_coin_above_genesis_and_leaves_the_one_below() {
    let root = PrivateKey::random();
    let (ctx, txids) =
        wallet_with_proven(&root, &[(600_000, 50_000), (700_000, 30_000)], Chain::Main).await;
    let ctx = quiet(ctx);
    let own = brc29::deposit_address(&root, Chain::Main).unwrap();
    run(&ctx, &own, 1_000, crate::spend_gate::AllowPending::Off, &[])
        .await
        .expect("the self-send spends the coin above Genesis");
    assert_eq!(ctx.wallet.services().call_count("post_beef"), 1);
    assert_eq!(spender(&ctx, &txids[0]).await, None, "the old coin left");
    assert!(
        spender(&ctx, &txids[1]).await.is_some(),
        "the coin above spent"
    );
    assert_eq!(
        kept(&ctx, &txids[1], 1_000).await,
        1,
        "the payment recorded through the door"
    );
}

/// The boundary: a lone coin one block below Genesis (620,537) is held and
/// the self-send refused, nothing posted; a lone coin at Genesis (620,538)
/// is sent and its payment recorded.
#[tokio::test]
async fn a_self_send_holds_one_below_genesis_and_spends_one_at_it() {
    let root = PrivateKey::random();
    let own = brc29::deposit_address(&root, Chain::Main).unwrap();

    let (ctx, txids) = wallet_with_proven(&root, &[(620_537, 30_000)], Chain::Main).await;
    let ctx = quiet(ctx);
    let err = run(&ctx, &own, 1_000, crate::spend_gate::AllowPending::Off, &[])
        .await
        .expect_err("refused");
    let refusal = refusal_of(&err);
    assert_eq!(refusal.code(), "INSUFFICIENT_FUNDS", "{refusal}");
    assert_eq!(
        refusal.held[0].reason,
        "mined before Genesis: coin height 620537 is before Genesis height 620538; a send to this wallet's own address does not offer it"
    );
    assert_eq!(ctx.wallet.services().call_count("post_beef"), 0);
    assert_eq!(spender(&ctx, &txids[0]).await, None);

    let (ctx, txids) = wallet_with_proven(&root, &[(620_538, 30_000)], Chain::Main).await;
    let ctx = quiet(ctx);
    run(&ctx, &own, 1_000, crate::spend_gate::AllowPending::Off, &[])
        .await
        .expect("the self-send spends the coin at Genesis");
    assert_eq!(ctx.wallet.services().call_count("post_beef"), 1);
    assert_eq!(kept(&ctx, &txids[0], 1_000).await, 1);
}

/// A self-send `--from` the coin mined before Genesis: the gate's refusal
/// of a named input (`INPUT_NOT_OFFERED`), the coin under `held` with the
/// self-send's reason, nothing posted.
#[tokio::test]
async fn a_self_send_from_the_coin_mined_before_genesis_is_refused_by_name() {
    let root = PrivateKey::random();
    let (ctx, txids) =
        wallet_with_proven(&root, &[(600_000, 50_000), (700_000, 30_000)], Chain::Main).await;
    let ctx = quiet(ctx);
    let own = brc29::deposit_address(&root, Chain::Main).unwrap();
    let before = ledger(&ctx).await;
    let err = run(
        &ctx,
        &own,
        1_000,
        crate::spend_gate::AllowPending::Off,
        &[(txids[0].clone(), 0)],
    )
    .await
    .expect_err("refused");
    let refusal = refusal_of(&err);
    assert_eq!(refusal.code(), "INPUT_NOT_OFFERED", "{refusal}");
    assert_eq!(refusal.held.len(), 1, "{refusal}");
    assert_eq!(
        refusal.held[0].before_genesis,
        Some(GenesisHolder::SelfSend)
    );
    assert!(refusal.to_string().ends_with(SELF_SEND_PRE_GENESIS_WHY));
    assert_eq!(ctx.wallet.services().call_count("post_beef"), 0);
    assert_eq!(ledger(&ctx).await, before);
}

/// A `send` to another party's address holds nothing for this reason:
/// `send::run` of the wallet's only coin, proven at 600,000, to a
/// stranger's address is posted.
#[tokio::test]
async fn a_send_of_the_coin_mined_before_genesis_to_another_party_is_posted() {
    let root = PrivateKey::random();
    let (ctx, txids) = wallet_with_proven(&root, &[(600_000, 50_000)], Chain::Main).await;
    let ctx = quiet(ctx);
    // An obviously-synthetic key (0x02 x32), never anyone's wallet.
    let stranger = PrivateKey::from_hex(&"02".repeat(32)).unwrap();
    let to = brc29::deposit_address(&stranger, Chain::Main).unwrap();
    run(&ctx, &to, 1_000, crate::spend_gate::AllowPending::Off, &[])
        .await
        .expect("a send to another party is not held for this reason");
    assert_eq!(ctx.wallet.services().call_count("post_beef"), 1);
    assert!(
        spender(&ctx, &txids[0]).await.is_some(),
        "the old coin spent"
    );
}
