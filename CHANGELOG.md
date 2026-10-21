# Changelog

Releases before 0.7.0 carry their notes in the release commit's message
(`RELEASING.md`).

## Unreleased

The belt of Calhooon/bsv-wallet-cli #44: a seat daemon's `/createAction` answered `400` with `database is locked (code 517)`. `bsv-wallet-toolbox-rs` stays `=0.10.0`; the structural fix (one writer, `BEGIN IMMEDIATE`) is the toolbox's.

- A store busy answer is `503 STORE_BUSY` with `Retry-After: 1`, never `400`; the body keeps `{code, message}`. `AppError::classify` reads the toolbox's typed `Store busy:` (its single writer's error) and today's messages carrying SQLite 5, 6 or 517: `database is locked`, `database table is locked`, `(code N)` and sqlx's `(code: N)` (the form 0.10.0 actually prints: `SQLx error: error returned from database: (code: 517) database is locked`). A transaction that opens IMMEDIATE can only fail at BEGIN, so a 503 means nothing was written and a client may retry blind. Tests: `store_busy_prefix_is_503`, `busy_snapshot_517_is_503`, `busy_5_is_503`, `locked_6_is_503`, `database_is_locked_alone_is_503`, `other_store_errors_are_not_busy`, `store_busy_answers_retry_after_one` (`src/server/handlers.rs`).
- The reproduction, `create_action_beside_a_monitor_shaped_writer_answers_no_store_error` (`src/spend_gate/tests.rs`), ignored: on one wallet file, a writer shaped like the monitor (a pooled transaction that reads, writes, commits) beside 300 rounds of the served handler's `createAction` path (the spend guard, the gate, a template aborted after). On 0.10.0 every run fails within four rounds, with 517 or 5. The follow-up lane un-ignores it once the toolbox's writer lands, and moves the CLI's own transaction sites onto it.

## [0.15.0] - 2026-10-20

The CLI on bsv-wallet-toolbox-rs 0.10.0, the reorg below the user in the toolbox (the stack's rulings 5 and 7 of 2026-10-20, bsv-stack-lean `docs/RULINGS-2026-10-12.md` 19:4xZ to 20:1xZ; `docs/DECISIONS-2026-10-19.md` section 8c). The `reproof` verb's output and the status word `actions` prints change, so a minor.

- `bsv-wallet-toolbox-rs` `=0.9.0` to `=0.10.0` (the lock moves that one entry). `bsv-rs` stays `=0.7.0` (0.10.0 asks `0.7.0` by caret) and `bsv-tracker` `=0.7.0`.
- `reproof` on the new API. A stored proof is stale when, and only when, its root is not the active header's root at its height (ruling 5); with `--execute` the header is read again for each row and decides, at most `REPROVES_PER_PASS` (4) a run, the rest postponed. A kept root moves nothing; a provider's path for the active block replaces the proof; otherwise the proof is demoted (rulings 7a, 7b: the action `unproven`, its inputs kept, nothing released or failed) and the courier is asked once (7d). The witnesses, `Deferred` and its count are gone. The table's action column reads `replaced by <height> <hash>`, `kept: the active header carries the stored root`, `confirming: demoted to unproven, nothing released; <what the courier said>`, `postponed to the next run` or `retained: <fault>`; the summary reads `executed: N replaced | N confirming (demoted, N announced once) | N kept | N postponed | N retained on error`. The JSON report loses `deferred` and gains `announced` and `postponed`. Tests: `execute_reads_the_header_again_demotes_a_moved_root_and_keeps_a_kept_one`, `execute_reproves_a_few_a_run_and_postpones_the_rest`.
- `actions` prints each action's status word; `unproven` (a proof not yet stored, and again after a reorg took it away) reads "confirming" (ruling 7b (b)). `--json` keeps the wire word. The daemon's served tier (`GET /tx/{txid}`) is 0.14.0's. Test: `an_unproven_action_reads_confirming`.
- One rule for the follow-up of a `stale` episode, not two. Toolbox 0.10.0's monitor re-asks the courier once after its demotion (`reask_lost_proof`), and 0.14.0's served pass did the same for the tracker's `stale` word, so a daemon could read the status twice and announce twice for one episode. Where the monitor runs its reorg or review task (`daemon`, a tenant of `serve-fleet --daemon`) the follow-up is the monitor's: the pass marks the row `monitor` (`tracker_states.follow_up`) and reads and announces nothing (`TickOptions::monitor_runs`; the summary counts them as "the monitor's"). Under `serve` and `tracker-tick`, which run no monitor, the pass's own follow-up of 0.14.0 runs unchanged. Test: `in_the_daemon_a_stale_episode_is_followed_up_once_by_the_monitor_not_twice` (one status read, no announce, across the monitor's path and two passes).

