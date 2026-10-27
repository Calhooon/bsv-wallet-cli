# The nightly (#75, #76)

`.github/workflows/nightly.yml` runs at 03:00Z every day and on `workflow_dispatch`. GitHub fires `schedule` only from the default branch (`main`): on `beta` the cron is inert and the night runs only by hand (`gh workflow run nightly.yml --ref beta`); the cron becomes live when `beta` merges into `main`. It is not a PR gate (`ci.yml` is); it is the map of where the tests do not yet hold the machine. Two halves: fuzz targets on the bytes that come from outside, and mutation testing on the hot paths.

**One night at a time, six shards at a time (#144).** The workflow's concurrency group is the one name `nightly` (never one per ref: the schedule runs from `main` and a dispatch from the ref it names, so a per-ref group would let the two run side by side), with `cancel-in-progress: false`: a night started while one runs waits for it and is never cut short. The mutants matrix runs at most six of its twenty shards at once (`max-parallel: 6`). The dispatched night of 2026-10-24 (run 40074725488) started all twenty shards and the five fuzz targets together, held every runner for over an hour and starved every PR check; its shards took about 1,160 runner-minutes, so at six a night's mutants take near three and a half hours, and the night peaks at eleven runners (six shards beside the five fuzz targets' hour) and holds six after it.

## The fuzz targets

`fuzz/` is a `cargo-fuzz` crate with its own manifest, workspace and `Cargo.lock`; the root `Cargo.toml` does not name it. Each target runs one hour a night on the corpus under `fuzz/corpus/<target>/`.

| target | the bytes | what may not happen |
|---|---|---|
| `beef_reader` | the AtomicBEEF and BEEF bytes the server consumes, through the reads it makes: `internalizeAction`'s subject (`post_word::atomic_subject`), the toolbox wallet's pre-store parse, the toolbox store's step 1 (streaming reader, parse, atomic txid, structure check, the subject's outputs), `createAction`'s AtomicBEEF build and unproven ancestors, `receive`'s courier (`src/atomic_beef.rs`, compiled into the target by `#[path]`) | a panic; an atomic txid the door's subject disagrees with; an AtomicBEEF the CLI writes that it cannot read back |
| `brc29_remittance` | the internalize arguments of a BRC-29 payment (derivation prefix and suffix, sender identity key, output index, the carrying transaction), through `server::handlers::internalize_args` and the toolbox wallet's derivation checks | a panic; a field changed by the mapping; an empty derivation, an index past the outputs, or a payment to another key taken; a payment to the derived key refused |
| `served_json` | a `/createAction` or `/internalizeAction` body: the lenient-JSON layer (`server::sanitize_lone_surrogates`), serde into the MetaNet Client shapes, the handlers' argument mappings (`server::handlers::create_action_args`, `internalize_args`) | a panic; a sanitizer that is not idempotent; a caller's rule label surviving the internalize mapping |
| `explorer_bodies` | each explorer body the CLI still reads itself: `receive`'s couriers (WhatsOnChain's `/beef`, BananaBlocks' `/hex`, by `Explorers::at`'s calls) and the broadcast verifier's chain indexes (WhatsOnChain, Bitails, BananaBlocks, by `BroadcastVerifier::single_pass`), served with a fuzzed status from a local socket; the spender rungs left in 0.18.2 (#196) | a panic; a courier's delivery from a non-200, WhatsOnChain's BEEF other than the body's hex, BananaBlocks' bytes that do not hash to the txid; a chain index present from anything but its 200, absent from anything but its 404, `Rejected` from neither, `Mined` without its field; BananaBlocks' `mined` or `known` for a body about another transaction, or its silence standing against the voting indexes' 404s |
| `allow_pending` | `allowPending` as the door and the commands read it (`AllowPending::from_json`, `from_cli`, `parse_outpoint`) | a panic; an empty, unsorted or repeated scope; a scope that does not reach a coin it names; an outpoint that does not print back to itself |

**The mirror.** The served doors' mappings are functions the targets call (`create_action_args`, `internalize_args`), so no copy of the CLI's code drifts. One step stays a copy, because it is written inline in the toolbox's async `Wallet::internalize_action` and cannot be called without a wallet and a store: `fuzz/src/lib.rs` `internalize_precheck` (bsv-wallet-toolbox-rs 0.14.0-beta.1, the derivation checks, unchanged from 0.13.0) names the lines it copies. It drifts when the toolbox's source changes; the cure is a function in the toolbox the target calls.

**Locally.** `cargo +nightly fuzz run <target> <scratch-dir> fuzz/corpus/<target> -- -max_total_time=300` from the checkout's root (the scratch directory before the seeds, so new entries land there and not in the seeds). The targets build in release without debug assertions, as the shipped binary does. `cargo run --example seed_corpus` from `fuzz/` rewrites the seeds from the shapes the tests and e2e scenarios use.

## Where the results land

