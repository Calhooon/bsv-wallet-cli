# The differential harness (#70)

The oracle of a refactor. `tools/differential` builds two versions of the binary, the base and the head, runs the hermetic e2e suite (`docs/E2E-HERMETIC.md`) once against each with the same wallets, ports and stub, records every call the scenarios make, and compares the two recordings byte for byte after a listed normalization. A refactor changes no output, so its differential is zero. The node's harness is the pattern (`bsv-script-lean/harness`): the base build is the oracle, the recorded suite is the corpus, the head is what is judged.

## Running it

From the checkout, with the cargo toolchain, Node 24 (22.21 works too) and `tests/e2e`'s `npm ci` done (scenario 20 needs Playwright's Chromium):

```bash
CARGO_TARGET_DIR=<your target dir> python3 tools/differential
```

That one command builds the base (the merge-base of `HEAD` and `origin/beta`) in a scratch worktree with its own target dir and the head from the tree, runs both, and prints the report. The variants:

```bash
tools/differential --base REV                    # the base at REV
tools/differential --base-bin B --head-bin H     # built binaries (CI); bsv-wallet-mcp is read beside each
tools/differential --same [--head-bin H]         # the floor: one binary run twice
tools/differential --compare R1 R2               # diff two recordings already made
tools/differential --exclusions                  # the normalization list with its reasons
tools/differential --out DIR                     # where the recordings, logs and report go
tools/differential --base-target DIR             # the base build's CARGO_TARGET_DIR
```