## [0.14.0] - 2026-10-20

The daemon on bsv-tracker 0.7.0 under the stack's rulings (5), (2) and 7 of 2026-10-20 (bsv-stack-lean `docs/RULINGS-2026-10-12.md`, 19:4xZ to 20:1xZ; `docs/DECISIONS-2026-10-19.md` section 8c; the draft `corpus/scenarios/2026-10-20-reorg-below-the-user.md`). The daemon's observable words change, so a minor. `bsv-wallet-toolbox-rs` stays `=0.9.0` (no newer release on crates.io at the start of the lane) and `bsv-rs` `=0.7.0`.

- `bsv-tracker` `=0.6.0` to `=0.7.0` (the lock moves that one entry). A reorg's `stale` is by the root: the crate rechecks each named word's stored root against the active header at its height. The daemon feeds the event with the new branch's headers: a height where the header service still answers a header the reorg deactivated has no answer, so the word stands suspect with `recheck` until the next tip, never `stale` by the height alone and never `mined` on the old header. A reorg whose new block carries the same root moves nothing and fetches nothing. A row keeps its reorg and replays its stored proof against the header the reorg took away, so a `stale` word stands with `reorg@<forkHeight>` across passes until a new proof; until 0.14.0 the next pass dropped the path and read `proofFailed`.
- With the feed held (`daemon`, a tenant of `serve-fleet --daemon`), the pass keeps no ring of tips and rereads no mined row (`TickOptions::feed_held`): the recheck is the crate's, once. `serve` keeps the ring. The words whose root moved are asked before any other re-ask, at most four a pass (`REORG_ASKS_PER_PASS`).
- The follow-up of a `stale` word (ruling 7d (c)): once per episode, the status read at the broadcasters (`get_broadcaster_statuses`); a broadcaster that knows it, no announce; every one that answered not knowing it, one announce of the stored BEEF, never a second; a refusal the node gave writes the node's verdict (`rejected`, or `conflicted` with the named competitors), stored on the row (`tracker_states.verdict`) and replayed. The episode's mark is `tracker_states.follow_up`. The pass's summary counts them.
- `GET /tx/{txid}` serves a `stale` word with `tier` (the word its hints give: `announced` or `seen`, read as "confirming"; ruling 7b (b)) and its re-ask. Every other answer keeps its shape.
- Ruling (2): a stored courier presence hint (`mempool_present`) on a `built` transaction now replays to `announced`.
- Tests: `step_3_a_reorg_before_the_service_moves_leaves_the_word_suspect_until_the_next_tip`, `a_reorg_whose_new_block_carries_the_root_moves_nothing_and_fetches_nothing`, `step_4_i_a_broadcaster_that_knows_the_stale_transaction_is_not_told_again`, `step_4_ii_a_stale_transaction_no_broadcaster_knows_is_announced_once` (with step 7), `a_refused_announce_of_a_stale_transaction_writes_the_nodes_verdict` (`src/chain_feed_tests.rs`), `a_stored_presence_hint_on_a_built_transaction_replays_to_announced`, `the_route_serves_a_stale_word_with_its_tier_and_its_reask`. No witness of `tracker_words_witnesses.rs` moved: none of its words is a reorg's or a presence hint's.
- The paved road's deny gate (bsv-stack-lean #105, wave lane W17): `deny.toml` is the stack's `template/deny.toml` byte for byte (sha256 `cdad82c09e2c88aa8315c7bf1a26172753187ada58936a2480670d3c9afc05ee`), `template/stack.toml` carries the line's `[deny]` table at bsv-stack-lean `c319cf6`, and CI gains the `deny` job (the hash check, then cargo-deny 0.20.2 from a temporary root, `--locked check`). The required contexts gain `deny` in the protection write, not in this commit.
- Four advisories cleared in the lock by each dependency's own release on crates.io, no manifest change: `anyhow` 1.0.102 to 1.0.104 (RUSTSEC-2026-0190, unsound), `h2` 0.4.13 to 0.4.20 (RUSTSEC-2026-0258), `event-listener` 5.4.1 to 5.4.2 (RUSTSEC-2026-0221, unsound; `concurrent-queue` leaves the lock), `rand` 0.8.5 to 0.8.8 (RUSTSEC-2026-0097, unsound). `cargo deny --locked check`: advisories ok, bans ok, licenses ok, sources ok.

