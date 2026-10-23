# The nightly (#75, #76)

`.github/workflows/nightly.yml` runs at 03:00Z every day and on `workflow_dispatch`. It is not a PR gate (`ci.yml` is); it is the map of where the tests do not yet hold the machine. Two halves: fuzz targets on the bytes that come from outside, and mutation testing on the hot paths.

## The fuzz targets

`fuzz/` is a `cargo-fuzz` crate with its own manifest, workspace and `Cargo.lock`; the root `Cargo.toml` does not name it. Each target runs one hour a night on the corpus under `fuzz/corpus/<target>/`.

| target | the bytes | what may not happen |
|---|---|---|
| `beef_reader` | the AtomicBEEF and BEEF bytes the server consumes, through the reads it makes: `internalizeAction`'s subject (`post_word::atomic_subject`), the toolbox wallet's pre-store parse, the toolbox store's step 1 (streaming reader, parse, atomic txid, structure check, the subject's outputs), `createAction`'s AtomicBEEF build and unproven ancestors, `receive`'s courier (`src/atomic_beef.rs`, compiled into the target by `#[path]`) | a panic; an atomic txid the door's subject disagrees with; an AtomicBEEF the CLI writes that it cannot read back |
| `brc29_remittance` | the internalize arguments of a BRC-29 payment (derivation prefix and suffix, sender identity key, output index, the carrying transaction), through `server::handlers::internalize_args` and the toolbox wallet's derivation checks | a panic; a field changed by the mapping; an empty derivation, an index past the outputs, or a payment to another key taken; a payment to the derived key refused |
| `served_json` | a `/createAction` or `/internalizeAction` body: the lenient-JSON layer (`server::sanitize_lone_surrogates`), serde into the MetaNet Client shapes, the handler's argument mapping | a panic; a sanitizer that is not idempotent; a caller's rule label surviving the internalize mapping |
| `explorer_bodies` | each explorer answer `src/explorers.rs` reads (WhatsOnChain's spend and BEEF routes, BananaBlocks' txo and hex routes), served with a fuzzed status from a local socket, read by `Explorers::at`'s calls | a panic; a named spender that is not 64 lowercase hex; "knows no spend" from anything but WhatsOnChain's 404; an answer from a non-200; bytes delivered for a txid they do not hash to |
| `allow_pending` | `allowPending` as the door and the commands read it (`AllowPending::from_json`, `from_cli`, `parse_outpoint`) | a panic; an empty, unsorted or repeated scope; a scope that does not reach a coin it names; an outpoint that does not print back to itself |

**The mirrors.** Two steps the targets need are written inline in an async handler, so a target cannot call them without a wallet and a store. `fuzz/src/lib.rs` copies them and names the lines: `create_action_mapping` (from `src/server/handlers.rs` `create_action`, the `allowPending` read to the audited total) and `internalize_precheck` (bsv-wallet-toolbox-rs 0.12.1 `Wallet::internalize_action`'s derivation checks). A mirror drifts when its source changes; the cure is to lift the source into a function the target calls.

**Locally.** `cargo +nightly fuzz run <target> <scratch-dir> fuzz/corpus/<target> -- -max_total_time=300` from the checkout's root (the scratch directory before the seeds, so new entries land there and not in the seeds). The targets build in release without debug assertions, as the shipped binary does. `cargo run --example seed_corpus` from `fuzz/` rewrites the seeds from the shapes the tests and e2e scenarios use.

## Where the results land

- **The run's summary**: each fuzz job's libFuzzer statistics in its log and the number of corpus additions in the job summary; the mutation table in the `Mutation score` job's summary.
- **Corpus growth**: the entries of the hour that add coverage beyond the checked-in corpus (libFuzzer's `-merge=1`) are the artifact `corpus-<target>`, kept 30 days. The repository does not let Actions open pull requests, so the workflow cannot; whoever lands the night's growth downloads the artifacts (`gh run download <run> -p 'corpus-*'`), copies each into `fuzz/corpus/<target>/` and opens one PR, corpus only.
- **Mutation outcomes**: each shard's `outcomes.json`, `missed.txt` and the mutants' diffs are the artifact `mutants-<shard>`.

## What a crash input becomes

A target that stops (a panic, a failed property, a timeout, out of memory) fails its job, uploads the input as `crash-<target>` (kept 90 days), and opens an issue titled `Nightly fuzz: <target> crashed` with each input in base64 in the body (an issue takes no file), the log's tail and the run's link; a crash on a target with that issue open comments on it. The fix lands with the input as a regression: the input added to `fuzz/corpus/<target>/` and, where the bytes are the wallet's to judge, a unit test on the reader that crashed. A crash in a BEEF reader is a witness scenario for the stack's beef-of-any-size charter.

## The mutation score and what a surviving mutant becomes

`cargo-mutants` (copy mode, never `--in-place`) mutates `src/spend_gate.rs`, `src/tracker_host.rs`, `src/server/handlers.rs`, `src/broadcast_verify.rs` and `src/explorers.rs` in twenty shards; a mutant's tests stop at five minutes. `.github/nightly/mutants_score.py` scores each file as caught / (caught + missed + timeout), a timeout counted as caught and an unviable mutant (one that does not build) counted nowhere, and fails the job when a file is below its floor or has no outcome. The floor is 80 percent; a file below 80 when the floor was set is floored at that score rounded down, named in the workflow with the issue that raises it, and a floor only goes up.

A surviving mutant is a place a lane could change the code and no test would notice. On a hot path it becomes an issue (or a line on the file's raise-the-floor issue): the mutant's name and diff from the artifact, and the test that would kill it. The fix is a test, never an exclusion; a mutant that is equivalent (no behaviour can tell it apart) is named in the issue with the reason, and only then skipped by an `exclude_re` line in `.cargo/mutants.toml` that names that issue.
