//! The AtomicBEEF and BEEF bytes the server consumes, through the reads it
//! makes on them: `internalizeAction`'s subject (`post_word`), the toolbox
//! wallet's pre-store parse, the toolbox store's step 1 (the streaming
//! reader, the parse, the atomic txid, the structure check, the subject's
//! outputs), `createAction`'s AtomicBEEF build and its unproven ancestors,
//! and `receive`'s courier (`src/atomic_beef.rs`, compiled here unchanged).
//! None may panic; what the CLI writes as AtomicBEEF it must read back.
#![no_main]

#[allow(dead_code)]
#[path = "../../src/atomic_beef.rs"]
mod atomic_beef;

use bsv_sdk::transaction::{Beef, Transaction};
use bsv_wallet_cli::server::post_word::atomic_subject;
use bsv_wallet_toolbox::storage::sqlx::refuse_invalid_beef_bytes;
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let subject = atomic_subject(data);

    // The toolbox wallet's derivation check parses the bytes before it derives.
    let _ = Transaction::from_beef(data, None);

    // The toolbox store's step 1 (internalize_action_internal).
    if refuse_invalid_beef_bytes(data).is_ok() {
        if let Ok(mut beef) = Beef::from_binary(data) {
            if let Some(txid) = beef.atomic_txid.clone() {
                assert_eq!(
                    subject.as_deref(),
                    Some(txid.as_str()),
                    "the door's subject is not the parse's atomic txid"
                );
                if beef.verify_valid(true).valid {
                    if let Some(tx) = beef.find_txid(&txid).and_then(|b| b.tx()) {
                        let _ = tx.outputs.len();
                        let _ = tx.to_binary();
                        for input in &tx.inputs {
                            let _ = input.get_source_txid();
                        }
                    }
                }
            }
        }
    }

    // createAction's answer: the toolbox's BEEF to AtomicBEEF on the new
    // txid, and the unproven ancestors the package carried.
    if let Ok(mut beef) = Beef::from_binary(data) {
        if let Some(last) = beef.txs.last().map(|t| t.txid()) {
            let _ = beef.to_binary_atomic(&last);
            let _ = bsv_wallet_toolbox::unproven_ancestors_in_beef(data, &last);
        }
    }
    if let Some(subject) = &subject {
        let _ = bsv_wallet_toolbox::unproven_ancestors_in_beef(data, subject);
    }

    // receive's courier: a stranger's BEEF made atomic on its leaf.
    if let Ok(atomic) = atomic_beef::ensure_atomic(data) {
        let back = Beef::from_binary(&atomic).expect("the CLI's AtomicBEEF reads back");
        assert!(
            back.atomic_txid.is_some(),
            "the CLI's AtomicBEEF names no subject"
        );
    }
});