## [0.13.0] - 2026-10-18

The line of the wave of 2026-10-18 (bsv-stack-lean #119): `bsv-rs` to `=0.7.0`, `bsv-wallet-toolbox-rs` to `=0.9.0` and `bsv-tracker` to `=0.6.0`, together, because the toolbox and the tracker put bsv-rs types in their public APIs. The committed lock carries one SDK version, bsv-rs 0.7.0. Supersedes the update loop's #35 (tracker 0.4.1).

- bsv-tracker 0.5.0 widens `HintStatus` by `MempoolPresent { source }`. The stored hint row (`tracker_states.hints`) writes it as `mempool_present` with its courier in a new optional `courier` field, and reads it back; a `mempool_present` row without a courier reads as `unknown`. The CLI produces no such hint yet. The tracker's compact State form (`State::to_compact`, `from_compact`, `Tracker::restore`) is not adopted: the CLI stores a row of inputs that every load replays (whether built, the hints in order, the raw merkle path, the chain events), never the tracker's state, so its stored path is checked again against the headers on each load; the compact form would store the word.
- bsv-rs 0.7.0's signing digest (the node's dispatch on FORKID and CHRONICLE, the SINGLE sentinel, an explicit scriptCode). The CLI signs on two paths of its own, both with FORKID and without CHRONICLE, where the digest is unchanged: the gift claim's `compute_sighash_for_signing` (scopes 0x41 and 0xc3) and the respend's `P2PKH::unlock` through `Transaction::sign`. Every other signature is the toolbox's `wallet::signer`, which computes its own digest. `tests/signing_digest_vector.rs` pins both paths to bsv-script-lean v1.1.1 `corpus/signer/p2pkh.jsonl` row `signer-p2pkh-00001`, the row the toolbox 0.9.0 test `the_digest_the_toolbox_signs_with_is_the_nodes_for_a_p2pkh_all_forkid_spend` pins: the digest `7370…06b3` and the signed spend's txid `e418…7a96`, the control column of the row's record, judged valid by bitcoin-sv v1.2.3 `verifyscript` (cited, not re-run).
- `bsv-wallet-mcp` is unchanged (0.3.0, 34 tools); it depends on none of the three crates.

## [0.12.0] - 2026-10-18

From the stack's coin read (bsv-stack-lean #164, findings F1 to F4 and F6; Calhooon/bsv-wallet-cli #32).

