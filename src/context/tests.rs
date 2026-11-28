//! One chain word per wallet (#639): a context built exactly as `--testnet`
//! builds it (`WalletContext::load_with` over a store `init` migrated from
//! the same configuration) answers `testnet` at the served `/getNetwork`,
//! its door judges a received payment under testnet's heights, and the
//! holds of `split` and `gift-claim` read the same word; a mainnet context
//! is unchanged; a store whose chain word is another chain's is refused by
//! name at open.
//!
//! What the concrete `Services` of `load_with` can reach (#637): the header
//! service and the broadcaster are `CHAINTRACKS_URL` and `ARC_URL`, both a
//! closed loopback port here; the CLI's explorer rungs and `GorillaPool` are
//! the same port or off (`BANANABLOCKS_BASE` empty, `ARC_GORILLAPOOL_URL`
//! empty); the toolbox's own `WhatsOnChain` and `Bitails` providers take no
//! base, and every non-loopback request of this process goes to the closed
//! port ([`seal_network`]). Before any request the door's services and
//! tracker (the storage's) are a mock broadcaster and a tracker that
//! answers every root, so nothing is posted anywhere.

use super::*;
use crate::test_support::genesis_coins::payment_over;
use crate::test_support::{response_answer, seal_network, temp_db, TestChainTracker};
use bsv_wallet_cli::config::Flags;
use bsv_wallet_toolbox::services::mock::{
    success_post_beef_result, MockResponse, MockWalletServices,
};
use bsv_wallet_toolbox::{MonitorStorage, WalletStorageProvider, WalletStorageWriter};
use serde_json::{json, Value};
use tower::ServiceExt;

/// A closed loopback port: every service of these wallets.
const CLOSED: &str = "http://127.0.0.1:9";

/// Testnet's Genesis height and mainnet's (bsv-rs's `ChainParams`).
const TEST_GENESIS: u32 = 1_344_302;
const MAIN_GENESIS: u32 = 620_538;
/// A height after mainnet's Genesis and before testnet's: a coin proven
/// here tells the two chains apart.
const BETWEEN: u32 = 1_000_000;
/// The tip the door's tracker answers.
const TIP: u32 = 1_400_000;

/// The configuration of a wallet at `db`, `--testnet` when `testnet`, every
/// service a closed loopback port.
fn configured(db: &str, testnet: bool) -> Arc<Config> {
    seal_network();
    let flags = Flags {
        db: db.to_string(),
        testnet,
        ..Flags::default()
    };
    let vars = [
        ("CHAINTRACKS_URL", CLOSED),
        ("ARC_URL", CLOSED),
        ("WOC_BASE", CLOSED),
        ("BITAILS_BASE", CLOSED),
        ("BANANABLOCKS_BASE", ""),
        ("ARC_GORILLAPOOL_URL", ""),
    ]
    .map(|(k, v)| (k.to_string(), v.to_string()));
    Arc::new(Config::from_vars(&flags, vars).expect("the configuration reads"))
}

/// The store `init` writes at `db` for `root` under `config` (its
/// `storage_options`, its `migrate`).
async fn initialised(db: &str, root: &PrivateKey, config: &Config) {
    let storage = StorageSqlx::open_with_options(db, config.wallet.storage_options())
        .await
        .unwrap();
    storage
        .migrate("bsv-wallet-cli", &root.public_key().to_hex())
        .await
        .unwrap();
    storage.make_available().await.unwrap();
}

/// A wallet `init`ed and loaded on `testnet` or mainnet, as the commands
/// load it; its door's broadcaster the toolbox's mock (taking `takes`) and
/// its tracker one that answers every root at [`TIP`].
async fn loaded(db: &str, testnet: bool, takes: &[&str]) -> WalletContext {
    let config = configured(db, testnet);
    let root = PrivateKey::random();
    initialised(db, &root, &config).await;
    let ctx = WalletContext::load_with(db, &root.to_hex(), testnet, false, &config)
        .await
        .expect("the context loads");
    door_mocked(&ctx, takes).await;
    ctx
}

async fn door_mocked(ctx: &WalletContext, takes: &[&str]) {
    let storage = ctx.wallet.storage();
    let services = MockWalletServices::builder()
        .height(TIP)
        .post_beef_response(MockResponse::Success(vec![success_post_beef_result(
            "arc", takes,
        )]))
        .build();
    WalletStorageProvider::set_services(storage, Arc::new(services));
    storage
        .set_chain_tracker(Arc::new(TestChainTracker::always_valid(TIP)))
        .await;
    storage.set_max_acceptable_proof_height(TIP).await.unwrap();
}

