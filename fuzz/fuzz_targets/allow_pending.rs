//! `allowPending` as the served `/createAction` and the commands read it
//! (`AllowPending::from_json`, `AllowPending::from_cli`, `parse_outpoint`).
//! None may panic; a scope is never empty, sorted, without repeats, and
//! reaches exactly the coins it names; an outpoint parsed prints back to
//! itself.
#![no_main]

use bsv_wallet_cli_fuzz::props::allow_pending::check;
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| check(data));