- **The run's summary**: each fuzz job's libFuzzer statistics in its log and the number of corpus additions in the job summary; the mutation table in the `Mutation score` job's summary.
- **Corpus growth**: the entries of the hour that add coverage beyond the checked-in corpus (libFuzzer's `-merge=1`) are the artifact `corpus-<target>`, kept 30 days. The repository does not let Actions open pull requests, so the workflow cannot; whoever lands the night's growth downloads the artifacts (`gh run download <run> -p 'corpus-*'`), copies each into `fuzz/corpus/<target>/` and opens one PR, corpus only.
- **Mutation outcomes**: each shard's `outcomes.json`, `missed.txt` and the mutants' diffs are the artifact `mutants-<shard>`.

## What a crash input becomes

A target that stops (a panic, a failed property, a timeout, out of memory) and leaves its input (libFuzzer's `crash-`, `timeout-`, `oom-` or `leak-` file) fails its job, uploads the input as `crash-<target>` (kept 90 days), and opens an issue titled `Nightly fuzz: <target> crashed` with each input in base64 in the body (an issue takes no file; an input over 16 KB is cut to its first 16 KB there and says so, the whole input in the artifact), the log's tail and the run's link; a crash on a target with that issue open comments on it. A non-zero exit with no input (the target did not build, cargo-fuzz failed) is not a crash: the job fails with that word and no issue is opened. The fix lands with the input as a regression: the input added to `fuzz/corpus/<target>/` and, where the bytes are the wallet's to judge, a unit test on the reader that crashed. A crash in a BEEF reader is a witness scenario for the stack's beef-of-any-size charter.

## The mutation score and what a surviving mutant becomes

`cargo-mutants` (copy mode, never `--in-place`) mutates `src/spend_gate.rs`, `src/tracker_host.rs`, `src/server/handlers.rs`, `src/broadcast_verify.rs` and `src/explorers.rs` in twenty shards; a mutant's tests stop at five minutes. `.github/nightly/mutants_score.py` scores each file as caught / (caught + missed + timeout): a timeout has its own column and is not caught (its tests ran past the limit, which no test's verdict showed wrong; it weighs as a miss does), and an unviable mutant (one that does not build) is counted nowhere, and fails the job when a file is below its floor or has no outcome. The target is 80 percent a file. A measured floor only rises: a file's floor is the score a night measured, rounded down, named in the workflow with the issue that raises it, and it is raised in the same commit as the tests that earn the raise, never by an edit alone. A floor set before any night measured its file was provisional; the first full night replaced those with what it measured. A floor is measured when `docs/scoreboard/mutants.json` names its file and provisional when it does not. `tools/scoreboard` holds the floors' rule, numbered as in its header:

1. **The ceiling.** A measured floor is never above its file's score rounded down, whether it rose, stayed, fell or is new. It is the tree's alone, so every `tools/scoreboard --check` holds it for every floor of `MUTANTS_FLOORS`, with a base or without one (CI on a push whose previous commit cannot be fetched), and a fault names the file, the floor and the score.
2. **The fall** (`--check --base`, the CI job `ratchet`). A floor with a measurement at the base never falls; a provisional floor never falls without a measurement in the tree's record; a measurement at the base never vanishes from the tree.
3. **The first measurement** (`--base`). A provisional floor at the base whose file the tree's record measures may fall once, and then to exactly its score rounded down, never lower (and by 1 never higher), with a notice naming the file and both floors.
4. **The provisional notice.** A floor with no measurement is allowed as provisional and held to no score; every `--check` prints a notice naming it.

The first full night on `beta` (run 40074725488, dispatched 2026-10-24 at `93f160f`, cargo-mutants 27.1.0, no timeout; the record is [`docs/scoreboard/mutants.json`](scoreboard/mutants.json)):

| file | caught | timeout | missed | unviable | score | floor | raised by |
|---|---:|---:|---:|---:|---:|---:|---|
| `src/spend_gate.rs` | 169 | 0 | 25 | 7 | 87.1 | 80 | stays 80: one night is not a trend; 85 when a second night holds 87.1 |
| `src/explorers.rs` | 46 | 0 | 16 | 16 | 74.2 | 69 | #124 |
| `src/broadcast_verify.rs` | 148 | 0 | 48 | 36 | 75.5 | 75 | #173 |
| `src/server/handlers.rs` | 92 | 0 | 32 | 252 | 74.2 | 74 | #174 |
| `src/tracker_host.rs` | 170 | 0 | 64 | 38 | 72.6 | 72 | #175 |

The floors of `broadcast_verify.rs`, `handlers.rs` and `tracker_host.rs` were provisional (80, never measured) until that night, which failed on them. #75's lane run (2026-10-23, 281 mutants: 214 caught, 46 missed, 21 unviable, no timeout) set `explorers.rs` at 69 from its 69.7. The scoreboard's measured column (`docs/NORTH-STAR.md`) is rendered from the record; a later night is recorded there by the captain, in the shape `tools/scoreboard`'s header defines.

A surviving mutant is a place a lane could change the code and no test would notice. On a hot path it becomes an issue (or a line on the file's raise-the-floor issue): the mutant's name and diff from the artifact, and the test that would kill it. The fix is a test, never an exclusion; a mutant that is equivalent (no behaviour can tell it apart) is named in the issue with the reason, and only then skipped by an `exclude_re` line in `.cargo/mutants.toml` that names that issue.
