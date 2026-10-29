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

**2026-10-23, LOW's fleet audit (#53, PR 54, main 31ce13e, unreleased).**
LOW's seats p3, p7, p35 (0.16.1) saw `reconcile-outputs --execute` print
"Net balance delta: -365540 sats" while `balance --json` read the same
satoshis before and after. The line was a sum over the rows the pass turned
off in every basket, never a measurement, and `balance` reads the default
basket only (protocol outputs in another basket, spent on chain by the
counterparty, are relinquished and counted); phase (a) also counted rows
already off. Fixed: both repair commands read the default basket's spendable
sum (the toolbox's list_outputs filter) before and after and print the
balance's own move, then the rows turned off (and on) per basket; phase (a)
counts only rows that were on. LOW's second question ("a seat's own change
locked by <txid> for six minutes after SEEN") was a misreading: the gate's
"locked by" names an input a live unproven action of ours consumed, spent for
good, listed until the spender is proven; renamed "spent by <txid>
(unproven)". LOW's harness counted it in its pool (LOW's #853).

**2026-10-23, 0.17.1 released (15:1xZ to 16:2xZ; the owner's word: "Go for
the 0.17.1 tag and publish when the gate's funder is topped; LOW re-pins on
the published version").** Tag v0.17.1 at 63566d2 on main; crates.io
0.17.1; the GitHub release with the three binaries (run 39981582764, every
job green). Three PRs: 56 (the bump: Cargo.toml, Cargo.lock, the README's
release of record, the CHANGELOG heading; then tokio 1.51.0 to 1.54.1 in the
lock for RUSTSEC-2026-0386, an unsound advisory on tokio's AsyncFd published
after 0.17.0, which turned the required `deny` check red under `unsound =
"all"` with no code change; `cargo deny --locked check advisories` passes
locally, cargo-deny 0.19.0), 57 (the harness: a failure before the scenarios
is FATAL and exits 1), and this. Pins unchanged (toolbox =0.12.0, bsv-rs
=0.8.1, bsv-tracker =0.8.0); the toolbox and bsv-rs track no lock, so their
deny checks resolve fresh. The gate ran to completion once: 22/22, net 1,771
sats, funder 147,040 to 145,269 (run 20261023T154459Z under
`/Volumes/Lanes/out/bsv-wallet-cli/cli44-e2e/`), the harness on the release
build in `~/bsv/targets/bsv-wallet-cli-0171` (symlinks `target/release/{bsv-wallet,bsv-wallet-mcp}`).
A first attempt (run 20261023T153739Z, nothing spent) ended at the funding:
the funder daemon (still the 0.16.1 binary started 2026-10-22 11:10, PID
25052, CHAINTRACKS_URL set) held all six coins "headers unavailable:
Headers(HeaderError("Service error: get_header_for_height: no header service
configured"))" at 15:37:41Z, two seconds after its new_header tick missed the
same way; the worker answered 200 in 0.2 s before and after. Five such misses
in forty hours of minute ticks (00:35, 07:07, 14:48, 15:06 on 10-22; 15:37
on 10-23); only the last met a run. The hold is the gate's rule ("could not
look" is never "offerable"); the reason is a toolbox defect: the header
ladder's two header calls log each rung at debug and return "no header
service configured" when every rung fails (get_height beside them keeps
last_error). Filed as toolbox #47; the stack keeper verified it at 1d0fec2
and runs a fix lane for 0.12.1, sha and version to follow; the CLI's pin to
0.12.1 is a 0.17.2 whose gate spends the funder, so it waits on the owner's
word. The harness had exited 0 on that failed run ("Passed: 0/0", the
funder's 402 unprinted): the funding threw inside the try and the finally
block's process.exit ran with zero results; PR 57 names it FATAL, exits 1,
and compares the results against what the run was asked for (a dry run or
a single scenario no longer keeps the wallet directories as a failed run).
LOW told (re-pin on 0.17.1; its own tracked locks under deny's unsound rule
to check); the stack keeper told (#47, the RUSTSEC note). Scratch worktree
under $TMPDIR removed; the merged branches deleted.

**2026-10-23, 0.17.2 released (16:5xZ to 17:3xZ; the owner's two yeses:
the funder restarted onto 0.17.1, and 0.17.2 pinning toolbox 0.12.1).**
Toolbox 0.12.1 (tag v0.12.1 at 7b874ee, PR 48, the #47 fix: the header
ladders answer "<call>: no header service answered (chaintracks: <error>;
bhs: <error>)", "not configured" only when neither client is configured,
rung failures at warn) reached crates.io at 16:49Z. CLI PR 60: the pin
=0.12.0 to =0.12.1, version 0.17.2 (Cargo.toml, Cargo.lock, README,
CLAUDE.md's pin mention, CHANGELOG); no CLI code change. The gate (run
20261023T165643Z, net 1,771 sats): 21/22, scenario 14 failed in its audit
step, not in the wallet: wallet B's return payment was accepted by ARC at
16:58:40Z, at 16:58:46Z WoC answered 404 while Bitails answered 200 and the
wallet's own ladder recorded it seen; the harness's `verifyOnChain` polled
WoC alone eight times over 88 s and gave up; both transactions on chain.
PR 61: the auditor is the explorer ladder (WoC, Bitails, BananaBlocks on
every poll, the first 200 the answer with `listedBy`, a 404 "not yet there",
a fault moved past, the throw names every rung's last answer; no caller read
the output list it used to return). The owner chose a rerun of scenario 14
alone under the fixed auditor (`--scenario 14`, run
20261023T170801Z-scenario14, PASS in 38 s, net 746 sats) over a full rerun
or a tag on the evidence. Tag v0.17.2 at a8a2759 (PR 60 cf2623b plus PR
61); crates.io 0.17.2; release run 39992762692 green with the three
binaries. The funder daemon: restarted onto the 0.17.1 binary at 16:17Z
(PID 44352, same directory, log and two environment variables; recipe in
memory `e2e-funder-setup`); balance 142,752 sats after the two runs. The
harness runs the release build in `~/bsv/targets/bsv-wallet-cli-0172`.
LOW told (0.17.2; its pin move after its loop 46 may go straight to it);
the stack keeper told (tag and sha; it moves the witness spend's binary).
Also this hour: the stack keeper reported the owner's ruling on #44's four
asks (storage charter S9, the busy hosting row, R8 (a)/(i), an events row)
and a finding that bsv-rs keys busy on any 503; checked and corrected: bsv-rs
0.8.0 and 0.8.1 already read WalletBusy from the body code only (3e96c52),
the keeper's premise was its lane branch; the residual (a bare 503 as a
typed transport fault a caller may retry without the busy promise, not a
plain WalletError) is bsv-rs #73, no date. CLI #59 (the gate's answer on a
header-service transport fault: one retry, then 503 HEADERS_UNAVAILABLE
with Retry-After, never 402 INSUFFICIENT_FUNDS) stays open for a later
minor, from LOW's ask.

Later the same hour: LOW's move to 0.17.1 landed on its four hosts at
17:0xZ (record 1136; 0.17.2 goes in by the same recipe before its loop 47),
and LOW asked for a scoped allowPending (its loop 46, pair 32: the one word
offers every received unproven coin, so a funder that cannot vouch for a
seat's other received coin withholds it and its own top-up lands held);
filed as CLI #63 (`--allow-pending received --coin txid:vout`, repeatable;
`allowPending: {"received": ["txid:vout"]}` on the served /createAction;
only the named received coins offered, the rest held), a later minor.

Open after this wave: CLI #59 (the gate's answer on a header-service
transport fault); CLI #63 (the scoped allowPending word); bsv-rs #73 (a
bare 503 as a typed transport fault);
toolbox #44 (is_utxo returns the bound spender);
0.11.1's open point 3 and the locked-input release path through is_utxo
(the stack's); whether BananaBlocks lists mempool or non-P2PKH outputs under
an address (unverified); no live run of get_utxo_leads against the real
routes beyond the probes.

2026-10-23, later: 0.18.0 released (John's word "love it all go" on #59
and #63). PR 65 (db2f32b): #59, a coin held because the header service did
not answer is marked as held on the service (`CreatorReading.headers_fault`,
`HeldCoin.headers_fault`), its reason the service's own error; the gate
reads those creators once more after two seconds, once per call; when the
coins still held on the fault could have covered the action (or every named
input is held on it) the answer is `503 HEADERS_UNAVAILABLE` with
`Retry-After: 2` and the refusal's body, never `402 INSUFFICIENT_FUNDS`.
#63, `AllowPending { Off, Received, ReceivedOnly }`: the served body takes
`{"received": ["txid:vout", ...]}`, the commands `--coin TXID:VOUT` with
`--allow-pending received`; only the named received coins are offered, the
rest held "not named under allowPending: received"; a named coin that is not
a received coin the wallet could spend under the word is a 400 before the
door. Ten new tests; CI green; `cargo deny` green. PR 66 (901703a): the
harness's ports and guard (below). Tag v0.18.0 at 901703a; release run
40016864586 green, crates.io 0.18.0, three binaries. The funder daemon stays on
the 0.17.1 binary (PID 44352; John's call to move it). The harness runs
`~/bsv/targets/bsv-wallet-cli-0180`.

The incident of the gate's first run (20261023T195137Z, 19/23): the
harness's default port for wallet A, 3323, is the port
`~/bsv/prod-test-wallets/REGISTRY.md` reserves for its `recipient` seat
(identity 035593a9, "soak funder", in use by captain-zanaadu since
19:40Z), and that seat's daemon (pid 12037, `~/.cargo/bin/bsv-wallet daemon
--port 3323`, started 15:40Z) was serving. The harness's own `serve` died
"Address already in use", its health check answered from the seat's daemon,
and the run funded the seat 100,000 (a02cf56f...), drove the scenarios
through it (its four CLI scenarios failed: the CLI read the directory's key,
the HTTP answers were the seat's) and swept 105,174 from it to the e2e
funder (e8636a66...): about 60,331 of the seat's own sats (60,413 before,
82 after) sit in the e2e funder, the run's fees from the seat's side about
1,755. Nothing lost; the seat's daemon untouched. Captain-zanaadu told at
once (the txids; it noted the seat's registry row) and asked for the return
as a BRC-29 wallet payment of 60,331 to the seat's identity, the AtomicBEEF
and remittance left in `~/bsv/prod-test-wallets/recipient/inbox/`; the
return is a spend from John's funder and waits on his word (the prepared
script: this session's scratchpad `return-recipient-seat.js`). The fix (PR
66): `startWallet` refuses a port that already answers and checks the
server's identity against the directory's key; defaults 3370 (A), 3371 (B),
3372 (C), off every registry port; the second run (20261023T195902Z) 23/23,
net cost 2,140. Seven leftover `/tmp/bsv-wallet-wallet-*` directories from
today's earlier runs (dust: 97, 47, 172, 23 sats, three at 0) were removed.

Also today: John's MCP config (both `.claude.json` files) reduced to one
`bsv-wallet` entry on the checkout's `target/release/bsv-wallet-mcp`
symlink, port 3322; the worm-era entry removed (memory `mcp-config-entry`).
bsv-rs #73 and toolbox #44 handed to the stack keeper on John's word (#44
confirmed: fewer explorer calls for the same answer, nothing near
break-glass); the CLI re-pins when they publish.

Later the same hour, John's word ("of course green light") on both: the
return went, txid e6c4751859ba44e16db60da6da79603fb93da1aff2e2a89c8cc03f268dfcc3c9,
60,331 sats, a BRC-29 wallet payment from the e2e funder to the seat's
identity (outputIndex 0; the AtomicBEEF and remittance in the seat's
inbox; captain-zanaadu internalizes it on the seat and updates the row);
the funder daemon restarted onto the 0.18.0 binary at 20:42Z (pid 24508,
same directory, log and two environment variables; the old 44352 stopped
clean), balance 138,834 after the return. The stack keeper has bsv-rs #73
(0.8.2) and toolbox #44 (0.13.0) in lanes and sends the versions when they
are on the index; the witness wallet stays on 0.17.2 until the re-pin.

Open after this wave: bsv-rs #73 and toolbox #44 (the stack keeper's; the
CLI re-pins when they publish); the seat's internalize of the return
(zanaadu's); 0.11.1's open point 3 and the locked-input release path through
is_utxo (the stack's); whether BananaBlocks lists mempool or non-P2PKH
outputs under an address (unverified); no live run of get_utxo_leads
against the real routes beyond the probes.

## Other open issues (not in this wave)

#39 backstop fetches proofs from an explorer (rule 21); #38 self-send
relinquishes sats; #23 reconcile asks an explorer; #7 Arcade consumer; #4
install docs. Seven local branches are not in main and have not been triaged.

2026-10-23 (the evening into the night of the 24th UTC): THE NORTH-STAR
PROGRAMME OPENED, AND PHASE 0 (THE ORACLE) LANDED ON BETA. John's word:
"I love it all go", "sky is the limit", "god tier north star on all
fronts", "keep going and going", "beta branches for all these". The
programme is epic #106: a measured scoreboard, seven milestones (Phase 0
the oracle to Phase 6 the engine API and the desktop app), issues #69 to
#156 by now, a `beta` line for the programme with `main` the stable line
(#107, PR 108), the loop (a lane returns the change, the proof and the
nothing-changed evidence; a cold review on another model; one PR per
concern; cold paths on green, hot paths with full proof), the never-without-
the-owner list, BRC-100 at the line's pin ed1b015 with upstream 5119a9b
cited where it differs. The same programme on the two libraries (toolbox
epic #80, bsv-rs epic #101) and on the Cloudflare layer (epics in the six
workers and bsv-middleware-rs), the dependents mapped to the apps. John
started captain-bsv-rs and captain-bsv-wallet-toolbox-rs; the keeper keeps
the stack, the line, the pins, the matrix, the gate vectors (stack #184:
conformance/wallet-gate-vectors.json, 490 rows) and the Cloudflare repos;
the division is on the stack's rulings page. The localhost page
http://127.0.0.1:8790/ (from /Volumes/Lanes/out/bsv-wallet-cli/north-star)
draws the layers, the repos on the stack's machines, the dependents and the
loop, with three nano-banana-2.1 images paid through the x402 skill (3 x
239,446 sats from the MetaNet desktop wallet on 3321).

0.18.1 (main 3858d21, PR 128): the two pin moves only, toolbox =0.13.0 and
bsv-rs =0.8.2; the probe's move onto is_utxo_with_spender was built,
reviewed and held back (the review found two ladder gaps the move would
expose: toolbox #96, its fix toolbox PR 101; the probe moves in the patch
that pins 0.13.1). The gate 23/23 (run 20261024T001317Z), net 2,200; the
tag v0.18.1 is John's word. main into beta went the same night, late (PR
150: the pins, fuzz/ following them, one nightly.yml for both lines): a
lane noticed beta still pinned 0.12.1 and 0.8.1 a day after the patch; the
rule is now in the captain's memory: the main-into-beta PR opens the night
the patch lands.

THE ORACLE WAVE on beta, each PR by cold review (eight reviews caught real
defects; the counts are the reviews' findings closed before landing):
- the lane rules and briefs (docs/lanes/README.md: foreground only, the
  26.5 SDK, the references at pins, rule 11 the differential line);
- the architecture rules as tests: six rules on one Rule runner, allowlists
  of (file, count, reason) tuples that only shrink, test code read as the
  ratchet reads it (#73, PR 111);
- the ratchets: seven numbers that may only fall, the record under the
  pinned toolchain 1.98.1, compared against the base ref; `ratchet` a
  required check (#72, PRs 112, 134, 135, 137; follow-ups #126 #127 #132
  #133);
- the wire fixtures: 106 recorded calls and refusals with a manifest,
  replayed by the required `wire` job; the known server defects table
  (#69, PR 121);
- the consumers' recorded calls: 48 over seven dependents, in
  tests/wire_consumers/ with docs/DEPENDENTS.md; one tests/wire_support
  for both replays (#110, PR 116);
- the fuzz targets and the mutation nightly (#75, #76, PR 123; the
  registration on main PR 139 then PR 151, since the first copy had two
  `with:` per step and could not parse; the first dispatched night was
  cancelled at 03:05Z because its 25 jobs took the whole runner pool and
  every PR check sat queued: #144 caps it; five shards finished at about
  45 minutes each);
- the architecture document (#83, PR 114) and the unwraps judged (#85, PR
  115; 33 to 7 in shipped code);
- the hermetic e2e: the 23 scenarios on a local stub of the header
  service, the explorers and the broadcaster, a wall that fails the run on
  any attempt to leave the machine, 23/23 in ten seconds on the runner;
  the knobs WOC_BASE, BITAILS_BASE, ARC_GORILLAPOOL_URL follow
  BANANABLOCKS_BASE's and ARC_URL's pattern and are open to John's change
  before the next minor (#74, PR 131; toolbox #98 asks the providers'
  knobs; #136 the OS-level egress block);
- the benches with the CI runner's baseline as the gate: the reference is
  always the base's entry, an appended run above the margin over it is an
  error, a pin-move re-record is a warning, the bench job pinned to the
  toolchain, warn-only until an entry carries five runs (#71, PR 125, five
  reviews; #142 the nightly bench run that fills the entries);
- BRC-100 at the pin: docs/brc100/fields.json (313 fields at ed1b015,
  each with its line), docs/BRC-100-EXTENSIONS.md (41 extensions with
  reasons, 68 gaps, the host routes apart, the deviations section empty
  with its rule), tests/brc100.rs fails naming a served key that is
  neither the BRC's nor listed, and a dead or malformed row (#100, PR 143);
  the gaps became #145 (privileged dropped on getPublicKey and the crypto
  calls: a privileged request quietly got the everyday key), #146
  (hashToDirectlyVerify ignored), #147 (createAction's sendWith,
  noSendChange, knownTxids, returnTXIDOnly, version unread), #148
  (createSignature requires counterparty), #149 (the refusal body's shape
  against the BRC's), and #122 item 8 (the certificate shapes, bsv-rs's);
- the differential harness: tools/differential runs the base build and
  the head on the hermetic suite with the same keys and stub, records
  every call, command and the stub's per-provider counts, and diffs byte
  for byte with an explicit exclusion list; signatures compared push by
  push; no recording holds a key; an incomplete recording its own exit;
  the CI job reports on every PR and fails a `refactor` PR once toolbox
  #105 is pinned (#70, PR 152, three reviews; follow-ups #156). Head
  against beta: 0 differences over 335 calls and 35 commands. The floor
  (the same binary twice) is nonzero from scenario 22 on: the toolbox's
  change pick tied on LENGTH(input_beef), which signature length moves
  (toolbox #105, in a lane the same night), and its RNG cannot be seeded
  (toolbox #106, John's word on the constructor shape);
- docs/NORTH-STAR.md with tools/scoreboard (the table rendered from the
  records in the tree, --check against the base so no count falls and no
  key vanishes, the arch rows counted from the tests' allowlist literals)
  and tools/check-links, two steps in the ratchet job; the ranked research
  queue every brief cites (#77, PR 141, three reviews; landed last with
  one re-render).

THE ADVISORY: at about 03:30Z RUSTSEC-2026-0387 (crypto-bigint 0.5.5,
timing variability, through k256 0.13.4 through bsv-rs) turned the
required `deny` job red on every line of every stack repo on k256 0.13;
the fix is crypto-bigint 0.7.6, which means k256 0.14. The keeper ruled
the ignore entry with its reason into the stack template within the hour;
PRs 153 (main) and 154 (beta) adopted deny.toml and the stamp byte for
byte; the bsv-rs captain has the k256 0.14 move as bsv-rs PR 127 (#125;
no public type moves; the RFC 6979 high-digest seeding moved to the
reference's) for a 0.8.3 John tags, after which the CLI's pin moves and
the entry leaves.

What the reviews caught before landing, this night: a fail-open scanner,
a gameable bench baseline (four times, four ways), a record under the
wrong toolchain, a missing-fixture pass, the hermetic wall failing open, a
workflow file that did not parse, a stranger's payment fixture that
verified nothing, a differential recording that held the root keys in
plain text, a scriptSig renamed as one atom so a wrong sighash byte on
every input passed as zero differences, a table a blank line ended
silently, a CI step whose exit mapping never ran under bash -e. And the
one that matters for money, from the fixtures: the internalize door
accepts an unproven payment whose unlocking script does not verify (#140;
toolbox #103 codes the verifier's verdict).

The lessons (memory lane-landing-lessons): lanes in the foreground only;
private target dirs; zsh modifiers in refspecs (twice more tonight); the
ratchet's toolchain; serial landing with a lock, a waiter that cannot
match its own command line; a required check must exist on the base; the
nightly starves the PRs; main into beta the same night; a RustSec
advisory reddens deny everywhere at once and only the template may answer.

Open after the wave: John's words (v0.18.1 tag; toolbox #66 questions 5
to 9; toolbox #106's constructor shape under toolbox #80; the 0.13.1 and
bsv-rs 0.8.3 tags when their captains ask; pool logins on the box); the
ratchet re-record on beta (two numbers fell under the record); the first
full night on beta and the three mutation floors from it; Phase 1 next by
the queue in docs/NORTH-STAR.md: #78 one spend function (the differential
line holds its proof), #79 the evidence layer, #80 typed config, #81 typed
errors, #84 observability; #130 the stack's gate vectors; #133 the harness
files; #145 to #149 the BRC-100 gaps (hot path: #145 first).

2026-10-24 (the day after the oracle wave): PHASE 1 UNDER WAY ON BETA, the
first full night measured, the gate's first two money findings closed. From
midnight UTC beta took the oracle wave's last landings and then, by the queue
in docs/NORTH-STAR.md, one PR per concern, each through a cold review on
another model, a fix lane for its findings and a re-check:
- #81 typed errors (PR 167; src/server/error.rs Kind, one status and code per
  refusal by variant; A8 keeps error words out of the served layer);
- #80 typed config (PR 169, three reviews: credentials redacted in URLs, an
  empty secret shown as empty, a malformed knob no longer blocking `config`),
  and its loose ends #172 (PR 188: --json nulls and booleans, services_env.rs
  parses nothing, CHAINTRACKS_URL=off stays the string off by the captain's
  word, the webhook token the one effective value that moved and why it is
  safe);
- #133 the harness files split into directory modules (PR 160, rebased twice
  as beta moved under it; a split is a move, a module prefix is a path);
- #130 the stack's 490 wallet-gate vectors replayed on every PR (PR 161; the
  divergences file exact, 302 over 64 rows in five clusters; the keeper read
  each: four ours, one the replay's label);
- #165 the gate reads the store's coin word (PR 189: a coin the store marks
  not spendable is held as `lost` before the creator is read and before the
  toolbox's allocation; the money finding: under acceptDelayedBroadcast the
  toolbox did not check spendable, so a lost coin the gate offered could be
  spent; now tested at the door; 24 rows left);
- #166 a named coin the wallet does not hold refused at the gate on every
  path (PR 200; the keeper's reading: a caller-carried inputBEEF input is not
  the gate's question, the exception holds only when the carried transaction
  verifies and the input is the caller's to sign; 14 rows left);
- #164 the spend guard no longer clears a fork's suspect mark (PR 205, three
  rounds: the guard refuses a suspect creator as it stands and feeds the
  tracker nothing, the keeper's reading; the fix uncovered three door rows the
  replay had read from the gate's whole hold, mark included, where the Lean
  checks the mark apart from the door: the captain's word made it the
  replay's projection, not a finding, and CreatorReading carries the door
  with the mark set aside; the ratchet's pedantic lint then made the gate's
  answer a method; 129 over 24 rows remain for #162 and #163; the re-check's
  hazard on the invalidated-block path is #206);
- #145 privileged read on the nine key, crypto and key-linkage calls and
  refused with 501 PRIVILEGED_NOT_SUPPORTED until a privileged key exists
  (PR 190, two reviews and two re-checks: refused before the audit, a
  non-boolean 422, the array bodies beta accepted still 200; the certificate
  calls' silent downgrade is the toolbox's: toolbox #142, CLI #194);
- hardening: #170 RELAY_POLL_SECS=0 refused at load (PR 180), #120 the daemon
  observes its server task (PR 181, three reviews), #182 the fleet's monitors
  stop under the grace (PR 185), #168 abortAction's busy answer (PR 202, one arm and
  a busy-read fixture; its rebase after PR 200 raised duplicated_tokens with
  no conflict, a fix lane shared the shape; its review found the two host routes serving a busy
  store as 500, #203, and that no hermetic busy scenario exists, #204), the
  version line beta was missing (PR 183);
- the ratchet record follows the day (PR 207, merged 6cac3be past midnight
  UTC: duplicated_tokens 32786 to 32759, missing_docs 154 to 150, the six
  others equal; its first record, taken at 150a8d5, read 42 under the tree
  once PR 205 landed under it, so the record was measured again on 45e0e48
  by a fix lane and re-checked number for number: a re-record PR is measured
  on the head it lands on, last in its queue);
- the nightly: the first full night (run 40074725488) measured spend_gate
  87.1, explorers 74.2, broadcast_verify 75.5, handlers 74.2, tracker_host
  72.6; the floors moved to the measured values (PR 176, six reviews: a floor
  only rises, only in the commit whose tests earn it, its score must equal
  the formula from its counts; #178 ties the record to the run); the matrix
  capped at six shards in one group on beta and on main (PR 179; the schedule
  runs main's file); the raises are #173 #174 #175.

The differential floor: against toolbox 0.13.0 the same binary twice read 0,
0, 13 and 34 over the day, every nonzero run starting at scenario 22's coin
tie; against the toolbox's beta tip 40cabb9 (toolbox #105, the change pick
ending on output_id) two pairs read 0 over 335 calls and 35 commands, and again two pairs of
0 at the toolbox's later tip 00c2e94 (every toolbox oracle PR landed; asked by
its captain, evidence under /Volumes/Lanes/out/bsv-wallet-cli/diff-00c2e94). The
`refactor` label's hard fail turns on in the patch that pins the release
carrying it (#196, with toolbox #96's probe move and #157's CLI half).

The keeper's words today, recorded on the issues: the vectors move only with
a scenario draft and a landing sha, a newly differing row is a finding on its
own side; the BRC pin of record moves ed1b015 to 5119a9b (#199, next
release, never ahead of the line); bsv-rs #149's varints never cross the
CLI's JSON wire, the bound does (#198); and, relayed by the toolbox captain,
the bsv-rs pin's one word is 0.9.0-beta.1 on the index, not a 0.8.3 tag (#198
and #199 are reread against that when John tags).

The landing lessons of the day (memory lane-landing-lessons): a landing
script must test the rebase itself, not a pipeline ending in tail (PR 160
sat two hours in a stuck checkout); a rebase without conflicts can still not
compile (PR 161 after the config merge: grep the open branches for a moved
name); the scheduled nightly runs main's workflow file; every pair of
landings conflicts on CHANGELOG's Unreleased top (#195, fragments); a merge
into beta never closes its issue (hand close, every time).

Issues filed from the reviews: #178 #184 #186 #187 #191 #192 #193 #194 #195
#196 #197 #198 #199 #201 #203 #204 #206 (an invalidated block's own recheck reads
the invalidated header: the mark can clear at delivery); toolbox #141 (the BEEF dump writer) and #142.
Closed: #75 #120 #130 #133 #144 #145 #164 #165 #166 #168 #170 #172 #182.

Open after the day: John's words (v0.18.1; the toolbox 0.13.1 and bsv-rs
0.8.3 tags; toolbox #106's constructor shape; toolbox #66 Q5-Q9; the model
rule to the two captains directly); #162 (its lane running at the close,
the keeper's reading already on the issue: a stale creator ranks as seen at
the door) then #163 by the sequence; #79 the evidence layer and #84 observability next by the queue;
#78 one spend function once toolbox #105 is pinned.

2026-10-25 (the night after the Phase 1 day, from midnight UTC): THE GATE'S
LAST TWO CLUSTERS, by the sequence, each a lane, a cold review on Opus, a
landing behind the one before:
- #162 a stale creator ranks as seen at the door (PR 209, 08df657; the
  keeper's reading on the issue: the owner's ruling 7a (b) of 2026-10-20, the
  Lean's doorWord; the gate read `stale` beside the dead words and held it
  "never offered"; now the pending tier's reading: our own change under
  SPEND_OWN_CHANGE, a received coin under the action's plain or scoped word,
  the hold naming the word, `stale: its block left the chain; ...`; the dead
  words byte for byte; HEADERS_UNAVAILABLE untouched; 57 differences over 12
  rows left the list, 129/24 to 72/12; the review's minor findings filed:
  #210 the stale wording's doc gaps and ARCHITECTURE's drifted line numbers,
  #211 the suspect mark read before the word for every word (hardening, not
  reachable with bsv-tracker 0.8.0), #212 the `built` label on a stale
  creator held under a header-service fault; the review's own finding on the
  store: the toolbox keeps a creator whose proof a reorg took as `unproven`
  with the proof row gone under the daemon and `completed` with its old
  proof under serve, and allocates the demoted shape then refuses it as
  "stored ancestry incomplete": the gate offers it, the toolbox's word is the
  toolbox's);
- #163 the lineage rule (PR 213, two rounds: the walk over our own store,
  outputs.spent_by forward from the dead actions, one recursive read for the
  whole call, nothing written, a cycle ends the walk; the replay builds the
  conflicted parent P from the row's flag; the list ends empty, 0 over 0
  rows, and the dead_lineage column is answered; the toolbox at 0.13.0 reads
  no lineage at allocation and refuses a child only at its BEEF walk when the
  parent is `failed` in its own store, never on the tracker's `conflicted`:
  the walk is the CLI's until toolbox #42's; the review on Opus found the
  walk and the door reading one creator two ways: a stored dead tracker word
  beside a proof the toolbox holds is dead for the walk and mined for the
  door, which checks the proof against the headers and lets it outrank the
  verdict, so P was offered while its child was held for P; the Lean's
  Tracker.onProof_of_rootChecks (a checked proof outranks every word;
  dead_onChain is chain events only) and State.skel (the lineage reads the
  same a.tracker.word the door reads) make the door's reading the charter's,
  and the fix lane gives the walk one reading with the door: a dead
  candidate with no proof is dead as stored, one with a proof is read
  through read_creators and dead only when that reading is not mined; also
  the joins on lower(txid), "an action of this wallet, made or received",
  the stored abandoned word named; the second re-check found the two
  readings still split on the header-service retry: the walk read a
  candidate under its own snapshot and the door re-read the creator after
  the pause, so P's coin was offered after the retry while D stayed held for
  P; round three moves the candidates' read into the gate's own readings
  map, one call with the first batch of creators, the one retry re-reading
  both, the dead set derived after it, and lineage.rs reads no headers; the
  fix lane's read-only finding, that a stored dead word beside a toolbox
  proof is never corrected in tracker_states while the door reads it mined
  each call, is #215; the third re-check found the named createAction
  door's older outer retry re-called assess_named only for a coin held on
  headers_fault, which a lineage hold never carries, so under one outage the
  allocation offered D after its retry and the named door refused it naming
  P; round four moves the retry into assess_named through the one helper,
  creators and candidates in one call, and the outer re-call goes; respend's
  exact inputs never retried and stay so; cognitive_complexity fell 20 to
  19 along the way, a record for its own PR; the fourth re-check said merge
  and PR 213 landed at 1c82737, 04:43Z: 641 tests, 20 lineage tests, the
  differential inside the floor; its minors are #216 (the silent-throughout
  test does not prove the retry ran; "each door" covers respend, which reads
  once; a long doc line) and #217 (a candidate holding a proof costs the
  pause on every door call even when it is no ancestor of the coins judged:
  filter the candidates to the judged coins' ancestors), and ARCHITECTURE's
  other stale spend_gate.rs pointers are noted on #210; the lane worktrees
  and targets removed, 39 GB);
- #197 the scoped word's check reads a named coin's transaction status
  before lost (PR 218; the lane put the status first of all, before spent by
  and frozen, and claimed the Lean's coinWord read backwards; the review on
  Opus caught the claim and asked the owner for the order; the captain's
  ruling on the issue: the Lean's coinWord order, frozen, the lock (spent
  by, lost), the creator's word, with `lost` qualified to a completed or
  unproven transaction as the offer path reads it, since a failed
  transaction's spendable = 0 is the toolbox's own write at the failure; the
  fix lane took the ruled order with the lost qualifier written once; the
  second re-check found the docs skipping own change, which the code reads
  before frozen beside not-ours (ruled: the scoped word's domain, read before
  the coin's word begins), two promised outcomes with no row (a spent coin of
  a failed transaction, a nosend coin with spendable = 0) and a test helper
  copying the toolbox's failure write instead of calling it; round three
  took those, and the third re-check asked for a row at every boundary of
  the order (a frozen own-change coin, a spent coin with spendable = 0, a
  spent and frozen coin, whose word moved from beta's "spent by" to frozen,
  the Lean's first read, now said in the CHANGELOG); round four, on Codex
  gpt-6.1-sol, added the rows (14, every adjacent swap failing a row; the
  re-check ran nine orders and planted twelve coins, all as ruled) and its
  only finding was the trailer `GPT-6`, which the captain reworded to the
  exact model with the tree unchanged (46aea0c); landed 81d31b5, 07:27Z;
  one word moved from beta, a spent and frozen coin reads frozen; the same
  gap on the named door is #219; the lane worktrees and targets removed);
- #201 every gate lookup scoped to the authenticated user (PR 220, the lane
  on Codex gpt-6.1-sol by cca's pick, run on the box: one helper
  UserCoins::query with WHERE o.user_id = ? for the named inputs, the scoped
  word, the offers, the spent and lost listings and respend's lookup, the id
  from the wallet as the toolbox finds it; two-user tests, four, each
  failing with the filter removed five ways; the toolbox's own allocation
  checks the owner (create_action.rs:1483 find_output_by_outpoint(user_id),
  storage_sqlx.rs:897), so nothing to file there; the review on Opus found
  the CHANGELOG claiming "every spend-gate output read" while the lineage
  walk and the rule labels still read the whole store: the wording narrowed
  in a fix round (on Codex again; the trailers reworded to the exact
  model, the tree unchanged), the two reads filed as #221 with
  frozen_outpoints noted there; the second re-check's one finding, the PR
  body's opening sentence, the captain edited; landed c462da4, 08:27Z);
- the ratchet record (cognitive 20 to 19 and duplicated_tokens 32759 to
  32643 by the night's PRs, the six others equal): its lane, routed to
  the box by cca's load rule, blocked at once, no pool account has a login
  on the box for Opus (John's pending item); two reruns went to the box
  again because zsh passed the chain script's `--host studio` as one word;
  the third, with cca's CCA_NO_PEER=1, runs on the studio on Codex
  gpt-6.1-sol (lane p1-ratchet-record3): PR 222, measured on c462da4,
  cognitive 20 to 19 and duplicated_tokens 32759 to 32643, the six others
  equal; the review re-measured it number for number and its one finding
  was the Codex trailer against the review brief's two-model list, which is
  not the rule (the trailer names the model the lane ran on): landed on the
  captain's word, 462ff67, 08:4xZ; beta's record after the night:
  duplicated_tokens 32643, missing_docs 150, clippy_pedantic 834,
  unwrap_expect_outside_tests 6, files_over_800_lines 10,
  tests_files_over_800_lines 3, functions_over_60_lines 89,
  cognitive_complexity_over_25 19;
- the HANDOFF addendum for the 24th into main (PR 208, 03e7af3).

The night's lessons (memory lane-landing-lessons, gate-vectors-pin-protocol):
one word per creator, the door's: any gate rule that reads a creator's word
goes through read_creators' replay when a proof is held, never the stored
tracker word alone (the Lean's State.skel and the door read the same word;
a checked proof outranks every word; dead_onChain is chain events only); a
review of an order change asks for a row at every adjacent boundary, the
test helper calling the real store write, and every moved word named in the
CHANGELOG, so brief it that way up front (#197 took four rounds for one
function's order); cca may place a lane on Codex, so every brief and review
brief names the three trailer forms (Claude Opus 5.5, Claude Fable 5.1,
GPT-6.1 Sol (gpt-6.1-sol)), and a trailer-only reword on an identical tree
is the captain's; cca routes a loaded studio's lanes to the box, where no
pool account is logged in for Claude, so a Claude chain sets CCA_NO_PEER=1
(and in zsh an unquoted ${VAR:+--host $VAR} is one word).

Filed: #210 #211 #212 #214 (README:282 still hands a named absent outpoint
to the toolbox, #166 made it the gate's 400) #215 (a stored dead word beside
a toolbox proof is never corrected in tracker_states) #216 #217 #219 (the
named door reads a failed input as lost) #221 (the lineage walk, the rule
labels and frozen_outpoints read the whole store). Closed: #162 #163 #197
#201.

Open after the night: John's words (v0.18.1; the toolbox 0.13.1 and bsv-rs
0.9.0-beta.1 tags, #198 and #199 reread against the index then; toolbox
#106's constructor shape; toolbox #66 Q5-Q9; the pool logins on the box;
the model rule to the two captains directly). The gate's divergence list
against the stack's 490 vectors is EMPTY; a newly differing row is a finding
on its own side (the pin protocol). Next by the queue: #203 #204 #206 (the
hardening and the host routes' busy answer), #210 to #212, #214 to #217,
#219, #221 (the gate's follow-ups), #186 #187 #184 #191 #193 #192 #195, #196
on 0.13.1, #194 (toolbox #142) and #155 (toolbox #141) at their pins, #157
with 0.13.1; the programme: #79 the evidence layer, #84 observability, #78
one spend function once toolbox #105 is pinned, #173 to #175 the mutation
floors, #171.

2026-10-26 (the second day of Phase 1 on beta): THE PIN LANDED, THE CI BALANCE,
THE NIGHTLY'S FIX, 0.18.2 ON MAIN UNTAGGED. The owner's words of the morning,
on a decision brief of seven items: 0.18.1 is never published and the next
patch is 0.18.2 (bsv-rs =0.8.3 for k256 0.14, toolbox =0.13.1, the probe's one
ladder pass, #157's bound); tag toolbox 0.13.1; one pin PR on beta moving the
prereleases together, then v0.19.0-beta.1 from beta after a green night; the
model rule written into cca's RULES.md (Fable on the account expiring soonest
within a day, else Opus 5.5 on the week-soonest account; a lane names a model
only on that rule or his word); "the reference" explained (the BRC first, the
TypeScript reference at a pinned commit only where the BRC is silent, codes
over words, divergences recorded). The keeper then relayed his "love it... yes
to all" on five process rulings: record landings batched, a documentation-only
fold lands on the standing MERGE without a re-check, a reference-shape change
needs no new word, one message an hour per captain, decisions through the
keeper's daily brief.

Landed on beta today, each through a cold review on Opus and a fix round where
the review asked (beta from caf72ac to f4bdab3): PR 253 (the three
version-agnostic tests, after beta's ratchet refused a doc comment main's CI
never checked); #221 (PR 252: the gate's last four store-wide reads are the
user's); #244 (PR 245: the retire guard reads the proof's presence; the PR had
been closed unmerged by a landing script's unconditional branch delete on a
draft, restored, and the scripts now mark a reviewed draft ready and delete only
on MERGED); #247 (PR 254: respend refuses a phantom whose request holds a
proof, through the guard's shared reader); #228 (PR 256: a tip settles a kept
reorg whose root a new header carries; the first round stored every unmoved
row and a stored verdict made a row churn, narrowed to plain mined); #232 (PR
260: a split key's hash lowercased at delivery and on read; a slice at byte 6
panicked on a malformed stored key, then A5's text rule refused strip_prefix,
written as an equality on the first six bytes with its reason recorded); #204
(PR 259: hermetic scenario 24 holds wallet A's store busy from a second
process; under WAL no outside lock refuses a reader with BUSY, the ask amended
to the two write doors' 503 and the reads' availability; three rounds: a
write-side probe for an outlived lock, the shared GET's contract kept so the
stranger guard still refuses a 401 daemon, and the differential's self-test
pinned at 24 by the captain); #248 (PR 263: the relay poller keeps its cursor
below an item that met a busy store, with the interval reset first since tokio
fires a missed tick and the next together); #249 (PR 268: the callback route's
8 s deadline under Arcade's 10 s, the message truthful about the header service,
the split state a retry converges proven on the rows); #258 (PR 270: a settled
row stores the header the recheck stood on, since bsv-tracker's recheck never
replaces its checked header; bsv-tracker #30 filed); #237 (PR 262: a carried
input naming no signer, or a mismatched length, refused at the gate with the
reference's words, fixtures 121 and 122; the wire recorder split into section
functions when the flat list crossed the complexity ceiling); #234 (PR 261: a
named change coin stays offered by the gate as the Lean's guard offers it, the
toolbox's refusal the served answer, fixture 120, the keeper citing the
reference's createAction.ts); #174 (PR 273: the ten handlers.rs mutants the
Nightly missed each caught by a behaviour test, the floor staying 74 until the
night's record); main into beta (PR 272: the six 0.18.2 commits, fixture 123
for #157's bound; two changelog text folds under ruling (b)); THE PIN (PR 274:
bsv-rs =0.9.0-beta.1, toolbox =0.14.0-beta.1 and bsv-tracker =0.9.0-beta.1
together, one bsv-rs in the lock, after the first launch found bsv-tracker
0.8.0 pinned to bsv-rs ^0.8 and the keeper cut the tracker's prerelease in two
hours; the underReceivedRule label on both received doors, the abort class on
the accepting-word sentences alone after the review caught a prefix match that
would have moved every abort refusal, fixture 103 flipped onto the verifier's
refusal, #235's agreement matrix with its one deferral the keeper read as not
a defect, the seed knob behind the hermetic wall, every BEEF already
parents-first; #140 and #235 closed, the beta.2 checklist gathered as #277);
PR 276 (the signAction guard test driven with a deferred caller input, so the
toolbox's dependents gate reads green against our beta, which it now does on
its own, run 40415011255, and fails their run whenever our beta breaks against
their tree); #265 (PR 275: an empty unlockingScript and a zero length refused
with the reference's words, the handler's hex refusal in the reference's
sentences down to UTF-16 counting and U+FEFF trimming, fixtures 125 to 128,
the at-bound acceptance the keeper's scenario needed).

On main: PR 264 (0.18.2: the two pins, the probe's one pass, #157's bound, the
deny template adopted with the advisory entry gone; two Opus reviews) landed
at 07adfcb; the mainnet gate ran once and read 18 of 23 because GorillaPool's
ARC held every submission at ANNOUNCED for over thirteen minutes, the funder's
own transaction included, while WhatsOnChain's node accepted the same
transactions at once and all 44 valid descendants followed (the funder's
accounting real, 136,634 to 134,464 sats); the tag waits on the owner's word
on a second run. The CI balance (PRs 266, 267, 269): a pull request runs
exactly the required set (the keeper patched both protections: macOS off, the
hermetic e2e required on beta), the bench and the differential on every push
against the push's before and on the tag's run, the differential on a PR only
under the refactor label, Dependabot at most two open with its one bsv-stack
group; the first push run under it green on all thirteen jobs. The checkout's
binary symlinks repointed at a 0.18.2 build after an older target was removed.

Across the stack: bsv-rs 0.9.0-beta.1 on the index (the gate's reading sent:
clean, no finding); #137's twenty refusal classes as the reference's WERR table
(the host map pinned by class name; INVALID_BEEF and UNSPENDABLE_COIN fold at
beta.2; the four not-found doors by the keeper's reading); #105's substrate
shapes; toolbox 0.13.1 and 0.14.0-beta.1 on the index on the owner's tags;
toolbox #66, #106, #163, #169, #177 landed with their shapes recorded; the
toolbox's #200, #201, #211, #212, #202, #203 (certificates issued before it
must be re-issued: the owner's note for beta.2) for the next prerelease; the
keeper's readings on #234, #237, fixture 103's row (status, class, offset and
reason, never the host's code string) and fixture 124. The lessons in memory:
a brief's proof list runs the tools' own self-tests; A5's shrink-only
allowlist; a mainnet gate failing on explorer listing is judged at the
broadcaster first; main's harness ignores BSV_WALLET_BIN; a review brief never
takes the chain's output name; a pin moves every dependent crate; cca refuses
to reuse a lane name whose branch holds commits; the box runs out of ephemeral
ports under parallel lanes.

Open after the day: the owner's words (the second gate run and the v0.18.2
tag; the v0.19.0-beta.1 tag after tonight's Nightly, in the keeper's brief;
the certificate re-issue at beta.2; the production daemon on 3322 was down
tonight for the certificate count); #277 the beta.2 pin; the floor raise for
handlers.rs after tonight's record (#174); #243, #215, #212, #211, #195 (the
CHANGELOG fragments, worth doing before the next busy day), #192, #191, #193
by the queue; #196 closes with the tag.

2026-10-27 (12:0xZ to 23:0xZ): THE TWO TAGS, THE PRODUCTION RESTARTS, THE
HYGIENE PASS, THE GATE ON MAIN AND THE CI BALANCE. The session was cut at
08:3xZ and resumed at 12:04Z; the chains, the watchers and one fix lane were
relaunched from their pushed heads. The owner's words of the day, verbatim
through the keeper: "arc is old af... we use arcade now for everyting" and
"arc is just deprecated, arcade is the standard" (#300: Arcade the default
broadcaster, GorillaPool's ARC never a rung, classic behind the deprecated
knob ARC_MODE=classic for the hermetic stub until #299); "why are we doing
nightly shit still... its a waste of resources" (#303: the Nightly's schedule
gone on both lines, the measurement on dispatch, one before a minor tag, none
for a prerelease); "I liked your reccomendtaions so those are approval" (the
beta.2 standing word, the restarts, the certificate count 46 on #277); "idk
you gotta find it man" (3322's wallet is ~/bsv/bob-wallet); "yeah do both of
cours e" (the hygiene pass and its scheme); "no merge to main until god tier
and 110% confidence nothing breaks and we in conformance with other SDKs and
all other sdk issues are logged on their repos" (THE GATE ON MAIN); "I like
all of your recommendations... as recommended" (19.3: #313's --from, the two
standing words on upstream PRs and Worker fixes, the daemons' restarts on a
released build the CLI's own); "sick of seeing all these CI failures... why
are none of them passing... they are all so hardcore and slowing us down"
(THE CI BALANCE).

The tags. v0.18.2 from main e7844c4 at 13:53Z by the keeper on the second
gate's 23 of 23 (net cost 2,170 sats, Arcade the first rung, the funder's
three `broadcast accepted name=ArcadeV2`, zero GorillaPool lines; the record
under /Volumes/Lanes/out/bsv-wallet-cli/gate-0182c/), on the index at
14:18:24Z; v0.19.0-beta.1 from beta 687f0b6 at 13:40Z (pinned bsv-rs
=0.9.0-beta.1, toolbox =0.14.0-beta.1, bsv-tracker =0.9.0-beta.1), on the
index at 14:17:41Z; the announcement to the dependents' captains per
RELEASING 1b (LOW's fleet, the stack's witness, the apps' payers; LOW stays
pinned through Wave E and moves by its section 8 recipe after the canary
day). #196 closed with the tag.

The restarts and the shared-binary rule. 3322 (~/bsv/bob-wallet), 3396
(~/.bsv-stack-coin-read, --db wallet.db) and the funder (3320) restarted at
14:22Z on the released 0.18.2 from ~/.cargo/bin, each logging
post_beef_ladder mode="arcade" first. That install replaced the shared binary
under two LOW collect jobs (bsv-low #922) and bsv-dd's gift sender (its
version guard refused the next launch): THE STUDIO RULE from here, a program
never replaces a binary another program runs; every seat pins its own root.
Each seat holds its own copy of 0.18.2 since 14:55Z (bob-wallet/bin,
.bsv-stack-coin-read/bin as a real file, e2e-funder/bin; sha 8be4b2d5...);
the funder restarted from its own at 14:57Z, 3396 at 15:27Z on the keeper's
call (pid 3354), 3322 at 20:04Z on the word that restarts on a released
build are the CLI's own (pid 25390); no daemon on this Mac runs from the
shared bin. The next release installs with --root per seat (memory:
shared-binary-rule).

Landed on beta today, each hand-closed with its record: #243, #198, #195,
#192, #212, #211, #285, #215, #282, #132, #286, #127, #126, #290 (before the
cut), then #138 (PR 307, the CORS layer outside the bearer), #301 (PR 306,
the fuzz crate's pins and a stable CI check; explorer_bodies fuzzing the five
readers; the fix round: the BananaBlocks case built through
explicit(...).with_bananablocks per case), the floors (PR 308 c3af6f1 from
the last scheduled Nightly, run 40473522272: broadcast_verify 86, explorers
82, handlers 100, spend_gate 90, tracker_host 75; 1,062 of 1,168 mutants
judged, shards 18 and 19's baselines timed out at 0bd48af, the score
unaffected; #173, #174, #124 closed, #175 at 75; the dictated fold PR 314),
#119 (PR 309, the harness's relinquishOutput body), #300's beta half (PR 312
fae65b1: main's 0.18.2 content as ONE LINEAR PORT COMMIT because beta's
protection requires a linear history and the lane's merge-shaped branch
could not land; then the typed ArcMode; the lesson: a stable patch comes to
beta as a cherry-pick backport, never a merge), #171 (PR 318 507ea28, the
MCP's typed Config read once with --config redacted, words.rs by #[path],
the arch scan's roots src and mcp/src; a one-line rebase round), #292 (PR
326 3d0cd8e, the adoption pass's failing rows marked and paused, the
regression the review caught fixed: a repaired path adopted on its due
check), the NORTH-STAR folds (PR 317, PR 319), the CI balance (PR 323
7b32844). On main: PR 302 (#300), PR 304 (the Nightly's schedule), PR 320
(the .env.example docs patch), PR 324 (the CI balance), PR 325 (bench.yml
and differential.yml registered). #303 closed (the schedule gone on both
lines).

THE HYGIENE PASS (#316, the record with the snapshot of every issue's former
labels and milestone): milestones name the lines (0.18.x the stable line;
0.19.0-beta.2 the pin and what lands with it; 0.19.0 Phases 0 and 1, the
divergences read, the line's bugs, REACHED ONLY AT THE GATE; hardening
Phases 2 to 6 with the phase as a label: phase-2-spend-path to
phase-6-desktop); the Phase 0 to 6 milestones closed, 0.20 and 0.21 re-cut
away on the gate's word; the labels the stack's six (parity, divergence,
bug, design, docs, ci) with api-break as CI's switch, refactor and
no-changelog the switches CI reads, dependencies Dependabot's, and
toolbox-dependent and stack-charter kept as qualifiers; twenty labels
deleted. #4, #7, #39 closed by pointer (#23 kept: its ask has no other
home); the oracle's six epics #69, #72, #74, #76, #107, #110 closed with
records. The three numbers at 17:02Z: open 55, without a milestone 0,
without a label 0.

THE GATE ON MAIN (verbatim above): beta merges into main only when every
scoreboard row reads at target with recorded evidence, every phase has
exited, the gates and the production reads are green, and conformance with
the other SDKs is proven (the divergence list empty, or every row carrying a
filing number on the other SDK's repository or the word "ours" with its
reading); never at a phase exit. 0.18.x keeps its patch releases from main;
0.19.0-beta.N continue on beta. Written in docs/NORTH-STAR.md (PR 319), on
#316 and in memory.

THE CI BALANCE (beta 7b32844, main fb683dd and ae3c710): a push runs exactly
the required set (ten on beta, seven on main), the same as a pull request,
nothing more; bench.yml, differential.yml, nightly.yml and build-macos.yml
run on workflow_dispatch and once before a tag (RELEASING step 4), each with
its badge, README's first badge ci.yml on main; a run a newer push cancels
is never a red; a refactor PR's differential is the lane's REPORT line and
its cold review's own run (lanes rule 11).

Findings filed today: #310 (beta's suite runs within 50 s of cargo-mutants'
300 s limit: a dispatch would judge nothing and show twenty green shards;
before the next dispatch), #311 (#174's second half), #313 (LOW's and
bsv-dd's: a received coin named under the scoped word is offered but the
allocator orders proven coins first; the owner ruled a --from flag; the lane
found the named-input door cannot spend a managed coin, the toolbox and the
reference refusing it; RE-SCOPED with the keeper and the toolbox captain to
toolbox #353, a must-spend list on create_action_offering, the CLI's --from
the thin wrapper after it lands; no new owner word needed), #315 (the change
floor is a quarter of the basket's minimum, 2,500 sat by default), #321 (the
MCP refuses a blank URL where the main crate treats blank as unset), #322
(the toolbox captain's: the classic branch names its own URL at the pin),
#327 (actions and outputs page nothing: limit 100 hard-coded, the oldest
served; bsv-dd), #328 (the tracker's pause under the daemon's feed, for the
owner). The lesson of #313 is in memory: a mechanism put under a ruling is
traced to the end of the path and driven by a test first.

THE BETA.2 PIN (#277, the brief complete with every intake: bsv-rs #329's
Some(originator) at 59 lines, #331's verify(&tracker, fee, None) (no CLI
call), #339's review-actions shape, #354's unsigned-answer refusal; the
toolbox's #295 allocators, #298, #301, #280, #291, #269 (the certifier's
x-bsv-auth-identity-key), #317 both halves (mainnet() refusing without
with_arcade: nine CLI sites; #322), #300's clientNonce shape, #307's
includeInputs, #313's purposeless coins, #304, #312/#357/#359's BRC-177
label refusals and texts; the fuzz crate's pins; the toolbox's dependents
check dispatched before the PR since their push run no longer reads our
pins): bsv-rs's 0.9.0-beta.2 bump follows its #331's fourth fold (the index
about 23:3xZ at the earliest), the toolbox's 0.14.0-beta.2 ready word two to
three hours after its pin lands, the common hour then; the chain
(chain-lane.sh p1-277-pin-beta2) fires on that hour. 18.10: bsv-rs beta.3
accepts a legacy minimal-length-key HMAC tag on a miss (#379), our verifyHmac
door inheriting it at that pin.

Open after today: #277 (the pin), #310 before any dispatch, #299 (the stub
plays Arcade), #313 after toolbox #353, #142 (the bench baseline, Phase 2's
exit needs it), #155 and #194 (toolbox-dependent), #178, #173-era follow-ups
#175 and #311, the flakes #191 and #193, the divergences #146 to #149, #117,
#122, #118, the structure work #78, #79, #82, #84, #101, and the hardening
milestone's 21. Production: 3322 pid 25390, 3396 pid 3354, the funder pid
62391, each on its own copy of 0.18.2 with Arcade first; the witness tick at
12:30Z tomorrow is the production read of Arcade. The keeper writes the
fleet's checkpoint page at 09:00Z from the three numbers.

2026-10-28 (01:5xZ to 12:5xZ): THE BETA.2 PIN LANDED AND ITS MAINNET GATE
PASSED. The common hour came in two halves: bsv-rs 0.9.0-beta.2 and
bsv-tracker 0.9.0-beta.2 on the index at 01:51Z and 01:59Z, the toolbox's
0.14.0-beta.2 at 08:52:46Z after its tag moved once to a workflow-permission
fix (a job in a called workflow may not ask a permission the caller does not
grant; our release.yml grants ci.yml exactly what it asks, checked on both
lines). A watcher fired the pin lane on the index's answer at 08:53Z.

The lane blocked twice, each time on a true fact. First, the published toolbox
erases every ordinary refusal to WalletError(String) at the WalletInterface
boundary (error.rs:352-365; toolbox #185 never reached the package) and its
certifier_info returns None so both discover doors answer nothing (toolbox
#388): the #277 intake had promised a typed host map and a host-supplied
trust setting the package never had. The ruling, under the owner's standing
hour: the pin lands against the package AS PUBLISHED; the gaps are #330 (the
typed host map, blocked on toolbox #404) and #331 (discovery trust settings,
blocked on toolbox #388), each landing at the first publish that carries its
fix, never a text-matched imitation. Second, two gates the brief had asked for
before the PR were not the PR's: the toolbox's Dependents workflow had no
branch input (toolbox #405, landed the same day; run 40679043409 against the
branch then failed on create_action_offering's fourth argument, toolbox
#353's must-spend list behind their tag, red by construction until the CLI
pin that carries it, #313), and the patched-tip build fails inside the toolbox
on bsv-rs's ordered maps (toolbox #406, their beta.3 re-pin). PR 332 was
opened by the captain from the lane's prepared body (29 commits by the end:
the three pins in both workspaces, 56 originator sites to Some, the classic
ARC construction, the base64 reference, the certificate JSON at bsv-rs #268's
shape, the review-actions carrier, 160 wire rows re-recorded in two named
commits, a beta.1-shaped tracker row still read), a cold review on Opus found
six things (the review-actions body in the reference's code-5 shape, the hex
tx rows, five bsv-rs and two toolbox changelog entries missed at the intake,
unticked items, scenario 08's bad BEEF refused before the BEEF, one changelog
name), a fix round closed all six, the re-check said merge, and it landed at
11:29:55Z as beta b101d67. #277 and #322 closed by hand with the record; the
toolbox's dependents doc carries the pin row and both expected-red runs
beside our green CI.

The mainnet gate ran once on b101d67 at 11:36Z: 23 of 24, net 2,200 sats.
Scenario 24 (busy-store, beta-only, its mainnet branch's first run) chose a
funder coin that could not cover the change outputs the allocator opens
toward the basket's desired count (the reference's rule: a quarter of the
10,000 minimum first, then 10,000 each until six coins; #315), so it pulled
all four of A's coins. #334: the scenario now computes that demand at run
time from A's own database and picks the funder's largest coin that clears
it (PR 335, a fixed threshold found insufficient by the cold review, the
run-time rule in the fix round), landed 12:43Z as beta 8a41565. The gate ran
once more at 12:46Z: 24 of 24, net 2,185 sats, Arcade first, the funder on
its own 0.18.2 copy with its database untouched, the test wallets on the
pin; the hour went to the bsv-rs captain (the canary's last step) and the
keeper. #336: the funder's largest mid-run coin (17,399) is too close to the
demand (14,023); a standing coin the funding never spends.

Two production facts for the owner's brief, confirmed by both reviews in the
toolbox's code: certificates issued before toolbox #203's scheme change must
be re-issued before production takes a build carrying it (the 46 of one
certifier, 2026-03-29 to 31; a 0.19.0 release step on the owner's word;
production on 0.18.2 untouched), and certificates --reverify audits signatures
only, so #333 asks for the keyring detector and names the re-issue tool (the
acquire door in its issuance form, then relinquish). The keeper put two
questions on the brief with a recommendation the captain shares: one toolbox
0.14.0-beta.3 at bsv-rs beta.3's common hour carrying #404 (landed on their
beta at a9bb733 the same afternoon: typed WalletRefusal at the boundary,
texts unchanged, the SDK's Display losing its "wallet error: " prefix), #353
and #374, the CLI's one pin PR taking #330 and #313 together; #388 later.

Lessons kept: a pin brief reads the package, never the intake's promise; a
proof the other side's workflow cannot serve is not this PR's gate; a
beta-only scenario's mainnet branch gets its first run at the line's first
gate, so a lane that adds one sizes its live inputs against the documented
floors; cca refuses an existing worktree folder without --reuse (the chain
scripts take CCA_REUSE=1, and chain-fix.sh CCA_WT=<worktree>); the CLI signs
nothing with the Chronicle hash type (the gift claim at 0x41 and 0xc3, the
respend replacement through bsv-rs's P2PKH template, all version 1).

Open after the day: the owner's two answers; the toolbox's beta.3 (its list
before it lands) and bsv-rs's beta.3 (the ordered maps reach us only through
serde key order; #404's &self receivers touch no CLI site; the auth transport
texts match nothing of ours); #310 before any Nightly dispatch; #313 and #330
at the next pin; #331 and #333 on 0.19.0; #336 before the next gate.

2026-10-28 (13:0xZ to 21:3xZ): THE BETA.3 PIN, LANDED THE SAME DAY WITH A
GREEN GATE. The owner's word at 14:2xZ, verbatim through the keeper: "I like
your reccomendtaion if its god tier and north star", read as 19.7 (b): one
prerelease each at the common hour (bsv-rs 0.9.0-beta.3 at its tip after
#429, the toolbox's 0.14.0-beta.3 against it carrying #404, #353 and #374),
the CLI's ONE pin PR on the crates taking #330 and #313 together, the
mainnet gate once, #331 after, nothing before the index hours; 19.8 stands
(the pre-#203 certificate re-issue a 0.19.0 release step, #333 the
detector). The hours: bsv-rs on the index 15:30:17Z (tag bf97cc74), the
toolbox 19:34:31Z (tag aaaf2e2a), and a third nobody had named: the pin
lane fired at 19:35Z on the toolbox's crate and blocked three minutes later
on a package fact, right by its brief: bsv-tracker's newest crate pinned
bsv-rs exactly at beta.2 while the toolbox's beta.3 pinned beta.3, so no
lock could hold one bsv-rs (#341, the proof from the downloaded crates).
The tracker is the keeper's own crate; its 0.9.0-beta.3 was cut by the
keeper's hand inside the word already given (index 19:46:17Z, no code
change), a watcher resumed the lane in its own worktree, and PR 345 landed
at 21:11:41Z as beta 6e1a7bc: 23 commits, the three pins in both workspaces
and locks, the typed host map with src/toolbox_words.rs deleted and every
served class from bsv-rs's WalletRefusal carrier (#330), --from through the
toolbox's must-spend list (#313), 174 wire rows and 48 consumer rows
re-recorded with reasons, 490 gate vectors at 0 divergences, the hermetic
suite 24/24, a differential of 112 attributed differences with the
same-build floor at 0, the toolbox's Dependents green against the branch
(their #405 input, the clone line read; plain on beta after), a cold review
on Opus: merge. The gate ran once on 6e1a7bc at 21:16Z: 24 of 24, net 2,181
sats, Arcade first, the funder on its own 0.18.2 copy. #338, #341, #330 and
#313 closed by hand with the record; the 0.19.0-beta.3 milestone is empty,
so the tag is the owner's word through the keeper.

Between the two pins, the gate's own scenario got two fixes under one lesson
(a beta-only scenario's mainnet branch runs first at the line's first gate):
#334 (the scenario computes the allocator's demand for its kept
external-input template from wallet A's own database, the basket's desired
count and minimum and A's available change by the toolbox's own query, and
picks the funder's largest coin that clears it; PR 335 at 12:43Z, beta
8a41565) and #336 (A's desired count set to one around the template and
restored, so the demand is 4,023 sats in every state, and a 15,000-sat coin
in the funder's named basket e2e-external that the funding transfer carries
once and the allocator never selects; PR 339 at 15:35Z, beta 1d8f4da); the
beta.3 gate created that coin on mainnet for the first time and ran scenario
24 with A at three coins, this morning's failing state, at a margin of
10,977.

Deferrals the pin lane filed as package facts, never imitated: #342 with
bsv-rs #439 (typed evidence for generic faults and abort acceptance), #343
(a 64 KiB decrypted-field figure that neither crate holds, by the toolbox
captain's reading: ours to find and delete, a lane on it), #344 with toolbox
#430 (relinquish of an unknown serial must refuse WERR_BAD_REQUEST as the
reference does; the published zero-row success is their divergence). Three
consumer refusals moved with the typed classes and were told: app-payers'
abortAction of an unknown reference 400 VALIDATION_ERROR where 404 NOT_FOUND;
low-fleet's potPayout and the x402 skill's refund VALIDATION_ERROR where
INVALID_BEEF; all three without the generic SDK prefix; LOW's and bsv-dd's
captains hold no matcher on them (LOW's funder already reads the class;
bsv-dd's gift sender stays on 0.18.2), and the x402 skill's is on the
owner's brief. The stack's rule of the day on live script verdicts (every
live door judges under the node's word per input; the default script mode
is parity only) is read on the CLI as #340: no interpreter of its own, every
live judgment the toolbox's internalize door's, two test-only verifies named;
a lane on it after the pin.

Lessons kept: a pin set is every crate the manifest pins, and the pin's
watcher and brief name each one's prerelease as a precondition (the
tracker's hour was nobody's until it blocked); a proof the other side's
workflow cannot serve is not this PR's gate, and one it can serve is run
and its clone line read; a lane that blocks on a package fact pushes its
branch and its evidence so the resume costs nothing; the keeper's hourly
cap is kept by folding, never by silence on a stop.

Open after the evening: the beta.3 tag (the owner's word); #340 and #343 in
lanes; #331 on toolbox #388, #342 on bsv-rs #439, #344 on toolbox #430;
#333 and the re-issue at 0.19.0; #316 and #315 on the beta.2 milestone (a
record and a doc); the next pin's issue opens at the next prerelease's hour.
## 2026-10-29: the beta.3 tag, the Studio's browser rule on both lines,
scenario 24's probe, the re-record's render, the pin brief's tells

The day opened on a new profile (the session restart, the memory directory
moved intact) with the owner's hygiene ruling applied: 42 GiB freed at
15:53Z by this captain's own recorded paths, six directories named as not
mine and left. Four rulings arrived through the keeper on the owner's "I
agree with all your recommendations" (16:3xZ): 19.10, the keeper tags
v0.19.0-beta.3 on a version commit, a version PR first; 19.11, the toolbox's
`chain` field api-break rides the beta.4 pin; 20.2, #331 closes at beta.4 on
toolbox #388; 20.4 (a), #342 at beta.4 on bsv-rs #439. The version PR (351;
`Cargo.toml` 0.19.0-beta.3, the changelog assembled from fourteen fragments,
the fuzz lock's one line authorized in a fix round) landed at 440cd39, and
the keeper tagged it at 17:30:34Z: the Release run green at 17:46Z, the
crate on the index at 17:40:34Z, the release object a prerelease. The gate
on 440cd39 was a composite: 23 of 24 at 17:25Z, with scenario 20 failing
because `~/Library/Caches/ms-playwright` had vanished under the owner's
cache clearing, then scenario 20 alone at 17:29Z after a restore, 0 sats;
the record says so plainly. #352 gave `release.yml` the prerelease mark from
the tag's hyphen on both lines (PR 353, beta 0030013; PR 354 by cherry-pick,
main 3ff6fdb), and corrected one statement of mine: the two lines'
`release.yml` differ by one intended line (beta grants its CI call
`pull-requests: read` for the ratchet and differential jobs; main has
neither), read by diffing `origin/beta` against `origin/main`, never the
local refs.

The Studio's browser rule came from the keeper at 19:4xZ, after the shared
Playwright cache lost a pinned build three times in a day (the owner's
clearing, then two installs, since `npx playwright install` removes builds
it reads as stale: my own 17:29Z restore of 1208 removed LOW's 1228, and
LOW's bisect refused twice): a program that drives a browser sets
`PLAYWRIGHT_BROWSERS_PATH` to a folder of its own and installs with
`PLAYWRIGHT_SKIP_BROWSER_GC=1`; the harness checks its pinned build before
any run and reinstalls by its own step in the gate's preflight. This
program's folder on the Studio Mac is
`/Volumes/Lanes/targets/bsv-wallet-cli-playwright` (standing; the cleanup
never removes it); the gate script and the chain scripts export both
variables. In the repo, #355: the harness sets the variable before
Playwright is first required (a caller's value wins, else its own
`tests/e2e/.browsers`), its step `node tests/e2e/install-browsers.js`
installs through the local binary with the GC skipped, `run.js` checks the
pinned headless build as Playwright's launch resolves it and refuses before
any wallet is started or the funder asked, naming the folder, the revision
and the step with Playwright's first line only; both workflows cache and
install the harness's folder. PR 356 (beta a8c0053; two cold reviews on
Opus, the first asking only the first-line refusal) and PR 359 on main
(ed2dcc1; a port, since main's harness has no hermetic mode and its CI runs
no harness: the cherry-picks kept with their conflict resolutions, the first
lane's browser install in main's build job dropped by a commit on top, never
a rewrite). The gate ran once on main's tip at 22:05Z: 23 of 23, net 2,155
sats, scenario 20 under the program's folder; a first attempt exited on the
gate script's own unset single-scenario hook before the harness started, 0
sats.

At 23:2xZ the keeper amended the rule per host: on every host a program
keeps one standing browsers folder, its standing checkout's
`.cca/pw-browsers` (gitignored), which the harness's default resolves to
before any per-worktree fallback, so a lane on the box never installs into
its own target folder; the box runs every scenario a lane runs. The two box
lanes of the day had unpacked the CDN zips by hand after the step's unzip
hung there twice, which leaves no record the preflight can check: the hang
is a defect of the step, to fix on the box. That is #362, a lane on the box
with the hermetic suite once, its main copy to follow as PR 359 did; the
Studio's exported path stands as named.

Found on the way, #357: scenario 24's EXCLUSIVE lock probe asserted on the
sqlite3 shell's words, and a 3.53-or-later shell answers "no such table:
e2e_busy" (exit 1) where 3.51 answers "database is locked (5)"; reproduced
offline on a standalone WAL database with one open reader under the system
3.51.0 and the Homebrew 3.53.1, and on the box's 3.54.0 where the #355 lane
had failed it. The probe now asserts on behaviour (a non-zero exit, `wal`,
no `e2e_busy` table afterwards, the holder's normal write next), proven
under four shells with the mechanism sourced from the shell's code; PR 358,
beta ea8fb49, a 24/24 hermetic run on the box. Two ratchet numbers had
fallen since the record of 2026-10-27, with the beta.2 and beta.3 pins (#277
and #338, the Rust that left with the typed host map), not with the day's
harness changes, which touch no Rust: the lane I briefed with the wrong
cause read the diffs and corrected the attribution, and the numbers
(functions_over_60_lines 86 to 85, duplicated_tokens 32486 to 32382) were
re-recorded in their own commit under the pinned toolchain (PR 360). Its
first CI run went red at the ratchet job's scoreboard step: the table in
docs/NORTH-STAR.md is rendered from the records, and tools/scoreboard's rule
asks a PR that re-records to render it in the same PR, which my brief had
not named (the precedent 406c579 predates the scoreboard) and the cold
review, holding to the brief, had not checked. A fix round added the
rendered table as a commit on top, landing as beta 82ce5c2 at 23:29Z
(ea2b106 the record, 82ce5c2 the table). The procedure gap is #361: the
tool's own hint, RATCHETS.md's step 4 and lanes README rule 5 name the
render, by a PR after that landing, carrying the fingerprint re-record the
tool edit forces with the table rendered in the same commit.

The pin brief for beta.4 (#347) took the libraries' hours as tells: bsv-rs
#447 (bsv-rs #439) at 455fe780 is #342's pin sha, its abort record and fault
matched by class and family inside the pin PR, beta's typed map carrying
both through its wildcard until then; #455's typed not-judged verdict costs
this repo no arm (it matches neither `SpendRefusal` nor the stream's
`Verdict`), and `Error::TransactionNotJudged` gets a typed arm on the input
index and the cause; `ChainTip.genesis_height` deprecated hits the test
tracker's literal and one reader under CI's `-D warnings`; #467's totals
widen to u64 where four commands read them, two through an `i32` cast; #457,
#459, #462, #468 and #471 touch nothing here. The 64 KiB question (#343)
closed: neither SDK bounds a field value on the direct acquire path; the
toolbox's #374 keyring proof is where the bound bites, now row D26 of their
divergence table at a8107be2, and fixtures 155, 173 and 174 keep the
behaviour and cite it at the pin. The toolbox landed #388 (6f234bc9, shape
(b2)) for #331.

Lessons kept: a lane launched without `--host` may run on the box (the
branch comes back as `box/lane/<name>`; the box's toolchain differs, the
captain's exports do not reach it, and the step's unzip hung there twice); a
harness assertion on a tool's words is a bug waiting for a version; a pool
account fault (a Codex token expired, a model refused) kills a launch in
five seconds and is relaunched plainly, the account and the model never a
captain's choice; a gate script's hooks are exercised unset before they
guard a mainnet run; a brief that names a precedent also names the rules
that postdate it (a re-record renders the scoreboard's table in the same
PR), and a review brief asks for CI's own tool steps, never the record
alone.

Open after the evening: the beta.4 pin on its watcher (#347 with every tell;
#331, #342, #344 ride it where the packages carry them); #333 and the
certificate re-issue at 0.19.0; #350; #315 and #316 on the beta.2 milestone;
the beta.1 release object's prerelease mark (the keeper's); #361 and #362 in
lanes after the evening (the re-record procedure; the box's standing browser
folder), their main copies after.
