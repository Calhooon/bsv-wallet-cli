//! The internalize arguments of a BRC-29 payment: the remittance (the
//! derivation prefix and suffix, the sender's identity key), the output
//! index and the transaction carrying it, through `internalize_args` and
//! the toolbox wallet's derivation checks (mirrored in this crate's
//! library). None may panic; the mapping carries every field unchanged;
//! an output index past the outputs is refused; a payment to the key the
//! remittance derives is taken and one to another key is refused.
#![no_main]

use std::sync::OnceLock;

use arbitrary::Arbitrary;
use bsv_sdk::primitives::{PrivateKey, PublicKey};
use bsv_wallet_cli::server::handlers::internalize_args;
use bsv_wallet_cli::server::types::{
    McBasketInsertion, McInternalizeActionReq, McInternalizeOutput, McWalletPayment,
};
use bsv_wallet_cli_fuzz::{
    atomic_payment, brc29_key, internalize_precheck, p2pkh, raw_tx, synthetic_root,
};
use libfuzzer_sys::fuzz_target;

#[derive(Arbitrary, Debug)]
enum Sender {
    /// No sender: the wallet's own payment (`Counterparty::Self_`).
    Empty,
    /// One of four synthetic identity keys, hex.
    Known(u8),
    /// Any text where the key's hex goes.
    Text(String),
}

#[derive(Arbitrary, Debug)]
enum Carrier {
    /// A proven parent and a child with `outputs` outputs (1 to 4), the
    /// one at `pay_at` locked to `to_derived` ? the derived key : another.
    Built {
        outputs: u8,
        pay_at: u8,
        to_derived: bool,
    },
    /// Any bytes as the transaction.
    Raw(Vec<u8>),
}

#[derive(Arbitrary, Debug)]
struct Input {
    prefix: String,
    suffix: String,
    sender: Sender,
    output_index: u32,
    protocol: String,
    basket: Option<String>,
    labels: Option<Vec<String>>,
    carrier: Carrier,
}

fn root() -> &'static PrivateKey {
    static ROOT: OnceLock<PrivateKey> = OnceLock::new();
    ROOT.get_or_init(synthetic_root)
}

fn known(k: u8) -> PublicKey {
    PrivateKey::from_bytes(&[(k % 4) + 2; 32])
        .expect("a key")
        .public_key()
}

fuzz_target!(|input: Input| {
    let sender_hex = match &input.sender {
        Sender::Empty => String::new(),
        Sender::Known(k) => known(*k).to_hex(),
        Sender::Text(s) => s.clone(),
    };
    // The key the wallet derives for this remittance; `None` when the
    // sender's text is not a key.
    let sender_key = (!sender_hex.is_empty()).then(|| PublicKey::from_hex(&sender_hex).ok());
    let derived = match &sender_key {
        None => brc29_key(root(), &input.prefix, &input.suffix, None),
        Some(Some(key)) => brc29_key(root(), &input.prefix, &input.suffix, Some(key)),
        Some(None) => None,
    };

    let (tx, built) = match &input.carrier {
        Carrier::Built {
            outputs,
            pay_at,
            to_derived,
        } => {
            let n = (*outputs % 4) as usize + 1;
            let at = *pay_at as usize % n;
            let paid = match (&derived, to_derived) {
                (Some(key), true) => key.hash160(),
                _ => [0xdb; 20],
            };
            let outs: Vec<(u64, Vec<u8>)> = (0..n)
                .map(|i| {
                    let hash = if i == at { paid } else { [i as u8 + 1; 20] };
                    (1000, p2pkh(hash))
                })
                .collect();
            let parent = raw_tx(&[(&"11".repeat(32), 0)], &[(5000, p2pkh([0x22; 20]))]);
            let parent_id = bsv_wallet_cli_fuzz::txid_of(&parent);
            let child = raw_tx(&[(&parent_id, 0)], &outs);
            (atomic_payment(parent, child), Some((n, at, paid)))
        }
        Carrier::Raw(bytes) => (bytes.clone(), None),
    };

    let req = McInternalizeActionReq {
        tx: tx.clone(),
        outputs: vec![McInternalizeOutput {
            output_index: input.output_index,
            protocol: input.protocol.clone(),
            payment_remittance: Some(McWalletPayment {
                derivation_prefix: input.prefix.clone(),
                derivation_suffix: input.suffix.clone(),
                sender_identity_key: sender_hex.clone(),
            }),
            insertion_remittance: input.basket.clone().map(|basket| McBasketInsertion {
                basket,
                custom_instructions: None,
                tags: None,
            }),
        }],
        description: "fuzz".to_string(),
        labels: input.labels.clone(),
    };
    let args = internalize_args(req);

    assert_eq!(args.tx, tx);
    assert_eq!(args.outputs.len(), 1);
    let out = &args.outputs[0];
    assert_eq!(out.output_index, input.output_index);
    assert_eq!(out.protocol, input.protocol);
    let payment = out.payment_remittance.as_ref().expect("the remittance");
    assert_eq!(payment.derivation_prefix, input.prefix);
    assert_eq!(payment.derivation_suffix, input.suffix);
    assert_eq!(payment.sender_identity_key, sender_hex);

    let verdict = internalize_precheck(&args, root());
    let Some((n, at, paid)) = built else {
        return;
    };
    if input.prefix.is_empty() || input.suffix.is_empty() {
        assert!(verdict.is_err(), "an empty derivation was taken");
        return;
    }
    let index = input.output_index as usize;
    if index >= n {
        assert!(
            verdict.is_err(),
            "an output index past the outputs was taken"
        );
        return;
    }
    if index != at {
        // Another P2PKH output, locked to a hash no remittance derives.
        assert!(verdict.is_err(), "a payment to another key was taken");
        return;
    }
    match &derived {
        Some(key) if key.hash160() == paid => {
            assert!(
                verdict.is_ok(),
                "a payment to the derived key was refused: {verdict:?}"
            )
        }
        _ => assert!(verdict.is_err(), "a payment to another key was taken"),
    }
});
