//! The Genesis height and the BEEF check, on synthetic transactions; the
//! boundary read through the toolbox's own door, on a mainnet store and on
//! a store migrated as `--testnet` migrates it.

use super::*;
use bsv_sdk::primitives::PrivateKey;
use bsv_sdk::script::{LockingScript, UnlockingScript};
use bsv_sdk::transaction::{Beef, MerklePath, Transaction, TransactionInput, TransactionOutput};
use bsv_sdk::wallet::{BasketInsertion, InternalizeActionArgs, InternalizeOutput, WalletInterface};
use bsv_wallet_toolbox::services::mock::{
    success_post_beef_result, MockResponse, MockWalletServices,
};
use bsv_wallet_toolbox::storage::sqlx::StorageSqlxOptions;
use bsv_wallet_toolbox::{Chain, MonitorStorage, Wallet, WalletStorageWriter};

/// The lock the placeholder input `00` spends, `14 <20 zero bytes> 75 75
/// 51` (the toolbox's fixture for its door).
fn anyone_lock() -> LockingScript {
    LockingScript::from_hex(&format!("14{}757551", "00".repeat(20))).unwrap()
}

/// A transaction of one input `00` spending `txid`.0 and one output of
/// `satoshis` locked by [`anyone_lock`].
fn spending(txid: &str, satoshis: u64) -> Transaction {
    let mut tx = Transaction::new();
    let mut input = TransactionInput::new(txid.to_string(), 0);
    input.unlocking_script = Some(UnlockingScript::from_hex("00").unwrap());
    tx.add_input(input).unwrap();
    tx.add_output(TransactionOutput::new(satoshis, anyone_lock()))
        .unwrap();
    tx
}

/// The AtomicBEEF of a parent proven at `height` (when given; else carried
/// unproven, beside its own proven parent at 900,000) and the payment
/// spending it, unproven. (bytes, parent txid, payment txid.)
fn payment_over(seed: u8, height: Option<u32>) -> (Vec<u8>, String, String) {
    let mut beef = Beef::new();
    let parent = match height {
        Some(height) => {
            let parent = spending(&hex::encode([seed; 32]), 10_000);
            let bump = beef.merge_bump(MerklePath::from_coinbase_txid(&parent.id(), height));
            beef.merge_raw_tx(parent.to_binary(), Some(bump)).unwrap();
            parent
        }
        None => {
            let grand = spending(&hex::encode([seed; 32]), 11_000);
            let bump = beef.merge_bump(MerklePath::from_coinbase_txid(&grand.id(), 900_000));
            beef.merge_raw_tx(grand.to_binary(), Some(bump)).unwrap();
            let parent = spending(&grand.id(), 10_000);
            beef.merge_raw_tx(parent.to_binary(), None).unwrap();
            parent
        }
    };
    let payment = spending(&parent.id(), 9_000);
    beef.merge_raw_tx(payment.to_binary(), None).unwrap();
    let bytes = beef.to_binary_atomic(&payment.id()).unwrap();
    (bytes, parent.id(), payment.id())
}

const MAIN: u32 = 620_538;
const TEST: u32 = 1_344_302;

/// Each network word the door reads names bsv-rs's public height; a word
/// it does not read names none.
#[test]
fn each_network_names_the_genesis_height_the_door_judges_by() {
    assert_eq!(genesis_height("main"), Some(MAIN));
    assert_eq!(genesis_height("test"), Some(TEST));
    assert_eq!(
        genesis_height("regtest"),
        Some(ChainParams::regtest().genesis_height)
    );
    for word in ["mainnet", "testnet", ""] {
        assert_eq!(genesis_height(word), None, "{word}");
    }
}

/// An unproven payment spending a coin the BEEF proves one block below
/// mainnet's Genesis is named, with its input, its source and the height;
/// at the Genesis height itself nothing is named.
#[test]
fn an_unproven_spend_of_a_coin_proven_below_genesis_is_named_and_one_at_it_is_not() {
    let (below, parent, payment) = payment_over(0x31, Some(MAIN - 1));
    assert_eq!(
        unproven_spend_of_pre_genesis_coin(&below, MAIN),
        Some(PreGenesisSpend {
            txid: payment,
            input: 0,
            source_txid: parent,
            coin_height: u64::from(MAIN - 1),
            genesis_height: MAIN,
        })
    );
    let (at, _, _) = payment_over(0x32, Some(MAIN));
    assert_eq!(unproven_spend_of_pre_genesis_coin(&at, MAIN), None);
}

/// The height asked decides: a coin proven at 700,000 is after mainnet's
/// Genesis and before testnet's.
#[test]
fn the_height_is_the_networks_asked() {
    let (beef, _, _) = payment_over(0x33, Some(700_000));
    assert_eq!(unproven_spend_of_pre_genesis_coin(&beef, MAIN), None);
    let named = unproven_spend_of_pre_genesis_coin(&beef, TEST).expect("named");
    assert_eq!((named.coin_height, named.genesis_height), (700_000, TEST));
    let (at, _, _) = payment_over(0x34, Some(TEST));
    assert_eq!(unproven_spend_of_pre_genesis_coin(&at, TEST), None);
}

