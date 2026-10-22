# HANDOFF

The captain's record of the wave in flight. Re-orient from this file and
`cca runs --project bsv-wallet-cli --open` after any restart.

## Wave of 2026-10-21: #44, the store's single writer

**The issue.** Calhooon/bsv-wallet-cli #44: a seat daemon's served
`/createAction` answered 400 `database is locked (code 517)`
(SQLITE_BUSY_SNAPSHOT). The toolbox's store opens one sqlx pool over the WAL
file; every write transaction opens with a deferred `BEGIN`, reads under a
snapshot, then writes after another pooled connection (the monitor's proof
task) committed. `busy_timeout` never applies to 517. The `SpendingLock`
serialises createAction against createAction only.

**The design (the owner's go of 2026-10-21).** Five layers, each a rule the
code enforces:

1. One writer by construction: the store owns a writer pool of one
   connection (every transaction `BEGIN IMMEDIATE`) and a reader pool under
   `PRAGMA query_only=ON`. In-process writers queue on the pool's fair async
   acquire. `busy_timeout` stays as the wall against out-of-process writers.
   The `SpendingLock` stays (it spans the guard's hold plus createAction).
2. The rule is lint: `StorageSqlx::write_tx()` and `writer()` are the only
   write paths; `Pool::begin` is a clippy disallowed method in both repos.
3. No network await under the write lock: createAction's BEEF verification
   moves before `BEGIN`; every other transaction audited.
4. The belt, typed: the toolbox's `Error::StoreBusy { code }` (Display
   `Store busy: sqlite code N`) for SQLite 5/6/517; the CLI answers 503
   `STORE_BUSY` with `Retry-After: 1`, never 400. An IMMEDIATE transaction
   fails only at `BEGIN`, so a 503 means nothing was written.
5. Proof: a deterministic 517 reproduction in the toolbox, a monitor-vs-
   createAction soak test in the CLI (fails on toolbox 0.10.0, passes after),
   the full suites, clippy `-D warnings`, cargo deny, e2e once.

**The owner's word of 2026-10-21, later:** bsv-rs is ours, so the typed error
goes end to end: the toolbox's `WalletInterface` impl erases every error into
`bsv_rs::Error::WalletError(String)` today (toolbox `wallet.rs` ~1020-1077);
bsv-rs gains `Error::WalletBusy`, the toolbox maps `StoreBusy` to it in a
`From` impl, the CLI matches the variant (no string match), and the SDK's
HTTP client maps a 503 back to it. A new variant on the exhaustive `Error`
enum is breaking: bsv-rs 0.8.0, so bsv-tracker and the toolbox repin too (the
lock rule: one bsv-rs per lock).

**Sequence.** bsv-rs 0.8.0 first; then bsv-tracker repin (0.8.0) and toolbox
0.11.0 (the writer plus the `From` impl, bsv-rs 0.8.0) together; then the CLI
0.16.0 pinning all three. The CLI's nine transaction sites need the toolbox's
API. LOW gets the belt after the CLI
release; LOW re-runs its FLEET-REPIN hand on the new binary.

### The stack's rules that bind this wave (read 2026-10-21 from ~/bsv/bsv-stack-lean)

- **The SDK reads the status word, never the HTTP code** (`docs/BSV-NORTH-STAR.md`
  section 5). So bsv-rs's http_json client must map the BODY's code
  (`STORE_BUSY`, or whatever word the wallet charter names) to `WalletBusy`;
  the 503 and `Retry-After` are the transport's courtesy, not the signal. The
  lane's commit 8be73fb maps on the 503 alone: a fix after its review.
- **Every incident becomes a scenario in the corpus before it becomes a fix**
  (`docs/WHAT-THIS-IS.md` section 3, the handoff loop); a fix that does not
  change a matrix cell is not a fix. #44 needs a witness scenario under
  `corpus/scenarios` and a row in the wallet or storage charter (the stack
  captain lands it; this captain drafts the scenario and the ask).
- **A claim without a run is labeled as such**; evidence is a log at a path.
- **Releases**: a PR per change with the required checks green on its head,
  linear history, zero approvals, publish from the tag by trusted publishing
  (each repo's RELEASING.md); the stack keeper records pins in `stack.toml`
  and the dependabot loop carries the repin.
- **Exhaustive enums** (`docs/RULINGS-2026-10-09.md:32-36`): a variant added to
  an exhaustive public enum is one minor through the gate AND the enum made
  `#[non_exhaustive]`. bsv-rs's `Error` and the toolbox's `Error` are both
  exhaustive today; the fix lanes add the attribute with the variant.
- **Caret in libraries, exact in deployables** (`docs/RULINGS-2026-10-12.md:58`):
  the toolbox and the tracker declare bsv-rs by caret (a new line is a
  minor), the CLI pins exact with a committed lock. Who lands bsv-rs: the
  stack runs the lane and lands end to end, the bump and the CHANGELOG
  heading in the PR, the tag equals the manifest (`:453,457`). The stack
  keeper stamps `stack.toml` (stale at L1 today; the re-stamp is owed).
- **The door is one step** (`lean/Wallet.lean:105-108,339-350`): a refusal
  changes no state; `onCreate` locks every input, appends the action and adds
  the coins in one step. `ReleaseOnlyOnEvidence` (`:568-577`): a coin goes
  from `locked(t)` to free only when `t`'s word became abandoned, rejected or
  conflicted. So the toolbox lane's undo must be indistinguishable from a
  refused create (nothing visible remains), and the crash window's leftover
  (an `unsigned` row with locked inputs and no BEEF) is the model's `unknown`
  action, abortable, with no rule on who finds it. **Option B, preferred if
  the review bears it out:** keep the BEEF build inside the lock but make it
  local by construction (stored proofs and the host's headers only; the
  provider fetch leaves createAction, it is the backstop's job and already a
  finding, #39), so the door stays one step and there is no undo and no
  window. Decide on the review's answer to its questions 1 and 5.
- **Nearest precedents for the answer's shape**: 503 when the server could
  not decide and a retry may clear it (`conformance/README.md:77-88`); the
  payment middleware's 503 `ERR_PAYMENT_UNAVAILABLE` when its store throws
  (`docs/RULINGS-2026-10-12.md:453`); the SDK's typed 429 carrying the wait
  (`lean/Sdk.lean:102-112`, `rateLimited_carries_wait`). No rule names
  `STORE_BUSY` or a wallet door's status map: the wallet charter gains the
  row with this wave.
- **No scenario covers store contention** (`maps/events.json` has no such
  event; nearest cells `storage_split`, `race`). The witness exists as the
  CLI's soak test (fails on toolbox 0.10.0); the corpus scenario and the
  charter row are drafted for the stack captain after the reviews.
- **The six tests** (T1 scale to T6 executable) hold for this design: no
  scan, no explorer, no operator switch; the rule is in the store's
  construction and a replayed test.

**Attribution note (the tracker lane's finding):** lane commits were briefed to carry `Co-Authored-By: Claude Fable 5.1` while the lanes ran on Opus 5.5; from here a lane's brief says to name the model that ran it. The captain's own commits keep the Fable line.

**The owner's word of 2026-10-21 on models:** review and design lanes of this
wave run on Opus 5.5 (`--model opus`); the launcher's Fable default is not
needed. Code lanes already landed on Opus.

**Two gaps the scenario's Boundary found in toolbox 02727dd (for the toolbox
fix lane, with the review's findings):**

- **A busy after the commit is a false promise.** At 02727dd createAction
  commits the allocation, then assembles the BEEF unlocked; a failed assembly
  is undone in a second transaction. If the assembly's own error is a store
  busy (another process past busy_timeout), the daemon would answer 503
  "nothing was written" over a committed allocation. Either the door is one
  step again (option B) or busy is classified only for a failure at BEGIN,
  never after a commit.
- **The writer pool's acquire ceiling.** sqlx's pool acquire timeout (30 s by
  default) yields a pool error that is not StoreBusy and that the belt does
  not name. A writer not acquired is a busy that wrote nothing: classify it
  so, and bound the wait on purpose (the acquire timeout is the store's
  ceiling, set and documented).

**LOW's word of 2026-10-21 18:0xZ (captain-bsv-low, cross-session):** #44 hit
LOW three times today on three different 0.15.0 seats, each within seconds of
a monitor tick, none with a concurrent createAction (the evidence is LOW's
comment on the issue). The owner's word to LOW: no workaround on their side;
the fleet re-pins the day a release carries the fix. Replied: nothing
pinnable yet, the belt branch is not the fix, the CLI 0.16.0 is the pin, the
version and sha go to LOW the minute the tag publishes, and a fleet hand the
same hour is the field proof.

**The contract word (LOW's second message, bsv-low #828):** LOW's stake driver
keeps its in-flight guard on every 5xx (other wallets answer 5xx after a
broadcast), so the body word `STORE_BUSY`, exact, is the whole contract: on
it LOW takes the promise, clears the intent and retries after Retry-After,
bounded. The CLI writer lane and the 0.16.0 release notes carry the word
exact, the shape `503`, `Retry-After: 1`, `{"code":"STORE_BUSY","message"}`,
and cite bsv-low #828 beside #44.

**Two gaps the captain found in his own F1 brief (2026-10-21), for the
re-review and a follow-up lane if cli44-toolbox-fix2 misses them:**

- A dry run uncovered only because other callers' in-flight reservations hold
  the coins is not insufficient funds: the call waits a bounded time for
  those reservations to release (they release on return), re-picks, and if
  still uncovered refuses StoreBusy (MovedUnderUs or a reason of its own),
  never InsufficientFunds. A test: eight coins, nine concurrent callers, the
  ninth answers busy or done, never insufficient funds.
- The BEEF built from stored bytes down to a stored proof is uncapped (the
  reviewer: bounded only by how often the backstop stores proofs). Either a
  documented bound with a refusal naming the depth, or the monitor's
  guarantee that every held coin's ancestry reaches a stored proof within N
  blocks (the backstop's charter), stated and tested; not silence.

**LOW's land condition (third message, 2026-10-21; FIX-828 not sound until it
holds):** the CLI's spend gate runs `restore` AFTER the toolbox's createAction
has posted (`src/spend_gate.rs` run_held: hold, the door, restore), so a busy
there would be a STORE_BUSY that follows a broadcast, and LOW would re-stake.
Condition: STORE_BUSY only when nothing was posted or written; any fault after
the post keeps its own word; expose Retry-After in CORS; the word in no 4xx.

**The captain's decision (read: spend_gate.rs 834-916; the toolbox branch's
`ChangePick::Within { allowed }` at create_action.rs 2369-2372 already
restricts the allocation to a named id set):** the hold-and-restore is itself
a shortcut and is retired, not re-coded.
- Toolbox (a third lane on `lane/cli44-toolbox`, after fix2): an OFFER: an
  inherent `Wallet::create_action_offering(args, originator, offer: &[i64])`
  (the WalletInterface method unchanged) threading the host's offered output
  ids into create_action_internal, where the dry-run pick and the locked
  `Within` set are intersected with the offer; an empty offer with a required
  change input refuses insufficient funds (the host's listing stays the
  refusal's source); a test that a coin outside the offer is never allocated
  under concurrency.
- CLI (the writer lane): `run_held` becomes one call with the offer; `hold`,
  `restore`, `spend_gate_holds` and STALE_HOLD_SECS go; a one-time
  `restore(None)` at daemon start clears holds a 0.15 daemon left, then the
  table is unused (a note in the CHANGELOG). The rule record
  (`spend_gate_actions`, the allowPending word) rides as an action LABEL the
  toolbox writes inside its one transaction; `underRule` is read from the
  label; no CLI write after the door. STORE_BUSY only on the door's
  `WalletBusy` or a pre-door busy of the gate's own read or write; every other
  fault keeps its word and status; `Access-Control-Expose-Headers: Retry-After`;
  the word in no 4xx body; the belt's string match replaced by the typed match.

**E2E prep for the 0.16.0 release (2026-10-21):** the e2e harness deps and
Playwright's chromium installed under tests/e2e; no daemon on :3320 or :3322
right now; the funder wallet (`e2e-funder/`, 0.15.0 binary at
`~/bsv/targets/bsv-wallet-cli-0150/release/bsv-wallet`) holds 14,227 sats, a
run funds 50,000 by default (FUND_AMOUNT) and must come back but for fees: a
top-up from the main wallet is the owner's call (asked). The run happens ONCE,
on the repinned release binary. The funder daemon runs on :3320 on the 0.16.0
binary (pid 91957, log /Volumes/Lanes/out/bsv-wallet-cli/cli44-e2e/funder-daemon.log)
with CHAINTRACKS_URL=https://rust-chaintracks.dev-a3e.workers.dev, the
production header service the other daemons use (api.calhouninfra.com answered
521 on 2026-10-21; the memory corrected). The harness's spawned wallets inherit
the environment: export CHAINTRACKS_URL before `node tests/e2e/run.js`.

**The funder top-up, blocked on the wallet (2026-10-21, owner's green light
given):** the owner said go; the "main wallet" the memory names (deposit
address 19Xh3PH7exNnBC5mmYVfrBKQnjLyya93vm, identity 034aa446..., the worm's
wallet) holds 19,816,671 sats on chain in 34 UTXOs, but no wallet database on
this Mac or on /Volumes/Lanes carries that identity (every wallet.db scanned by
users.identity_key and by spendable outputs at its hash160), and no .env on the
machine derives it; the checkout's own ROOT_KEY derives
1699U2kLDdU1tyr8DEEYqms5xK9XgscB2K (7,207 sats on chain, the checkout's
wallet.db is empty). The desktop MetaNet wallet on :3321 is the owner's
(registry: never used by tests) and lists no spendable default output. The
bsv-dd wallets in the vault (x402 15.6M, return 38.5M, gifts 4.6M) belong to
that project's captain and were written today. `~/bsv/bob-wallet` is April's
e2e wallet-A (its db says 550,034 spendable; its deposit address holds
5,000,000 on chain the db does not know). The minimum fund amount is 50,000:
scenario 22 (rapid-internalize-spend) needs 10 x 5,000 and scenario 03 sends
15,000, so FUND_AMOUNT cannot be lowered. The ask to the owner: either send
~100,000 sats to the funder's address 1DgDKDU4cxMekmj8ZrQKmXEVutPuHjybpX from
wherever the main wallet lives now (the funder imports it with `fund <beef>` or
the courier `receive <txid>`), or name a wallet on this machine the captain may
spend from. No spend made.

**The top-up made (2026-10-22 01:36 UTC, on the owner's word "you do it" and
"port 3321 works too"):** the owner's desktop MetaNet wallet on :3321 paid
100,000 sats to the funder's deposit address by a plain `/createAction`
(txid 4617addc6a9ee7264ca3c8b749bc7856079f663c32b3e1f7225f2ff85ea8bec8, vout 0,
broadcast by the desktop wallet, GorillaPool ARC accepted the funder's
re-announce; TAAL answered an error on the same bytes). The funder daemon was
stopped, `fund <atomic beef> --vout 0` internalized it on the 0.16.0 binary,
the daemon restarted (pid 30944, same log). Funder balance 114,227 sats, the
100,000 unmined; the gate offers a received coin only at `mined`, depth 1, so
the e2e run waits for the funder's own proof record (proven_txs) of that txid,
not the explorer's word. bob-wallet was reconciled in dry-run only (13 of its
22 rows unknown to the chain; nothing applied, nothing spent from it).

**The e2e runs of 2026-10-22 (the harness was stale, not the wallet):**
run 1 (01:49Z, log e2e-run-20261022T014948Z.log): 9/22; every spending
scenario answered 402 "no offerable coin ... offered under allowPending:
received": the test wallet's only coin is the funder's received, unmined
payment, which the 0.15 gate (R1 (d)) holds from a plain spend; the harness
had not run since the gate landed (its last change 2026-04-02). Also `address`
prints two lines now and the harness took both. The harness deleted wallet A
with 50,000 sats in it (teardown removed the dir, key included): LOST.
Fix f2d6fc4: the test wallets' client names allowPending: received on every
createAction, the CLI scenarios pass --allow-pending received, the address is
one line, a failed sweep keeps the dir. Run 2 (01:52Z, log
e2e-run-20261022T015259Z.log): 17/22; from scenario 14 on A's HTTP answers
hung 180 s after each action went through: serve was spawned with stderr on a
pipe nobody read, and the log (bsv_wallet=info,tower_http=info) filled it.
Scenario 15's nosend locked A's 7,563 and the abort never ran; A (0 at the
read) and B (5,000 paid to it, never internalized) were deleted: LOST ~12,563
more. Fix 56b4e2b: the wallet's log goes to serve.log in its dir, the funder's
funding transfer names the word (its coins after a run are the sweeps,
received and unmined), a failed run keeps both dirs. Fix c029d4e: a full run
funds 100,000 (scenario 22 needs 50,000 in A at its start and the earlier
scenarios move ~40,000 out of A; the April runs funded ~300,000), and the run
refuses when the funder cannot cover it; CLAUDE.md updated. Losses in all:
~62,563 sats of the owner's, named here. LOW told STOP after run 1; nothing
tagged.

**The second top-up (02:03Z) is UNSIGNED on the desktop wallet:** the
request for another 100,000 exceeded the MetaNet client's per-origin spending
allowance (its "Renew spending permission" actions), the client waits on a
prompt on the owner's screen, the HTTP request timed out at 120 s. No second
request was sent (approving the prompt would sign the first). A watcher polls
the funder's address for the payment; on arrival: stop the funder daemon,
`receive <txid>`, restart. The funder holds 50,772 (all three coins offerable
under the word); run 3 needs 101,000.

**The top-up prompt overnight (02:03Z to 10:10Z, 2026-10-22):** no approval
came. Learned: the MetaNet client holds an unsigned action only while the HTTP
request that asked for it stays connected; when the request times out
client-side the action drops off `listActions` (the count fell 8027 to 8026
each time), so the two "disappearances" of the top-up were the captain's own
timeouts, not a dismissal by the owner. A held-open request spends nothing
until the owner clicks the client's spending-permission prompt; the captain
re-posted it three times (30 min, 6 h, 6 h), one pending at a time, never two.
The chain watcher on the funder's address saw nothing over 150 minutes; the
funder's address holds 10,000 (old) and 36,658 (B's sweep, height 972119), its
4,114 own change sits on a derived address: 50,772. At 10:10Z one request is
pending on the desktop wallet, held to 16:10Z; LOW on the stop (record 1041);
run 3 not started.

**Run 3 (2026-10-22 10:55:57Z, log e2e-run-20261022T105557Z.log under
/Volumes/Lanes/out/bsv-wallet-cli/cli44-e2e/): 22 of 22 PASS.** The owner
approved the desktop wallet's prompt at 10:55Z (a "Renew spending permission"
action signed; the held-open top-up errored on a stale reference in the
client, a fresh request then signed at once: ee7423ed…, 100,000 to the funder,
couriered in by `fund`, funder 150,772). The run exited 1 on its budget line
only: wallet B's full sweep (70,405 of 70,705) was refused "need 70724, have
70705" (the 300-sat fee margin was 19 short for a 13-input sweep), the halving
retry swept 35,202 and left 35,360 in B's kept dir. The 13 "coins held back"
in that refusal are B's own spent inputs (locked by its earlier txids), not
gate holds: all of B's balance was offered. The captain drained B back to the
funder (5d355f65…, 34,985 after a 375 fee, `--allow-pending received`) and
the funder received it: 148,830. Spent by the run: 1,942 sats, within the
5,000 budget. Harness fix (this commit): the sweep retry reads the refusal's
"need N, have M" and retries short by exactly that; the budget judges spent
sats apart from stranded ones and names stranded sats as their own failure.
No Rust changed; the e2e gate is met.

**Tagged and published (2026-10-22 11:1xZ to 11:3xZ):** PR 46 rebase-merged
(main 5e3264b, the branch deleted); `v0.16.0` annotated on that commit and
pushed; release run 39768248997 success; crates.io bsv-wallet-cli 0.16.0;
the GitHub release carries the three binaries. LOW got the tag message with
items 1 to 4 and the green light (msg 0ce601ad…); #44 (already closed by the
merge) got the evidence comment. The local main had 54 unpushed HANDOFF-only
commits (the captain's journal, every line of it present in the release
branch's HANDOFF but three superseded ones): saved on branch
`handoff-journal-local` (7512a45), local main reset to origin/main.

**The stack's scenario landed (the stack captain, 11:1xZ):** lane/cli44-scenario
acc2d10 is on the stack's main as 07180d1 (picked as 806338e); the two red
generated-doc legs were drift, re-rendered at the pick; 32 PASS at the
landing; the worktree and branch removed. The four asks (S9, the busy hosting
row, R8 (a)/(i), the events row (a)) are with the owner as options; the word
comes through the stack captain. A second bsv-rs finding from the scenario's
Boundary, beside the BEEF-order one above: bsv-rs 0.8.0 keys WalletBusy on
the HTTP 503 alone (http_json.rs, the SERVICE_UNAVAILABLE branch), so a
proxy's 503 in front of a wallet that never ran reads as the wallet's promise
that nothing was written; the proposed rule reads the body's word STORE_BUSY
(and ERR_SERVER_BUSY). Both findings belong to a bsv-rs 0.8.1, which would
move this crate's pin.

## The bsv-rs 0.8.1 wave and CLI 0.16.1 (2026-10-22, 11:4xZ to 15:1xZ)

**The defect and the ruling.** bsv-rs 0.8.0 judged the atomic BEEF rule by wire
position (the Lean's NL-1 Tip: the subject the last raw transaction, every other
entry spent by a later one) in both readers; BRC-95 states a graph rule; the
TypeScript SDK 2.x writer emits a raw unproven ancestor after the proven
transaction that spends it (the bsv-dd x402 refunds wallet C refused, P0-65).
The owner ruled, on the stack's DECISIONS-2026-10-19.md section 10 and epoch
channel line 1044 (12:03Z, "yes to all 3"): (a) the graph rule, BRC-62's
ordering clause kept; (c) example_atomic_parent reads unrelatedTransaction;
(d) BRC-95 vectors upstream. A second question from the review: the current
TypeScript reference (ts-stack 4d0ea63, sdk 3.x) cuts the graph at each
proof, refusing the shape the ruled rule accepts; the owner kept the rule
(13:23Z, line 1046, "I agree with your recommendation"): readers follow every
raw ancestor, the writer emits the closure cut at proofs as TS 3 and Go do.
Settled on the ruled text's letter with the stack keeper: the refusal names
the earliest entry, by the trie order, the walk from the subject does not
reach (the Lean's walk).

**bsv-rs 0.8.1.** Lane lane/atomic-graph (code, Opus): PR 72, four commits
(c1837d1 the graph rule in both readers, one predicate; eda49f5 the review's
folds: TS 3's cut stated, the two TS-3-refused vectors `intended`, the
edge set only under a subject, a BSC2 cursor read as BSC3; 55404b6 the atomic
writer emits the BRC-95 closure; bf46677 the refusal names the earliest
unreached entry, the edges kept per raw entry under a subject, spec_size and
work pins moved to the Lean's own numbers at 3bb286e). Review lane (Opus):
RELEASE WITH FIXES, 6 findings, all folded; its proof that 0.8.1 accepts a
superset of 0.8.0 (0.8.0's stream tests pass but for the moved words).
Verified downstream before the tag against the PR's head: this crate's suite
446/0 (the CLI pins bsv-rs exactly, so the patch needed the pin moved to
0.8.1 in the throwaway), the toolbox 0.11.0 suite 1,191/0 (CI's commands).
Merged (rebase), tagged v0.8.1 at bf46677, release run 39795082316 success,
crates.io 0.8.1 at 14:40Z. The stack's BEEF door runner re-ran on it: every
structural verdict agrees, the three ruled words included (wrong_subject
278, unrelated 278, example_atomic_parent 521). Downstream to re-check when
they move to 0.8.x: bsv-messagebox-cloudflare-public's beef_door.rs tip_of
hard-checks the cursor magic (BSC3 now carries the input count and the
edges); pinned =0.7.0 so nothing broke.

**CLI 0.16.1.** PR 48 (one commit bf0634f: the pin =0.8.1, CHANGELOG, README,
lock; no code), rebase-merged, tagged v0.16.1, release run 39799648326
success, crates.io 0.16.1 at 15:08Z, three binaries. LOW and the stack keeper
have the tag messages; the bsv-dd captain told wallet C moves off 0.15.0.
The e2e harness binary link moved to ~/bsv/targets/bsv-wallet-cli-0161.

**Upstream, on the owner's green light ("you have the green light for that"),
from the Calgooon account:** bsv-blockchain/BRCs#350 (BRC-95's two readings,
a wording proposed: membership through every raw ancestor, sufficiency up to
the proofs); ts-stack#1320 (the 3.x reader refuses the 2.x writer's bytes;
2.x accepts and writes unrelated entries; mergeRawTx's reversed txid; no
corpus vectors) since bsv-blockchain/ts-sdk is archived; go-sdk#384 (no
unrelated rule at all); the corpus PR ts-stack#1321 (vectors/sdk/transactions/
atomic-beef.json, 8 vectors registered in META.json under BRC-95 and BRC-62,
validator 78 files / 6,714 vectors clean; PARITY_MATRIX.json is generated and
untouched).

**LOW's loop 43 question (13:5xZ), resolved by LOW with no defect:** two fleet
seats answered 402 while holding a fat pool; every coin was unproven after a
69-minute block gap, their payouts received without the word and their own
hops chained, so the only offerable coin was the last hop's change, under the
stake. The gate's rule is 0.15.0's unchanged (hold_reason: own made change
offered under SPEND_OWN_CHANGE=1 with no ancestry condition; a received
pending coin needs the word; a headers fault holds every coin). The fleet's
RUST_LOG excluded bsv_wallet, which is why the 402's held list (WARN
"request error" at bsv_wallet::server::handlers, one line per held coin; the
body's `held`) was not in its logs; LOW adds bsv_wallet=info and records the
body. The toolbox's "stored ancestry is incomplete; picking again without it"
is a different door, answered 409 UNSPENDABLE_COIN, never 402.

**LOW's four asks at the tag (bsv-low record 1037, 2026-10-21):** items 2 and 4
answered in the interim (lock figures 0.11.0 / 0.8.0 / 0.8.0; the upgrade rule
verified: no migration between toolbox 0.10.0 and 0.11.0, the CLI's tables
unchanged since 0.15.0, the one first-open write is `heal_holds` when
spend_gate_holds has rows, rollback to 0.15 clean but for underRule on
0.16-made allowPending actions); items 1 (tag, sha, crates.io) and 3 (tests
added since 30bb1f060, expected none) go the minute the tag exists.

**A bsv-rs finding relayed by the owner (2026-10-21, from the bsv-dd x402
captain):** the in-memory BEEF structure check in bsv-rs refuses the TypeScript
SDK's order (proven transactions first, unproven ancestors after) as
UnrelatedTransaction while the streaming reader accepts the same bytes; the fix
belongs in bsv-rs (judge the atomic rule on the graph, not wire position). That
captain carries the fix in their queue; recorded here because 0.16.0 pins
bsv-rs 0.8.0, which has the defect, and a 0.8.1 would move the pin.

### Lanes

| lane | repo | class | state | brief |
|---|---|---|---|---|
| cli44-toolbox | bsv-wallet-toolbox-rs (`~/bsv/bsv-wallet-toolbox-rs`, worktree cli44-toolbox, on the box) | code | DONE: `box/lane/cli44-toolbox` at 02727dd (second attempt; the first ended with no commit) | pool() is the reader (10 conns, query_only), writer() one conn, write_tx() BEGIN IMMEDIATE, Pool::begin a clippy disallowed method; 15 transaction sites and ~186 single-statement writes moved; Error::StoreBusy { code } for SQLite 5/6; create_action now checks BEEF roots before BEGIN, commits the allocation, builds the BEEF unlocked, undoes on failure (a half state: an unsigned row without input_beef between commit and build; the review must weigh it); tests/store_write_lock_tests.rs (6) + 4 lock-ordering tests; cargo test 1136 pass 35 ignored; clippy, fmt, deny green; log at /Volumes/Lanes/out/bsv-wallet-toolbox-rs/cli44/cargo-test.log |
| cli44-toolbox-review | bsv-wallet-toolbox-rs (on the box, Opus) | review | DONE 2026-10-21: merge with fixes F1 to F3; a design call on createAction | the store-wide part is right and fixes #44. F1 `#[non_exhaustive]`. F2 the undo's failure returned StoreBusy over a committed allocation (false promise). F3 the undo is not a refused create: labels, tags, baskets survive; inputs restored to spendable=1 regardless; the sequence moves. F4 code 6 is never transient: StoreBusy should be 5 only; sqlx PoolTimedOut (30 s default) is not StoreBusy. F5 a test checks Display only. F6 (predates) find_or_insert_output_basket reads on the reader inside the write tx: two outputs to one new basket fail. F7 (predates) the public allocate_change_input has no guard. F8 the CLI's tests write through pool() and set busy_timeout on one reader: the CLI writer lane moves them. Q1: the crash leftover is recovered by FailAbandonedTask/abort_abandoned after 5 min (counts as abandoned), but the undo path itself cannot be read as a machine step. Q2: option B is not local by construction (the CLI's FallbackChainTracker caches 1000 positive heights in memory; a miss is HTTPS), so the reviewer recommends option B with a bound: fetches out, tracker cache-first with a 2 s deadline, a timeout a refusal. Q3 all 280 writes on the writer. Q6 no non-DB await under any lock. 7 mutations each caught; 517 repro 20/20. fmt, clippy, tests 1136/0/35, deny green. Logs /Volumes/Lanes/out/bsv-wallet-toolbox-rs/cli44-review/ |
| cli44-toolbox-fix | bsv-wallet-toolbox-rs (worktree cli44-toolbox, on the box) | code | DONE: four commits over 02727dd, tip 87d3d43 on `lane/cli44-toolbox` | 49a90bf F1 non_exhaustive, F4 StoreBusy = code 5 only + the writer's acquire ceiling 5 s (STORE_BUSY_CEILING_MS), F5; 5147df6 F6 the basket read on the transaction; 89184eb F7 the guarded claim (the race test fails 3/3 without it); 87d3d43 OPTION B, the preferred shape: candidates picked and their proofs checked against the tracker before the lock, one BEGIN IMMEDIATE allocating only the checked set, a moved store rolls back and re-checks once then refuses, no undo, no provider fetch in createAction (a missing ancestor refuses naming the txid), the success response equal to main's in a masked scratch comparison; 7 tests added, the undo test replaced by a_beef_failure_refuses_with_nothing_written; mutations caught; cargo test 1144/0/35, clippy, fmt, deny green; two literal /tmp dumps moved to temp_dir. Caveat it raised: a reader-pool timeout also reads as StoreBusy (fixed by the follow-up). Logs /Volumes/Lanes/out/bsv-wallet-toolbox-rs/cli44/ |
| cli44-toolbox-sdk | bsv-wallet-toolbox-rs (the studio worktree `/Volumes/Lanes/worktrees/bsv-wallet-toolbox-rs/cli44-toolbox`) | code | DONE: four commits, tip bef4d43 on `lane/cli44-toolbox` (bba6130 bsv-rs 0.8.0 caret, one in the lock; 576ed44 the typed path, From impl, the erasure sites replaced; 1a8aede the writer-only timeout rule; bef4d43 CHANGELOG for 0.11.0) | its note to read: 'signAction no longer aborting on a busy store' (the second review judges it) |
| cli44-toolbox-review2 | bsv-wallet-toolbox-rs (on the box, Opus) | review | DONE 2026-10-21: DO NOT RELEASE, two reasons | (1) the box saw only 02727dd..87d3d43: the SDK lane's four commits lived only in the studio worktree (the captain pushed `lane/cli44-toolbox` to origin after; every lane now fetches origin first). (2) F1 HIGH: concurrent createActions pick the same best coin in the dry run; losers retry once then refuse InsufficientFunds (402) on a funded wallet: 2 of 4 refused at 87d3d43, 4 of 4 succeed at 02727dd, 1 of 4 on main (the #44 bug). F2 MED a pre-checked coin with incomplete ancestry refuses the whole action, text blames inputBEEF (400). F3 MED PoolTimedOut from any pool was busy at 87d3d43 (1a8aede claims the fix; unseen). F4 begin/commit non-busy text moved to 'SQLx error' (host 500 to 400). F5 at 87d3d43 signAction wrapped and aborted on busy (576ed44 claims the fix; unseen). F6 pre-check writes (root-check records, demotion, default basket) survive a refusal: legal evidence steps, wording 'the action writes nothing'. F7 listOutputs also still fetches. Option B otherwise one step: one write_tx, the pick bounded to the pre-checked set, moved-store detection complete but for a header move (main's window too), two pre-checks at most; the masked response diff identical; the longer raw chain is valid BRC-62, uncapped. Mutations: each caught by one test; none catches F1. Checks at 87d3d43 green. Evidence /Volumes/Lanes/out/bsv-wallet-toolbox-rs/cli44-review2/ |
| cli44-toolbox-fix2 | bsv-wallet-toolbox-rs (on the box) | code | DONE: six commits, tip c443eff pushed to origin (90e07a5 StoreBusy { reason }; 610225e the in-flight reservation + MovedUnderUs; 9c10e69 the pre-check skips an unbuildable coin, 'Unspendable coin:' refusal (host 409 recommended); b827bda begin/commit prefixes; 516195e createAction's processAction busy not aborted; c443eff CHANGELOG) | 1165/0 and 1270/0 with full; clippy, fmt, doc, deny green; six mutations red. THE CAPTAIN'S FINDING: 516195e breaks the contract for createAction (WalletBusy with the create step committed and coins locked); 610225e sends an unread root to MovedUnderUs (not honest: the header service did not answer). Both go to fix3. The lane's open question (the reference in the busy message) is moot once the door is one transaction |
| cli44-toolbox-fix3 | bsv-wallet-toolbox-rs (on the box) | code | DONE: five commits, tip d0efed5 pushed (54090b5 the one-transaction door via create_action_at_door + a local signer closure, process_action_writes on the open connection; 4080d44 the post-broadcast rule, marker post_broadcast_status_write_failed, SendWaitingTask heals; 37c455f an unread root refuses 'Service error: ... at height h'; 108335a the offer create_action_offering; d0efed5 docs) | 1173/0 and 1278/0; clippy, fmt, deny, doc clean; ten mutations caught. Unverified by the lane: the internalizeAction post-broadcast site has no injected-busy test; the StorageClient (remote storage) fallback uses separate steps with an abort (busy there is 'Storage error:', never WalletBusy) |
| cli44-toolbox-review3 | bsv-wallet-toolbox-rs (second attempt on the box, Opus) | review | DONE 2026-10-21: RELEASE WITH FIXES (F1 required) | F1 HIGH abort_action.rs:233: after the root's abort commits, retire_poisoned_descendants acquires the writer; a busy there is WalletBusy after a committed write (a repeat then fails, descendants half-retired). F2 MED the ninth caller: a dry run short only because of others' in-flight reservations answers InsufficientFunds (the captain's own flagged gap, confirmed by probe). F3 internalize's post-broadcast site untested (mutation m14 survives). F4 the locked-set offer check never decides (m15 survives; defense in depth). F5 the door's commit non-busy text now 'Database error:' (CHANGELOG). F6 the raw ancestry runs through unmined own change (SPEND_OWN_CHANGE=1) to the first stored proof: a stated guarantee + a filed follow-up, not a blocker. The door holds (Q1), the post-broadcast rule holds at all three sites (Q2), outcomes honest but F2 (Q3), texts unchanged but F5 (Q5), 15 mutations 13 caught. Checks all green. Report /Volumes/Lanes/out/bsv-wallet-toolbox-rs/cli44-review3/REPORT.md |
| cli44-toolbox-fix4 | bsv-wallet-toolbox-rs (the studio) | code | DONE: four commits, tip 24db2ad pushed (e1ae38e F1: a busy retirement after the root's abort commit is the abort's success, marker post_commit_retire_failed, ReviewStatusTask retires the descendants, retire_one_poisoned writes the request status first; d1d7cea F3 the internalize test + hook; 26761ff F2 StoreBusy { InFlight }, immediate, never InsufficientFunds, nine-on-eight 8/8; 24db2ad CHANGELOG/docs) | 1179/0 and 1284/0; clippy, fmt, deny, doc green; three mutations caught. The audit: every other door clean. The F6 follow-up FILED: Calhooon/bsv-wallet-toolbox-rs #40 (the monitor guarantee, measured) |
| cli44-toolbox-review4 | bsv-wallet-toolbox-rs (on the box, Opus) | review | DONE 2026-10-21: RELEASE WITH FIXES | F1, F2, F3, F5, F6 hold; the audit redone: every door OK but one. G1 MED (contract): acquireCertificate commits the certificate row then writes each field separately (wallet.rs:2451/2472; certificate_issuance.rs:760/777, the latter after the HTTP exchange with the certifier): a busy at a field insert answers WalletBusy with the row committed and no fields; the repeat hits UNIQUE; the certificate is stuck. Fix: one write_tx for the certificate and its fields (preferred) or not_busy at the sites. G2 LOW-MED: review_status check 4 reads chain evidence for the root only: a spender carrying a mined word is retired (the unfail canary recovers it later); skip the root when any descendant has evidence. G3 a proven descendant repeats a log line every pass. G4 the req-before-status reorder has no test (m4 survived). G5 'wrote nothing' is absolute while the pre-check's idempotent read-check/proof-cache writes exist. G6 doubleSpend roots acceptable. Checks green; nine-on-eight 30/30; 6 of 7 mutations caught. Evidence /Volumes/Lanes/out/bsv-wallet-toolbox-rs/cli44-review4/ |
| cli44-cli-writer | bsv-wallet-cli (on the box) | code | DONE: nine commits + a DEV ONLY patch commit, tip 6a05e73 pushed to `origin/lane/cli44-cli-belt` (476aa34 every write on the writer: 11 transactions, 19 production writes, 67 test seeds, clippy bans Pool::begin; abe7f1a the offer, hold/restore gone, heal_holds once in server::run; fdec089 the label spend-gate:received-pending, record_rule/fill_txid gone; a3f5d1e the typed 503/409/CORS, the substring belt deleted; e01ed16 the soak live, 300 rounds zero errors x4; 7b5d6cc --json busy; 65e003d README/CHANGELOG; 49cb1ff a NULL-reference fix; 6a05e73 the patch) | workspace 431/0, integration 57/57, clippy, fmt, deny green. Waits for toolbox 0.11.0 on the index (never appeared in 30 min, as expected). Its notes to check: the broadcast follow-up still writes after the door (a fault there must never be the call's error: the review checks); 503 at every door on the typed busy; reconcile_outputs.rs keeps its own pool (autocommit only) |
| cli44-cli-review | bsv-wallet-cli (on the box, Opus) | review | DONE 2026-10-21: RELEASE WITH FIXES | F1 the pin must carry fix4 (abort's post-commit busy, InFlight). F2 MED split's post-broadcast internalize `?` loses the txid. F3 MED respend's post-post `?`s lose the replacement txid. F4 LOW-MED the rule label is forgeable by a caller (underRule, respend allow_received). F5 follow-up writes lack a marker. F6 heal_holds only in serve/daemon. F7 the lazy offer never retries on Unspendable coin. F8 docs (counts, scope, #828, the post-broadcast half). F9 info: PoolTimedOut from the reader counts as busy; THE DAEMON OPENS TWO STORES ON ONE FILE (its writers queue on SQLite's busy handler, not one fair pool); an offer above ~32k coins exceeds SQLite's variable limit. The CLI's own paths keep the contract (Q1: the follow-up's failures never the call's error; Q2 the typed rule's origin exact). Soak 3/3 (68k to 91k writer commits, 0 failed); the old witness fails 3/3 at round 1 on 0.10.0. Checks green. Report /Volumes/Lanes/out/bsv-wallet-cli/cli44-cli-review/REPORT.md (the box) |
| cli44-cli-fix | bsv-wallet-cli (the box) | code | DONE (second attempt): ten commits ee24e67..fcec3c8 + the patch 9eec737 (now at toolbox c5c341d), pushed with --force-with-lease | F2 to F7, F9a (writer-only PoolTimedOut), F9c (OFFER_CAP 16384 largest), F9b REDONE: store::open_shared opens the file once, the Wallet gets the store, the Monitor and the chain feed Arc::new(store.clone()), the test fails with two opens; the old two-stores commit dropped. workspace 445/0, integration 57/57, clippy, fmt, deny green, soak 104,727 commits 0 failed, all on c5c341d. Logs /Volumes/Lanes/out/bsv-wallet-cli/cli44-cli-belt/ (the box) |
| cli44-cli-review2 | bsv-wallet-cli (on the box, Opus) | review | DONE 2026-10-21: RELEASE AS IS on a =0.11.0 pin carrying c5c341d; three LOW | D1 a test gap (the daemon's second open for the Wallet is caught by nothing); D2 reconcile_outputs still opens the file a second time; D3 coins past the 16384 cap are neither offered nor listed in a refusal. INFO: the Monitor and the Wallet share one services slot (the Wallet's wins); mixed versions on one file unsupported (the heal); two println in tests. The ten fix commits each answer their finding; workspace 445/0, integration 57/0, clippy, fmt, deny; soak 59,213 commits 0 failed; the label strip, the double open in open_shared and the widened PoolTimedOut mutations caught. Report /Volumes/Lanes/out/bsv-wallet-cli/cli44-cli-review/delta/REPORT.md (the box) |
| cli44-cli-fix2 | bsv-wallet-cli (on the box) | code | DONE: four commits 13cf006, f401c0e, 625bf71, 3c3d9bb + the patch d9e3ad1 (at toolbox c5c341d), pushed | D1 the include_str! tests (daemon_tenant one open_shared; no command opens the file beside its store; init/tick/ui once each); D2 reconcile-outputs on the store's writer; D3 the refusal names the capped coins (AppError's refusal boxed for clippy); the mixed-versions note; workspace 448/0, integration 57/57, clippy, fmt, deny; both mutations fail their tests. THE CLI BRANCH IS COMPLETE pending the =0.11.0 repin; the release PR's review covers these four commits and the repin |
| cli44-cli-belt | bsv-wallet-cli (worktree cli44-cli-belt) | code | DONE: `lane/cli44-cli-belt` at e0ff758 | the soak reproduction fails on toolbox 0.10.0 by round 4 in every run (517 in half the runs, code 5 in the rest, both from the deferred BEGIN), committed `#[ignore]`; 503 STORE_BUSY + Retry-After 1 in `AppError::classify`; integration 57/57, clippy clean |
| cli44-bsv-rs | bsv-rs (`~/bsv/bsv-rs`, worktree cli44-bsv-rs off origin/main 00a7bf4 v0.7.2) | code | DONE: `box/lane/cli44-bsv-rs` at 8be73fb, not pushed; NO CHANGELOG entry in the commit (the release PR must add it) | `Error::WalletBusy { detail, retry_after_ms }` (Display `wallet busy: {detail}`), http_json client maps 503 to it (delta-seconds to ms, HTTP-date None); 5 integration + 3 unit tests; wire has no busy code (TS 5/6/7, Go none) so unchanged; fmt, clippy, doc, wasm32, wallet-only green; `cargo deny` fails on origin/main too (no deny.toml there, a separate PR) |
| cli44-bsv-rs-review | bsv-rs (on the box, Opus, trifinlabs) | review | DONE 2026-10-21: merge with fixes F1 to F4 | F1 HIGH the client keyed on the 503 alone and cannot parse the daemon's real body (`code` is `Option<u8>`, a string code falls through); fix: the body's word (STORE_BUSY, ERR_SERVER_BUSY) decides at any status, a 503 without it stays WalletError. F2 HIGH `#[non_exhaustive]` on Error (the 10-09 ruling). F3 the doc states the server's promise, not a fact. F4 the CHANGELOG's breaking paragraph and two false parity claims (the TS client does not carry the text; the TS toolbox edge does answer ERR_SERVER_BUSY). F5 LOW doc the unbounded Retry-After. The wire has no busy code (TS 5/6/7, Go none): unchanged. fmt, clippy, doc, wallet-only, wasm32 green; full suite not run by the review |
| cli44-bsv-rs-fix | bsv-rs (worktree cli44-bsv-rs, `--reuse`) | code | DONE: 546f2ae on `lane/cli44-bsv-rs` | F1 to F5 applied; the 10 busy tests: 5 fail with the branch disabled, 8 of 10 fail on 8be73fb; fmt, clippy, doc, wallet-only, wasm32, full suite 3,335/0/131 green; no downstream exhaustive match on the SDK's Error (grepped by the captain) |
| release bsv-rs 0.8.0 | bsv-rs | captain | PR #71 green on every check (6 test suites, clippy, fmt, docs, msrv, vectors, secrets, unused deps), MERGED (rebase) and tagged `v0.8.0` 2026-10-21; the release workflow (run 39645450451) completed success; crates.io carries 0.8.0 (the index read 17:5xZ) | the consumers repin to `"0.8"` (toolbox, tracker) and `=0.8.0` (CLI) once the index carries it |
| cli44-scenario | bsv-stack-lean (worktree `/Volumes/Lanes/worktrees/bsv-stack-lean/cli44-scenario`, Opus) | design | DONE in substance: `lane/cli44-scenario` at acc2d10 (the lane exited while `make check` still ran, no REPORT; the commit stands) | the scenario (md + json), the README row, storage charter S9 proposed, the wallet hosting row proposed, R8 asked with the recommendation (a) busy as a host answer outside the machine, (i) the body word is the busy, ERR_SERVER_BUSY read as busy too; the events ask: option (a) a new wallet-group event row. FOR THE STACK CAPTAIN TO LAND: its `make check` in the worktree (record `docs/records/20261021-170414-make-check.txt`, still running at 17:4xZ) has the Lean leg green and 40 PASS, with two generated-doc legs red (the divergence report and the public audit re-render: the funnel rule, re-render and land together; whether the lane's rows or drift from origin/main caused them is the stack captain's read). Its Boundary section found two gaps in the fix under review, folded below |
| cli44-tracker-repin | bsv-tracker (on the box) | code | DONE: ee22107 on `lane/cli44-tracker-repin` | bsv-rs "0.8.0" caret, the lock's one bsv-rs; no match on bsv_rs::Error anywhere; test --locked, the vectors, clippy, fmt, doc, wasm32, +1.88 check, deny all green; CHANGELOG Unreleased, README header on ^0.8 |
| release bsv-tracker 0.8.0 | bsv-tracker | captain | PR #25 green on the five checks (+ msrv, secrets), MERGED (rebase) 3cb9e40, tagged `v0.8.0`; release run 39647546150 success; crates.io carries bsv-tracker 0.8.0 (2026-10-21) | |
| cli44-toolbox-fix5 | bsv-wallet-toolbox-rs (the studio) | code | DONE: 5d62af8 pushed | StorageSqlx is Clone: the pools cloned, every other field one shared Arc (field table in its report); five tests incl. a Wallet and a Monitor over one store 300 rounds no busy, and opened-twice (two writers, SqliteBusy after the timeout) vs shared (one writer, the second waits); 1289/0 with full; clippy, fmt, deny, doc green. Its own finding: the settings/identity/name getters hand out references from inside locks through pre-existing unsafe; mitigated by replace-if-changed; fix6 makes it safe. Next in the CLI: daemon_tenant.rs opens one store and gives the monitor a clone |
| cli44-toolbox-fix6 | bsv-wallet-toolbox-rs (on the box) | code | DONE: six commits, tip c5c341d pushed (1f51041 G1 the certificate and its fields in one transaction, a busy after the certifier's exchange a storage error; 69d88d1 G2; c71f89a G3; a899c44 G4 the reorder's test; 9dab6af G5 wording; c5c341d G7 the getters return owned copies, the unsafe gone) | the lane's report text was not captured (its results monitor timed out after the work); review 5 runs every check |
| cli44-toolbox-review5 | bsv-wallet-toolbox-rs (on the box, Opus) | review | DONE 2026-10-21: RELEASE AS IS | all 16 fields of StorageSqlx shared, nothing assumes uniqueness, no unsafe left in the crate, the trait changed with its 3 implementors; the certificate door one transaction (P3 through a second handle: WalletBusy, nothing stored, the repeat succeeds; matches the TS reference's insertCertificate); check 4 exact (P1, P2, P5). H1 docs: insert_certificate_with_fields is a required trait method (the minor's trait break) unstated. H2 optional: a bounded retry on StoreBusy at the post-exchange insert and the serial in the message. Notes: into_error missing from the CHANGELOG; destroy() through any clone. 1190/0 and 1295/0; fmt, clippy, doc, deny; 9 of 9 mutations caught; repeats 20/20 |
| cli44-toolbox-fix7 | bsv-wallet-toolbox-rs (on the box) | code | DONE: three commits, tip ba9c9be pushed (fb9f856 H1; 0a6ffe5 H2 three tries 250/500 ms, the serial and certifier named; ba9c9be notes) | 1191/0 and 1296/0; fmt, clippy, doc, deny green; the mutation (tries 1) fails the test |
| cli44-toolbox-review6 | bsv-wallet-toolbox-rs (the studio, Opus) | review | DONE 2026-10-21: TAG WITH THESE FIXES | J1 MED the final issuance error doubles 'Storage error:' (not_busy wrapped again at certificate_issuance.rs:815; fix: e); J2 LOW the true worst case is ~16 s in one process, ~31 s across (each try waits the 5 s acquire ceiling): acceptable, reword; J3 LOW the tries assertion compares against the constant (pin 'tried 3 times'). The retry sound (StoreBusy only, after the exchange, around the one transaction), the CHANGELOG complete but J1/J2's words; 1191/0, 1296/0, clippy, fmt, doc, deny |
| cli44-toolbox-fix8 | bsv-wallet-toolbox-rs (on the box) | code | DONE: three commits, tip f60771d pushed (J1, J2, J3 exactly); 1191/0, 1296/0, clippy, fmt, deny, doc; tries=1 fails both tests | THE TOOLBOX BRANCH IS COMPLETE: 31 commits, six reviews |
| release toolbox 0.11.0 | bsv-wallet-toolbox-rs | captain | PR #41: the five required checks green after the captain's one-line CI fix (the deprecated `fetch_update` in a test hook, now a compare-exchange loop); MERGED (rebase) 2026-10-21 at ebcfe43, tagged `v0.11.0`; release run 39682136729 success; crates.io carries bsv-wallet-toolbox-rs 0.11.0 | the lesson in every lane brief from here: clippy under RUSTFLAGS=-Dwarnings on the newest stable, as CI does |
| cli44-cli-repin | bsv-wallet-cli (the studio) | code | DONE (second attempt, foreground): d178465 pushed = 3c3d9bb + the repin; the dev patch gone | the registry's 0.11.0 src identical to tag v0.11.0 in all 96 files; the lock: toolbox 0.11.0, one bsv-rs 0.8.0, tracker 0.8.0, no patch (the base lock had been stale at bsv-rs 0.7.0 against the =0.8.0 pin); workspace tests, integration 57, clippy, clippy under RUSTFLAGS=-Dwarnings, fmt, deny, the soak (55,696 commits, 0 failed) all green. THE CLI BRANCH IS RELEASE-READY: the captain's release commit (0.16.0, the heading, HANDOFF.md) and PR next; e2e once before the tag |
| release CLI 0.16.0 | bsv-wallet-cli | captain | DONE 2026-10-22: PR #46 rebase-merged, main 5e3264b, tagged `v0.16.0` (tag object 42f0894); release run 39768248997 success (tag check, nine CI jobs, three binaries on the GitHub release, publish); crates.io carries bsv-wallet-cli 0.16.0 at 11:3xZ | the e2e gate: run 3 22/22 on the 0.16.0 binary; LOW sent the tag message (items 1 to 4) and the green light; #44 commented with the evidence |

**HANDOFF.md lives only on the local main (origin main is protected; every change lands by PR): the 0.16.0 release PR carries it.**

Still to launch, in order:

- after review 5: the 0.11.0 release (the PR with the bump and the heading, the tag publishes).

- review of cli44-bsv-rs; release bsv-rs 0.8.0 through its PR flow.
- review of cli44-toolbox once it reports; a follow-up toolbox lane adds
  `impl From<Error> for bsv_rs::Error` (StoreBusy to WalletBusy with
  retry_after_ms 1000, the rest WalletError as today), replaces the erasure
  sites in wallet.rs, repins bsv-rs 0.8.0; then release toolbox 0.11.0.
- cli44-cli-writer (class code): migrate the CLI's nine `pool().begin()` sites
  (respend.rs x2, spend_gate.rs x2, spend_guard.rs x2, tracker_host.rs,
  chain_feed.rs, cleanup_abandoned.rs) and every single-statement write to
  the toolbox's `write_tx()` / `writer()`; clippy disallowed-methods here too;
  un-ignore the soak test; pin toolbox `=0.11.0`, bsv-rs `=0.8.0`, tracker
  `=0.8.0`; replace the belt's string classification with a match on
  `bsv_sdk::Error::WalletBusy` (and on the sqlx error code for the CLI's own
  pool sites); keep the belt's tests on the typed path.
- review of the CLI lane; the full suite; e2e ONCE (funder on :3320); release
  CLI 0.16.0; close #44 with the evidence; tell LOW.

### State of the repos at launch

- bsv-wallet-cli: main at d26a501 (v0.15.0), toolbox pinned `=0.10.0`.
- bsv-wallet-toolbox-rs: main at 83b38d3 (v0.10.0).

## The explorer ladder wave: toolbox 0.12.0 and CLI 0.17.0 (2026-10-22, 17:0xZ to 22:3xZ)

**The rule (the owner's, 2026-10-22, "each has like rate limits so if one
fails we just go to the next").** Every chain check is a ladder: WhatsOnChain,
BananaBlocks and Bitails are each other's fallbacks; a fault moves to the next
rung at once; a negative ("unspent", "not listed", "absent") needs a second
provider's clean answer; "could not look" is an error, never "nothing there".
The captain's decided readings, proven by exhaustive tests in the toolbox:
the UTXO ladder (an unspent needs two clean answers, a not-listed needs two, a
bound spender stands on one, the verdict independent of the rotation start:
the 64-combination test from every start) and the status ladder (an absence
needs two chain indexes, WhatsOnChain and Bitails; BananaBlocks' 404 is a
gap; two unknowns turn a broadcaster's known to unknown; one chain index's
unknown never stands alone: the 81-row test from both starts). FaultKind
Transient/Deterministic/RateLimited, only Transient retried. RateGate::shared
per host and key: BananaBlocks burst 30 then 0.5/s (60 the first minute, 30 a
minute sustained under its 80 limit); WhatsOnChain's key as the `woc-api-key`
header, no 429 sleeps anywhere.

**BananaBlocks (no key).** base https://bananablocks.com/api/v1 (testnet
test.bananablocks.com/api/v1): `/tx/{txid}/hex`, `/tx/{txid}`,
`/txo/{txid}/{vout}` (script_pubkey, spent, spent_txid; no script_hash),
`/tx/{txid}/proof` (TSC, target = block root), `/address/{addr}/utxos`
(?limit= up to 1000, cursor paging, hasMore; by address only, every
script-hash route 404). A mempool gap exists (the review's live counterexample
659e6e1d…/826718bf…:0).

**Toolbox 0.12.0 (PR 43, seven commits over 0.11.2, five reviews folded;
main 1d0fec2, tag v0.12.0, crates.io 0.12.0 at 21:5xZ).** The lane's first
run was placed on the box by the load picker and died with the box's SSH
(exit 255, nothing committed); relaunched `--host studio`. Rebased onto the
stack's 0.11.2 (= 0.11.1's content: #42 points 1, 2, 4, 5; 0.11.1's tag fired
no release run): Cargo.toml ours (0.12.0), CHANGELOG 0.12.0 above 0.11.2 and
0.11.1, error.rs keeps RateLimited and FailedAncestor. Reconciled: Arcade's
holding words (RECEIVED, SENT_TO_NETWORK, ACCEPTED_BY_NETWORK) come out
unknown and a broadcaster's unknown is a gap, never an absence vote (witness
`arcades_holding_word_is_a_gap_in_the_status_ladder` with the row holding
word + WhatsOnChain unknown + Bitails fault = None); nothing in the branch
hands an explorer's word to a release path (the boundary is `is_utxo`,
services.rs, which 0.12.0 makes stricter; main's release paths through it:
un_fail, locked_inputs.rs:351, poisoned_chain.rs:721, 0.11.1's changelog
lists the locked-input one as open); a retire on a competitor's checked proof
never asks a status source. `get_utxo_leads(script_hash, format) ->
UtxoLeads { leads: [UtxoLead { txid, vout, satoshis, listed_by }], answered,
faults, partial }`: discovery, never a verdict; the union of every explorer's
unspent list keyed by outpoint (txid lowercased); every page read through the
gate (WhatsOnChain nextPageToken, Bitails limit/from with a probe past the
cap, BananaBlocks cursor; LEAD_PAGES_MAX 10 pages of 1000); a list cut short
(the cap, or a later page unreadable or 429) keeps its leads and is named in
`partial`; a malformed page, or any txid not 64 hex, is that explorer's
deterministic fault and none of its leads are kept; BananaBlocks asked only
for the exact 25-byte P2PKH template in the Script format (0x00 main, 0x6f
test); Err(NoExplorerAnswered) only when none answered. The mock has
`get_utxo_leads_response` (unset: built from the status response without
moving its count). Test infrastructure: the lead tests deadlocked at 16 or
more test threads on macOS (mockito's shared pool of 20 servers, scenes
taking theirs one at a time; passes at 10, hangs at 16, CI's Linux pool is
50); fixed by `services::mock_server()` (src/services/mod.rs): servers built
outside the pool, recycled through a free list; proven at 24 and 64 threads;
the cost is file descriptors growing with concurrent tests (the parent
already failed at a 256 limit). A Bitails list of exactly 10,000 is complete
(one more request, limit=1&from=10000). The structs stay plain (no
non_exhaustive; the CLI builds them with ..Default::default()).

**CLI 0.17.0 (PR 51, twelve commits; main e1a6758, tag v0.17.0).**
`src/explorers.rs`: the CLI's own rungs (WhatsOnChain with the toolbox
provider's gate, `shared_woc_gate`, so a 429 either side meets closes both;
`Explorers::at` keeps a private gate for tests); every explorer call logs
marker `explorer_call`. `receive`: WhatsOnChain /beef, then BananaBlocks /hex
with the services' checked proof, then `get_beef`. The spend probe
(`probe_input_spend`): step 1 WhatsOnChain /spent and BananaBlocks /txo (its
script_pubkey must hash to the asked script; another script, the empty one
included, is a fault) with a rotating start, a name bound by the spender's
bytes and proof; step 2 `is_utxo` over three explorers (two clean answers
each way). `broadcast_verify`: BananaBlocks /tx as a third chain index read
through the toolbox's provider; its 404 a gap; its `known` a presence word
like a broadcaster's SEEN, never chain evidence, a mined from any index
outranks it; `chain_index_by` names the index; `ask_chain_indexes` returns a
`ChainIndexReply`. `sync`: `get_utxo_leads` with the deposit script in the
Script format (BananaBlocks by address); a cut-short list is still a lead
list; `--json` adds `unspent_listed_by`, `unspent_could_not_look`,
`unspent_leads` (each lead's listed_by), `unspent_partial`; the summary line
says "N explorers answered (M cut short)" and, when every list was cut short,
that it was not a full look; `--reconcile-spent`: an outpoint missing from
the leads is a question for the spend probe only. Fixtures hold their test
server's port for the process (`hold_port`, src/test_support.rs) so a gate
keyed by a fixture's base never carries a 429 into another test. The
deliberate respend witnessed under 0.12.0 (tests/respend_witness.rs): a
phantom permits only once Arcade stops SEEN; a single gap refuses and
releases nothing. Environment (src/services_env.rs): WOC_API_KEY,
BITAILS_API_KEY, BANANABLOCKS_BASE (unset the chain's default, empty off, a
URL moves it). The repin: the temporary [patch.crates-io] path to the
toolbox worktree replaced by the registry lock (the lock gained the source
and checksum lines only; `cargo update -p` refuses a package that was a
patch, `cargo fetch` rewrote the entry). Gates green against crates.io
0.12.0: fmt, clippy, workspace 501 tests, doc. CI on PR 51 was red while the
path patch stood (GitHub's runners cannot see the worktree) and green at the
repin.

**The e2e gate.** The release build in ~/bsv/targets/bsv-wallet-cli-0170
(the harness links target/release/bsv-wallet and target/release/bsv-wallet-mcp
point there; the October-17 MCP file moved to
~/bsv/targets/bsv-wallet-cli-0161/release/bsv-wallet-mcp.old-oct17). One run:
22/22, funder 148,830 to 147,040, net 1,790 sats, leak 1,049, nothing kept
(log /Volumes/Lanes/out/bsv-wallet-cli/cli44-e2e/e2e-run-20261022T220326Z.log).

**Told.** LOW (items 1 to 4: the ladder, sync's leads, the environment, the
0.11.1 rules underneath) and the stack keeper (0.12.0's content, the
reconcile, the mockito fix) at 22:2xZ; #50 commented with the evidence and
closed.

**The release.** Run 39862565333 success at 22:4xZ: the tag check, the nine CI jobs, publish, three binaries (x86_64 and aarch64 darwin, x86_64 linux) on the GitHub release; crates.io carries bsv-wallet-cli 0.17.0.

| lane | repo | class | result | notes |
|---|---|---|---|---|
| bananablocks-leads (first) | toolbox (the box) | code | DIED: SSH to box.local timed out (exit 255), nothing committed | relaunched --host studio |
| bananablocks-rebase-leads | toolbox (studio, Opus) | code | DONE: rebased onto 41da5da, 10a653f reconcile, 8f0b4ae get_utxo_leads; all gates | the BananaBlocks probe: by address only |
| bananablocks-leads-review | toolbox (studio, Opus) | review | FOLD: M1 a bad txid emitted; M2 a list cut short reported full; L1 non-minimal push; L2 the mock's default; L3 the witness row | |
| bananablocks-fold4 | toolbox (studio, Opus, worktree bananablocks-fold) | code | DONE 6f958f3: every page read, partial, bad txid a fault, the P2PKH template, the mock, the witness row | probes: WoC pages by nextPageToken, Bitails by limit/from, BananaBlocks by cursor |
| bananablocks-review5 | toolbox (studio, Opus) | review | FOLD: M1 the mockito pool hang at 16+ threads (already in the parent); L1 exactly 10,000; L2 the limit assumption; L3 the mock's side effects; four test gaps | 216/216 twice at 10 threads |
| bananablocks-fold5 | toolbox (studio, Opus) | code | DONE 06b5212: mock_server() outside the pool, 24 and 64 threads 49/49 in 4.4 s; the rest | the captain read the hunks; the CLI workspace green against it |
| release toolbox 0.12.0 | toolbox | captain | PR 43 rebase-merged at 1d0fec2, tag v0.12.0, release run 39859774259 success, crates.io 0.12.0 | |
| bananablocks-fold (CLI) | CLI (studio, Opus) | code | DONE 80db50f: M1 sync leads, M2 the shared gate, L2-L5, the respend witnesses | |
| bananablocks-partial (CLI) | CLI (studio, Opus) | code | DONE c970426: partial in sync, the trait end to end, the allow's reason | |
| release CLI 0.17.0 | CLI | captain | the repin cbb99b0 (captain, foreground), PR 51 ready, rebase-merged at e1a6758, e2e 22/22, tag v0.17.0 | |

Open after this wave: toolbox #44 (is_utxo returns the bound spender);
0.11.1's open point 3 and the locked-input release path through is_utxo
(the stack's); whether BananaBlocks lists mempool or non-P2PKH outputs under
an address (unverified); no live run of get_utxo_leads against the real
routes beyond the probes.

## Other open issues (not in this wave)

#39 backstop fetches proofs from an explorer (rule 21); #38 self-send
relinquishes sats; #23 reconcile asks an explorer; #7 Arcade consumer; #4
install docs. Seven local branches are not in main and have not been triaged.
