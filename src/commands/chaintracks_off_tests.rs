//! The commands that end in `internalizeAction`, with no chain tracker (a
//! wallet built with `CHAINTRACKS_URL=off`). The toolbox's door holds every
//! `internalizeAction` of such a wallet (#509), so each of `fund`,
//! `receive`, `sync`, `split`, `gift-claim` and a `send` to the wallet's
//! own address refuses by name, a typed
//! [`NoChainTracker`], before it fetches, builds, signs or broadcasts
//! anything: the services were asked nothing (no post, no fetch, no lead)
//! and nothing was stored. The control: the same `fund` with a tracker is
//! stored. (The stable line's 0.18.6, ported.)

use bsv_sdk::primitives::PrivateKey;
use bsv_sdk::transaction::{Beef, MerklePath};
use bsv_wallet_toolbox::services::mock::MockWalletServices;
use bsv_wallet_toolbox::{Chain, StorageSqlx, Wallet, WalletServices, WalletStorageWriter};

use crate::commands::{fund, gift_claim, receive, send, split, sync};
use crate::context::WalletContext;
use crate::services_env::NoChainTracker;
use crate::spend_gate::AllowPending;
use crate::test_support::{raw_tx, txid_of, NoTracker};

async fn storage(root: &PrivateKey) -> StorageSqlx {
    let storage = StorageSqlx::in_memory().await.unwrap();
    storage
        .migrate("chaintracks-off", &root.public_key().to_hex())
        .await
        .unwrap();
    storage.make_available().await.unwrap();
    storage
}

async fn context<S: WalletServices + Send + Sync + 'static>(
    root: &PrivateKey,
    storage: StorageSqlx,
    services: S,
) -> WalletContext<S> {
    let wallet = Wallet::new(Some(root.clone()), storage, services)
        .await
        .unwrap();
    WalletContext {
        wallet,
        identity_key: root.public_key().to_hex(),
        root_key: root.clone(),
        chain: Chain::Main,
        json_output: true,
        db_path: ":memory:".into(),
        config: std::sync::Arc::default(),
    }
}

/// A wallet as `CHAINTRACKS_URL=off` builds it: no tracker on the storage,
/// none on the services.
async fn off(root: &PrivateKey) -> WalletContext<NoTracker> {
    context(root, storage(root).await, NoTracker::new()).await
}

/// A proven payment of 1,000 sats to the wallet's deposit script, as a hex
/// BEEF, and its txid.
fn deposit(root: &PrivateKey) -> (String, String) {
    let script = crate::brc29::deposit_script(root).unwrap();
    let raw = raw_tx(&[(&"11".repeat(32), 0)], &[(1_000, script)]);
    let txid = txid_of(&raw);
    let mut beef = Beef::new();
    let bump = beef.merge_bump(MerklePath::from_coinbase_txid(&txid, 900_000));
    beef.merge_raw_tx(raw, Some(bump)).unwrap();
    (beef.to_hex().unwrap(), txid)
}

async fn rows<S: WalletServices + Send + Sync + 'static>(ctx: &WalletContext<S>) -> (i64, i64) {
    sqlx::query_as("SELECT (SELECT COUNT(*) FROM transactions), (SELECT COUNT(*) FROM outputs)")
        .fetch_one(ctx.wallet.storage().pool())
        .await
        .unwrap()
}

/// The refusal is the typed [`NoChainTracker`] naming `command`, and its
/// words name the setting and what to set.
fn assert_refused<T: std::fmt::Debug>(result: anyhow::Result<T>, command: &str) {
    let err = result.expect_err("refused");
    assert_eq!(
        err.downcast_ref::<NoChainTracker>(),
        Some(&NoChainTracker {
            command: command.to_string()
        }),
        "{err:#}"
    );
    assert_eq!(
        err.to_string(),
        format!(
            "{command} refused: no chain tracker is configured (CHAINTRACKS_URL=off); with no \
             chain tracker no payment is accepted (every internalizeAction is refused). Set \
             CHAINTRACKS_URL to your header service."
        )
    );
}

/// Nothing was asked of the services (no post, no fetch, no lead: every
/// call is counted, `get_chain_tracker` included) and the store holds
/// `rows`.
async fn nothing_asked(ctx: &WalletContext<NoTracker>, rows_held: (i64, i64)) {
    assert_eq!(ctx.wallet.services().total_calls(), 0, "nothing asked");
    assert_eq!(rows(ctx).await, rows_held, "nothing written");
}

