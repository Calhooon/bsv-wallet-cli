//! Rule A1 (#73): the process environment is read in one place. Every
//! `env::var` (and `var_os`) of the shipped code lives in
//! `src/services_env.rs` or `src/config.rs` (the config module of #80, not
//! yet written); every other file that reads one today is listed with its
//! count, and #80 empties the list.

mod arch_support;

use arch_support::{count, hits, judge};

/// The files that may read the environment freely.
const HOMES: &[&str] = &["src/services_env.rs", "src/config.rs"];

/// `(file, today's reads, why)`. Only shrinks; #80 empties it.
const ALLOWED: &[(&str, usize, &str)] = &[
    ("src/bin/bsv-wallet-relay.rs", 5, "the relay binary's own settings (RELAY_DB, RELAY_PORT, BIND_ADDR, the admin token, RELAY_REQUIRE_REGISTER)"),
    ("src/broadcast_reconcile.rs", 2, "the reconciler's switch and its tuning reads"),
    ("src/broadcast_verify.rs", 6, "the broadcasters' URLs and keys and the verify attempts and delay"),
    ("src/commands/daemon.rs", 1, "ROOT_KEY"),
    ("src/commands/daemon_tenant.rs", 11, "the daemon's settings: intervals, TLS, tokens, MIN_UTXOS, the abandoned reconcile, the relay"),
    ("src/commands/serve.rs", 4, "TLS and the tokens of serve"),
    ("src/commands/serve_fleet.rs", 5, "each tenant's keys, TLS and the tokens of serve-fleet"),
    ("src/commands/tick.rs", 1, "ROOT_KEY"),
    ("src/context.rs", 1, "ROOT_KEY"),
    ("src/server/mod.rs", 2, "BIND_ADDR and AUTH_TOKEN"),
    ("src/spend_gate.rs", 1, "SPEND_OWN_CHANGE"),
    ("src/tracker_host.rs", 1, "the tracker host's tuning reads"),
];

#[test]
fn the_environment_is_read_only_in_the_config_module() {
    let found = hits(|l| {
        if HOMES.contains(&l.source.file.as_str()) {
            return 0;
        }
        count(l.bare, "env::var")
    });
    judge("A1: env reads outside the config module", &found, ALLOWED);
}
