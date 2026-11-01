//! Each fuzz target's property, the body its `fuzz_target!` calls and
//! nothing else (#396). No item here names `libfuzzer-sys`, so the
//! package's `tests/fuzz_replay.rs` compiles this crate's library by
//! `#[path]` on stable and replays each target's committed seed corpus
//! through the same function: the assertions judge the floored files in
//! the Nightly's mutation oracle (docs/NIGHTLY.md). Each module's target
//! file states its properties.
//!
//! The library's own items are reached as `super::super::`, which names
//! this crate's root here and the module the replay mounts it as there.

pub mod allow_pending;
pub mod beef_reader;
pub mod brc29_remittance;
pub mod explorer_bodies;
pub mod served_json;
