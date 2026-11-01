//! The internalize arguments of a BRC-29 payment: the remittance (the
//! derivation prefix and suffix, the sender's identity key), the output
//! index and the transaction carrying it, through `internalize_args` and
//! the toolbox wallet's derivation checks (mirrored in this crate's
//! library). None may panic; the mapping carries every field unchanged;
//! an output index past the outputs is refused; a payment to the key the
//! remittance derives is taken and one to another key is refused.
#![no_main]

use bsv_wallet_cli_fuzz::props::brc29_remittance::{check, Input};
use libfuzzer_sys::fuzz_target;

fuzz_target!(|input: Input| check(input));
