//! The AtomicBEEF and BEEF bytes the server consumes, through the reads it
//! makes on them: `internalizeAction`'s subject (`post_word`), the toolbox
//! wallet's pre-store parse, the toolbox store's step 1 (the streaming
//! reader, the parse, the atomic txid, the structure check, the subject's
//! outputs), `createAction`'s AtomicBEEF build and its unproven ancestors,
//! and `receive`'s courier (`src/atomic_beef.rs`, compiled here unchanged).
//! None may panic; what the CLI writes as AtomicBEEF it must read back.
#![no_main]

use bsv_wallet_cli_fuzz::props::beef_reader::check;
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| check(data));
