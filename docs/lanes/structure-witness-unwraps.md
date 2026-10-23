# Lane structure-witness-unwraps: the witness file's unwraps, judged and fenced (#85)

**Repository.** `bsv-wallet-cli`, your worktree on a branch from `origin/beta`. `docs/lanes/README.md` binds you. Two hours.

**Read in this order.** #85 and the epic #106. The lanes' findings of 2026-10-23: `src/tracker_words_witnesses.rs` holds 75 unwrap/expect calls and `src/test_support.rs` 13; the ratchet lane (#72) and the lints lane (#73) both found those two files are compiled only for tests (`#[cfg(test)] mod` declarations), so the shipped count is 9 in `src` and 24 in `mcp/src/main.rs`. Read `src/lib.rs` and `src/main.rs` for how the two files are declared, `src/tracker_words_witnesses.rs` itself (what the witnesses are: the stack's witness scenarios run as tests), `tests/arch_no_unwrap.rs` on `lane/oracle-lints` (PR 111) for the rule's allowlist, `docs/ratchets.json` on `lane/oracle-ratchets` (PR 112) for the baseline.

**The work.**
1. Confirm, by reading the declarations and by a build with `cargo build --release` plus `nm` or the binary's symbols, that nothing of `tracker_words_witnesses.rs` and `test_support.rs` reaches the release binary. Record the command and its tail.
2. The 9 shipped unwraps in `src` (the lints lane's A4 allowlist names the files) and the 24 in `mcp/src/main.rs`: judge each. One that can only fail on a programming error stays as `expect("<why it cannot fail>")` with the reason in the string; one that can fail on input or state becomes an error the caller sees, with the smallest change. List each with its line and your verdict.
3. The MCP crate's 24: the same judgment; the MCP's answers are JSON to a tool caller, so a panic there is a lost call. Keep the change to `mcp/src/main.rs` and its tests; the crate's version does not move (the captain decides).
4. `tests/arch_no_unwrap.rs`'s allowlist shrinks to what remains, in your commit, if PR 111 has merged into beta by the time you land; if not, say so and leave the allowlist to the captain.

**Bounds.** No behaviour change beyond a panic becoming an error; the wire fixtures (if PR for #69 has merged) show zero diff, else the existing tests do. No edit to `Cargo.toml`. Two hours.

**Report.** `REPORT: STATUS done|blocked`, the base and tip commits, the symbol check's tail, the per-site table, the commands and their tails, the six tests each choice passes.
