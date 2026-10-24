# The benches: the hot path's p50 and p99 (#71)

Four criterion benches of the wallet's hot path, on a file-backed SQLite wallet of 1,000 and 10,000 coins (seven in ten mined, two own change unmined, one received unmined) and the toolbox's mock services; nothing reaches the network.

| bench | what one call is |
|---|---|
| `gate_assess` | the spend gate's assessment, for a need and for all |
| `create_action_signed` | the lock, the spend guard, the gate's door, a signed noSend action, aborted after the clock |
| `internalize_action` | one BRC-29 payment with its proof, and the post word |
| `list_outputs` | a list of 100 |

Each call is timed on its own; p50 and p99 are over the calls of criterion's measurement samples (its warm-up calls are dropped), one JSON per bench id. `tools/bench-compare` runs them and compares each p99 with a baseline in `benches/baseline.json`.

```bash
tools/bench-compare                    # run, compare with the profile's entry; no entry fails
tools/bench-compare --record           # run, write the profile's entry; refuses a dirty tree
tools/bench-compare --record-missing   # compare, or record when the profile has no entry
tools/bench-compare --base B           # the reference is B's entry; also fails a tree entry above B's
tools/bench-compare --out F            # write the recorded (or, on a compare, the candidate) baseline to F
tools/bench-compare --compare-only D   # compare the JSON already in D, run nothing
```

## The gate is the CI runner's, never a developer machine's

On 2026-10-23 four lanes tried to record a baseline on the development Mac and every one found p99 swinging by hundreds of percent between back-to-back runs under the other lanes' load. A p99 is a tail, and a shared machine's tail is its neighbours'. The decision: the gate's baseline is the Linux CI runner's.

**Profiles.** Every entry of `benches/baseline.json` is keyed by a runner profile, under `profiles`. The CI job `bench` sets `BENCH_PROFILE=ubuntu-latest`; that entry is the gate (`"gate": true`), and `--record` writes a gate profile on a CI runner only (`GITHUB_ACTIONS=true`). With `BENCH_PROFILE` unset the profile is `dev-` and the CPU model and core count (for example `dev-apple-m1-max-10-cores`), `"gate": false`: a developer's reading. A laptop entry is a reading, not a gate. `tools/bench-compare` on a developer machine prints the comparison with its own entry and exits 0 whatever the numbers; CI reads its own profile only and never compares a developer's entry. A `BENCH_PROFILE` beginning `dev-` is refused.

**An entry** names what it measured: `profile`, `gate`, `machine` (CPU model and cores), `toolchain` (`rustc -V`), `commit` (`git rev-parse HEAD`), `recorded` (the date), the optional `margin`, and `benches` (each id's `p50_us`, `p99_us` and `calls`). `--record` refuses a dirty tree before it runs anything, so an entry is always the measure of the commit it names. The CI job pins the toolchain (1.98.1, the ratchet job's): a new compiler moves the numbers with no change to the tree. A compare whose entry names another toolchain warns, and the profile is re-recorded on the new pin.

## The CI job

1. **The reference is the base's baseline**, read as the ratchet job reads its record: `git show origin/<base>:benches/baseline.json` on a PR, the commit before it on a push, never the PR's own file (a PR that edited the baseline would pass itself). A PR may lower the base's baseline, never raise it: the job fails when the PR's entry has a p99 above the base's. A base with no entry for the profile (the PR that commits the profile's first entry) compares against the tree's own entry.
2. **No entry for the profile** in either: the job runs `--record-missing`, compares nothing, writes the measured baseline (the tree's file with this run's entry) to `$RUNNER_TEMP/bench/baseline.json`, uploads it with the per-bench JSON as the artifact `bench-baseline`, prints a notice that the captain commits it, and exits 0.
3. **An entry exists**: each p99 more than the margin (5 percent, `margin` at the top of the file or on the entry) above the reference's fails the job, which prints the bench, both numbers and the change. A bench of the entry not measured fails; a bench measured and not in the entry is named. The run also leaves its candidate baseline in the artifact `bench-baseline`.

**The captain commits a baseline** from a run's artifact `bench-baseline` (its `baseline.json` as `benches/baseline.json`), in a PR that says so: the profile's first entry; a lowering after a PR made the hot path faster; a re-record after the toolchain pin moved. A PR never commits a gate entry measured anywhere but the job.

**A PR that changes a bench** (its fixtures, its call, its sizes) records the reason in its commit message: the numbers before it are of another bench, and the baseline is re-recorded from that PR's artifact in its own commit, saying so.
