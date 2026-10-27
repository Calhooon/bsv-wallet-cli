//! Rule A1 (#73): the process environment is read in one place. Every
//! `env::var` and `env::var_os` of the shipped code lives in
//! `src/config.rs` (#80 emptied the list; `src/services_env.rs`, the home
//! before it, now builds the services from the configuration and reads
//! nothing). A7 (`tests/arch_config_env.rs`) holds the same home for every
//! door to the environment; this rule stays as the first one's record.
//!
//! A read is a call of `var(` or `var_os(` by any path (`std::env::var(`,
//! `env::var_os(`, `e::var(`) or bare (after a `use std::env::var`), and the
//! `use` that brings the name in (`use std::env::var`, `use std::env::{var
//! as v}`), one site each.

mod arch_support;

use arch_support::{calls_any, imports, Rule};

/// The files that may read the environment freely: each crate's config
/// module (#171 added the MCP companion's).
const HOMES: &[&str] = &["mcp/src/config.rs", "src/config.rs"];

/// The readers, by name.
const READS: &[&str] = &["var", "var_os"];

/// `(file, today's reads, why)`. Emptied by #80; stays empty. Kept open over
/// two lines, as `tools/scoreboard` reads an allowlist.
#[rustfmt::skip]
const ALLOWED: &[(&str, usize, &str)] = &[
];

const RULE: Rule = Rule {
    name: "A1: env reads outside the config module",
    homes: HOMES,
    allowed: ALLOWED,
    sites: |l| calls_any(l, READS) + imports(l, READS),
};

#[test]
fn the_environment_is_read_only_in_the_config_module() {
    RULE.check();
}