/// The router `serve` builds over `ctx`'s wallet.
fn served(ctx: WalletContext) -> axum::Router {
    bsv_wallet_cli::server::make_router(
        bsv_wallet_cli::server::make_wallet_state(ctx.wallet),
        bsv_wallet_cli::server::ServerConfig::admitting(["test.local"]).expect("an originator"),
    )
}

/// The served body of `method` at `path` (a JSON `body` when given).
async fn answer(app: &axum::Router, method: &str, path: &str, body: Option<&Value>) -> Value {
    let mut request = axum::http::Request::builder()
        .method(method)
        .uri(path)
        .header("Origin", "http://test.local");
    let body = match body {
        Some(json) => {
            request = request.header("Content-Type", "application/json");
            axum::body::Body::from(serde_json::to_vec(json).unwrap())
        }
        None => axum::body::Body::empty(),
    };
    let response = app.clone().oneshot(request.body(body).unwrap()).await;
    response_answer(response.unwrap()).await.2
}

async fn network(app: &axum::Router) -> Value {
    answer(app, "GET", "/getNetwork", None).await
}

async fn internalize(app: &axum::Router, tx: Vec<u8>) -> Value {
    let body = json!({
        "tx": tx,
        "outputs": [{
            "outputIndex": 0,
            "protocol": "basket insertion",
            "insertionRemittance": { "basket": "chain" }
        }],
        "description": "one chain word",
    });
    answer(app, "POST", "/internalizeAction", Some(&body)).await
}

/// Refused as not judged: the coin at `height` is before `genesis`.
fn not_judged(answer: &Value, height: u32, genesis: u32) {
    let message = answer["message"].as_str().unwrap_or_default();
    assert!(
        message.ends_with(&format!(
            "input 0 not judged: coin height {height} is before Genesis height {genesis}."
        )),
        "{answer}"
    );
}

/// `--testnet`: the wallet's chain, the store's word, the holds' height,
/// the served network and the door all say testnet. A coin proven at
/// 1,000,000 (after mainnet's Genesis) is not judged; one proven at
/// testnet's Genesis is taken.
#[tokio::test]
async fn a_testnet_context_is_one_chain_word_served_held_and_judged() {
    let (_dir, db) = temp_db();
    let between = payment_over(0x51, Some(BETWEEN));
    let at = payment_over(0x52, Some(TEST_GENESIS));
    let ctx = loaded(&db, true, &[between.2.as_str(), at.2.as_str()]).await;

    assert_eq!(ctx.chain, Chain::Test, "the context's chain");
    assert_eq!(
        bsv_wallet_cli::genesis::door_network(ctx.wallet.storage()),
        "test",
        "the store's word"
    );
    // The height `split`'s hold and `gift-claim`'s check ask
    // (`src/commands/split.rs`, `src/commands/gift_chain.rs`).
    assert_eq!(
        bsv_wallet_cli::genesis::door_genesis_height(ctx.wallet.storage()),
        Some(TEST_GENESIS),
        "the holds' height"
    );
    assert_eq!(ctx.wallet.chain(), Chain::Test, "the Wallet's own chain");

    let app = served(ctx);
    assert_eq!(network(&app).await, json!({"network": "testnet"}));
    not_judged(&internalize(&app, between.0).await, BETWEEN, TEST_GENESIS);
    let taken = internalize(&app, at.0).await;
    assert_eq!(taken["accepted"], true, "{taken}");
}

/// A mainnet context: every word says mainnet, as before. A coin proven
/// at 1,000,000 is taken; one proven a block below mainnet's Genesis is
/// not judged.
#[tokio::test]
async fn a_mainnet_context_is_mainnet_served_held_and_judged() {
    let (_dir, db) = temp_db();
    let between = payment_over(0x61, Some(BETWEEN));
    let below = payment_over(0x62, Some(MAIN_GENESIS - 1));
    let ctx = loaded(&db, false, &[between.2.as_str(), below.2.as_str()]).await;

    assert_eq!(ctx.chain, Chain::Main);
    assert_eq!(
        bsv_wallet_cli::genesis::door_network(ctx.wallet.storage()),
        "main"
    );
    assert_eq!(
        bsv_wallet_cli::genesis::door_genesis_height(ctx.wallet.storage()),
        Some(MAIN_GENESIS)
    );
    assert_eq!(ctx.wallet.chain(), Chain::Main);

    let app = served(ctx);
    assert_eq!(network(&app).await, json!({"network": "mainnet"}));
    let taken = internalize(&app, between.0).await;
    assert_eq!(taken["accepted"], true, "{taken}");
    not_judged(
        &internalize(&app, below.0).await,
        MAIN_GENESIS - 1,
        MAIN_GENESIS,
    );
}

