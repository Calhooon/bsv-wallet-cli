# Architecture rules as tests (#73)

Six rules of where things may live in the shipped code, each a test under `tests/arch_*.rs` in the shape of `tests/rule_28_sites.rs`: a scan of the crate's own `src/` with an allowlist. Each allowlist names today's sites by file, with a count and a reason, so a rule passes on `beta` and refuses any new site. A count is exact: a file over its count fails with every line it holds, a file under it fails until the entry is lowered (or removed at zero) in the same PR. An allowlist only shrinks; it grows only with a reason in the PR that grows it.

`cargo test --locked` runs them; one rule alone: `cargo test --locked --test arch_no_unwrap`.

## What is scanned

`tests/arch_support/mod.rs`, shared by the six. Every `.rs` file under `src/`, minus test code: every item a `#[cfg(test)]` attribute carries (a `mod tests { .. }` block, a function, a statement) and every file such an attribute declares (`#[cfg(test)] mod x;`, at its `#[path]` when given; today `src/test_support.rs`, `src/tracker_words_witnesses.rs`, `src/chain_feed_tests.rs`, `src/spend_gate/tests.rs`, `src/server/handlers/tests.rs`). Comments are never read. The env, unwrap and spawn rules count calls with string and char literals blanked; the SQL, explorer and prefix rules read string literals. `the_scan_reads_only_the_shipped_code` (in `tests/arch_no_unwrap.rs`) pins the scanner. The crate root is cargo's `CARGO_MANIFEST_DIR` at run time, so a test binary reused from a shared target dir scans the checkout it runs in.

Not scanned: `mcp/` (its own crate) and `tests/`.

## The rules

| Rule | Test | Allowed whole | Allowlist today | Emptied by |
|---|---|---|---|---|
| A1 the environment is read in the config module (`env::var`, `var_os`) | `tests/arch_env_reads.rs` | `src/services_env.rs`, `src/config.rs` (not yet written) | 12 files, 40 reads | #80 |
| A2 SQL behind the storage boundary (an `sqlx` query call, or a statement keyword in a string literal) | `tests/arch_sql_sites.rs` | `src/store.rs` (and the toolbox) | 15 files, 273 sites | #89 |
| A3 explorers named only at their rungs (`whatsonchain`, `bitails`, `bananablocks`, any case, in code or a literal) | `tests/arch_explorer_sites.rs` | `src/explorers.rs`, `src/broadcast_verify.rs` | 6 files, 29 names | a later lane; the four printed links and `services_env.rs`'s env names may stay with their reasons |
| A4 no `unwrap()` / `expect(..)` | `tests/arch_no_unwrap.rs` | test code | 9 files, 9 sites | a later lane |
| A5 no error classified by a message prefix (`starts_with("`) | `tests/arch_no_prefix_errors.rs` | `AppError::classify` in `src/server/handlers.rs` | 1 file, 3 sites (`src/commands/ui.rs`, the inspector's read-only check, not an error) | #81 (`classify`) |
| A6 tasks start from the task table (`tokio::spawn`, `task::spawn`, `spawn_blocking`, `spawn_local`, `thread::spawn`) | `tests/arch_spawn_sites.rs` | `src/commands/daemon_tenant.rs`, `src/broadcast_reconcile.rs`, `src/server/mod.rs` | 3 files, 4 sites | a later lane |

Each allowlist, file by file with its reason, is the `ALLOWED` table at the top of its test.

## Notes

- A4's count is 9 where the 2026-10-23 brief counted 97. A text count that stops only at `#[cfg(test)] mod tests` gives 102 on this tree: it reads the `pub(crate) mod tests` bodies of `src/tracker_host.rs` (74) and `src/commands/receive.rs` (17), and the second test module of `src/server/mod.rs` (2), all test code. The brief's 97 is not reproduced here.
- A2 counts statements, so a subquery's `SELECT` counts too; a reflow of a statement does not change its count.
- A5 is a text rule: it cannot tell an error message from another string, so its allowlist says which each site is. A typed error removes the need (#81).
- Where a grep cannot say the rule (a prefix test through `strip_prefix`, a spawn through a helper), the rule waits for a dylint (#73).
