# The ratchets: the numbers that may only fall (#72)

Seven numbers of the tree, measured by `tools/ratchet` and recorded in `docs/ratchets.json`. The CI job `ratchet` measures every PR and push and fails when any number is above the record, printing which and by how much. Nothing has to be fixed at once; nothing may get worse. A lane picks a number and moves it down.

```bash
tools/ratchet            # measure the tree, print one JSON object
tools/ratchet --check    # measure, compare with docs/ratchets.json, exit 1 on a rise
tools/ratchet --record   # measure, rewrite docs/ratchets.json
```

The tool is a Python script with the standard library only (Python 3.8 or later, the cargo toolchain on `PATH`). A Rust bin would need its own manifest and lock and a build before it measures; the script adds no crate to the workspace, the CI runner already has `python3`, and the house's gate tools (`bsv-stack-lean/tools/`) are Python. It runs one `cargo clippy` pass and one `cargo doc` pass (about 15 seconds on a warm target dir) and reads every tracked `*.rs` file itself.

## The scope

Every tracked `*.rs` file of the repository (`git ls-files '*.rs'`): `src/`, `mcp/src/` and `tests/`. The clippy numbers are the workspace's every target (`--workspace --all-targets`, the gate's scope); `missing_docs` is what `cargo doc --workspace` documents (the lib and the bins).

**Test code**, for the unwrap count: a file under the top-level `tests/` or `benches/`, a file named `tests.rs`, a file reached through `#[cfg(test)] mod name;` (with its `#[path]` when it has one: `src/chain_feed_tests.rs`, `src/test_support.rs`, `src/tracker_words_witnesses.rs`), every file under such a module's directory, and every item under `#[cfg(test)]` inside any other file.

## The numbers

| key | what is counted | how |
|---|---|---|
| `clippy_pedantic` | clippy warnings at `-W clippy::pedantic` | `cargo clippy --workspace --all-targets --locked -- -W clippy::pedantic ...`, every `clippy::` warning except the two counted on their own rows below; `detail.clippy_pedantic_by_lint` lists them by lint |
| `unwrap_expect_outside_tests` | `.unwrap()` and `.expect(` outside test code | the tool's lexer (comments and string literals are not code); the tokens `.` `unwrap` `(` and `.` `expect` `(`; `unwrap_or`, `unwrap_err` and the like are not counted; `detail.unwrap_expect_by_file` lists them |
| `files_over_800_lines` | files of more than 800 lines | newline count of each file in scope; `detail.files_over_800_lines` is the list |
| `functions_over_60_lines` | functions of more than 60 lines of code | `clippy::too_many_lines` at `too-many-lines-threshold = 60` (blank and comment lines are not counted, clippy's rule) |
| `cognitive_complexity_over_25` | functions of cognitive complexity over 25 | `clippy::cognitive_complexity` at `cognitive-complexity-threshold = 25` |
| `duplicated_blocks` | blocks of exact duplicate code | below; `detail.duplicated_tokens` is the tokens they cover, reported and not ratcheted |
| `missing_docs` | public items without docs | `RUSTDOCFLAGS="-W missing_docs" cargo doc --workspace --no-deps --locked`, the `missing_docs` warnings |

Every clippy and rustdoc warning is deduplicated by (lint, file, line, column): a lib's warning is reported once for the lib and once for its test harness, and is one warning. The two thresholds live in `tools/ratchet.d/clippy.toml`, which the tool names through `CLIPPY_CONF_DIR` for its own pass only; the gate's `Clippy` job reads the repository's `clippy.toml` as before.

**Duplicated blocks.** The tool lexes every file in scope (identifiers, literals, the common multi-character operators as one token each; comments and whitespace dropped), slides a window of 40 tokens over each file and hashes each window (BLAKE2b-128 of the tokens' text). A window is duplicated when its hash stands at two places that do not overlap: in two files, or 40 tokens or more apart in one. A block is a maximal run of tokens covered by duplicated windows. These are exact clones (the same names and literals); a clone with renamed identifiers is not counted. The block count is the ratchet; a fix that removes the middle of a block can split it in two, and such a PR re-records with the token count shown falling beside it.

## Determinism

Two runs on the same tree give the same JSON byte for byte, on a warm target dir and on an empty one (cargo replays a fresh crate's cached warnings). No timestamp is recorded; every list is sorted. The JSON names the commit measured (`git rev-parse HEAD`) and the toolchain (`rustc --version`, `cargo clippy --version`). `--check` refuses to compare when the toolchain differs from the record's: a new clippy brings new pedantic lints and the numbers would move with no change to the tree. The CI job pins the record's toolchain (1.98.1).

## Why they may only fall

Each number is a debt a reviewer cannot see growing one PR at a time: a panic path in production code (`unwrap`/`expect`), a file or function too long to hold in the head, a branch structure too tangled to test, a copy that will be fixed in one place and not the other, a public item a reader must reverse-engineer. Fixing them all at once is a refactor nobody can review; letting them rise is how a codebase ends up at the numbers above. A ratchet asks neither: every PR leaves each number where it was or lower.

## How a lane moves one

1. Pick a number and a file (the `detail` lists say where the count lives).
2. Fix it in its own PR, one concern (a refactor never carries a behavior change).
3. Run `tools/ratchet --check`: it prints the number lower than the record.
4. In its own commit of that PR, run `tools/ratchet --record` and commit `docs/ratchets.json`. The PR says it re-records and names each number that fell. A PR that does not say so does not touch `docs/ratchets.json`.

A number may rise only by a PR that says why, re-records, and is landed by the captain on that word. Moving the toolchain is such a PR: bump the pin in the `ratchet` job, re-record, and name every number that moved.
