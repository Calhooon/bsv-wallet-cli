# The ratchets: the numbers that may only fall (#72)

Eight numbers of the tree, measured by `tools/ratchet` and recorded in `docs/ratchets.json`. The CI job `ratchet` measures every PR and push and fails when any number is above the base's record, printing which and by how much. The base's record is read from the base ref (`git show origin/<base>:docs/ratchets.json` on a PR, the commit before it on a push), never from the PR's own tree: a PR that edits `docs/ratchets.json` may lower a number of the base's record, never raise one, and the job fails when the PR's record is above the base's on any number. A base with no record yet (the PR that brings the ratchet) checks the tree against its own record alone. The job keeps the measured JSON as the run's artifact `ratchets`. Nothing has to be fixed at once; nothing may get worse. A lane picks a number and moves it down.

```bash
tools/ratchet            # measure the tree, print one JSON object
tools/ratchet --check    # measure, compare with docs/ratchets.json, exit 1 on a rise
tools/ratchet --check --base B   # also exit 1 when the tree or docs/ratchets.json is above the record B
tools/ratchet --record   # measure, rewrite docs/ratchets.json; refuses a dirty tree
tools/ratchet --out F    # with any of the above, also write the measured JSON to F
```

The tool is a Python script with the standard library only (Python 3.8 or later, the cargo toolchain on `PATH`). A Rust bin would need its own manifest and lock and a build before it measures; the script adds no crate to the workspace, the CI runner already has `python3`, and the house's gate tools (`bsv-stack-lean/tools/`) are Python. It runs one `cargo clippy` pass and one `cargo doc` pass (about 15 seconds on a warm target dir) and reads every tracked `*.rs` file itself.

## The scope

Every tracked `*.rs` file of the repository (`git ls-files '*.rs'`): `src/`, `mcp/src/` and `tests/`. The clippy numbers are the workspace's every target (`--workspace --all-targets`, the gate's scope); `missing_docs` is what `cargo doc --workspace` documents (the lib and the bins).

