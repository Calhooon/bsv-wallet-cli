# Lane oracle-lints: architecture rules as tests (#73)

**Repository.** `bsv-wallet-cli`, your worktree on a branch from `origin/beta`. `docs/lanes/README.md` binds you. Two hours.

**Read in this order.** #73 and the epic #106. `tests/rule_28_sites.rs` (the one architecture test the tree has: where an explorer request may live; your tests take its shape), `CLAUDE.md` (the explorer ladder rule, the gate reads its own records), `src/explorers.rs`, `src/services_env.rs` (the env reads that will become the config module's, #80), `src/server/handlers.rs` `AppError::classify` (the prefix table #81 removes), `src/spend_gate.rs` (the raw SQL #89 moves). The 2026-10-23 counts: 28 `env::var` names; 97 unwrap/expect outside test code; SQL in `src/spend_gate.rs`, `src/tracker_host.rs`, the repair passes.

**The work.** One test file per rule under `tests/arch_*.rs`, each a source scan with an allowlist that names today's violations by file so the rule passes on `beta` now and refuses any new site; the allowlist shrinks as later lanes land (#80, #81, #89), never grows without a reason in the PR:
1. `arch_env_reads.rs`: `std::env::var` only in `src/services_env.rs` and `src/config.rs` (the latter does not exist yet; the allowlist names today's other sites).
2. `arch_sql_sites.rs`: `sqlx::query` and raw SQL strings only under a storage boundary you name (today: the allowlist is the list of files that hold them).
3. `arch_explorer_sites.rs`: the rule 28 test extended to the toolbox's providers by name (`WhatsOnChain`, `Bitails`, `BananaBlocks`, their base URLs) outside `src/explorers.rs` and `src/broadcast_verify.rs`.
4. `arch_no_unwrap.rs`: unwrap/expect outside `#[cfg(test)]` modules and `tests.rs` files, allowlisted by file with today's counts as ceilings.
5. `arch_no_prefix_errors.rs`: `starts_with("` on an error message outside `AppError::classify`, which is allowlisted whole until #81.
6. `arch_spawn_sites.rs`: `tokio::spawn` only in the daemon's task table files (name them from `src/commands/daemon_tenant.rs`, `src/broadcast_reconcile.rs`, `src/server/mod.rs`).
Each test prints the offending file and line. `docs/ARCH-RULES.md` lists the six rules, their allowlists and the issue that empties each.

**Bounds.** Tests and the document only. No edit under `src/`, no edit to `Cargo.toml`, no workflow edit (`cargo test --locked` already runs `tests/`). A planted violation in a scratch copy must fail the test; show it in the report.

**Report.** `REPORT: STATUS done|blocked`, the base and tip commits, each rule with its allowlist size, the planted-violation runs, the commands and their tails, the six tests each choice passes.