#[tokio::test]
async fn fund_with_chaintracks_off_refuses_by_name_and_stores_nothing() {
    let root = PrivateKey::random();
    let ctx = off(&root).await;
    let (beef_hex, _) = deposit(&root);
    assert_refused(fund::run(&ctx, &beef_hex, 0).await, "fund");
    nothing_asked(&ctx, (0, 0)).await;
}

/// The control: the same BEEF with a tracker (the toolbox's mock services
/// carry one) is stored.
#[tokio::test]
async fn fund_with_a_tracker_is_stored() {
    let root = PrivateKey::random();
    let ctx = context(&root, storage(&root).await, MockWalletServices::new()).await;
    let (beef_hex, txid) = deposit(&root);
    fund::run(&ctx, &beef_hex, 0).await.expect("stored");
    let held: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM outputs WHERE txid = ?")
        .bind(&txid)
        .fetch_one(ctx.wallet.storage().pool())
        .await
        .unwrap();
    assert_eq!(held, 1);
}

/// No courier is asked: no `get_beef`, no merkle path.
#[tokio::test]
async fn receive_with_chaintracks_off_refuses_before_any_courier_is_asked() {
    let root = PrivateKey::random();
    let ctx = off(&root).await;
    let (_, txid) = deposit(&root);
    assert_refused(receive::run(&ctx, &txid, Some(0)).await, "receive");
    nothing_asked(&ctx, (0, 0)).await;
}

/// No explorer is asked for leads, with or without `--reconcile-spent`.
#[tokio::test]
async fn sync_with_chaintracks_off_refuses_before_any_explorer_is_asked() {
    let root = PrivateKey::random();
    let ctx = off(&root).await;
    for reconcile_spent in [false, true] {
        assert_refused(sync::run(&ctx, reconcile_spent).await, "sync");
    }
    nothing_asked(&ctx, (0, 0)).await;
}

/// A wallet holding a coin (received while it had a tracker) and then run
/// with `CHAINTRACKS_URL=off`: the services hand the storage no tracker.
async fn funded_then_off(root: &PrivateKey) -> WalletContext<NoTracker> {
    let store = storage(root).await;
    let funded = context(root, store.clone(), MockWalletServices::new()).await;
    let (beef_hex, _) = deposit(root);
    fund::run(&funded, &beef_hex, 0).await.expect("the coin");
    assert_eq!(rows(&funded).await, (1, 1));
    // `Wallet::new` hands the storage these services: no tracker anywhere.
    context(root, store, NoTracker::new()).await
}

/// `split` refuses before a coin is selected, and the broadcaster is never
/// asked. The coin is offered (`--allow-pending received`), so without the
/// refusal the split is built, signed and posted, and only then refused at
/// the door: outputs on chain the wallet never records.
#[tokio::test]
async fn split_with_chaintracks_off_refuses_before_any_coin_is_posted() {
    let root = PrivateKey::random();
    let ctx = funded_then_off(&root).await;
    let split = split::run(&ctx, 2, AllowPending::Received, &[]).await;
    assert_refused(split, "split");
    nothing_asked(&ctx, (1, 1)).await;
}

/// A `send` to the wallet's own address (the self-payment, #38, whose
/// second step is an `internalizeAction`) refuses before a coin is
/// selected, the coin offered as `split`'s is: nothing built, nothing
/// posted, the rows as they were.
#[tokio::test]
async fn a_send_to_the_wallets_own_address_with_chaintracks_off_refuses_before_any_coin_is_posted()
{
    let root = PrivateKey::random();
    let ctx = funded_then_off(&root).await;
    let own = crate::brc29::deposit_address(&root, Chain::Main).unwrap();
    let sent = send::run(&ctx, &own, 500, AllowPending::Received, &[]).await;
    assert_refused(sent, "send");
    nothing_asked(&ctx, (1, 1)).await;
}

/// The deposit is not fetched and no claim is posted, forced or not.
#[tokio::test]
async fn gift_claim_with_chaintracks_off_refuses_before_the_deposit_is_fetched() {
    let root = PrivateKey::random();
    let ctx = off(&root).await;
    for force in [false, true] {
        let claim = gift_claim::run(&ctx, &"22".repeat(32), force).await;
        assert_refused(claim, "gift-claim");
    }
    nothing_asked(&ctx, (0, 0)).await;
}
