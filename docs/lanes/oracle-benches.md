# Lane oracle-benches: criterion benches with a stored baseline; CI fails a regression (#71)

**Repository.** `bsv-wallet-cli`, your worktree on a branch from `origin/beta`. `docs/lanes/README.md` binds you. Three hours.

**Read in this order.** #71 and the epic #106 (the scoreboard's latency lines are read off these benches and nowhere else). `src/spend_gate.rs` (the door: `assess`, `create_action`; the header reads through `tracker_host::read_creators`; the lock is the handler's, `src/server/mod.rs` `SpendingLock`), `src/spend_gate/tests.rs` (`storage_for`, `coin`, `mined_coin`, `headers_with`: the fixtures a bench reuses), `src/server/handlers.rs` `create_action` and `internalize_action`, `list_outputs`. `~/bsv/bsv-rs/benches/` for the house's criterion shape.

**The work.**
1. `benches/` with criterion: `gate_assess` (a wallet of 1,000 and of 10,000 coins, mined and pending mixed, the mock header service answering; `assess` with `need` and with `None`), `create_action_signed` (the door to a signed transaction with `noSend`, no broadcast, the lock not held by anyone else), `internalize_action` (one BRC-29 payment with a stored proof), `list_outputs` (10,000 coins, limit 100). Each reports p50 and p99 over enough samples to be stable (say what you chose and why).
2. `benches/baseline.json`: the numbers on the machine the lane ran on, with the machine named by its model and core count only. A script `tools/bench-compare` (shell or a small Rust bin) that runs the benches and fails when any p99 is more than 5 percent above the baseline; a `--record` mode that rewrites the baseline, used only by a PR that says so.
3. The CI job `bench` running `tools/bench-compare` on the Linux runner against a Linux baseline you record on that runner (one PR run to record, the next to check; say in the report if the runner's noise needs a wider margin, with the numbers).
4. The scoreboard table in #106: the measured p50 and p99 at 1,000 and 10,000 coins, as a comment on the issue in the report's words; the captain moves them into the epic.

**Bounds.** You are the one lane that edits `Cargo.toml` this wave: `criterion` as a dev-dependency and the `[[bench]]` entries, nothing else; `Cargo.lock` with it, `--locked` green after. No change under `src/` except a `pub` the bench needs, named in the report. No network: the mock services only.

**Report.** `REPORT: STATUS done|blocked`, the base and tip commits, the four benches' p50 and p99 at both sizes, the commands and their tails, the baseline file's machine line, the six tests each choice passes.