/// A store `init` wrote on one chain, opened by a context of the other:
/// refused at open, naming both words, both ways.
#[tokio::test]
async fn a_store_of_another_chain_is_refused_by_name_at_open() {
    for (written, opened, word, asked, how) in [
        (false, true, "main", "test", "without --testnet"),
        (true, false, "test", "main", "with --testnet"),
    ] {
        let (_dir, db) = temp_db();
        let root = PrivateKey::random();
        initialised(&db, &root, &configured(&db, written)).await;
        let refused =
            WalletContext::load_with(&db, &root.to_hex(), opened, false, &configured(&db, opened))
                .await
                .err()
                .expect("refused at open");
        assert_eq!(
            refused.to_string(),
            format!(
                "the wallet database {db} was created for chain {word} (its settings' chain) \
                 and this command runs on chain {asked}: run it {how}"
            ),
            "{refused:#}"
        );
    }
}

/// A testnet proof's height in the legacy store: above mainnet's tip, at
/// a height testnet has passed.
const LEGACY_PROOF_HEIGHT: u32 = 1_600_000;

/// A store as `init --testnet` wrote it under every release that pinned
/// the toolbox at 0.14.0-beta.3 or below (every 0.18.x and earlier,
/// 0.19.0-beta.1 and beta.3): that toolbox's `migrate` wrote the settings'
/// `chain` as the literal `mainnet` whatever the chain, so the row says
/// `mainnet` and reads as `main` (the toolbox's read map); its coins are
/// testnet's, here one proof at a testnet height.
async fn legacy_testnet_store(db: &str, root: &PrivateKey) {
    initialised(db, root, &configured(db, true)).await;
    let pool = sqlx::SqlitePool::connect(&format!("sqlite:{db}"))
        .await
        .unwrap();
    sqlx::query("UPDATE settings SET chain = 'mainnet'")
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO proven_txs (txid, height, idx, block_hash, merkle_root, merkle_path, raw_tx) \
         VALUES (?, ?, 0, ?, ?, X'00', X'00')",
    )
    .bind("aa".repeat(32))
    .bind(i64::from(LEGACY_PROOF_HEIGHT))
    .bind("bb".repeat(32))
    .bind("cc".repeat(32))
    .execute(&pool)
    .await
    .unwrap();
    pool.close().await;
}

/// #682, the test that decides: the legacy testnet store opened with
/// `--testnet` is refused, and the refusal tells its owner to run it
/// without the flag.
#[tokio::test]
async fn a_legacy_testnet_store_opened_with_testnet_is_refused_toward_mainnet() {
    let (_dir, db) = temp_db();
    let root = PrivateKey::random();
    legacy_testnet_store(&db, &root).await;
    let refused =
        WalletContext::load_with(&db, &root.to_hex(), true, false, &configured(&db, true))
            .await
            .err()
            .expect("refused at open");
    assert_eq!(
        refused.to_string(),
        format!(
            "the wallet database {db} was created for chain main (its settings' chain) \
             and this command runs on chain test: run it without --testnet"
        ),
    );
}

/// #682, the test that decides: following that advice opens the legacy
/// testnet store as a mainnet wallet, served as `mainnet`.
#[tokio::test]
async fn a_legacy_testnet_store_opened_without_testnet_is_served_as_mainnet() {
    let (_dir, db) = temp_db();
    let root = PrivateKey::random();
    legacy_testnet_store(&db, &root).await;
    let ctx = WalletContext::load_with(&db, &root.to_hex(), false, false, &configured(&db, false))
        .await
        .expect("opened as mainnet");
    assert_eq!(ctx.chain, Chain::Main);
    assert_eq!(
        bsv_wallet_cli::genesis::door_network(ctx.wallet.storage()),
        "main"
    );
    door_mocked(&ctx, &[]).await;
    assert_eq!(network(&served(ctx)).await, json!({"network": "mainnet"}));
}

/// #682, the test that decides: `init`'s store steps (`init.rs`: its
/// `storage_options`, its `migrate`, `make_available`) over an existing
/// store of the other chain run without a word, and the store keeps its
/// own chain word.
#[tokio::test]
async fn init_over_a_store_of_the_other_chain_today() {
    for (written, again, word) in [(false, true, "main"), (true, false, "test")] {
        let (_dir, db) = temp_db();
        let root = PrivateKey::random();
        initialised(&db, &root, &configured(&db, written)).await;
        initialised(&db, &root, &configured(&db, again)).await;
        let storage =
            StorageSqlx::open_with_options(&db, configured(&db, again).wallet.storage_options())
                .await
                .unwrap();
        storage.make_available().await.unwrap();
        use bsv_wallet_toolbox::WalletStorageReader;
        assert_eq!(storage.get_settings().chain, word);
    }
}
