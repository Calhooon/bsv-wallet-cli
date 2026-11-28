//! The store's chain word at open (#639): `open_shared` refuses a store of
//! another chain by name, and a legacy word reads as its chain.

use super::*;

/// A store migrated at `db` on `chain`, as `init` writes it.
async fn initialised(db: &str, chain: bsv_wallet_toolbox::Chain) {
    use bsv_wallet_toolbox::WalletStorageWriter;
    let options = bsv_wallet_toolbox::storage::sqlx::StorageSqlxOptions {
        chain,
        ..Default::default()
    };
    let store = bsv_wallet_toolbox::StorageSqlx::open_with_options(db, options)
        .await
        .unwrap();
    store
        .migrate("chain-word", &("02".to_string() + &"ab".repeat(32)))
        .await
        .unwrap();
}

fn options_on(
    chain: bsv_wallet_toolbox::Chain,
) -> bsv_wallet_toolbox::storage::sqlx::StorageSqlxOptions {
    bsv_wallet_toolbox::storage::sqlx::StorageSqlxOptions {
        chain,
        ..Default::default()
    }
}

/// #639: the daemon's open (`open_shared`, the tenant's chain in its
/// options) takes a store of its own chain and refuses one of the
/// other, by name, both ways.
#[tokio::test]
async fn open_shared_refuses_a_store_of_another_chain_by_name() {
    use bsv_wallet_toolbox::Chain;
    for (written, asked, refusal) in [
        (
            Chain::Main,
            Chain::Test,
            "was created for chain main (its settings' chain) and this command runs on chain test: a testnet wallet made before the chain word was recorded reads so; if this wallet's coins are testnet's, do not run it without --testnet: run `bsv-wallet --testnet repair-chain` to see the evidence and repair the word",
        ),
        (
            Chain::Test,
            Chain::Main,
            "was created for chain test (its settings' chain) and this command runs on chain main: run it with --testnet",
        ),
    ] {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("wallet.db");
        let db = db.to_str().unwrap();
        initialised(db, written).await;
        open_shared(db, options_on(written)).await.expect("its own chain");
        let err = open_shared(db, options_on(asked)).await.err().expect("refused");
        assert_eq!(
            err.downcast_ref::<ChainMismatch>(),
            Some(&ChainMismatch {
                db: db.to_string(),
                stored: written.as_str().to_string(),
                asked: asked.as_str(),
            })
        );
        assert_eq!(
            err.to_string(),
            format!("the wallet database {db} {refusal}")
        );
    }
}

/// A store whose row still carries the legacy word `testnet` reads as
/// `test` (the toolbox's read map), so a `--testnet` context takes it.
#[tokio::test]
async fn a_legacy_testnet_word_is_the_test_chain() {
    use bsv_wallet_toolbox::Chain;
    let (_dir, db) = crate::test_support::temp_db();
    let db = db.as_str();
    initialised(db, Chain::Test).await;
    let pool = SqlitePool::connect(&format!("sqlite:{db}")).await.unwrap();
    sqlx::query("UPDATE settings SET chain = 'testnet'")
        .execute(&pool)
        .await
        .unwrap();
    pool.close().await;
    let (store, _) = open_shared(db, options_on(Chain::Test)).await.unwrap();
    assert_eq!(require_chain(&store, db, Chain::Test), Ok(()));
    assert!(require_chain(&store, db, Chain::Main).is_err());
}