- `daemon` runs the served pass `serve` runs (`run_served_pass`: the tracker's pass, the sweep, the spend guard), every `BROADCAST_RECONCILE_INTERVAL_SECS` (default 60), with no SSE drain (the monitor holds Arcade's stream). In 0.11.0 the daemon ran the feed but no tracker pass, so a daemon host's creators had no rows (F1).
- A proven creator adopted: a `completed` transaction with a stored proof (`proven_tx_id` set) that creates a coin we hold and has no tracker row is adopted by replaying its stored path against the headers of the moment: `mined(h)` with that path as evidence when the root checks, else no word and the re-ask `proofFailed` (never `rejected`, never a write to the toolbox's rows); at most 100 per pass. A creator the toolbox later completes keeps its row. `GET /tx/{txid}` answers for every coin the wallet holds. The pass's summary counts them.
- An open split holds its coins: the feed records each `split` (key, `forkHeight`) in `tracker_meta` `open_splits` in the delivery's transaction and drops it at its `splitResolved`; the guard holds every coin whose creator is `mined` at or above an open split's `forkHeight` (the reason `suspect: a fork or an unresolved split at or below its height`). The guard's recheck alone cleared the tracker's suspect mark while the header service still carried the root, so a split held nothing (F2; the scenario `tracker-split-observation-writes-no-word.md` column 4).
- The proof backstop's provider at info: each proof the toolbox monitor's `check_for_proofs` stores is logged with the provider that served its merkle path (marker `proof_backstop_provider`, `provider`, `txid`, `height`; `status-answer` when the triage carried it), read from the toolbox's own debug and info lines by a tracing layer; the README lists the site with the rule-28 rows (F3). No setting turns the backstop's explorer providers off.
- `/chain-feed`'s `status` is rewritten after every delivery (N = 1): its `cursor`, `delivered` and `updatedAt` follow the stored cursor while following (F6).
- `send`, `drain`, `split` and `gift-send` under `--json` print a refusal as the route's body, `{code, message, offeredSatoshis, held}`, on stdout, exit 1 (F4).

## [0.11.0] - 2026-10-18

- R1 (d) at the door (Calhooon/bsv-wallet-cli #29; bsv-stack-lean #168; the owner's ruling of 2026-10-18 00:1xZ): the served `/createAction` reads `allowPending` beside `options` (the one value `"received"`; absent or anything else is off; the toolbox's `CreateActionOptions` is unchanged), and `send`, `drain`, `split` and `gift-send` take `--allow-pending received`. Under it, and only for that action, the spend guard offers a received coin whose creator's word claims nothing about the chain (`unknown`, `built`, `announced`, `seen`); its body was checked at internalize (toolbox 0.8.0 `internalize_action.rs` 96-119). No environment variable. The action records the word in `spend_gate_actions` (txid, `received-pending`, when); `actions` and `/listActions` show it as `underRule`. A deferred-signing template (`signAndProcess: false`) has no txid yet: its row is recorded by the action's `reference` (a nullable `reference` column, txid nullable, one of the two set), the served `signAction` fills the txid it was signed as and keeps the reference, the listing shows `underRule` on the template by its reference (where no unsigned template of the same status, description and satoshis carries another rule or none) and on the signed action by its txid, and `abortAction` leaves the row. A held received pending coin's reason ends `offered under allowPending: received`. The cost: a double-spending sender makes the action `conflicted`.
- `GET /tx/{txid}`: the tracker's stored word for one transaction, `{txid, word, height, reask, evidence}` (`evidence` the stored proof's root and height for `mined`, else `null`), behind the wallet's auth; 404 for an unknown txid. `bsv-wallet-mcp` 0.3.0 restores the `tx_word` tool over it (34 tools) and gives `create_action` the `allowPending` parameter.
- Explicit inputs under the guard: coins named in `createAction`'s `inputs` and `respend`'s exact inputs pass the same per-coin judgment as the toolbox's allocation; a named coin the guard holds refuses the action (`403 INPUT_NOT_OFFERED`, the same `held` list). A respend inherits the word its phantom was created under.
- The frozen record: the feed's `frozen` event writes its outpoint to `frozen_outpoints` (txid, vout, cursor, when) in the delivery's transaction; the guard never offers such a coin (`frozen: never offered`; never cleared here, R6); `GET /chain-feed` gains `frozenCount`.

## [0.10.0] - 2026-10-18

- The spend guard on the binary's real coin selection (Calhooon/bsv-wallet-cli #28; the stack's wallet charter R1 (b) and R3 (b), rulings of 2026-10-12): every coin the served `/createAction`, `send`, `drain`, `split` and `gift-send` offer to the toolbox's `createAction` is either mined at `SPEND_DEPTH` (floor 1) under the headers of the moment, or our own `made` change under `SPEND_OWN_CHANGE` (new, default `1`). A received coin with no proof is never offered. The toolbox allocates inside its own query, so the guard holds the coins it does not offer (`outputs.change = 0`, recorded in `spend_gate_holds`) for the call and restores exactly those after; a hold left by a call that did not finish is restored by the next one after ten minutes. Departs from the reference's allocation of unproven change (ts-stack `edf6e03` `StorageKnex.ts:2222-2231`).
- The refusal lists every coin held back, with its word, origin, age in minutes and the rule that holds it; `/createAction` answers `402 INSUFFICIENT_FUNDS` with the list under `held`.
- `bsv-wallet-mcp` 0.2.0: rmcp 3.5; four tools over the host's existing routes, `chain_feed_status`, `health`, `pull`, `register` (the relay's at `RELAY_URL`, `RELAY_ADMIN_TOKEN` for `register`). The README says how the binary is built (`cargo build --release -p bsv-wallet-mcp`, in the workspace's `target/release`) and gives a client config. Not in this release: a route serving the tracker's word for one transaction with its evidence, and its tool (#29).

## [0.9.0] - 2026-10-17

- `daemon` holds the header service's chain-event feed (`CHAINTRACKS_URL`; Calhooon/bsv-wallet-cli #26, bsv-stack-lean #167): a replay through `GET /events?since=` from the stored cursor until `hasMore` is false, then the SSE stream; every envelope through `bsv_tracker::decode_envelope` and `Tracker::on_chain` with the daemon's headers; the cursor stored after each delivery in the same transaction as the tracker's rows (`tracker_meta`, `feed_cursor`). A decode fault, `cursorAhead` (409), an `event: error` or a refused request stops the loop at its cursor and is reported at `GET /chain-feed`; a dropped stream reconnects (the page from the cursor, then the stream with `Last-Event-ID`). Load: one stream per daemon plus one page request per reconnect.
- A chain event that leaves a tracked transaction other than a plain checked `mined` (a reorg's `stale`, a fork's or a split's suspect mark) is kept on its row (`tracker_states.chain`, added to an existing table empty) and replayed on each load; a new proof, or a stored path dropped as no longer checking, clears it.
- `SPEND_DEPTH`: the spend depth the tracker host's spend guard reads, default 1 and the floor (the stack's wallet charter R3, `lean/Wallet.lean` `guardDepth`).

## [0.8.0] - 2026-10-17

- Pin `bsv-wallet-toolbox-rs` to `=0.8.0`, `bsv-rs` to `=0.6.1` and `bsv-tracker` to `=0.4.0`. They move together because toolbox 0.8.0 and tracker 0.3.0 put bsv-rs 0.6 types in their public APIs. The committed lock carries one SDK version, bsv-rs 0.6.1, on the line the toolbox's caret `0.6.0` admits.
- bsv-tracker 0.4.0 adds two `ChainEvent` variants, `Split` and `SplitResolved`. The CLI has no exhaustive `match` on `ChainEvent` because it does not decode the header feed, so neither variant needs an arm and no split moves a coin.

## [0.7.5] - 2026-10-12

- Pin `bsv-wallet-toolbox-rs` to `=0.7.6`, `bsv-tracker` to `=0.2.2`, and `bsv-rs` to `=0.5.0`. The committed lock carries one SDK version.
- Report a refused courier BEEF with its transaction id, courier and validation reason in `receive` and `sync`. `sync` processes the remaining outpoints and exits nonzero if any were refused. Incomplete unmined stubs are refused before storage by toolbox 0.7.6 ([toolbox #23](https://github.com/Calhooon/bsv-wallet-toolbox-rs/issues/23)).
- Add mock courier regressions for refusal without storage and continued sync processing. Completing missing ancestry through a courier remains separate work.

## [0.7.4] - 2026-10-12

`respend <txid>`: the host's deliberate competitor for a transaction a broadcaster accepted that never mines (bsv-stack-lean #67; the tracker rule: no hand writes a status, the chain decides). The L2 re-pin: bsv-wallet-toolbox-rs 0.7.5, bsv-tracker 0.2.1, bsv-rs 0.4.5 (the lone leaf at offset 0 only; the LOCAL work bound inside one input's script); no behavior change under the defaults.

## Unreleased

- Pin `bsv-wallet-toolbox-rs` to `=0.7.5` and `bsv-rs` to `=0.4.5`.
  bsv-rs 0.4.5: the lone leaf at offset 0 only; the LOCAL work bound; no behavior change under the defaults.

- Add `respend <txid>`: deliberately spend an accepted transaction's exact
  wallet-owned inputs back to wallet change, using the wallet fee policy.
  Refuse while any answering status source holds the original, when status
  lookups fail, after proof, or for an input the wallet cannot sign.
  The replacement follows the toolbox process and broadcast path and is
  retried on its cadence. Construction and broadcaster hints leave the
  original's word and input reservations intact; only a checked competitor
  proof writes `doubleSpend`. A served pass assigns locked inputs to the
  proven replacement; the spend guard keeps those inputs reserved even
  when an unspent hint reaches the rollback path. A recorded build interrupted before processing is
  completed by that pass; repeated construction is refused.
  No change to the age policy for accepted unproven change, dependencies,
  or crate version (bsv-stack-lean #67).

## 0.7.3

The toolbox 0.7.4 (bsv-stack-lean #66: every broadcaster word is a hint; a
refusal on the immediate post is not an error), and the host's side of it:
no path of the CLI retires a transaction a broadcaster took, the served
doors answer a refused post with the tracker's word, and a coin whose
transaction drew no accepting word is not spent.

- **Dependencies.** `bsv-wallet-toolbox-rs` 0.7.4 (was 0.7.3). `fastrand`
  2.5.0 and `spin` 0.9.9 (were 2.4.0 and 0.9.8, both yanked, in the lock
  since 0.7.1): a lock-only move.
- **The vector.** `tests/vectors/arcade_status_verdicts.json` is the toolbox
  0.7.4's file, byte-identical (`cmp` against the crate's file); four
  `toolbox_push` values move to `none` (`push_rejected_467`,
  `push_rejected_466_conflict`, `push_double_spend_attempted`,
  `push_rejected_lower_case`): a pushed REJECTED or DOUBLE_SPEND_ATTEMPTED
  writes no word.
- **No retire of a transaction a broadcaster took** (the tracker's
  `abandoned` is the host's act on a template never announced; #24). An
  accepting word in the wallet's records (the proof request past the post,
  an `accepted`/`seen`/`mined` memory row, the tracker's word past `built`)
  keeps a transaction from every retire path: the served sweep (which
  retired any unproven transaction with a `rejected` memory row, now a
  hint), `reconcile-broadcasts`, `cleanup-abandoned` and the daemon's
  ticker (which abandoned one the broadcaster held past
  `BROADCAST_ABSENCE_MINUTES`, or found nowhere), the after-send follow-up,
  and `/abortAction` (409). It stays, re-asked, until a proof, a node
  verdict or a competitor's checked proof. A transaction no broadcaster took
  is retired as before; a confirmed competitor's spend still abandons.
- **The doors' word for a refused post.** "Transaction broadcast failed" no
  longer reaches the CLI, and the `BROADCAST_REJECTED` mapping that promised
  "the tx is already failed and its inputs released" is gone.
  `/createAction` and `/signAction` answering `sending`, and
  `/internalizeAction`, carry `broadcast: {word: "built", request, hint,
  inputs: "locked"}` (the request's status, the toolbox's
  `immediateBroadcastHint`); the MCP tools say the same. The inline
  follow-up's 502 promises a release only where the chain vouches the coin.
- **The spend guard.** A coin of a transaction whose immediate post drew no
  accepting word, with no status source holding it, is held (not
  spendable, recorded in `spend_guard_holds`) before every spend and on each
  served pass, and released once the toolbox promotes the request
  (`unmined`) or a proof completes it; a retire drops the hold.

### Upgrade

The wallet database gains `spend_guard_holds`; no existing table changes. A
held coin reads `spendable = 0` until released; the toolbox's balance does
not count it meanwhile. A client of the served doors that read 502
`BROADCAST_REJECTED` on a refused post now receives 200 with `sending` and
`broadcast.word = "built"`, and reads the refusal there or from the
monitor. A daemon that relied on the absence threshold, or on
`/abortAction`, to free the inputs of a transaction a broadcaster accepted
keeps them locked. The library's report structs (`ReconcileBroadcastsReport`,
`SweepReport`, `ServedPassReport`) and the doors' response types gain a
field each. Rollback: pin 0.7.2 (with the toolbox 0.7.3); `spend_guard_holds`
is left unread, and a coin it held stays `spendable = 0` until set back by
hand (`UPDATE outputs SET spendable = 1 WHERE output_id IN (SELECT
output_id FROM spend_guard_holds)`).

## 0.7.2

The toolbox 0.7.3 and bsv-rs 0.4.3, moved together; no line of the CLI's
code changed, one test added.

- **Dependencies.** `bsv-wallet-toolbox-rs` 0.7.3 (was 0.7.0) and the CLI's
  own `bsv-rs` (`bsv-sdk`) 0.4.3 (was 0.4.1), in one commit, so the graph
  holds one bsv-rs; `bsv-tracker` 0.2.0 resolves the same 0.4.3.
- **A transaction with no output is invalid bytes** (bsv-rs 0.4.3,
  bsv-stack-lean #59; the node's rule, bsv-script-lean@87f0461
  `lean/BsvScript/TxRules.lean:85-86`), as one with no input has been since
  0.7.0. The CLI's reader of a stranger's BEEF (`atomic_beef`, the
  toolbox's `refuse_invalid_beef_bytes`, which the served doors also call)
  refuses one at the transaction's offset, kind `NoOutputs`, under a BUMP
  too; the witness `a_transaction_with_no_output_is_refused_at_its_offset`
  fails on 0.7.1's dependencies (the BEEF taken) and passes here. No fixture
  of the CLI carries a transaction with no output, and no code of the CLI
  matches on bsv-rs's `Kind` or `Reason`, so the new variant breaks no match
  here.
- **What the toolbox 0.7.1 to 0.7.3 brings to the daemon.** A txid-only
  entry is valid only when the BEEF proves it, except that `createAction`
  with `trustSelf: 'known'` (the wallet's default) resolves an entry the
  wallet's own storage holds before it verifies. ARC is posted the plain BEEF
  as written, and ARC's 400 is a request fault that schedules a re-ask, not
  the transaction's rejection. The monitor retires no announced transaction
  on transient words or by age: a `sending` transaction stays `sending`, its
  inputs locked, re-asked on a 1, 2, 4 ... 64 minute cadence, until a
  definitive word, a proof, or the host's explicit retire.

### Upgrade

Nothing stored is migrated. A daemon that relied on the toolbox failing a
transaction stuck in `sending` (after seven attempts, or five minutes) now
keeps it announced and its inputs locked; the CLI's own retire paths
(`broadcast_reconcile`'s absence threshold, `/abortAction`, the release rule
on a definitive rejection) are unchanged. Rollback: pin 0.7.1.

## 0.7.1

- **No open bind without a token.** `serve`, `daemon` and `serve-fleet`
  refuse at startup, before the wallet is opened or a socket bound, to bind
  an address beyond loopback (`BIND_ADDR`, IPv4 or IPv6) when no bearer token
  (`AUTH_TOKEN`, an empty value counting as none) is set. The error names the
  one flag, `--allow-no-token`, that permits the open bind deliberately. A
  loopback bind without a token is unchanged.
- **The risk it closes.** Since 0.7.0 the served `/internalizeAction` and
  `/createAction` doors take a body of any size, auth before the body; a
  daemon bound to `0.0.0.0` with no token was a wallet any host on the
  network could drive, and made to hold any body it sent.

### Upgrade

A daemon started with `BIND_ADDR` beyond loopback and no `AUTH_TOKEN` now
exits non-zero at startup: set `AUTH_TOKEN` (and send it as
`Authorization: Bearer`), or add `--allow-no-token` to keep the open bind.
The library gains `server::refuse_open_bind`,
`server::refuse_open_bind_from_env` and `server::bind_addr_from_env`;
`server::run` itself does not check, so a host calling it directly checks
first.

## 0.7.0

The 0.4 line: bsv-rs 0.4.1 under the CLI, the toolbox and the tracker, one
copy in the graph, and the posture that a valid BEEF is never refused for its
size or its counts; a BEEF is refused only for invalid bytes, at their offset.

- **Dependencies, moved together.** `bsv-wallet-toolbox-rs` 0.7.0,
  `bsv-tracker` 0.2.0 and the CLI's own `bsv-rs` (`bsv-sdk`) 0.4.1. The
  tracker 0.2.0 takes bsv-rs 0.4's `MerklePath`, so the three move in one
  commit; with any one left behind the graph holds two bsv-rs copies and the
  CLI does not compile.
- **A transaction with no input is invalid bytes** (bsv-rs 0.4.1). A BEEF
  carrying one is refused at the transaction's leading byte (`NoInputs`),
  where 0.6.0 internalized it when no chain tracker was set.
- **The commands' BEEF funnel.** `fund`, `receive` and `gift-claim` hand a
  stranger's BEEF to the wallet through `atomic_beef::ensure_atomic`, which
  now reads it with the streaming reader first: a refusal names the byte in
  the caller's own bytes and the reader's kind (`Invalid BEEF at byte N:
  Kind`). `receive` reads the courier's bytes the same way before it matches
  an output.
- **The served wallet's BEEF doors.** `/internalizeAction` (`tx`) and
  `/createAction` (`inputBEEF`) no longer refuse a body over 50 MiB (0.6.0:
  413 `TOO_LARGE`, a valid BEEF of about 13 MB and more refused for its
  size); every other route keeps the 50 MiB cap. An invalid BEEF is 400
  `INVALID_BEEF`, the message naming the offset and the kind.
- **Auth before the body.** The bearer token is checked before any request
  body is read, so the uncapped doors are open only to the wallet's caller.

### The bounds that stay, named

- The served doors hold the JSON body whole (the BRC-100 JSON wire carries
  the BEEF as one array, about four bytes of body per byte of BEEF): memory
  linear in the BEEF, never a refusal. With no bearer token set (the default,
  bound to 127.0.0.1), any local caller can send a body of any size.
- The commands hold a BEEF whole: a hex argument (`fund`), a courier's
  response body (`receive`), the toolbox's argument (`internalizeAction`'s
  `tx`, one `Vec<u8>`).
- `/arc-callback` and the relay keep a 1,000,000-byte cap; they carry a
  broadcaster's status and a merkle path, never a BEEF.

### Upgrade

The library's public API names the toolbox's and bsv-rs's types
(`server::make_wallet_state` takes the toolbox's `Wallet`; `tracker_host`
binds the tracker's and bsv-rs's), so a host linking `bsv_wallet_cli` moves
to the toolbox 0.7.0 and bsv-rs 0.4.1 with it, and one matching on the
toolbox's `Error` meets `Error::InvalidBeef { offset, kind }`. A payer whose wallet emits a BEEF with an input-less transaction is
refused where 0.6.0 credited it; the node would not mine such a transaction.
