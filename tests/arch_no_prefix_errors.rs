//! Rule A5 (#73): an error is classified by its type, never by the prefix
//! of its message. `starts_with(` and `strip_prefix(` in the shipped code,
//! whatever the argument (a literal, a const, a UFCS `str::starts_with(s,
//! ..)`), are counted file by file, each file that holds one with its count
//! and why. No error site is exempt since #330; the served layer's
//! own rule is A8 (`tests/arch_no_error_words.rs`). A text
//! rule cannot tell an error message from a header scheme or a byte
//! prefix, so each reason says which its sites are.

mod arch_support;

use arch_support::{calls_any, Rule};

/// The prefix tests.
const TESTS: &[&str] = &["starts_with", "strip_prefix"];

/// `(file, today's count, why)`. Only shrinks.
const ALLOWED: &[(&str, usize, &str)] = &[
    (
        "src/chain_feed.rs",
        2,
        "the SSE reader's field split (a `:` comment line, the space after a field name), not an error",
    ),
    (
        "src/commands/ui.rs",
        1,
        "the inspector's courtesy statement check (SELECT, WITH, PRAGMA, EXPLAIN; the read-only pool is the guard, #440), not an error",
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
        "src/server/mod.rs",
        2,
        "the Authorization header's `Bearer ` scheme, not an error",
    ),
    (
        "src/store/origin_grants/power.rs",
        7,
        "a power's names (#414, the design's W17): the reference's admin-reserved names (`admin` and `p ` first, WalletPermissionsManager.ts 1509-1511, 1589, 1806-1808) and the label and linkage name forms of a key, never an error's text",
    ),

];

const RULE: Rule = Rule {
    name: "A5: a string prefix test",
    homes: &[],
    allowed: ALLOWED,
    sites: |l| calls_any(l, TESTS),
};

#[test]
fn no_error_is_classified_by_its_prefix() {
    RULE.check();
}
