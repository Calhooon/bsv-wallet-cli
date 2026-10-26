# NORTH STAR: bsv-wallet-cli

**Born 2026-10-23 (#77, under the epic #106).** The WHY and the standard every lane of the programme is measured against, the scoreboard CI keeps true, and the queue a lane takes its next item from. Numbers live in the records the table below is rendered from; this file carries pointers, never restatements. The shape is the stack's own north star (`~/bsv/bsv-stack-lean/NORTH-STAR.md` at d6dd2bf: the WHY, the standard, the pointers).

## 1. Why

The wallet is one of the stack's nine machines; its definition is the stack's wallet charter with its Lean specification (`~/bsv/bsv-stack-lean/docs/charters/wallet.md`, `lean/Wallet.lean`), and this binary is a host of that machine. The programme (#106, the owner's word of 2026-10-23: "sky is the limit") makes the host fast, large and self-improving: a spend in milliseconds from a wallet of a hundred thousand coins, a thousand tenants in one process, proven-coin spends that never stall when every outside service is down, while it changes hosts and speed and never the machine. Speed without proof is how a wallet loses money, so nothing autonomous is safe before the oracle (Phase 0): the gate that proves, on every change, that nothing broke. Every design choice is held to the stack's six tests, T1 to T6 (`~/bsv/bsv-stack-lean/docs/BSV-NORTH-STAR.md` lines 25 to 36; the wallet's own weighing in `docs/charters/wallet.md` section 8, line 165; a lane names the tests its choices pass, `~/bsv/bsv-stack-lean/docs/lanes/l2-README.md` line 6 at d6dd2bf). They are cited here, not restated.

## 2. The standard

**The loop** (#106, "The loop"): a ranked research queue in the repo (section 6). A code lane takes one item into its own worktree and returns three things: **the change, the proof, and the nothing-changed evidence** (the differential report, the bench delta, the fixtures' diff). **A review lane on a different model reads the diff cold.** Small PRs, **one concern each**; refactors never mixed with behavior. Nightly: mutation, fuzz, bench. Quarterly: the scoreboard re-baselined upward. LOW's fleet is the canary on the previous version until a release has soaked a day; rollback is the target symlink and the tag. The lanes' shared rules are [lanes/README.md](lanes/README.md).

**The blast-radius classes** (#77, as the epic #106 gives them):

| class | where the epic names it | what lands it |
|---|---|---|
| cold path | Phase 1, "structure (cold paths)" | green CI ("Cold-path PRs merge on green CI") |
| hot path | Phase 2, "the spend path (hot; differential proof, mutation floor, adversarial review, the mainnet gate)" | the full proof: the differential proof (#70), the mutation floor (#75), an adversarial review, and the mainnet gate |

The epic's own words for the two classes (#106, "The loop"): "Cold-path PRs merge on green CI; hot-path PRs need the full proof." The epic classes no other phase; a PR of Phases 3 to 6 is classed at landing by the captain.

**The captain's addition** (2026-10-23, not the epic's): a PR of any phase that touches the spend path is hot. The reason: #78 (Phase 1, cold by its phase) rewrites the spend path, and its exit test already demands the differential proof, so the class follows what a PR touches and not only the phase it sits in.

**Never without the owner's word** (#106, "Never without the owner's word"): the wire contract, spend semantics, crypto, the pins, environment variable names, the database schema, any release, any spend from a wallet. An optimization that would change a BRC-100 shape is a named deviation with its measurement, landed only on the owner's word and recorded in the extensions document (#106, "BRC-100"; #100).

**The lines** (#106, "The lines"; #107): the programme lands on `beta`; `main` is the stable line; "`main` merges into `beta` after every stable patch; `beta` merges into `main` at a phase exit as the next minor, after the mainnet gate and a day of canary on a `vX.Y.0-beta.N` prerelease tag."

## 3. The phases

Each line is the phase's issues and its exit test. The epic gives no exit line per phase; each line below gathers the exit tests of the phase's issues (each issue carries its own **Exit test**), and the phase exits when all of them hold.

| phase | issues | exit test |
|---|---|---|
| 0, the oracle | #107 #110 #69 #70 #71 #72 #73 #74 #75 #76 #77 #100 | a refactor PR carries a zero-difference differential report, CI renders and checks every row of the scoreboard, and every lane brief cites its queue line |
| 1, structure (cold) | #78 #79 #80 #81 #82 #83 #84 #85 #101 | the env and prefix lints pass with empty allowlists, the three largest files are under 800 lines, and the HTTP handlers and CLI call `Engine` only with zero differential change |
| 2, the spend path (hot) | #86 #87 #88 #89 | the bench shows no HTTP call in the spend path and p99 at 10,000 coins under the target, with the wire fixtures unchanged and the mainnet gate passed |
| 3, throughput | #90 #91 #92 #93 | sustained spends per second at the target with the outbox draining, proof lag within one block for 99 percent, BEEF p99 under 10 KB at depth at most 3 |
| 4, fleet | #94 #95 #96 | the 1,000-tenant load test on one machine keeps the door's p99 within the target, with one feed subscription and one explorer budget per process |
| 5, hardening | #97 #98 #99 | the chaos suite runs on every PR, two builds of a tag are identical and signed with an SBOM, and RELEASING.md carries the canary and rollback policy |
| 6, the engine API and the desktop app | #102 #103 #104 #105 | `bsv-wallet-core` builds with no axum and no clap, and the desktop app and both scenario drivers pass the same scenarios |

The milestones hold each phase's later issues beside these (`gh issue list --milestone "Phase 1: structure"`); the queue below ranks them all.

## 4. The scoreboard

The targets are the epic's table (#106, "The scoreboard"); a target joins this table as a row when its measure exists in the tree (the bench baseline of #71, the load test of #96). The rows below are the records as they stand, rendered by [tools/scoreboard](../tools/scoreboard) and never edited by hand: a PR that moves a record (re-records [ratchets.json](ratchets.json), adds a fixture, moves a floor in [nightly.yml](../.github/workflows/nightly.yml)) runs `python3 tools/scoreboard --render` and commits the table with it; the CI job `ratchet` runs `--check` and fails on a stale table, and on any number that moved the wrong way from the base ref's tree (`--base`: no floor, fixture count, scenario count or rule count may fall, and no allowlist may grow). A record that is absent renders as a row that says so; a record that is present and does not parse fails the job, naming its file and field. The architecture rows count the `ALLOWED` literals of `tests/arch_*.rs` themselves, never a doc's prose. The measured column of the floors is `docs/scoreboard/mutants.json`, written by the captain from the nightly's "Mutation score" summary in the shape the tool's header defines; until it exists every floor reads "unmeasured". How each number is counted: [RATCHETS.md](RATCHETS.md), [WIRE.md](WIRE.md), [NIGHTLY.md](NIGHTLY.md).

<!-- scoreboard:begin -->
| row | value | measured | record |
|---|---:|---|---|
| ratchet `clippy_pedantic` | 834 |  | `docs/ratchets.json` at c462da4 |
| ratchet `unwrap_expect_outside_tests` | 6 |  | `docs/ratchets.json` at c462da4 |
| ratchet `files_over_800_lines` | 10 |  | `docs/ratchets.json` at c462da4 |
| ratchet `tests_files_over_800_lines` | 3 |  | `docs/ratchets.json` at c462da4 |
| ratchet `functions_over_60_lines` | 89 |  | `docs/ratchets.json` at c462da4 |
| ratchet `cognitive_complexity_over_25` | 19 |  | `docs/ratchets.json` at c462da4 |
| ratchet `duplicated_tokens` | 32643 |  | `docs/ratchets.json` at c462da4 |
| ratchet `missing_docs` | 150 |  | `docs/ratchets.json` at c462da4 |
| wire fixtures | 127 |  | `tests/wire/MANIFEST.json` |
| consumers' fixtures | 48 over 7 consumers |  | `tests/wire_consumers/` |
| architecture rules | 8 |  | `tests/arch_*.rs` |
| arch rule A1 allowlist | 0 files, 0 sites |  | `tests/arch_env_reads.rs` ALLOWED |
| arch rule A2 allowlist | 14 files, 262 sites |  | `tests/arch_sql_sites.rs` ALLOWED |
| arch rule A3 allowlist | 6 files, 48 sites |  | `tests/arch_explorer_sites.rs` ALLOWED |
| arch rule A4 allowlist | 4 files, 4 sites |  | `tests/arch_no_unwrap.rs` ALLOWED |
| arch rule A5 allowlist | 6 files, 11 sites |  | `tests/arch_no_prefix_errors.rs` ALLOWED |
| arch rule A6 allowlist | 5 files, 10 sites |  | `tests/arch_spawn_sites.rs` ALLOWED |
| arch rule A7 allowlist | 0 files, 0 sites |  | `tests/arch_config_env.rs` ALLOWED |
| arch rule A8 allowlist | 0 files, 0 sites |  | `tests/arch_no_error_words.rs` ALLOWED |
| mutation floor `src/broadcast_verify.rs` | 75 | 75.5 at 93f160f | `.github/workflows/nightly.yml` |
| mutation floor `src/explorers.rs` | 69 | 74.2 at 93f160f | `.github/workflows/nightly.yml` |
| mutation floor `src/server/handlers.rs` | 74 | 74.2 at 93f160f | `.github/workflows/nightly.yml` |
| mutation floor `src/spend_gate.rs` | 80 | 87.1 at 93f160f | `.github/workflows/nightly.yml` |
| mutation floor `src/tracker_host.rs` | 72 | 72.6 at 93f160f | `.github/workflows/nightly.yml` |
| bench baseline | no baseline yet |  | `benches/baseline.json` |
| e2e scenarios | 24 |  | `tests/e2e/scenarios/` |
| hermetic flag `E2E_HERMETIC` | present |  | `tests/e2e/lib/hermetic.js` |
<!-- scoreboard:end -->

## 5. The three crates and the stack

One scoreboard, three crate rows: this one, `Calhooon/bsv-wallet-toolbox-rs#80` and `Calhooon/bsv-rs#101`, the same standard on all three; the libraries' lanes and landings are the stack keeper's (#106, "The three crates and their dependents"). A change to a transition, a word or the evidence rule is a witness scenario in the stack before it is a CLI change; every issue carries a **Stack alignment** line naming the charter section, the tests T1 to T6 or the ruling it refines (#106, "Staying in line with bsv-stack-lean").

## 6. The research queue

A lane takes the highest open line it can work; **every lane brief cites the queue line it works** (`Queue: Qn`), and a brief without one is not launched. A line's number never changes: a new item takes the next number and its rank is its place in the list; a landed item is struck from the list by the PR that closes its issue. Format: `Qn: #issue title; why next; what it waits on.`

**In flight, not in the queue** (a PR open or landed, the issue awaiting its exit check): #69 (PR 121 landed), #71 (PR 125), #72 (PRs 112, 134, 135, 137 landed), #73 (PR 111), #74 (PR 131), #75 and #76 (PR 123 landed), #77 (this document), #110 (PR 116).

**Phase 0, the oracle**

- Q1: #70 the differential harness, old binary against new on recorded inputs; the nothing-changed evidence every refactor of Phase 1 must carry, and #78 waits on it; waits on nothing (the wire fixtures of #69 are on `beta`).
- Q25: #140 internalizeAction accepts a payment whose transaction does not verify; a stranger's invalid spend is taken as a coin, ranked here because it is the programme's one correctness defect (its milestone is Phase 1); waits on the toolbox (`Calhooon/bsv-wallet-toolbox-rs#103` codes the check in the toolbox's internalize path) and the owner's word (spend semantics); the CLI's wire fixture 103 changes, in its own commit, when the toolbox's fix ships in a beta prerelease.
- Q2: #100 BRC-100 conformance at a pin; the extensions document every shape question (#117, #122) is settled against; a lane is on it now; waits on nothing.
- Q3: #130 the stack's wallet-gate vectors in the vectors job; the gate checked against the Lean definition's rows on every PR, the ground #88 is proven against; waits on nothing (the stack's #184 landed).
- Q4: #133 split the oracle's harness files over 800 lines; empties `detail.tests_files_over_800_lines` so the key becomes a ratchet; waits on #110 (PR 116 brings `tests/wire_consumers.rs`).
  Its milestone is Phase 1 (structure); it is ranked in Phase 0 because the files it splits are the oracle's own harness and the key it empties is the ratchet's (#72), so it finishes an oracle record rather than restructuring the shipped code.
- Q5: #126 ratchet: a unit test for `check()` against a base record; the base comparison has never executed; waits on nothing.
- Q6: #127 ratchet: a tool fingerprint in the record; a PR that edits the tool changes what is counted unseen; waits on Q5 (the same function, landed in turn).
- Q7: #132 ratchet `--record` refuses a toolchain other than the pinned one; a re-record under the wrong toolchain is written today; waits on Q6 (the same file, landed in turn).
- Q8: #124 raise `src/explorers.rs`'s mutation floor to 80; one of four floors below 80 since the first full night (its row in section 4; the others are #173, #174, #175); the night on `beta` measured it at 74.2 (`docs/scoreboard/mutants.json`); waits on nothing.
- Q9: #119 the e2e harness's `relinquishOutput` body shape; the harness sends a body the server answers 422; waits on #110 (its exit test is a consumers' fixture).
- Q10: #122 the places the served shape disagrees with BRC-100 or itself; each a recorded decision; waits on Q2, and each shape change on the owner's word.
- Q11: #117 the refusal shape `HTTPWalletJSON` expects; Zanaadu and teragunv2 read our refusals as transport errors; waits on Q2 and the owner's word (the wire contract).
- Q12: #107 the beta line; CI runs on `beta` and `release.yml` holds the tag rules already, RELEASING.md names `beta` once and the two lines and their cadence are not written there yet; waits on nothing (the captain's; any release is the owner's word).
- Q44: #136 hermetic e2e: block the runner's egress at the OS, not only the proxy wall; a client that ignores the proxy environment would reach the network unseen; waits on #74 (PR 131).
- Q45: #142 a nightly bench run on `beta` so the CI runner's CPU entries reach the gate's run count; the bench gate of #71 gates only on an entry that carries `MIN_RUNS` runs (`tools/bench-compare`); waits on #71 (PR 125) and #76's nightly.

**Phase 1, structure** (the epic's order, then the milestone's later issues)

- Q13: #78 one spend function for send, drain, split and gift-send; four copies of the spend become one and the duplication ratchet falls; waits on Q1.
- Q14: #79 one evidence layer and one report type for the repair passes; brings the three largest files under 800 lines; waits on Q1.
- Q15: #80 typed configuration, loaded once; empties the env-read rule's allowlist; waits on #73 (PR 111), and any knob's name on the owner's word.
- Q16: #81 typed errors end to end; empties the prefix rule's allowlist; waits on #73 (PR 111) and Q1.
- Q17: #82 feature flags: ui, gift, fleet, mcp; the minimal build's crate count; waits on #74 (PR 131, scenarios 1 to 19 hermetic).
- ~~Q18: #83 docs/ARCHITECTURE.md~~ (closed 2026-10-23, PR 114).
- Q19: #84 observability: a stats line, a metrics route, request ids; the refusal counter the hermetic e2e asserts; waits on #74 (PR 131).
- ~~Q20: #85 the unwraps in tracker_words_witnesses.rs~~ (closed 2026-10-23, PR 115).
- Q21: #101 the engine's public surface; the seam Phase 6 is built on; waits on Q1, Q13 and Q14.
- Q22: #118 the MCP's `create_action` cannot send the scoped `allowPending`; waits on #110 (the MCP's consumers' fixture).
- Q23: #120 daemon discards the server task's result; a listener fault leaves the daemon up; waits on nothing.
- Q24: #138 the bearer is checked before CORS; a browser client cannot reach an authenticated server; waits on the owner's word (the wire contract).

**Phase 2, the spend path** (hot: every line needs the full proof of section 2)

- Q26: #86 the lock covers allocation only; the order-of-magnitude throughput step; waits on Q1, #71 and #74.
- Q27: #87 a local header chain fed by the chain feed; no HTTP call at the door; waits on Q1 and #74.
- Q28: #88 the verdict as a maintained column; the 10,000-coin p99, proven equal to the Lean `offerable` by replay; waits on Q3 and Q1.
- Q29: #89 the gate's reads behind a toolbox API; the SQL rule's allowlist for the gate; waits on #73 and the toolbox's API (the stack keeper's).

**Phase 3, throughput**: Q30 #90 the broadcast outbox (waits on Q26); Q31 #91 proofs pushed, not polled (waits on #74); Q32 #92 the coin policy engine (waits on #71); Q33 #93 the BEEF size budget (waits on Q32). Each why: the phase's exit test, section 3.

**Phase 4, fleet**: Q34 #94 one monitor loop, per-tenant writers; Q35 #95 shared header chain and explorer budgets; Q36 #96 the sharding guide and the 1,000-tenant load test. Each waits on Phase 3's exit.

**Phase 5, hardening**: Q37 #97 the chaos suite (waits on #74); Q38 #98 reproducible and signed builds; Q39 #99 the canary and rollback policy (waits on #107).

**Phase 6, the engine API and the desktop app**: Q40 #102 the crate split (waits on Q21); Q41 #103 the event stream (waits on Q21); Q42 #104 the desktop shell (waits on Q40); Q43 #105 the in-process scenario harness (waits on Q21).

## 7. Pointers

- The epic and its phases: #106; the milestones `Phase 0: the oracle` to `Phase 6: the engine API and the desktop app`.
- The machine: `~/bsv/bsv-stack-lean/docs/charters/wallet.md` and `lean/Wallet.lean`; the six tests: `~/bsv/bsv-stack-lean/docs/BSV-NORTH-STAR.md` lines 25 to 36.
- The oracle's records: [ratchets.json](ratchets.json) and [RATCHETS.md](RATCHETS.md); [WIRE.md](WIRE.md) and [MANIFEST.json](../tests/wire/MANIFEST.json); [NIGHTLY.md](NIGHTLY.md) and [nightly.yml](../.github/workflows/nightly.yml); [ARCHITECTURE.md](ARCHITECTURE.md); the e2e harness [run.js](../tests/e2e/run.js). Coming with their PRs: `docs/ARCH-RULES.md` and `tests/arch_*.rs` (#73), `docs/DEPENDENTS.md` (#110), `benches/baseline.json` (#71), `docs/E2E-HERMETIC.md` (#74), `docs/BRC-100-EXTENSIONS.md` (#100).
- The tools: [tools/ratchet](../tools/ratchet), [tools/scoreboard](../tools/scoreboard), [tools/check-links](../tools/check-links) (every relative link in `docs/**/*.md` resolves, and an anchor names a heading of its target), and [tools/check-cites](../tools/check-cites) (the architecture's named citations and file-table line counts agree with its pinned commit); the CI job `ratchet` runs them. The planted cases are `tools/test_scoreboard.py`, `tools/test_check_links.py` and `tools/test_check_cites.py`, run by the same job.

*Update rule (the stack's): delete the sentence you falsify, re-stamp the date, never a banner over a wrong line.*
