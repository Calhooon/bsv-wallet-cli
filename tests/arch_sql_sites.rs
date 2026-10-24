//! Rule A2 (#73): SQL lives behind the storage boundary. The boundary is
//! `src/store.rs` (the crate's own tables beside the toolbox's) and the
//! toolbox itself; every other file that holds an `sqlx` query call (by
//! path, `sqlx::query(`, or bare after a `use sqlx::query`, and that `use`
//! itself) or an SQL statement in a string literal is listed with its
//! count, and #89 moves them behind the boundary.

mod arch_support;

use arch_support::{calls, count, imports, Line, Rule};

/// The storage boundary.
const BOUNDARY: &[&str] = &["src/store.rs"];

/// The words that open an SQL statement, as they appear in a literal.
const STATEMENTS: &[&str] = &[
    "SELECT ",
    "INSERT ",
    "UPDATE ",
    "DELETE FROM",
    "CREATE TABLE",
    "CREATE INDEX",
    "CREATE UNIQUE INDEX",
    "ALTER TABLE",
    "DROP TABLE",
    "PRAGMA ",
];

/// The `sqlx` functions and macros that run a statement, by name: by path
/// (`sqlx::query(`) or bare (after a `use sqlx::query`). A method of the
/// same name (`.query(`, a request builder's) is not one of them.
const CALLS: &[&str] = &[
    "query(",
    "query_as(",
    "query_as::<",
    "query_scalar(",
    "query_scalar::<",
    "raw_sql(",
    "query!(",
    "query_as!(",
    "query_scalar!(",
];

/// The same functions as names a `use` brings in.
const NAMES: &[&str] = &["query", "query_as", "query_scalar", "raw_sql"];

/// `(file, today's SQL sites, why)`. Only shrinks; #89 empties it.
const ALLOWED: &[(&str, usize, &str)] = &[
    (
        "src/broadcast_reconcile.rs",
        9,
        "the reconciler's reads of the toolbox's tables",
    ),
    (
        "src/chain_feed.rs",
        5,
        "the feed's reorg marks on proven rows",
    ),
    ("src/commands/balance.rs", 3, "the balance's direct sums"),
    ("src/commands/cleanup_abandoned.rs", 28, "a repair pass"),
    ("src/commands/reconcile_outputs.rs", 14, "a repair pass"),
    (
        "src/commands/ui.rs",
        29,
        "the database inspector, which runs read-only SQL by design",
    ),
    (
        "src/relay.rs",
        16,
        "the relay binary's own database, not the wallet's",
    ),
    (
        "src/respend.rs",
        44,
        "the deliberate competitor's reads and writes",
    ),
    ("src/retire_guard.rs", 12, "the retire guard's reads"),
    (
        "src/server/broadcast_follow_up.rs",
        2,
        "the follow-up's request history",
    ),
    ("src/server/mod.rs", 2, "the server's own audit reads"),
    (
        "src/server/post_word.rs",
        2,
        "the post word's request history",
    ),
    (
        "src/spend_gate.rs",
        28,
        "the spend gate's reads of its own proof records (#89 moves them)",
    ),
    ("src/spend_guard.rs", 17, "the spend guard's reads"),
    (
        "src/tracker_host.rs",
        62,
        "the tracker host's reads and writes",
    ),
];

/// The statements in the line's string literals, its `sqlx` calls (not a
/// method of the same name), and the `use` that brings one in.
fn sql_sites(l: &Line) -> usize {
    let strings = l.strings();
    let statements: usize = STATEMENTS.iter().map(|w| count(&strings, w)).sum();
    let calls: usize = CALLS
        .iter()
        .map(|c| calls(l.bare, c).saturating_sub(count(l.bare, &format!(".{c}"))))
        .sum();
    statements + calls + imports(l, NAMES)
}

const RULE: Rule = Rule {
    name: "A2: SQL outside the storage boundary",
    homes: BOUNDARY,
    allowed: ALLOWED,
    sites: sql_sites,
};

#[test]
fn sql_lives_behind_the_storage_boundary() {
    RULE.check();
}
