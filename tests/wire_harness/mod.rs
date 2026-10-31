//! The wire oracle's harness, by concern: the fixture and its normalizer, the
//! worlds a fixture is served from, the recorder and the cases it records,
//! the judge of the corpus's coverage, the manifest of the recorded set, and
//! the dependents gate's text report.
//! The record, the replay and the planted cases are `tests/wire.rs`'s.

pub(crate) mod cases;
pub(crate) mod cases_beta2;
pub(crate) mod cases_beta3;
pub(crate) mod cases_beta4;
pub(crate) mod cases_certificate_privacy;
pub(crate) mod cases_page_bounds;
pub(crate) mod cases_routes;
pub(crate) mod cases_spending;
pub(crate) mod certificate_material;
pub(crate) mod fixture;
pub(crate) mod judge;
pub(crate) mod manifest;
pub(crate) mod recorder;
pub(crate) mod review_race;
pub(crate) mod text_report;
pub(crate) mod tracker_fault;
pub(crate) mod worlds;
