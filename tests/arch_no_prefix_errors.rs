//! Rule A5 (#73): an error is classified by its type, never by the prefix
//! of its message. `starts_with(` and `strip_prefix(` in the shipped code,
//! whatever the argument (a literal, a const, a UFCS `str::starts_with(s,
//! ..)`), are counted file by file: `AppError::classify` in
//! `src/server/handlers.rs` (the prefix table #81 replaces with typed
//! errors) by its exact number, and every other file that holds one today
//! with its count and why. A text rule cannot tell an error message from a
//! header scheme or a byte prefix, so each reason says which its sites are.

mod arch_support;

use arch_support::{calls, hits, judge};

/// The prefix tests.
const TESTS: &[&str] = &["starts_with(", "strip_prefix("];

/// `(file, today's count, why)`. Only shrinks.
const ALLOWED: &[(&str, usize, &str)] = &[
    (
        "src/chain_feed.rs",
        2,
        "the SSE reader's field split (a `:` comment line, the space after a field name), not an error",
    ),
    (
        "src/commands/ui.rs",
        3,
        "the inspector's read-only statement check (SELECT, PRAGMA, EXPLAIN), not an error",
    ),
    (
        "src/gift/covenant.rs",
        1,
        "the covenant template's byte prefix on a script, not a message",
    ),
    (
        "src/relay.rs",
        2,
        "the Authorization header's `Bearer ` scheme, not an error",
    ),
    (
        "src/server/handlers.rs",
        21,
        "AppError::classify: the SDK's `wallet error: ` strip and the 20 prefix tests (17 lines) #81 replaces with typed errors",
    ),
    (
        "src/server/mod.rs",
        2,
        "the Authorization header's `Bearer ` scheme, not an error",
    ),
    (
        "src/spend_gate.rs",
        2,
        "the gate's reasons read back: the HEADERS_UNAVAILABLE word of a held coin (#59) and the SDK's `wallet error: ` strip; a typed reason removes both",
    ),
];

#[test]
fn no_error_is_classified_by_its_prefix() {
    let found = hits(|l| TESTS.iter().map(|t| calls(l.bare, t)).sum());
    judge("A5: a string prefix test", &found, ALLOWED);
}
