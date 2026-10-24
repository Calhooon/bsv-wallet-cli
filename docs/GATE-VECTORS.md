# The wallet-gate vectors (#130)

The stack's `conformance/wallet-gate-vectors.json` (bsv-stack-lean #184, its page `docs/p0/wallet-gate-vectors.md`) is the spend guard of `lean/Wallet.lean` as data: for each witness trace of the wallet, each step of it, each coin and each value of the action's word at the door (`allow_received`), the inputs the Lean evaluator saw and the answers it gave. Every row is `[M]`, computed by the evaluator and never edited by hand. The CLI replays all 490 rows against its own gate in the `vectors` job, so the gate is checked against the definition's rows on every PR, not against a reading of the charter.

## The files

| file | what it is |
|---|---|
| [`tests/vectors/wallet_gate_vectors.json`](../tests/vectors/wallet_gate_vectors.json) | a byte-identical copy of the stack's file |
| [`tests/vectors/wallet_gate_vectors.pin`](../tests/vectors/wallet_gate_vectors.pin) | the pin: the stack's commit, the path, the file's sha256 |
| [`tests/vectors/wallet_gate_divergences.json`](../tests/vectors/wallet_gate_divergences.json) | every difference between the gate and a row (row, door, field, the Lean's value, the CLI's), each with its finding |
| [`tests/gate_vectors.rs`](../tests/gate_vectors.rs) | the replay; `cargo test --locked --test gate_vectors -- --nocapture` prints the counts |

The pin at this writing: bsv-stack-lean `103c5bd6101917b829e5d403152ec87fa8046aae` (origin/main on 2026-10-24; the file last moved at `0e543b9`), sha256 `8ea731e553ec1ac0ffb6711e7007adae51d89d9d6793241d6174b0d1d1bab87d`.

## How a row is replayed

Each row gets a fresh in-memory store, written in the store's own records, and the toolbox's mock header service:

- **the creator**: its action (`is_outgoing` for `made`, `completed` when `mined`, else `unproven`) and its evidence as the host keeps it. `mined` is the toolbox's `proven_txs` path, `built` a tracker row, `announced` and `seen` the hints `heard` stores, `stale` the stored path with the stored reorg that took its block, the suspect mark a stored fork, `rejected` and `conflicted` a stored verdict, `unknown` no record but the action.
- **the coin**: its satoshis, and its lock: `locked` is `spent_by` a live action of ours, `spent` is `spent_by` a completed one, `lost` is the relinquished record (`spendable = 0`, `spent_by` empty, as `reconcile-outputs` writes it), and an absent coin is no output.
- **the headers**: the tip at `headers_tip`, and at the creator's proof height a header carrying `header_root_at_proof_height` (the proof's own root when the row's names agree).
- **the host and the action**: `spend_depth`; `SPEND_OWN_CHANGE` on when the row names a rule for made change; the action's word off, `received`, and for a row with the word, the scoped word naming the row's coin (#63) as well.

Then the gate is asked through its public functions:

| function | compared against |
|---|---|
| `tracker_host::read_creators` | the creator's word must be the row's (a difference means the replay built the store wrong, and the test fails as a construction error); `creator_suspect`, read after the guard's own recheck |
| `spend_gate::hold_reason` on that reading | `door_spendable`; `guard_depth` where the reason names the depth |
| `spend_gate::assess_named` (a named input) | `offerable`, `offerable_clause` |
| `spend_gate::assess` (the allocation's offer) | `offerable`; `offerable_clause` when the coin is offered or held with a reason |

A hold's reason maps to the Lean's clause: `frozen` to frozen, `spent by` and `lost:` (the store's `spendable = 0` with no spender, #165) to held by a lock, `suspect:` to creator suspect, `<word>: never offered` to a chain word against the creator, `received, no proof` and `SPEND_OWN_CHANGE=0` to no rule for the creator's origin, a bare `depth N:` to mined shallower than the guard's depth.

The columns the gate gives no value for are named by the test's `COLUMNS_NOT_ANSWERED` and printed on every run: `rule_for`, `received_pending`, `door_word`, `coin_word`, `dead_lineage` (the gate computes none of them apart from its hold), and `host_answer` (a row names its door but carries none of its arguments, so the host's answer at that door cannot be replayed from the row).

## The unexpressed rows

`UNEXPRESSED` in the test lists the rows whose state the CLI cannot hold, each with why. The list is exact: the test computes the class from the rows and fails when it differs from the list, so a row that becomes expressible (or a new row that is not) fails until the list is changed. At this pin, 486 rows are expressed and 4 are not:

- **the creator is `abandoned`** (2 rows, `WalletGate.contendedSteps` step 5, `T1.0`): the CLI's tracker row stores no abandon, so no store reads `abandoned`. An action the host gives up is the toolbox's `failed` status, which the allocation never reads as a candidate.
- **the creator is mined above the headers' tip** (2 rows, `Wallet.Check.depth_floor`): the CLI checks the stored proof against the headers on every read and the tracker refuses a proof above the tip, so the reading is `unknown`, never `mined`.

## The divergences

A difference between the gate and a row is a finding. It is never fixed by changing the gate in this test's commit, nor by changing a row. `wallet_gate_divergences.json` records each one. That list is exact too: a difference it does not hold fails the test, printed as a `DIVERGENCE` line ready to record, and so does an entry that no longer differs. When the gate is fixed, the fix's commit removes its entries. At this pin there are 164 differences over 40 rows, in four findings (the file describes each); the fifth, `lost-coin-not-read` (24 rows, 138 differences, 22 rows on `offerable`: the gate did not read the relinquished record), left with its fix (#165):

| finding | rows | `offerable` differs on |
|---|---|---|
| `stale-door-reads-seen`: the gate never offers a `stale` creator; `doorWord` reads stale as seen | 12 | 5 |
| `no-lineage`: the gate reads no ancestor; `deadLineage` holds the coin | 12 | 12 |
| `fork-recheck-clears-suspect`: the guard's recheck clears a stored fork's suspect mark while the root holds | 2 | 2 |
| `named-absent-coin`: `assess_named` leaves an outpoint that is not ours to the toolbox (by design); the Lean answers no coin | 14 | 14 |

## Moving the pin

1. Copy the file at the stack's new commit: `git -C ~/bsv/bsv-stack-lean show <commit>:conformance/wallet-gate-vectors.json > tests/vectors/wallet_gate_vectors.json`.
2. Write the commit and `shasum -a 256 tests/vectors/wallet_gate_vectors.json` into the pin, and the sha256 into `vectors_sha256` of the divergences file.
3. Run `cargo test --locked --test gate_vectors -- --nocapture`. A new row the CLI cannot hold fails the unexpressed list; a new difference prints its `DIVERGENCE` line. Record each under a finding (a new finding gets its description in the file), and update the counts on this page.
4. Commit the copy, the pin and the lists together, the stack's commit named in the message.
