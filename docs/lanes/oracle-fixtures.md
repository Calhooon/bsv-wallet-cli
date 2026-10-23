# Lane oracle-fixtures: golden wire fixtures for every served endpoint (#69)

**Repository.** `bsv-wallet-cli`, your worktree on a branch from `origin/beta`. `docs/lanes/README.md` binds you. Three hours.

**Read in this order.** #69 and the epic #106 (the scoreboard, the loop). `CLAUDE.md` (the wire-compatibility constraint; the translation layer). `src/server/types.rs` (every request and response struct), `src/server/handlers.rs` (the 28 handlers, `AppError` and its bodies: 400, 401, 402 with `held`, 403, 404, 409 with `coin`, 500, 502, 503 `STORE_BUSY` with `Retry-After: 1`, 503 `HEADERS_UNAVAILABLE` with `Retry-After: 2`), `src/server/mod.rs` (the router, CORS, the bearer, the host routes), `tests/integration.rs` and `src/server/handlers/tests.rs` (how the router is driven without a network today). BRC-100 at the pin (`git -C ~/bsv/brcs show upstream/master:wallet/0100.md`) for the field names each shape must keep. `~/bsv/bsv-script-lean/harness/README.md` for the shape of a corpus that judges.

**The work.**
1. `tests/wire/`: one JSON fixture per served call and per refusal body, each a recorded request and the exact response (status, headers that matter, body), produced by a recorder that drives the router in-process against the toolbox's mock services and an in-memory store (as `src/server/handlers/tests.rs` does), with the fields that must differ (txids, keys, signatures, timestamps, references) replaced by stable placeholders through one normalizer. Every one of the 28 endpoints, the five host routes, and every `AppError` code at least once.
2. `cargo test --test wire`: replays every fixture and fails on any byte of difference after normalization, printing the diff. A `WIRE_RECORD=1` environment switch re-records (and nothing else re-records).
3. The CI job `wire` in `.github/workflows/ci.yml`, required on `beta` later by the captain; you add the job, not the protection.
4. `docs/WIRE.md`: how to read a fixture, how a behavior PR changes one on purpose in its own commit, what the normalizer hides and why.

**Bounds.** No change to any served shape: if a fixture reveals two handlers disagreeing with BRC-100 or with each other, record it as found and name it in the report; do not fix it here. Touch nothing under `src/` except a test-only hook if the recorder cannot drive a route without one, named in the report. `Cargo.toml` untouched (dev-dependencies are `oracle-benches`'s this wave; if you need one, say so and stop).

**Report.** `REPORT: STATUS done|blocked`, the base and tip commits, the fixture count by endpoint and by code, the commands and their tails, every disagreement found, the six tests each choice passes.
