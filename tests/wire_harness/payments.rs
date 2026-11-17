//! The stranger's payments the `funded` world is offered at
//! `internalizeAction`: each a mined parent paying the stranger and a child
//! spending it, checked before it is served.

use bsv_sdk::primitives::PrivateKey;

use super::worlds::{
    p2pkh, parent_tx, BRC29_PROTOCOL, P2PKH, PAYMENT_PREFIX, PAYMENT_SUFFIX, STRANGER_KEY,
};

/// The stranger's payments `internalizeAction` is offered in `funded`.
/// Each is a mined parent paying the stranger's P2PKH and a child spending
/// it.
#[derive(Clone, Copy)]
pub(crate) enum Payment {
    /// A valid spend paying 9,000 to [`P2PKH`] (a basket insertion).
    Insertion,
    /// The same, its input's unlocking script `OP_0`: a spend whose script
    /// fails (`OP_EQUALVERIFY`).
    BadUnlock,
    /// A valid spend paying 9,000 to the wallet's BRC-29 key for the
    /// stranger, [`PAYMENT_PREFIX`] and [`PAYMENT_SUFFIX`].
    Wallet,
    /// A second valid spend paying 9,000 to [`P2PKH`], for the insertion at
    /// the custom-instructions bound (fixtures 127, 128). Its own: the
    /// toolbox at 0.14.0-beta.8 refuses a basket insertion naming an output
    /// already in another application basket, as the reference does, and
    /// [`Payment::Insertion`]'s output is in `wire` by then.
    BoundInsertion,
}

impl Payment {
    pub(crate) const ALL: [Payment; 4] = [
        Payment::Insertion,
        Payment::BadUnlock,
        Payment::Wallet,
        Payment::BoundInsertion,
    ];

    /// The seed of the parent's input and the height it is mined at.
    pub(crate) fn parent_at(self) -> (u8, u32) {
        match self {
            Payment::Insertion => (0xe1, 899_905),
            Payment::BadUnlock => (0xe2, 899_906),
            Payment::Wallet => (0xe3, 899_907),
            Payment::BoundInsertion => (0xe5, 899_909),
        }
    }

    /// The lock the payment's one output pays.
    fn pays_to(self, wallet: &PrivateKey) -> Vec<u8> {
        use bsv_sdk::wallet::{Counterparty, KeyDeriver, Protocol, SecurityLevel};
        match self {
            Payment::Insertion | Payment::BadUnlock | Payment::BoundInsertion => {
                hex::decode(P2PKH).unwrap()
            }
            // As the payer derives it: the stranger's key, the wallet the
            // counterparty.
            Payment::Wallet => p2pkh(
                &KeyDeriver::new(Some(stranger_key()))
                    .derive_public_key(
                        &Protocol::new(SecurityLevel::Counterparty, BRC29_PROTOCOL),
                        &format!("{PAYMENT_PREFIX} {PAYMENT_SUFFIX}"),
                        &Counterparty::Other(wallet.public_key()),
                        false,
                    )
                    .unwrap()
                    .hash160(),
            ),
        }
    }
}

pub(crate) fn stranger_key() -> PrivateKey {
    PrivateKey::from_hex(STRANGER_KEY).unwrap()
}

/// A [`Payment`]'s mined parent and child `AtomicBEEF`, checked before serving.
pub(crate) async fn stranger_payment(
    payment: Payment,
    wallet: &PrivateKey,
) -> (bsv_sdk::transaction::Transaction, Vec<u8>) {
    use bsv_sdk::script::templates::P2PKH as Template;
    use bsv_sdk::script::SignOutputs;
    let stranger = stranger_key();
    let (seed, at) = payment.parent_at();
    let mut parent = parent_tx(seed, 10_000, &p2pkh(&stranger.public_key().hash160()));
    parent.merkle_path = Some(bsv_sdk::transaction::MerklePath::from_coinbase_txid(
        &parent.id(),
        at,
    ));
    let mut child = bsv_sdk::transaction::Transaction::new();
    let mut input =
        bsv_sdk::transaction::TransactionInput::with_source_transaction(parent.clone(), 0);
    match payment {
        Payment::BadUnlock => {
            input.unlocking_script =
                Some(bsv_sdk::script::UnlockingScript::from_hex("00").unwrap());
        }
        Payment::Insertion | Payment::Wallet | Payment::BoundInsertion => input
            .set_unlocking_script_template(Template::unlock(&stranger, SignOutputs::All, false)),
    }
    child.add_input(input).unwrap();
    child
        .add_output(bsv_sdk::transaction::TransactionOutput::new(
            9_000,
            bsv_sdk::script::LockingScript::from_binary(&payment.pays_to(wallet)).unwrap(),
        ))
        .unwrap();
    child.sign().await.unwrap();
    // Parity-mode replay checks the planted good/bad payment specimens.
    // A node-word check needs spend-era flags plus coin-era UTXO bits per input;
    // see docs/ARCHITECTURE.md for the combined-word rule.
    // The tracker's tip is 100 blocks above the parent's proof: bsv-rs
    // 0.9.0-beta.8 refuses an offset-0 proof under 100 blocks deep at
    // `Transaction::verify` (#734: "set a mock's height 100 above its
    // proofs"); the parent's path is a one-leaf coinbase-offset path.
    let verified = child
        .verify(
            &bsv_sdk::transaction::AlwaysValidChainTracker::new(at + 100),
            None,
            None,
        )
        .await
        .unwrap_or(false);
    assert_eq!(
        verified,
        !matches!(payment, Payment::BadUnlock),
        "the stranger's spend verifies exactly when it is meant to"
    );
    let mut beef = bsv_sdk::transaction::Beef::new();
    beef.merge_bump(parent.merkle_path.clone().unwrap());
    beef.merge_transaction(parent.clone());
    beef.merge_transaction(child.clone());
    let atomic = beef.to_binary_atomic(&child.id()).unwrap();
    (parent, atomic)
}
