//! The door's synthetic coins (the toolbox's fixture for its door): a lock
//! the placeholder input `00` spends, and a payment over a coin proven at
//! a height, for the Genesis boundary read through the door
//! (`src/genesis/tests.rs`, `src/context/tests.rs`).

use bsv_sdk::script::{LockingScript, UnlockingScript};
use bsv_sdk::transaction::{Beef, MerklePath, Transaction, TransactionInput, TransactionOutput};

/// The lock the placeholder input `00` spends, `14 <20 zero bytes> 75 75
/// 51` (the toolbox's fixture for its door).
pub fn anyone_lock() -> LockingScript {
    LockingScript::from_hex(&format!("14{}757551", "00".repeat(20))).unwrap()
}

/// A transaction of one input `00` spending `txid`.0 and one output of
/// `satoshis` locked by [`anyone_lock`].
pub fn spending(txid: &str, satoshis: u64) -> Transaction {
    let mut tx = Transaction::new();
    let mut input = TransactionInput::new(txid.to_string(), 0);
    input.unlocking_script = Some(UnlockingScript::from_hex("00").unwrap());
    tx.add_input(input).unwrap();
    tx.add_output(TransactionOutput::new(satoshis, anyone_lock()))
        .unwrap();
    tx
}

/// The `AtomicBEEF` of a parent proven at `height` (when given; else carried
/// unproven, beside its own proven parent at 900,000) and the payment
/// spending it, unproven. (bytes, parent txid, payment txid.)
pub fn payment_over(seed: u8, height: Option<u32>) -> (Vec<u8>, String, String) {
    let mut beef = Beef::new();
    let parent = if let Some(height) = height {
        let parent = spending(&hex::encode([seed; 32]), 10_000);
        let bump = beef.merge_bump(MerklePath::from_coinbase_txid(&parent.id(), height));
        beef.merge_raw_tx(parent.to_binary(), Some(bump)).unwrap();
        parent
    } else {
        let grand = spending(&hex::encode([seed; 32]), 11_000);
        let bump = beef.merge_bump(MerklePath::from_coinbase_txid(&grand.id(), 900_000));
        beef.merge_raw_tx(grand.to_binary(), Some(bump)).unwrap();
        let parent = spending(&grand.id(), 10_000);
        beef.merge_raw_tx(parent.to_binary(), None).unwrap();
        parent
    };
    let payment = spending(&parent.id(), 9_000);
    beef.merge_raw_tx(payment.to_binary(), None).unwrap();
    let bytes = beef.to_binary_atomic(&payment.id()).unwrap();
    (bytes, parent.id(), payment.id())
}
