//! The wire oracle's harness, by concern: the fixture and its normalizer, the
//! worlds a fixture is served from, the recorder and the cases it records,
//! the judge of the corpus's coverage, and the manifest of the recorded set.
//! The record, the replay and the planted cases are `tests/wire.rs`'s.

pub(crate) mod cases;
pub(crate) mod cases_routes;
pub(crate) mod cases_spending;
pub(crate) mod fixture;
pub(crate) mod judge;
pub(crate) mod manifest;
pub(crate) mod recorder;
pub(crate) mod worlds;
