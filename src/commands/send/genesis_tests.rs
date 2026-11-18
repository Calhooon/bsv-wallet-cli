//! `send` to the wallet's own address (#38) and a coin mined before Genesis
//! (#634): the self-payment posts and then records its own output through
//! `internalizeAction`, the door that does not judge a spend of such a
//! coin. The command runs through its own code path (`send::run`) over the
//! toolbox's mock services, the post-broadcast verifier off (no explorer is
//! asked).

use super::*;
use crate::commands::split::genesis_tests::{ledger, wallet_with_proven};
use crate::context::WalletContext;
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
}
