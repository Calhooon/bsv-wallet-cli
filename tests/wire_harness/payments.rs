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
    /// The `internalize` world's payments (#819's and #823's rows): a
    /// valid spend paying 9,000 to [`P2PKH`], another key than a
    /// remittance derives, offered as a wallet payment.
    OtherKey,
    /// The derived key's P2PK lock (the toolbox's shapes that are not the
    /// derived key's P2PKH, #819).
    DerivedP2pk,
    /// An `OP_RETURN` script.
    OpReturn,
    /// The derived key's P2PKH with one byte after it.
    DerivedP2pkhAndByte,
    /// An empty script.
    EmptyScript,
    /// The derived key's P2PKH, offered as a wallet payment that also
    /// carries an insertion remittance naming `default` (#823).
    StrayInsertion,
    /// A valid spend paying 9,000 to [`P2PKH`], for the basket insertion
    /// rules (#823): the default basket refused, a named basket, the same
    /// basket again, another basket refused.
    Insertions,
    /// The derived key's P2PKH, as [`Payment::Wallet`], whose parent is
    /// proven at a height the header stub was not seeded with: the stub
    /// answers another root there, so the tracker refutes the BEEF's root
    /// (the toolbox's 0.14.0-beta.8.1 door, the stable line's 0.18.6 row).
    /// Not in [`Payment::ALL`], whose roots the worlds seed.
    UnknownRoot,
    /// The port of 0.18.9 (#634): a valid spend paying 9,000 to [`P2PKH`]
    /// of a parent proven at 600,000, before mainnet's Genesis (620,538):
    /// the door refuses it as not judged.
    PreGenesis,
    /// A mined parent, an unproven middle spending it with the unlocking
    /// script `00` (its script fails, `OP_EQUALVERIFY`), and a valid
    /// unproven spend of the middle paying 9,000 to [`P2PKH`]: a failing
    /// script in an unproven ancestor.
    AncestorBadUnlock,
    /// A mined parent and two unproven ancestors, every input a valid
    /// signature of the stranger's, the last paying 9,000 to [`P2PKH`]:
    /// the passing shape through two unmined parents.
    AncestorsPass,
    /// A mined parent and a spend of it with the unlocking script `00`
    /// paying 9,000 to [`P2PKH`], the spend itself mined at
    /// [`HELD_AT`]: the store takes it carrying its own BUMP (its scripts
    /// not run), and the same spend handed again unproven beside its parent
    /// is refused ([`held_repeat`]).
    HeldBadUnlock,
}

/// The height [`Payment::HeldBadUnlock`]'s spend is mined at.
pub(crate) const HELD_AT: u32 = 899_887;

impl Payment {
    pub(crate) const ALL: [Payment; 15] = [
        Payment::Insertion,
        Payment::BadUnlock,
        Payment::Wallet,
        Payment::BoundInsertion,
        Payment::OtherKey,
        Payment::DerivedP2pk,
        Payment::OpReturn,
        Payment::DerivedP2pkhAndByte,
        Payment::EmptyScript,
        Payment::StrayInsertion,
        Payment::Insertions,
        Payment::PreGenesis,
        Payment::AncestorBadUnlock,
        Payment::AncestorsPass,
        Payment::HeldBadUnlock,
    ];

    /// The seed of the parent's input and the height it is mined at.
    pub(crate) fn parent_at(self) -> (u8, u32) {
        match self {
            Payment::Insertion => (0xe1, 899_905),
            Payment::BadUnlock => (0xe2, 899_906),
            Payment::Wallet => (0xe3, 899_907),
            Payment::BoundInsertion => (0xe5, 899_909),
            // At least 100 below the header stub's tip, as the others
            // (bsv-rs #734).
            Payment::OtherKey => (0xe6, 899_890),
            Payment::DerivedP2pk => (0xe8, 899_891),
            Payment::OpReturn => (0xe9, 899_892),
            Payment::DerivedP2pkhAndByte => (0xea, 899_893),
            Payment::EmptyScript => (0xeb, 899_894),
            Payment::StrayInsertion => (0xec, 899_895),
            Payment::Insertions => (0xed, 899_896),
            Payment::UnknownRoot => (0xf0, 899_880),
            Payment::PreGenesis => (0xf1, 600_000),
            Payment::AncestorBadUnlock => (0xf2, 899_884),
            Payment::AncestorsPass => (0xf3, 899_885),
            Payment::HeldBadUnlock => (0xf4, 899_886),
        }
    }

    /// The lock the payment's one output pays.
    fn pays_to(self, wallet: &PrivateKey) -> Vec<u8> {
        let derived = || derived_key(wallet);
        match self {
            Payment::Insertion
            | Payment::BadUnlock
            | Payment::BoundInsertion
            | Payment::OtherKey
            | Payment::Insertions
            | Payment::PreGenesis
            | Payment::AncestorBadUnlock
            | Payment::AncestorsPass
            | Payment::HeldBadUnlock => hex::decode(P2PKH).unwrap(),
            Payment::Wallet | Payment::StrayInsertion | Payment::UnknownRoot => {
                p2pkh(&derived().hash160())
            }
            Payment::DerivedP2pk => [&[0x21][..], &derived().to_compressed(), &[0xac][..]].concat(),
            Payment::OpReturn => vec![0x00, 0x6a],
            Payment::DerivedP2pkhAndByte => {
                let mut lock = p2pkh(&derived().hash160());
                lock.push(0x51);
                lock
            }
            Payment::EmptyScript => Vec::new(),
        }
    }
}

