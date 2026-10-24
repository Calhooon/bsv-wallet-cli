//! The consumers' recorded calls' harness, by concern: the fixture and its
//! normalizer, the worlds a consumer's fixtures are served from, and the
//! cases each consumer sends. The list of consumers, the record and the
//! replay are `tests/wire_consumers.rs`'s.

pub(crate) mod cases;
pub(crate) mod fixture;
pub(crate) mod worlds;
