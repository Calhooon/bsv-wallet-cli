//! The Genesis height and the BEEF check, on synthetic transactions; the
//! boundary read through the toolbox's own door.

use super::*;
use bsv_sdk::primitives::PrivateKey;
use bsv_sdk::script::{LockingScript, UnlockingScript};
use bsv_sdk::transaction::{Beef, MerklePath, Transaction, TransactionInput, TransactionOutput};
use bsv_sdk::wallet::{BasketInsertion, InternalizeActionArgs, InternalizeOutput, WalletInterface};
use bsv_wallet_toolbox::services::mock::{
    success_post_beef_result, MockResponse, MockWalletServices,
};
use bsv_wallet_toolbox::{StorageSqlx, Wallet, WalletStorageWriter};

/// The lock the placeholder input `00` spends, `14 <20 zero bytes> 75 75
/// 51`: the toolbox's fixture for its door (0.13.7, Hosts, **Test
/// fixtures**).
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
            beef.merge_raw_tx(parent.to_binary(), Some(bump));
            parent
        }
        None => {
            let grand = spending(&hex::encode([seed; 32]), 11_000);
            let bump = beef.merge_bump(MerklePath::from_coinbase_txid(&grand.id(), 900_000));
            beef.merge_raw_tx(grand.to_binary(), Some(bump));
            let parent = spending(&grand.id(), 10_000);
            beef.merge_raw_tx(parent.to_binary(), None);
            parent
        }
    };
    let payment = spending(&parent.id(), 9_000);
    beef.merge_raw_tx(payment.to_binary(), None);
    let bytes = beef.to_binary_atomic(&payment.id()).unwrap();
    (bytes, parent.id(), payment.id())
}

#[test]
fn each_chain_names_the_genesis_height_the_door_judges_by() {
    assert_eq!(genesis_height(Chain::Main), 620_538);
    assert_eq!(genesis_height(Chain::Test), 1_344_302);
}

/// An unproven payment spending a coin the BEEF proves one block below
/// mainnet's Genesis is named, with its input, its source and the height;
/// at the Genesis height itself nothing is named.
#[test]
fn an_unproven_spend_of_a_coin_proven_below_genesis_is_named_and_one_at_it_is_not() {
    let (below, parent, payment) = payment_over(0x31, Some(620_537));
    assert_eq!(
        unproven_spend_of_pre_genesis_coin(&below, Chain::Main),
        Some(PreGenesisSpend {
            txid: payment,
            input: 0,
            source_txid: parent,
            coin_height: 620_537,
            genesis_height: 620_538,
        })
    );
    let (at, _, _) = payment_over(0x32, Some(620_538));
    assert_eq!(unproven_spend_of_pre_genesis_coin(&at, Chain::Main), None);
}

/// The chain asked decides: a coin proven at 700,000 is after mainnet's
/// Genesis and before testnet's (1,344,302).
#[test]
fn the_height_is_the_chain_asked() {
    let (beef, _, _) = payment_over(0x33, Some(700_000));
    assert_eq!(unproven_spend_of_pre_genesis_coin(&beef, Chain::Main), None);
    let named = unproven_spend_of_pre_genesis_coin(&beef, Chain::Test).expect("named");
    assert_eq!(
        (named.coin_height, named.genesis_height),
        (700_000, 1_344_302)
    );
    let (at, _, _) = payment_over(0x34, Some(1_344_302));
    assert_eq!(unproven_spend_of_pre_genesis_coin(&at, Chain::Test), None);
}

/// A coin of an unproven parent is not judged by a height (the door gives
/// it the spend's era), and a transaction the BEEF proves is not run: a
/// mined spend of a coin mined before Genesis names nothing.
#[test]
fn an_unproven_parent_and_a_proven_spend_name_nothing() {
    let (beef, _, _) = payment_over(0x35, None);
    assert_eq!(unproven_spend_of_pre_genesis_coin(&beef, Chain::Main), None);

    let parent = spending(&hex::encode([0x36; 32]), 10_000);
    let spend = spending(&parent.id(), 9_000);
    let mut proven = Beef::new();
    let a = proven.merge_bump(MerklePath::from_coinbase_txid(&parent.id(), 600_000));
    proven.merge_raw_tx(parent.to_binary(), Some(a));
    let b = proven.merge_bump(MerklePath::from_coinbase_txid(&spend.id(), 900_000));
    proven.merge_raw_tx(spend.to_binary(), Some(b));
    let bytes = proven.to_binary_atomic(&spend.id()).unwrap();
    assert_eq!(
        unproven_spend_of_pre_genesis_coin(&bytes, Chain::Main),
        None
    );
    assert_eq!(
        unproven_spend_of_pre_genesis_coin(&[1, 2, 3], Chain::Main),
        None
    );
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
    beef.merge_raw_tx(grand.to_binary(), Some(bump));
    beef.merge_raw_tx(parent.to_binary(), None);
    beef.merge_raw_tx(payment.to_binary(), None);
    let bytes = beef.to_binary_atomic(&payment.id()).unwrap();
    let named = unproven_spend_of_pre_genesis_coin(&bytes, Chain::Main).expect("named");
    assert_eq!(named.txid, parent.id());
    assert_eq!(named.source_txid, grand.id());
    assert_eq!(named.coin_height, 600_000);
}

/// Through the toolbox's door, a mainnet wallet: a payment whose parent is
/// proven at 620,537 is refused as not judged, and one whose parent is
/// proven at [`genesis_height`] (620,538) is taken. A drift between this
/// host's number and the door's turns this red.
#[tokio::test]
async fn the_door_refuses_a_coin_proven_one_below_genesis_and_takes_one_at_it() {
    let root = PrivateKey::random();
    let storage = StorageSqlx::in_memory().await.unwrap();
    storage
        .migrate("genesis", &root.public_key().to_hex())
        .await
        .unwrap();
    storage.make_available().await.unwrap();
    let below = payment_over(0x41, Some(genesis_height(Chain::Main) - 1));
    let at = payment_over(0x42, Some(genesis_height(Chain::Main)));
    let services = MockWalletServices::builder()
        .post_beef_response(MockResponse::Success(vec![success_post_beef_result(
            "arc",
            &[below.2.as_str(), at.2.as_str()],
        )]))
        .build();
    let wallet = Wallet::new(Some(root), storage, services).await.unwrap();
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
        .internalize_action(args(below.0), "genesis.local")
        .await
        .expect_err("a coin proven below Genesis is not judged");
    assert!(
        refused.to_string().ends_with(&format!(
            "input 0 of {}: input 0 not judged: coin height 620537 is before Genesis height 620538.",
            below.2
        )),
        "{refused}"
    );
    let taken = wallet
        .internalize_action(args(at.0), "genesis.local")
        .await
        .expect("a coin proven at the Genesis height is judged");
    assert!(taken.accepted);
}