**Test code**, for the unwrap count: a file under the top-level `tests/` or `benches/`, a file a `#[cfg(test)] mod name;` declares (resolved as rustc does: beside `lib.rs`, `main.rs` and `mod.rs`, in a directory of the declaring file's name otherwise, at its `#[path]` when it has one), every file under such a module's directory, and every item under `#[cfg(test)]` inside any other file. Today the declared files are the three `tests.rs` of `mcp/src/`, `src/server/handlers/` and `src/spend_gate/`, `src/test_support.rs`, `src/tracker_words_witnesses.rs`, and `src/chain_feed_tests.rs` (the one with a `#[path]`). A file's name decides nothing: a `tests.rs` a plain `mod tests;` declares is production code and its unwraps count.

## The numbers

| key | what is counted | how |
|---|---|---|
| `clippy_pedantic` | clippy warnings at `--force-warn clippy::pedantic` | `cargo clippy --workspace --all-targets --locked -- --force-warn clippy::pedantic ...`, every `clippy::` warning except the two counted on their own rows below; `detail.clippy_pedantic_by_lint` lists them by lint |
| `unwrap_expect_outside_tests` | `.unwrap()` and `.expect(` outside test code | the tool's lexer (comments and string literals are not code); the tokens `.` `unwrap` `(` and `.` `expect` `(`; `unwrap_or`, `unwrap_err` and the like are not counted; `detail.unwrap_expect_by_file` lists them |
| `files_over_800_lines` | shipped code's files of more than 800 lines | newline count of each file in scope that is not test code; `detail.files_over_800_lines` is the list |
| `tests_files_over_800_lines` | test code's files of more than 800 lines | newline count of each file of test code (the scope's rule above); `detail.tests_files_over_800_lines` is the list |
| `functions_over_60_lines` | functions of more than 60 lines of code | `clippy::too_many_lines` at `too-many-lines-threshold = 60` (blank and comment lines are not counted, clippy's rule) |
| `cognitive_complexity_over_25` | functions of cognitive complexity over 25 | `clippy::cognitive_complexity` at `cognitive-complexity-threshold = 25` |
| `duplicated_tokens` | tokens of exact duplicate code | below; `detail.duplicated_blocks` is the blocks they form, reported and not ratcheted |
| `missing_docs` | public items without docs | `RUSTDOCFLAGS="--force-warn missing_docs" cargo doc --workspace --no-deps --locked`, the `missing_docs` warnings |

Every lint is passed with `--force-warn`, not `-W`: an in-source `#[allow(..)]` or `#[expect(..)]` cannot take a warning out of the count (an allow is a fix nobody made). The count therefore includes what a dependency's macro allows in the code it generates: today 5 `clippy::redundant_closure_for_method_calls` in clap's derive of the `Vec<String>` arguments of `src/cli.rs`, which rustc reports at the field with no macro span, so the tool cannot tell them apart; a new such argument adds one, and its PR says so. Every clippy and rustdoc warning is deduplicated by (lint, file, line, column): a lib's warning is reported once for the lib and once for its test harness, and is one warning. The two thresholds live in `tools/ratchet.d/clippy.toml`, which the tool names through `CLIPPY_CONF_DIR` for its own pass only; the gate's `Clippy` job reads the repository's `clippy.toml` as before.

**Duplicated tokens.** The tool lexes every file in scope (identifiers, literals, the common multi-character operators as one token each; comments and whitespace dropped), slides a window of 40 tokens over each file and hashes each window (BLAKE2b-128 of the tokens' text). A window is duplicated when its hash stands at two places that do not overlap: in two files, or 40 tokens or more apart in one. A block is a maximal run of tokens covered by duplicated windows. These are exact clones (the same names and literals); a clone with renamed identifiers is not counted. The ratchet is the token count, the tokens the duplicated windows cover: a fix that removes the middle of a clone splits its block in two and raises the block count, while the covered tokens fall. The block count stays in `detail.duplicated_blocks`.

## Determinism

Two runs on the same tree give the same JSON byte for byte, on a warm target dir and on an empty one (cargo replays a fresh crate's cached warnings). No timestamp is recorded; every list is sorted. The JSON names the commit measured (`git rev-parse HEAD`, with `-dirty` after it when the working tree differs from HEAD; `--record` refuses a dirty tree, so a record always names the commit it measured) and the toolchain (`rustc --version`, `cargo clippy --version`). `--check` refuses to compare when the toolchain differs from the record's: a new clippy brings new pedantic lints and the numbers would move with no change to the tree. The CI job pins the record's toolchain (1.98.1).

## Why they may only fall

Each number is a debt a reviewer cannot see growing one PR at a time: a panic path in production code (`unwrap`/`expect`), a file or function too long to hold in the head, a branch structure too tangled to test, a copy that will be fixed in one place and not the other, a public item a reader must reverse-engineer. Fixing them all at once is a refactor nobody can review; letting them rise is how a codebase ends up at the numbers above. A ratchet asks neither: every PR leaves each number where it was or lower.

## How a lane moves one

1. Pick a number and a file (the `detail` lists say where the count lives).
2. Fix it in its own PR, one concern (a refactor never carries a behavior change).
3. Run `tools/ratchet --check`: it prints the number lower than the record.
4. In its own commit of that PR, run `tools/ratchet --record` and commit `docs/ratchets.json`. The PR says it re-records and names each number that fell. A PR that does not say so does not touch `docs/ratchets.json`.

A number may rise only by a PR that says why, re-records, and is landed by the captain on that word. Moving the toolchain is such a PR: bump the pin in the `ratchet` job, re-record, and name every number that moved.


## Two file-length ratchets: the shipped code's and the test code's (2026-10-24)

`files_over_800_lines` counts the shipped code's files (`src/` and `mcp/src/`, test code excepted). The test code's files over the bound were listed under `detail.tests_files_over_800_lines` and reported, not gated, while the oracle's harness files ran long; #133 split those (`tests/integration/`, `tests/wire_harness/`, `tests/wire_consumers_harness/`, `tests/respend_witness/`), and the count became the ratchet `tests_files_over_800_lines`, with the same only-falls rule. Its record is 3: the unit-test files of `src/` the list names (`src/spend_gate/tests.rs`, `src/tracker_words_witnesses.rs`, `src/chain_feed_tests.rs`), each a lane of its own. Every other ratchet (pedantic lints, unwraps outside tests, long functions, complexity, duplication, docs) measures test code as before.

A base record that predates a ratchet (the PR that adds the key) holds nothing to be above: `--check --base` prints the key as `new` and compares the rest. The record the tree is checked against must hold every key the tool measures; one it lacks fails the check as `UNRECORDED`, and a key a record drops is a rise.

`fuzz/` is test code to the ratchet, as `tests/` and `benches/` are (2026-10-24): a fuzz target's unwraps are a harness's, and its files over the bound count in `tests_files_over_800_lines`, not `files_over_800_lines`; its duplication and lints still count.