Scratch (the base's worktree, its target dir unless `--base-target` names one, the default `--out`) goes under `$TMPDIR/bsv-wallet-differential/`, or `~/bsv/targets/bsv-wallet-differential/` when `$TMPDIR` is unset or `/tmp`. A lane keeps its runs under `/Volumes/Lanes/out/bsv-wallet-cli/` with `--out`. Two runs take about 15 seconds after the builds. The ports are the hermetic run's own (3370 to 3373, the stub on 3380), never a port of the registry's.

Exit 0 on zero differences, 1 on differences, 2 when a build, a path or a suite run failed (a failed suite's differences are no verdict), 3 when a recording is not a whole run. The tool fails closed: a recording is incomplete when it has no events, no wallet event (or none for a seat the setup starts: the funder, A, B), no HTTP calls or no commands, a line that is not a JSON event (a truncated file), a missing `setup` or `sweep` phase, a scenario phase count other than the suite's 24 (read from `tests/e2e/scenarios`) or a phase the suite lacks; and a pair is incomplete when a phase holds a different number of calls and commands in the two. The report names each gap on an `INCOMPLETE:` line and its last line reads `DIFFERENTIAL: INCOMPLETE recording (...)`; two empty recordings are never zero differences.

## What is recorded

Under `DIFF_RECORD=<file>` the harness writes one JSON line per event (`tests/e2e/lib/record.js`):

- every HTTP call a `WalletClient` makes (`tests/e2e/lib/wallet-client.js`): the seat (A, B, C, funder), the method, the path, the request body, the status and the response body;
- every command of the wallet binary a scenario or `setup.js` runs through `execSync`: the seat, the argv, the exit code, stdout and stderr;
- each wallet's directory, port and identity key once, as a `wallet` event;
- at the end of each phase, a `stub` event: the calls the hermetic stub answered in that phase, by provider and route with its status (`woc` `GET /tx/{id}/hex 200: 3`), so a refactor of the explorer ladder that asks another rung, or asks one more often, is a difference;
- each with its phase (`setup`, the scenario's name, `sweep`) and a sequence number taken when the call starts.

Not recorded: scenario 20's fetches of the inspector UI and its browser, scenario 21's JSON-RPC to the MCP server (the MCP server's own HTTP calls to the wallet included), the `serve` logs, and the stub's requests and answers themselves (only their counts, above). Those scenarios still run and must pass in both runs.

Under `DIFF_RECORD` the harness's HTTP calls run one at a time in the order the scenarios issue them. Scenario 10 fires five sends together; they would otherwise reach the wallet's spending lock in the scheduler's order, and every coin graph after them would follow that order. The race itself is the plain hermetic run's to test, which records nothing.

**The same wallets.** `DIFF_ROOT_KEYS=<A>,<B>,<C>[,<funder>]` (64-digit hex each) makes `setup.js` pass the seat's key to `bsv-wallet init --key` instead of drawing one. The tool draws four keys once per comparison and gives both runs the same ones. The variable is refused outside `E2E_HERMETIC=1`, so a known key never holds mainnet sats. The harness also sets `E2E_RNG_SEED` for the wallet and storage streams (toolbox #106), with a distinct repeatable seed for each CLI invocation so reopening a store cannot repeat its server's action references.

**No recording holds a key.** `record.js` writes every key of `DIFF_ROOT_KEYS` as its seat's name, `<root-key:A>` (so `init`'s argv is `--key <root-key:A>`), in every line and in the error a failed command throws; the tool refuses a run whose recording or log holds one (exit 2), and `tools/test_differential.py` greps a recording for each key. A failed run keeps the wallets' directories under `--out` (`base-wallets/`, `head-wallets/`: each `.env` holds `ROOT_KEY`); the CI job's upload leaves them out and carries the report, the recordings and the run logs only.

The harness, the scenarios, the stub and its fixtures are the head's tree for both runs: only the binary differs.

## The exclusions

`tools/differential --exclusions` prints the list: each field path the tool normalizes, and why two runs of the same code legitimately differ there. The report repeats it at its top. This page does not copy the list, so it cannot drift from the code.

The rule for the list: a field goes on it only with evidence from the floor (`--same`: one binary twice, same keys, same stub), and the reason names the source of the difference in the code. A txid or a signature is never excluded as such. The binary's randomness is the toolbox's `thread_rng` (change derivations, references) and BRC-2's per-call IV. The tool parses every transaction in the recording and renames the atoms one run holds alone to `<rand:N/LB>`, numbered in order of appearance, `L` the atom's decoded byte length (a reference, a derivation and a ciphertext as well as a txid or a key; a signature's DER body is `<rand:N/der>`), so an atom that grows or shrinks is a difference, so two equal normalized recordings are a consistent bijection of their random atoms. An input's script is split into its pushes: only a signature's DER body and a push of 20 bytes or more (a public key) are atoms; the push count, each push's opcode, each signature's sighash byte and its strict DER form (BIP 66, low-S) are compared, so a build signing with sighash `0xC1` against a `0x41` preimage is a difference. A renamed signature's length byte is written `der+1`: the DER length of a random r and s is the draw's (70 to 72 bytes), and with the literal byte the floor of 2026-10-23 showed 87 differences in 11 phases instead of 13. Every amount, count, order, script template and deterministic value is still compared byte for byte.

## The beta.1 floor

Toolbox 0.14.0-beta.1 ends equal-coin selection on output_id (#105) and
provides seeded storage draws (#106). References, derivationPrefix and
derivationSuffix are now compared literally. Auth nonces, manager salts,
monitor jitter and certificate issuance nonces remain outside that seed
(toolbox #191); BRC-2 IVs remain the SDK's draw. `--exclusions` names those
sources. The pin's floor reads `0 differences over 359 calls and 35 commands`, with
13 unshared atoms per run still renamed (BRC-2 ciphertexts). Both runs pass
24/24. Any remaining difference is reviewed, never hidden by adding an exclusion.

A pin move uses the re-record reading: the base and head intentionally differ,
so a non-refactor PR reports the difference as a warning under the job's
existing exit mapping. A failed build, failed suite or incomplete recording
still fails. Each re-recorded fixture and reason belongs in the pin PR.

## The label rule and CI

The CI job `differential` (`.github/workflows/ci.yml`) runs on every pull request. It builds the PR's base branch at its tip and the PR's head with the pinned toolchain 1.98.1, runs `tools/differential --base-bin ... --head-bin ...`, writes the `DIFFERENTIAL:` line to the run's summary, and uploads the report and both recordings as the artifact `differential`.

- A PR carrying the label `refactor` fails on any difference. `REFACTOR_FAILS: 'true'` is enabled with this pin: toolbox #105 and #106 are in the binary and its same-binary floor is zero. A failed or incomplete suite is never a comparison verdict.
- Any other PR passes and reports. A feature changes outputs by design; its differences are its review's reading, and a behaviour change changes its fixture in its own commit (docs/lanes/README.md, rule 5).
- A failed build or suite run, or an incomplete recording (exit 3), fails the job either way.

The labels are read when the job's differential step runs (`gh pr view --json labels`, with `pull-requests: read`), not from the event payload, which a re-run reuses: a label added or removed after the run started counts on a re-run of the job. A label change does not start a run by itself (`labeled` and `unlabeled` are not among the workflow's triggers: they would re-run, and cancel, every job of CI); a failed read of the labels fails the job. The job is not a required check yet; the captain decides after it has run a week.

## The exit test

A refactor PR carries the report with zero differences: `DIFFERENTIAL: 0 differences over N calls and M commands`. A lane cannot mark a refactor done without it. The pin records the pair: the head against its base and the floor (`--same`) on the head, both `DIFFERENTIAL:` lines in the REPORT; a difference the floor does not show (an earlier scenario, another field than the coin choice's) is the refactor's.
