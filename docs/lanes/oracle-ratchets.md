# Lane oracle-ratchets: the numbers that may only fall (#72)

**Repository.** `bsv-wallet-cli`, your worktree on a branch from `origin/beta`. `docs/lanes/README.md` binds you. Two hours.

**Read in this order.** #72 and the epic #106 (the code-health ratchet line of the scoreboard). The measurements of 2026-10-23 that seed the baseline: 33,791 source lines in 69 files; files over 800 lines: `src/tracker_host.rs` 3,247, `src/commands/cleanup_abandoned.rs` 2,893, `src/broadcast_verify.rs` 2,477, `src/spend_gate/tests.rs` 2,439, `src/broadcast_reconcile.rs` 1,793, `src/spend_gate.rs` 1,593, `src/server/handlers.rs` 1,337; unwrap/expect outside test code 97 (75 in `src/tracker_words_witnesses.rs`); 28 distinct `env::var` names. `~/bsv/bsv-stack-lean/docs/GATE-LINT.md` and `Makefile` for the house's gate-leg shape (a leg, a record).

**The work.**
1. `tools/ratchet` (a Rust bin under `tools/` with its own manifest, or a Python script; say why): measures clippy warnings at `-W clippy::pedantic` (count), unwrap/expect outside `#[cfg(test)]` and `tests.rs` files, files over 800 lines (count and the list), functions over 60 lines (count), cognitive complexity over 25 (clippy's lint, count), duplicated blocks (a simple token-window hash, 40 tokens; say the method), public items without docs (`cargo doc` warnings with `missing_docs` on, count). Output one JSON object.
2. `docs/ratchets.json`: the baseline, recorded from `origin/beta` at the lane's base commit, with the commit named inside.
3. `tools/ratchet --check`: measures the tree and fails when any number is above the baseline, printing which and by how much; `--record` rewrites the baseline, used only by a PR that says so (a PR that lowers a number re-records in its own commit).
4. The CI job `ratchet` in `.github/workflows/ci.yml`.
5. `docs/RATCHETS.md`: each number, how it is measured, why it may only fall, how a lane moves one.

**Bounds.** Measure only; fix nothing. No edit under `src/`. No edit to `Cargo.toml` (if a tool needs a crate, it lives in `tools/ratchet/Cargo.toml`, outside the workspace, and the report says so). Determinism: two runs on the same tree give the same JSON.

**Report.** `REPORT: STATUS done|blocked`, the base and tip commits, the baseline JSON in full, the commands and their tails, the six tests each choice passes.
