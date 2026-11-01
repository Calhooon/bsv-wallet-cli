//! The served `/createAction` and `/internalizeAction` bodies as the door
//! reads them: the lenient-JSON layer (`sanitize_lone_surrogates`), serde
//! into the MetaNet Client shapes, then the handlers' argument mappings
//! (`create_action_args`, `internalize_args`, called). No store. None may
//! panic; the sanitizer is idempotent; no rule label survives an
//! internalize's mapping.
#![no_main]

use bsv_wallet_cli_fuzz::props::served_json::check;
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| check(data));