/// A coin of an unproven parent is not judged by a height, and a
/// transaction the BEEF proves is not run: a mined spend of a coin mined
/// before Genesis names nothing; bytes that do not read name nothing.
#[test]
fn an_unproven_parent_and_a_proven_spend_name_nothing() {
    let (beef, _, _) = payment_over(0x35, None);
    assert_eq!(unproven_spend_of_pre_genesis_coin(&beef, MAIN), None);

    let parent = spending(&hex::encode([0x36; 32]), 10_000);
    let spend = spending(&parent.id(), 9_000);
    let mut proven = Beef::new();
    let a = proven.merge_bump(MerklePath::from_coinbase_txid(&parent.id(), 600_000));
    proven.merge_raw_tx(parent.to_binary(), Some(a)).unwrap();
    let b = proven.merge_bump(MerklePath::from_coinbase_txid(&spend.id(), 900_000));
    proven.merge_raw_tx(spend.to_binary(), Some(b)).unwrap();
    let bytes = proven.to_binary_atomic(&spend.id()).unwrap();
    assert_eq!(unproven_spend_of_pre_genesis_coin(&bytes, MAIN), None);
    assert_eq!(unproven_spend_of_pre_genesis_coin(&[1, 2, 3], MAIN), None);
}

/// The ancestor case: an unproven parent that itself spends a coin proven
/// below Genesis is named, the parent as the transaction.
#[test]
fn an_unproven_ancestor_spending_a_coin_proven_below_genesis_is_named() {
    let grand = spending(&hex::encode([0x37; 32]), 11_000);
    let parent = spending(&grand.id(), 10_000);
    let payment = spending(&parent.id(), 9_000);
    let mut beef = Beef::new();
    let bump = beef.merge_bump(MerklePath::from_coinbase_txid(&grand.id(), 600_000));
    beef.merge_raw_tx(grand.to_binary(), Some(bump)).unwrap();
    beef.merge_raw_tx(parent.to_binary(), None).unwrap();
    beef.merge_raw_tx(payment.to_binary(), None).unwrap();
    let bytes = beef.to_binary_atomic(&payment.id()).unwrap();
    let named = unproven_spend_of_pre_genesis_coin(&bytes, MAIN).expect("named");
    assert_eq!(named.txid, parent.id());
    assert_eq!(named.source_txid, grand.id());
    assert_eq!(named.coin_height, 600_000);
}

/// Through the toolbox's door, on a store migrated on `chain` (as `init`
/// migrates it from `--testnet`) and a wallet built as
/// `WalletContext::load_with` builds it (`Wallet::with_options`, whose own
/// chain is mainnet): the host reads the store's word, and a payment whose
/// parent is proven one below that word's Genesis height is refused as not
/// judged while one proven at it is taken. A drift between this host's
/// number or chain and the door's turns this red.
async fn the_door_agrees_on(chain: Chain, word: &str, tip: u32) {
    let root = PrivateKey::random();
    let storage = StorageSqlx::open_with_options(
        ":memory:",
        StorageSqlxOptions {
            chain,
            ..Default::default()
        },
    )
    .await
    .unwrap();
    storage
        .migrate("genesis", &root.public_key().to_hex())
        .await
        .unwrap();
    storage.make_available().await.unwrap();
    storage.set_max_acceptable_proof_height(tip).await.unwrap();
    let genesis = door_genesis_height(&storage).expect("the door reads this store's word");
    assert_eq!(door_network(&storage), word);
    let below = payment_over(0x41, Some(genesis - 1));
    let at = payment_over(0x42, Some(genesis));
    // The mock's chain tracker (its tip `tip`) is the door's header source.
    let services = MockWalletServices::builder()
        .height(tip)
        .post_beef_response(MockResponse::Success(vec![success_post_beef_result(
            "arc",
            &[below.2.as_str(), at.2.as_str()],
        )]))
        .build();
    let wallet = Wallet::with_options(
        Some(root),
        storage,
        services,
        bsv_wallet_toolbox::WalletOptions::default(),
    )
    .await
    .unwrap();
    assert_eq!(wallet.chain(), Chain::Main, "the Wallet's own chain");
    let args = |tx: Vec<u8>| InternalizeActionArgs {
        tx,
        outputs: vec![InternalizeOutput {
            output_index: 0,
            protocol: "basket insertion".to_string(),
            payment_remittance: None,
            insertion_remittance: Some(BasketInsertion {
                basket: "genesis".to_string(),
                custom_instructions: None,
                tags: None,
            }),
        }],
        description: "genesis boundary".to_string(),
        labels: None,
        seek_permission: None,
    };

    let refused = wallet
        .internalize_action(args(below.0), Some("genesis.local"))
        .await
        .expect_err("a coin proven below Genesis is not judged");
    assert!(
        matches!(
            &refused,
            bsv_sdk::Error::WalletRefusal {
                class: bsv_sdk::WalletRefusalClass::InvalidParameter,
                ..
            }
        ),
        "{refused:?}"
    );
    assert!(
        refused.to_string().ends_with(&format!(
            "input 0 of {}: input 0 not judged: coin height {} is before Genesis height {genesis}.",
            below.2,
            genesis - 1
        )),
        "{refused}"
    );
    let taken = wallet
        .internalize_action(args(at.0), Some("genesis.local"))
        .await
        .expect("a coin proven at the Genesis height is judged");
    assert!(taken.accepted);
}

#[tokio::test]
async fn the_door_refuses_a_coin_proven_one_below_genesis_and_takes_one_at_it() {
    the_door_agrees_on(Chain::Main, "main", 900_000).await;
}

/// The store `--testnet` migrates: the door judges by testnet's height
/// (1,344,302), though the wallet's own chain is mainnet, and the host
/// asks the same.
#[tokio::test]
async fn a_testnet_store_judges_by_testnets_height_and_the_host_asks_the_same() {
    the_door_agrees_on(Chain::Test, "test", 1_400_000).await;
}