/// The wallet's BRC-29 key for the stranger's payment, as the payer
/// derives it: the stranger's key, the wallet the counterparty,
/// [`PAYMENT_PREFIX`] and [`PAYMENT_SUFFIX`].
fn derived_key(wallet: &PrivateKey) -> bsv_sdk::primitives::PublicKey {
    use bsv_sdk::wallet::{Counterparty, KeyDeriver, Protocol, SecurityLevel};
    KeyDeriver::new(Some(stranger_key()))
        .derive_public_key(
            &Protocol::new(SecurityLevel::Counterparty, BRC29_PROTOCOL),
            &format!("{PAYMENT_PREFIX} {PAYMENT_SUFFIX}"),
            &Counterparty::Other(wallet.public_key()),
            false,
        )
        .unwrap()
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
    if let Some(chain) = chain_payment(payment, wallet).await {
        return chain;
    }
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
        _ => input.set_unlocking_script_template(Template::unlock(
            &stranger,
            SignOutputs::All,
            false,
        )),
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

/// A spend of `source`'s output 0 paying `satoshis` to `lock`: signed by
/// the stranger when `passes`, else the unlocking script `00`.
async fn spend_of(
    source: &bsv_sdk::transaction::Transaction,
    satoshis: u64,
    lock: &[u8],
    passes: bool,
) -> bsv_sdk::transaction::Transaction {
    use bsv_sdk::script::templates::P2PKH as Template;
    use bsv_sdk::script::SignOutputs;
    let mut tx = bsv_sdk::transaction::Transaction::new();
    let mut input =
        bsv_sdk::transaction::TransactionInput::with_source_transaction(source.clone(), 0);
    if passes {
        input.set_unlocking_script_template(Template::unlock(
            &stranger_key(),
            SignOutputs::All,
            false,
        ));
    } else {
        input.unlocking_script = Some(bsv_sdk::script::UnlockingScript::from_hex("00").unwrap());
    }
    tx.add_input(input).unwrap();
    tx.add_output(bsv_sdk::transaction::TransactionOutput::new(
        satoshis,
        bsv_sdk::script::LockingScript::from_binary(lock).unwrap(),
    ))
    .unwrap();
    tx.sign().await.unwrap();
    tx
}

/// The port of 0.18.9's payments (#634): the mined parent paying the
/// stranger, then the unproven chain (`None` for the other payments). The
/// `AtomicBEEF` carries the parent beside its BUMP and every unproven link.
async fn chain_payment(
    payment: Payment,
    wallet: &PrivateKey,
) -> Option<(bsv_sdk::transaction::Transaction, Vec<u8>)> {
    let stranger = p2pkh(&stranger_key().public_key().hash160());
    let pays = payment.pays_to(wallet);
    let (seed, at) = payment.parent_at();
    let parent = parent_tx(seed, 10_000, &stranger);
    let links = match payment {
        Payment::PreGenesis => vec![spend_of(&parent, 9_000, &pays, true).await],
        Payment::AncestorBadUnlock => {
            let middle = spend_of(&parent, 9_500, &stranger, false).await;
            let child = spend_of(&middle, 9_000, &pays, true).await;
            vec![middle, child]
        }
        Payment::AncestorsPass => {
            let older = spend_of(&parent, 9_800, &stranger, true).await;
            let newer = spend_of(&older, 9_500, &stranger, true).await;
            let child = spend_of(&newer, 9_000, &pays, true).await;
            vec![older, newer, child]
        }
        Payment::HeldBadUnlock => vec![spend_of(&parent, 9_000, &pays, false).await],
        _ => return None,
    };
    let mut beef = bsv_sdk::transaction::Beef::new();
    let bump = beef.merge_bump(bsv_sdk::transaction::MerklePath::from_coinbase_txid(
        &parent.id(),
        at,
    ));
    beef.merge_raw_tx(parent.to_binary(), Some(bump)).unwrap();
    for link in &links {
        beef.merge_raw_tx(link.to_binary(), None).unwrap();
    }
    let subject = links.last().unwrap().id();
    Some((parent, beef.to_binary_atomic(&subject).unwrap()))
}

/// [`Payment::HeldBadUnlock`]'s spend, carrying its own BUMP at
/// [`HELD_AT`] (the `AtomicBEEF`), and its txid: the root the header stub
/// answers at that height.
pub(crate) async fn held_repeat(wallet: &PrivateKey) -> (Vec<u8>, String) {
    let stranger = p2pkh(&stranger_key().public_key().hash160());
    let (seed, _) = Payment::HeldBadUnlock.parent_at();
    let parent = parent_tx(seed, 10_000, &stranger);
    let spend = spend_of(
        &parent,
        9_000,
        &Payment::HeldBadUnlock.pays_to(wallet),
        false,
    )
    .await;
    let txid = spend.id();
    (super::worlds::proven_atomic(&spend, HELD_AT), txid)
}
