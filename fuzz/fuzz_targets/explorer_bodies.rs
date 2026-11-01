//! Each explorer body the CLI still reads itself, on arbitrary bytes:
//! `receive`'s couriers (`src/explorers.rs`: WhatsOnChain's
//! `/tx/{txid}/beef`, BananaBlocks' `/tx/{txid}/hex` through the toolbox's
//! provider) and the broadcast verifier's chain indexes
//! (`src/broadcast_verify.rs`: WhatsOnChain's `/tx/hash/{txid}`, Bitails'
//! `/tx/{txid}`, BananaBlocks' `/tx/{txid}`). The spend probe's spender
//! rungs left the CLI in 0.18.2 (#196, the toolbox's `is_utxo_with_spender`)
//! and are fuzzed no longer here.
//!
//! The readers sit inside the calls, so the target serves the fuzzed status
//! and body from a local socket under `/fz` and makes the calls
//! (`Explorers::at`; for WhatsOnChain's and Bitails' indexes
//! `BroadcastVerifier::single_pass` over a configuration whose every base is
//! the socket and BananaBlocks off; for BananaBlocks' index
//! `BroadcastVerifier::explicit` with `with_bananablocks`, a fresh provider
//! at a turning base each case). `single_pass` takes its BananaBlocks
//! provider from `Explorers::for_settings`, a process-wide cell the first
//! verify case builds, so a base set there would hold for every later case
//! of the run: it is always built off, and BananaBlocks' index is never
//! reached through it. The sources a case does not fuzz answer a clean 404
//! under `/quiet`, the broadcaster a 500 under `/arc`, so a verdict is the
//! fuzzed reader's. The properties:
//!
//! - no reader panics;
//! - a courier delivers only from a 200: WhatsOnChain's BEEF only as the
//!   body's hex (its binding to the txid is the BEEF's own, read at
//!   internalization: `beef_reader`), BananaBlocks' bytes only when they
//!   hash to the txid; a malformed body is a fault;
//! - a chain index is present only from its 200 and absent only from its
//!   404; any other status is "could not look" and never `Rejected`;
//!   `Mined` from WhatsOnChain only for an integer `confirmations` of at
//!   least 1, from Bitails only for an integer `blockHeight`;
//! - BananaBlocks' index says `mined` or `known` only for a body about the
//!   asked txid, and its silence is a gap, never a vote: with the voting
//!   index's clean 404 the answer is `Absent` unless it says `mined`.
//!
//! Input: byte 0 picks the reader, byte 1 the status, the rest is the body.
//! With `EXPLORER_BODIES_VERDICT` set, each chain-index case prints its
//! verdict to stderr (`verdict index=... status=... answer=... by=...`).
//! The seeds are `examples/seed_corpus.rs`'s: for each reader one valid
//! body, one malformed and one about another transaction.
#![no_main]

use bsv_wallet_cli_fuzz::props::explorer_bodies::check;
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| check(data));
