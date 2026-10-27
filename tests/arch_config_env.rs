//! Rule A7 (#80): no environment read outside `src/config.rs`, and in the
//! MCP companion outside `mcp/src/config.rs` (#171). The configuration is
//! loaded once at start (`Config::load`, and `RelaySettings::load` for the
//! relay binary; the MCP's own `Config::load`) and everything after reads
//! the `Config` it was given: a command, the server, the spend gate per
//! request, an MCP tool. The allowlist is empty and stays so.
//!
//! A read is a call, by any path, as a method or bare after a `use`, of a
//! door to the process environment: `std::env`'s `var(`, `var_os(`,
//! `vars(` and `vars_os(`, and `tracing_subscriber`'s
//! `from_default_env(` and `try_from_default_env(` (which read `RUST_LOG`);
//! and the `use` that brings one of those names in. One site each. The
//! compile-time `env!` and `option_env!` read the build's environment, not
//! the process's, and are not counted.

mod arch_support;

use arch_support::{calls_any, imports, Rule};

/// The one file of each crate that reads the environment.
const HOMES: &[&str] = &["src/config.rs", "mcp/src/config.rs"];

/// The doors to the environment, by name.
const READS: &[&str] = &[
    "var",
    "var_os",
    "vars",
    "vars_os",
    "from_default_env",
    "try_from_default_env",
];

/// `(file, today's reads, why)`. Empty at landing (#80); never grows. Kept
/// open over two lines, as `tools/scoreboard` reads an allowlist.
#[rustfmt::skip]
const ALLOWED: &[(&str, usize, &str)] = &[
];

const RULE: Rule = Rule {
    name: "A7: no env read outside src/config.rs and mcp/src/config.rs",
    homes: HOMES,
    allowed: ALLOWED,
    sites: |l| calls_any(l, READS) + imports(l, READS),
};

#[test]
fn the_environment_is_read_only_in_the_config_modules() {
    RULE.check();
}
