# The wire fixtures (#69)

`tests/wire/` holds one JSON file per served call and per refusal body: the request as sent and the response as served (status, the headers that are part of the contract, the body), and `MANIFEST.json`, the list of every fixture the recorder wrote. `cargo test --test wire` replays every file the manifest names against the router and fails on any byte of difference after normalization, printing a line diff. The corpus is the contract of the served shape that MetaNet Client compatibility rests on: a refactor shows zero diff; a behavior change changes a fixture, and a reviewer sees the change in the fixture.

A refusal fixture names the status, the reference's class, the offset and reason where present, and what is stored; the exact sentence belongs to the toolbox and follows the reference's words (#290). The toolbox's dependents gate sets `WIRE_TEXT_DIFF=report`: a difference only in the top-level JSON `message` string prints one line `TEXT-DIFF: <fixture file name> <expected message> <actual message>`, with both messages JSON-quoted, and counts as replayed; the summary counts reported fixtures. Status, contract headers, `code`, every other field and key order remain strict, including later rows that read stored state; the internalize row's noted offset and script reason remain exact even inside its message. `response.text` bodies never receive this exception. The fresh-recording test uses the same comparison, keeping every other file byte strict. Unset is strict, any value other than `report` (including empty) is refused by name, and combining it with `WIRE_RECORD=1` is refused before recording. Our own CI stays byte for byte; a reported row is re-recorded at the next pin. The knob lives only in `tests/wire.rs`: the consumers' recordings and `wire_consumers` stay strict, despite sharing capture and normalization machinery.

## References, at pins

The field names are BRC-100 at `5119a9b` (2026-10-19), the line's pin of record: `git -C ~/bsv/brcs show 5119a9b:wallet/0100.md`. Its listOutputs module views admit a decimal-string balance and `spendable: false`; bsv-rs #105 supplies those JSON shapes at beta.2. This corpus records the ordinary basket view. The internalize-only 1,000-byte instruction bound already holds (fixture 123); createAction output instructions remain unbounded. The pin's varint changes are binary-wire changes and do not cross this JSON server. The beta.1 argument types do not change their JSON ranges or defaults; #198's paging checks are recorded in fixtures 129 to 140, fixture 141 records signAction's unchanged type refusal, and fixture 142 records listActions' unchanged offset type refusal.

## Reading a fixture

```json
{
  "world": "fresh",
  "request": {
    "method": "POST",
    "path": "/encrypt",
    "headers": { "origin": "http://wire.test" },
    "json": { "protocolID": [2, "wire test"], "keyID": "1", "counterparty": "02...", "plaintext": [119, 105, 114, 101] }
  },
  "response": {
    "status": 200,
    "headers": { "content-type": "application/json", "access-control-allow-origin": "http://wire.test", ... },
    "json": { "ciphertext": "<bytes:52>" }
  }
}
```

- **`world`**: the wallet and the server config the request is served from. Each world is built once per run from its name alone (`World::build` in `tests/wire_harness/worlds.rs`); a fixture naming any other world fails the run before a world is built, naming the file and the world.

  | world | what it is |
  |---|---|
  | `fresh` | an empty wallet, no bearer, the header stub up (tip 900010) |
  | `certificate-privacy` | an empty wallet whose only certificate carries the field and master key for the empty-disclosure witness |
  | `auth` | `fresh` behind a bearer token |
  | `callback` | `fresh` with an ARC callback token |
  | `tracked` | the tracker's words for a mined and a pending transaction, and one coin the chain feed froze |
  | `received` | one received coin, its creator mined at 900000 by a stored one-leaf proof, its raw transaction not stored |
  | `headers-down` | `received`, the header service answering 500 |
  | `broken` | `fresh` with the `outputs` table dropped (a store fault) |
  | `broken-host` | `fresh` with the tracker's rows unreadable: a feed status that is not JSON, a states table without its columns |
  | `busy` | a file store whose write lock another connection holds |
  | `busy-read` | a file store no reader can open: opened on SQLite's dotfile locking (no shared memory, so no WAL), its one lock held by another connection; carries the callback token and covers `abortAction`, `GET /chain-feed`, `GET /tx/{txid}` and `POST /arc-callback` answering 503 `STORE_BUSY` with `Retry-After: 1` |
  | `funded` | four mined coins the wallet can sign for (50,000, 40,000, 30,000, 20,000 sats, each a one-leaf block of its own that the header stub answers); the mined parents of three payments from a fixed stranger key and of a puzzle coin (`OP_1 OP_EQUAL`), whose blocks the header stub answers too; and a local ARC that takes every post |
  | `named-change` | one completed made change coin, offered under `SPEND_OWN_CHANGE=1`; the other worlds seed incoming coins and their spends make no change |
  | `trusted` | `fresh` whose wallet reads `TRUST_SETTINGS` through the CLI's typed config, naming the harness certificate's certifier (#331), and holds that certificate |
  | `stranger` | `trusted`'s certificate under settings naming another certifier |
  | `pages` | `fresh`, where a page the seed does not list is asked per power (#414 L5b): its held calls and the owner's answers, kept apart so no owner's-door fixture's rows move |
  | `shutdown` | `fresh`, whose server shuts down while a page's call is held (#414 L5b); it stays shut |
  | `spending` | `funded`'s four coins and its local ARC, where a page the seed does not list spends under the spending power (#414 L6): held, answered `once`, granted with a ceiling, past the ceiling held as a renewal; kept apart so no funded fixture's coins move |
  | `internalize` | `funded`'s four coins and its local ARC, and two outputs as an earlier release's basket insertion into `default` left them (`custom`, not change; written with an SQL update, as the toolbox's own test reproduces them), where the toolbox's two `internalizeAction` rules are served (rows 222 to 250), and the door of the toolbox's 0.14.0-beta.8.1 (rows 255 to 266); kept apart so its listings show only what those rows stored |
  | `internalize-held` | two change rows of the default basket with their derivation, written by SQL as `internalize`'s are, each a row the toolbox's 0.14.0-beta.8 `### Hosts` names: one with another `provided_by` and `purpose` than `storage` and `change`, one whose script is not the P2PKH of its derived key (rows 251 to 254); kept apart so `internalize`'s listings do not move |
  | `received-rule` | no coin of its own (the basket wanting none, so an exact spend makes no change), the stranger's payments' parents seeded, and a local ARC; its coins are the two received payments rows 272 and 273 take in through the toolbox's own `internalizeAction` (purpose `receive`, unproven), the coins the toolbox's received-coin rule reaches (rows 272 to 281, #627); kept apart so no other world's coins or listings move |

- **Order**: files run in name order, and the fixtures of one world share it, so a later file may read what an earlier one left (a certificate acquired, then listed; a template built, then signed or aborted). The permission audit is the process's, so `/permission-audit` lists every audited call of the files before it, in every world.
- **`request.json`** is sent as its compact bytes with `content-type: application/json`; **`request.raw`** is sent byte for byte (a malformed body); **`request.fill`** is a body of that many spaces (an oversized one; the file carries the count, not the megabyte).
- **`request.bind`**: a value of `request.json` that comes from an earlier fixture's served body in the same run. The toolbox draws a signable transaction's `reference` at random, so the fixture that signs or aborts it cannot carry the reference: its `json` carries `<bound>` there, and `bind` maps the JSON pointer in the request (`/reference`) to the file and the pointer it is read from (`096-createAction.templateToAbort.json`, `/signableTransaction/reference`). The recorder and the replay resolve it the same way, from the body that file was served; a bind to a file not served yet, or to a pointer its body lacks, is a failure naming both.
- **`owner`** (#414 L5b): the owner's act around the request, performed through the store module as the CLI does it (`tests/wire_harness/owner.rs`): `before`, grants and revokes written first (`{"grant": {"originator", "kind", "key", "expiry"}}`, `{"revoke": {...}}`); `holding`, calls sent first and held, their answers not recorded, each denied once the request is answered (the cap's fixture); `spent`, one call sent `times` times first, each answered 200 and not recorded (the open doors' bound's fixtures, #541: the allowance spent, so the request is the call past it; every world serves the bound on a clock that stands still, `OpenBounds::stopped`, so the refusal and its `Retry-After` never depend on how fast the spent calls ran); `after_ms` and `answer`, every open row answered `grant`, `once` or `deny`, or the server shut down (`shutdown`), once the request's call is held and that many milliseconds have passed; `amount` is a spending grant's ceiling (#414 L6). The recorder and the replay perform it the same way.
- **`response.json`** is a JSON body after normalization; **`response.text`** is a body that is not JSON (the framework's own rejections, an unknown route's empty 404, a preflight's empty 200).
- **`response.headers`** keeps `content-type`, `retry-after` and every CORS header: `access-control-allow-origin`, `-allow-credentials`, `-expose-headers`, `vary`, and the preflight's `-allow-methods`, `-allow-headers` and `-max-age` (the server sets no max-age; the fixture shows its absence). The rest is the transport's.
- The recorded JSON keeps the body's own key order: a field moved is a byte changed. Each served JSON body is also checked to be compact JSON in that order, so the comparison is of the body's bytes after normalization.

## What the run judges

The `certificate-privacy` world isolates rows 158 to 160 from the earlier
field-bound certificates: the toolbox's certificate lookup takes a page of
two before matching the serial. Its only certificate is the one being proved.

The manifest, before any fixture is served: every file `MANIFEST.json` lists must be on disk, every file on disk must be listed, and nothing but `.json` fixtures may sit beside the manifest. A fixture deleted, a fixture added by hand, or a stray file is a failure naming it, not a silent pass.

Beyond the byte comparison, the replay fails when the corpus stops covering:

- each of the 28 BRC-100 calls the router serves,
- the nine host routes (`/permission-audit`, `/permission-audit/reset`, `/chain-feed`, `/tx/{txid}`, `/arc-callback`, and the owner's four, `/origins`, `/origins/answer`, `/origins/grant`, `/origins/revoke`, #414),
- every world named above (a world no fixture is served from is a gap),
- every code the server answers with, at its status, at least once. `AppError`'s (`src/server/error.rs`, `APP_CODES`): 400 numeric `5` (`REVIEW_ACTIONS`), 400 `VALIDATION_ERROR`, 400 `ERROR`, 400 `ERR_PERMISSION_DENIED` (the browser gate's denial, #411; a held call the owner denies, a revoked power, an admin-reserved name, a host route a page asks, #414), 402 `INSUFFICIENT_FUNDS` (with `held`), 403 `INPUT_NOT_OFFERED`, 403 `ACCESS_DENIED` (the owner's door's refusal of a page, its preflight and a `Host` beyond loopback, #414), 409 `CONFLICT`, 500 `INTERNAL_ERROR`, 501 `PRIVILEGED_NOT_SUPPORTED` (`privileged: true` on the nine calls that name it, #145), 502 `SERVICE_ERROR`, 503 `STORE_BUSY` (`Retry-After: 1`), 503 `HEADERS_UNAVAILABLE` (`Retry-After: 2`), 429 `ERR_PERMISSION_PENDING_LIMIT` (`Retry-After: 5`, the held calls' cap, #414) and 503 `WALLET_BRIDGE_UNAVAILABLE` (a held call at the server's shutdown, #414). The router's own (`src/server/mod.rs`, `ROUTER_CODES`): 401 `UNAUTHORIZED`, 404 `CALLBACK_DISABLED`, 404 `NOT_FOUND` (the host's missing transaction word), 400 `BAD_JSON`, 400 `BAD_PAYLOAD`, 413 `TOO_LARGE`, 500 `ERR_CHAIN_FEED`, 500 `ERR_TX_WORD`. A fixture answering a code this list does not judge is a gap too: the list and the corpus agree in both directions.

`APP_CODES` is typed by hand, but it is checked against the source: `every_app_error_code_is_judged` reads every `(StatusCode::NAME, "CODE")` of `Kind::answer` in `src/server/error.rs`, the one table of the served layer's statuses and codes (#81), and fails unless `APP_CODES` and `UNPRODUCED` together are exactly that set, at the same statuses. A code added to the source with no fixture fails it by name, and so does one gone from the source.

| class in the pinned host map (#330) | status | served code and evidence |
|---|---|---|
| `UnsupportedAction` | 501 | `NOT_IMPLEMENTED` |
| `InvalidHmac` | 400 | `VALIDATION_ERROR` |
| `InvalidSignature` | 400 | `VALIDATION_ERROR` |
| `ReviewActions` | 400 | `numeric 5, typed carrier below` |
| `InvalidParameter` | 400 | `VALIDATION_ERROR` |
| `InsufficientFunds` | 402 | `INSUFFICIENT_FUNDS` |
| `AbortRefused` | 409 | `CONFLICT` |
| `NotImplemented` | 501 | `NOT_IMPLEMENTED` |
| `Internal` | 500 | `INTERNAL_ERROR` |
| `InvalidOperation` | 409 | `CONFLICT` |
| `UtxoReviewInconclusive` | 409 | `CONFLICT` |
| `ActionBatchState` | 409 | `CONFLICT` |
| `BroadcastUnavailable` | 502 | `SERVICE_ERROR` |
| `InvalidMerkleRoot` | 400 | `VALIDATION_ERROR` |
| `MissingParameter` | 400 | `VALIDATION_ERROR` |
| `BadRequest` | 400 | `VALIDATION_ERROR` |
| `NetworkChain` | 502 | `SERVICE_ERROR` |
| `Unauthorized` | 403 | `ACCESS_DENIED` |
| `NotActive` | 409 | `CONFLICT` |
| `InvalidPublicKey` | 400 | `VALIDATION_ERROR` |

The pinned host map (#140) serves the typed `REVIEW_ACTIONS` carrier as
`HTTPWalletJSON.ts` serializes it: HTTP 400 with `isError: true`, numeric
`code: 5`, `message`, `reviewActionResults` and `sendWithResults`. The
optional `txid` (hex), `tx` (byte array) and `noSendChange` (outpoint strings)
are omitted when absent. Each review result retains its competing txids
and BEEF; competing BEEF uses the reference's numeric-key byte object.
The code census reads numeric as well as string codes. The served-body
round trip through bsv-rs beta.3's `HttpWalletJson` checks all five members.
The ordinary classifier is deleted (#330). The table maps the published
reference classes directly. Generic `WalletError` faults retain their Display
and serve 400 ERROR; typed `TransportFault` serves 502 SERVICE_ERROR, with no
busy promise. Since the beta.4 pin (#342, bsv-rs #439) the toolbox's faults
are typed, `WalletFault` by its family: `Transport` and `Service` serve 502
SERVICE_ERROR (fixture 085), `Storage` and `Internal` 500 INTERNAL_ERROR
(fixture 089), a family a later package adds 500; the Display is the same
as the untyped fault's. An abort refused on stored acceptance is the typed
`AbortAccepted` record: its status is its class's, as the table maps it
(the status guard's `InvalidParameter` 400, the stored-acceptance guard's
`InvalidOperation` 409), and the record selects the recovery hint served
after the message, `(hint: the wallet holds an accepting word for this
transaction (<its known transaction, status and accepting word>); it stays
until a proof, node verdict or competitor's checked proof decides it)`
(fixture 124), never read from the words. An input the stack could not
judge (`TransactionNotJudged`, bsv-rs #452) is answered by its cause: a coin
or spend before Genesis 409 CONFLICT, the tip, activation heights, source
height or work budget 500 (no world reaches it; the toolbox's not-judged
refusals at the doors are `SpendRefused`, `InvalidParameter`). Invalid BEEF
folds into VALIDATION_ERROR, and incomplete ancestry into CONFLICT without
the former coin member. Prove and abort use InvalidParameter; missing/expired
sign sessions use NotImplemented, which the signAction door serves as the
caller's 400 VALIDATION_ERROR, as abort's (#122 item 5, fixture 040). Relinquishing an unknown certificate is the
toolbox's `BadRequest` refusal since beta.4 (#344, toolbox #430; fixture 045),
`Result must exist and be unique.`, served 400 VALIDATION_ERROR.

Two `AppError` codes no in-process world produces, so they are not judged (`UNPRODUCED` in `tests/wire_harness/judge.rs`; a fixture that answers one moves it to `APP_CODES`):

| code | why no world produces it |
|---|---|
| 501 `NOT_IMPLEMENTED` | the reference's `NotImplemented` and `UnsupportedAction` classes. The toolbox at the pin raises one of them at one site, `signAction`'s reference with no pending action (absent or expired), which the door serves as the caller's 400 `VALIDATION_ERROR` since #122 item 5 (fixture 040); the class stays mapped for a package that adds another |
| 502 `BROADCAST_REJECTED` | the host's verdict after the full post-broadcast probe window: no broadcaster took the transaction and an independent chain index says it is absent. The probe asks explorers; the run's proxy refuses every request, which the ladder reads as "could not look", never as absent, so the verdict is not reached in-process |

The framework's own rejections are served as every other refusal since #122 item 3: a body that does not parse (032) and a field missing (031, naming the wire field `protocolID`) or of the wrong type (141, 142) are `{code, message}` at 400 `VALIDATION_ERROR`, axum's words as the message (`src/server/handlers/json_body.rs`). The body is read as the MetaNet Client reads it: JSON under any content type (177, a valid body as `text/plain`, is the call's 200), and a request with no content type is the call with `{}` (178: 400 `VALIDATION_ERROR`, missing field `protocolID`, as `{}` answers). No wallet call's fixture records a text body.

Fixtures 129 to 140 record the five paging calls at `limit: 10000` (200)
and above it at `10001` (400 `VALIDATION_ERROR`), plus `listOutputs` with
`limit: 0` (400) and `offset: -1` (200). The limit messages preserve the
reference's `at least 1 length.` and `no more than 10000 length.` words,
followed by the bound and supplied value. Absent limits and offsets still
pass untouched; fixtures 035, 036, 048, 053 and 054 retain their bytes.
Fixture 134 records BRC-100's newest-first paging (`wallet/0100.md:911`):
the negative offset passes unchanged, returning an empty page with its true
total in the fresh world. A served test with two stored outputs checks that
`offset: -1` returns the newest one and `offset: 0` returns the oldest.
Fixture 141, `signAction.sequenceNumberAboveType`, records the type's refusal
of `4294967296` before the handler, 400 `VALIDATION_ERROR` since #122 item 3
(a 422 in text before): the `u32` type already enforces
the BRC bound. Fixture 142, `listActions.offsetAboveType`, records the same
type boundary for its unsigned offset. Their transport shape belongs to
#117 and #149. All 128 earlier fixtures remain byte-identical.

`119-arc-callback.storeBusy.json` records the callback's typed store busy
(#226), through the same `AppError::store_busy` arm as the host reads and
abort pre-check. Only this busy answer moves from beta's 400 `BAD_PAYLOAD`
to 503 `STORE_BUSY` with `Retry-After: 1`; other callback failures keep their
codes and messages. Fixtures 064 to 069 and 080, and all 118 older fixtures,
remain byte-identical.

`120-createAction.namedChange.json` records the toolbox's refusal after the
gate offers a named coin of our own change (#234): `400 VALIDATION_ERROR`,
with `Validation error: The inputs[0] parameter must be an unmanaged input. Change outputs are managed by your wallet.`
(toolbox #241), naming the input index. The request omits `allowPending`.
This is the served decision, explained in [A word after the door](GATE-VECTORS.md#a-word-after-the-door).
The dedicated world supplies made change because the existing seeded coins
are incoming and the funded spends leave none. It is recorded last;
all 119 earlier fixtures remain byte-identical.

`082-options.auth.createAction.json` records the preflight under the
bearer passing (#138, PR 307): the CORS layer now wraps the bearer, so a
browser's `OPTIONS`, which carries no credentials, is answered 200 with the
CORS headers and an empty body, as the `fresh` world's 076 is; it was
401 `UNAUTHORIZED` with no CORS headers, the defect this page named. The
bearer's refusals 070, 071 and 073 keep their status and body and gain the
CORS headers (`access-control-allow-credentials`, `-expose-headers`,
`vary`), so a browser's script reads the 401 instead of a network error.
All 138 other fixtures remain byte-identical.

## Closed server defect, retained in the corpus

A fixture records what the server answers, defect included; the fix changes the fixture in its own commit, and the reviewer sees the answer move.

| fixture | what is served | closed defect | issue |
|---|---|---|---|
| `103-internalizeAction.badUnlock.json` | `400 VALIDATION_ERROR`, the verifier's offset and OP_EQUALVERIFY reason, nothing stored | beta.0 accepted the invalid spend. Toolbox #103 closed it at beta.1; #330 maps its typed InvalidParameter at beta.3 | #140, #277, #330 |

## Changing a fixture on purpose

A PR that changes what the server answers changes its fixture in the same PR, in a commit of its own:

1. Make the change in `src/` (one commit).
2. `WIRE_RECORD=1 cargo test --locked --test wire` re-records every fixture and the manifest from the cases in `tests/wire_harness/` (`record_all` in `cases.rs` and the `record_*` functions it calls). A recording never passes: the run ends with the word that a recording is not a replay and must be replayed by a second run. Nothing else re-records: unset, the run only replays, and any other value (`0`, `true`, an empty string) fails the run naming it.
3. `cargo test --locked --test wire` replays what was recorded.
4. `git diff tests/wire` shows exactly the bytes the change moved. Commit them alone, with a subject that says which answer changed and why (`wire: /listActions carries underRule`).

CI's `wire` job replays only: it fails before cargo runs if `WIRE_RECORD` is set to anything, the empty string included, and it refuses a run that left a fixture changed. CI runs the binary under cargo-nextest, one process per test (#472): the harness's lock serializes the worlds within a process, whose permission audit they share, and each process has its own, so the tests replay at once.

A new served call or refusal gets a case in one of the `record_*` functions `record_all` calls in order (`tests/wire_harness/cases.rs`, `cases_routes.rs`, `cases_spending.rs`) and, if it is a route or a code the judge must keep, an entry in `ENDPOINTS`, `HOST_ROUTES`, `APP_CODES` or `ROUTER_CODES` (a new `AppError` code with no entry fails `every_app_error_code_is_judged`). Re-recording writes the files with fresh sequence numbers, so a case added in the middle renames the files after it; the diff shows them as moved, their bodies unchanged.

Two recordings in a row are byte-identical: a re-record with no change in `src/` leaves `git status` clean. A bound request value is written as `<bound>`, never as the value sent, for that reason.

## What the normalizer hides, and why

One normalizer, `NORMALIZER` in `tests/wire_harness/fixture.rs` (its rules; the walk and the capture it drives are `Normalizer` in `tests/wire_support/mod.rs`, the replay machinery `tests/wire_consumers.rs` shares), and only values that differ from run to run with no change to the code. A placeholder replaces a value, never a key, and it keeps the value's shape: its JSON type, a byte array's length, a string's kind and length, a number's sign and digit count. A field that changes from a byte array to a hex string, or grows by a byte, fails the replay.

| placeholder | for |
|---|---|
| `<bytes:N>` | an array of N integers 0 to 255 |
| `<hex:N>` | a hex string of N characters |
| `<rfc3339>` | a string that parses as an RFC 3339 time |
| `<string:N>` | any other string, N characters |
| `<uint:D>`, `<int:D>` | an integer of D decimal digits, unsigned or negative |
| `<float>`, `<bool>`, `<array:N>`, `<object:N>` | the other types, should a volatile key ever carry one |

The keys replaced, anywhere in a body (`VOLATILE_KEYS`); a `null` is kept as `null`:

| key | where | why it differs |
|---|---|---|
| `ciphertext` | `/encrypt` | AES-GCM under a fresh random IV each call |
| `encryptedLinkage`, `encryptedLinkageProof` | the two key-linkage calls | encrypted for the verifier the same way |
| `revelationTime` | `/revealCounterpartyKeyLinkage` | the clock |
| `ageMinutes` | a refusal's `held` coins | the clock, from the row's time; every seeded row is dated half an hour before its world is built, so the age has two digits on any day and `<uint:2>` is stable |
| `reference` | `signableTransaction` of `/createAction` | the toolbox draws it at random |
| `grantedAt`, `revokedAt` | a grant row of the owner's routes (`/origins`, #414) | the clock's now when the route wrote the row |
| `answeredAt` | a pending row the owner answered, in `/origins` (#414 L6, fixture 220) | the clock's now when the owner answered it |
| each `keyringForVerifier` value | `/proveCertificate` | freshly encrypted for the verifier; every key, its order and each value's shape/length remain strict |

Inside any string: `127.0.0.1:<n>` becomes `127.0.0.1:<port>` (the local stub's port), `<n> min old` becomes `<minutes> min old` (the same age, in words), `os error <n>` becomes `os error <errno>` (the platform's number for a refused connection; macOS and Linux differ), and `withdrawn at <an RFC 3339 time to the second>` becomes `withdrawn at <time>` (an abandoned row's window's end, the clock's now plus `ABANDON_WINDOW`, #534, fixture 221). One rule is not a run's: this wallet's served version string (`getVersion`, `handlers::WALLET_VERSION`) becomes `bsv-wallet-cli-<major>.<minor>.<patch>` (`package_version` in `tests/wire_support/mod.rs`, both corpora), because it moves with the package's version and never with a behaviour, so a release's version commit moves no fixture; `tests/integration/status.rs` pins the exact string and its shape (#122 item 6).

What is not hidden, because it does not differ: the identity key, every derived key, every signature, every txid, every built transaction. The worlds use one fixed root key, so the identity key, the BRC-42 derivations and the RFC 6979 signatures are the same on every run, and a change to derivation or signing fails the replay. The seeded txids are fixed values or the ids of fixed transactions. The `funded` world's actions each ask for exactly their coin less the toolbox's fee for one input and one output (20 sats at its rate for a P2PKH input, 9 for the puzzle's one-byte unlock), and the basket wants no more coins than it has, so the toolbox builds no change output; the template, the signed transaction, their txids and the AtomicBEEFs are the same bytes on every run. The stranger's payments are signed by a fixed key (RFC 6979), and the recorder checks each before it is offered: the two valid spends verify (their scripts run true, their parents' proofs at their heights), the bad one does not.

Not recorded, for that reason: an action with a change output, and so a non-empty `noSendChange`. The toolbox draws a change output's derivation suffix from `rand::thread_rng` (`random_derivation` in its `create_action.rs`), so the change's locking script, the transaction's bytes and its txid move on every run, and hiding them would hide the shape the fixture is for. It needs a seeded derivation in the toolbox, or a normalizer rule scoped to one fixture's txid and bytes; neither is this corpus's to add. The `/decrypt` request carries a fixed ciphertext of `wire` under that key (`DECRYPT_CIPHERTEXT`), so a recording does not move it.

## Nothing leaves the machine

The router is driven in-process (`tower::ServiceExt::oneshot`). The header service is a local stub on `127.0.0.1`; the broadcaster is a closed local port, except in `funded`, where it is a local ARC (`arc_stub`) answering `SEEN_ON_NETWORK` for every posted transaction; `HTTPS_PROXY` and `HTTP_PROXY` point at a closed local port with `NO_PROXY=127.0.0.1`, so an explorer request fails at once instead of reaching the network. The operator's settings (`SPEND_DEPTH`, `ARC_*`, `CHAINTRACKS_URL`, the explorer keys and the rest) are removed from the run's environment before any world is built. In `funded`, `createAction` in its default mode (`acceptDelayedBroadcast: false`, as x402 sends it) and each `internalizeAction` post to that stub, which takes them: the action answers `sendWithResults` `unproven`, and the toolbox posts to no other broadcaster after an acceptance.

The worlds use the toolbox's real `Services`, pointed at the stubs, not its `MockWalletServices` (whose `post_beef_success` the ARC stub stands in for): the router's state is `Arc<Wallet<StorageSqlx, Services>>`, a concrete type, and the brief allows no change under `src/` for it.

The remaining #277 intake was re-read in the fix round (finding 4):
bsv-rs #262 does not re-sign direct acquisitions; #280's retained signed
Peer text is not reconstructed by the host; #285's correctly rounded
number reader is feature-unified, with the host's integer field types and
bounds unchanged. Toolbox #202's remote storage wire has no direct CLI
caller. Bsv-rs #240's non-exhaustive stream enums have no CLI matches;
the toolbox's fallback refuses an unknown verdict. Bsv-rs #246 concerns
the binary wallet certificate writer; direct JSON acquisition's separate
field-size gap stays toolbox #374. The #105 shape moves were checked
toward HTTPWalletJSON, with no away-moving served byte found. The CLI
calls no #310/#315 IdentityClient publication helper; its explicit
proveCertificate path is covered above. Toolbox #395's internalize
verifier is uncapped at this published pin, without a host memory-limit
parameter. PR 332 has one checked line per item with these findings.

## The judge, judged

`tests/wire.rs` carries planted cases beside the replay: `AppError`'s codes in the source and the judged and unproduced lists agree in both directions; a recording run into a temporary directory ends with the word to replay and never passes, a second run replays it clean, and every file it wrote is the committed one byte for byte by default; a `WIRE_RECORD` other than `1` is refused by name; a message-only change in a copy of the corpus fails in strict mode and reports the exact JSON-quoted marker under `WIRE_TEXT_DIFF=report`; a changed `code`, or a message changed with another field, still fails with a line diff; changed status, headers, key order, nested reasons, the noted offset and script reason, and non-JSON or successful bodies remain strict; `WIRE_TEXT_DIFF=1`, an empty value, and the report word alongside `WIRE_RECORD=1` are refused by name; the fresh-recording comparison reports only changed message bytes, never a changed request, envelope, whitespace or manifest; a manifest with a listed file missing, a file on disk unlisted, and a stray non-JSON file names all three, and the replay refuses that set naming them; a fixture of an unknown world fails the replay naming the file and the world; the normalizer keeps each placeholder's shape and leaves a non-volatile key whole. The runs share one lock, since the permission audit is the process's.

## The beta.2 pin (#277)

The corpus now has 160 rows. Bsv-rs #105 flattens key-linkage results and
certificate list entries, spells the certificate type `type`, and carries
key-linkage bytes as arrays. The positive direct-acquisition row uses a
32-byte type and serial number with a canonical DER signature; the earlier
spelling probes still show the class served when their arguments refuse.
At that historical pin ordinary refusals were erased; #330 below replaces
that classifier at beta.3.

Rows 143 to 146 declare ten unlocking bytes, refuse eleven without consuming
the unsigned action, list that state and complete it with a shorter script
(toolbox #216). Rows 147 and 148 take a coin while the gate reads its headers,
then record the toolbox's typed review refusal as HTTP 400, numeric code `5`,
and the
remaining state (#245/#264, bsv-rs #339). Rows 149 to 154 cover BRC-177 grammar
and capability refusals, and remittance exclusivity before BEEF work.

At beta.2 rows 155 and 156 recorded 200 at and above the 1 MiB field
bound; beta.3 now verifies and decrypts fields before storage (toolbox #374). Row 157 refuses a 73-byte non-DER signature at its argument
gate. `tests/certificate_pin.rs` separately checks the certificate type's
1 MiB field, 16 MiB frame and 72-byte signature bounds and verify-false.

Rows 158 to 160 close finding 3: a stored encrypted certificate field and
master key, `proveCertificate` with `fieldsToReveal: []` returning an empty
keyring, and a 31-byte serial refused before recovery (toolbox #240).
The serial fixture reaches the public argument validator (#311), which
already enforces exactly 32 bytes before the helper. The empty disclosure
previously released all stored fields. The new rows preserve every older
fixture's number and are recorded with their reasons in the manifest.

Discovery rows 053, 054 and 137 to 140 retain their bytes: the fresh world
holds no certificate whose subject they ask and reads no trust settings, and
attribute discovery has no local leg; above-bound requests stop at the host's
page guard. The trusted answer is the beta.4 pin's (below).

## The beta.3 pin (#338)

The corpus has 174 rows. All moved and new files are named with their cause
in the single recording commit. Toolbox #404 removes the SDK Display's generic
wallet-error prefix from ordinary refusals, with statuses and codes from the
class table above. Toolbox #374 requires valid signed, encrypted certificate
material, including the disclosure fixtures; #343 sources the 64 KiB
decrypted-field bound in bsv-rs 0.9.0-beta.4
`src/auth/certificates/bounds.rs:26` (`MAX_DECRYPTED_FIELD_BYTES = 64 * 1024`),
applied at `master.rs:325-329` by toolbox 0.14.0-beta.4
`src/wallet/direct_acquire.rs:188-197` (the lines re-read at the beta.4 pin).
The reference applies the bound at issuance and at decryption only
(`MasterCertificate.ts` `createCertificateFields` and `decryptField`), and
this stack also at direct acquisition through toolbox #374's keyring proof,
a divergence from the reference's store-as-given (toolbox
`docs/ARCHITECTURE.md` section 7, row D26 at a8107be2, toolbox PR #476). Row 155 passes construction and
verification at the 1 MiB encoded-field bound, then fails that plaintext
check. Row 156 is non-canonical base64 and fails construction before the
separate verify bound; row 157 fails canonical-DER argument validation.
Rows 173 and 174 prove storage at the SDK's decrypted bound. Rows 161 to 165 pin SDK #415/#420 validators;
166 and 167 pin toolbox #383 descriptions; 168 pins #376's label before a bad
derivation; 169 pins the tracker's typed transport fault; 170 and 171 pin SDK
#427's compressed linkage aliases; 172 pins SDK #384's protocol before a bad
description. The existing certificate key order in 042 to 054 is inspected
alongside the recording. SDK #379 is absent, so no BRC-56 HMAC tag is changed.
Toolbox #388 is absent, and discovery remains #331's separate change.

## The beta.4 pin (#347)

The corpus has 176 rows, recorded once after the code (the moved and new
files named with their cause in the recording commit). Rows 045 (#344, the
relinquish refusal), 085 and 089 (#342, the typed fault's status), 124 (#342,
the abort record's hint) and 147 (toolbox #389, `competingBeef`) move; rows
175 and 176 are new and come last, so no earlier number moves. Two worlds
join the list: `trusted`, whose wallet reads `TRUST_SETTINGS` through the
CLI's typed config naming the harness certificate's certifier (#331, toolbox
#388), and `stranger`, whose settings name another key; each holds the
harness's certificate, acquired through the served door when the world is
built. Row 175 answers that certificate with `certifierInfo`, the toolbox's
transform of the setting (a description within 5 to 50 bytes and an icon
named, so it is the configured entry), read against bsv-rs's result rows
R74, R76, R79, R80, R47 and R49; row 176 drops it. Row 051 does not move:
its world still holds the row 050 relinquished, and the toolbox's lookup
includes soft-deleted rows, as the reference's `verifyOne` does. No
TEXT-DIFF row of toolbox #392, #416, #422, #426, #450, #453 or #464 is
reached by a fixture; no recorded `createHmac` key has a leading-zero X
coordinate (bsv-rs #379); the certificate key order in 042 to 054 is
unchanged (serde_json without `preserve_order`). Row 145 stays the one known
refusal of bsv-rs's result rows (R56, #371, ts-stack #1451).

## The beta.5 pin (#376)

The packages alone move no row: before the seed change below, the wire,
consumers and BRC replays under bsv-rs 0.9.0-beta.5, toolbox 0.14.0-beta.5
and bsv-tracker 0.9.0-beta.5 were clean. One row moves, for #371: the
harness's seed wrote the literal `c` as every seeded coin's
`output_description`, one byte under BRC-100's 5 to 2000 (R51), and now
writes the coin's own reason (`a funded coin`, `a caller puzzle`, `a received
coin`, `our change`). Row 145 (`listActions.afterOverlongSpend`) is the one
fixture that shows a seeded coin's description: its stored unsigned
template's input reads `a caller puzzle`. bsv-rs's R56 accepts an `unsigned`
action's input with no `unlockingScript` from this pin (bsv-rs #491, PR #539),
so row 145 now passes R51 and R56. Every recorded 200 answer of the wire and
consumers corpora (97 wallet calls, their volatile values denormalized and
fixture 159's bound certificate resolved) passes bsv-rs 0.9.0-beta.5's
result rows; nine 200 rows are host routes no wallet client reads
(`/permission-audit`, `/chain-feed`, `/tx/<txid>`, `/arc-callback`). No row
reaches toolbox #509's hold text (every wire world carries a tracker), the
discovery doors' chain evidence (toolbox #502: no world has an overlay, so
053, 054, 137 to 140, 175 and 176 answer from the local and configured
records), a work-budget refusal (bsv-rs #519), or a lenient or `05`-tagged
key (bsv-rs #498, #550); row 026's non-hex key keeps its words, and the
doors read a key argument as at beta.4 (#377).

## The divergence rows (#122, items 3 to 7)

The owner's word of 2026-11-01 (23.8, through the keeper), as given:
"one rule applied per item without a word per item: where the BRC and the
MetaNet Client agree, the server follows them; where they disagree, the
client's published behaviour is the port's and the BRC amendment is filed
in the stack's cadence; docs/BRC-100-EXTENSIONS.md only where the client
itself extends". BRC-100 is read at `5119a9b`; the amendment drafts are in
`docs/brc100/`. Item 7 is outside the rule: neither the BRC nor the client
names `allowPending`, and the client does not extend there, so no clause of
the rule decides it; it is decided by the extension's own consistency.

| item | call | served | fixtures | clause |
|---|---|---|---|---|
| 3 | every call's body | the framework's rejections `{code, message}` at 400 `VALIDATION_ERROR`, the wire field's name (`protocolID`); a body is JSON under any content type, and a request with none is the call with `{}`, as the client reads them | 031, 032, 141, 142 moved; 177 (`text/plain`, the call's 200) and 178 (no content type, `{}`'s 400) new | they disagree on the refusal: the client's 400, the draft `docs/brc100/http-refusal-body.md`; the content type, which the BRC never names, read as the client reads it by the captain's decision (the second D ensemble on #122, 3/3 follow) |
| 4 | a store fault | 500 `INTERNAL_ERROR` at the spend guard and at every wallet call | 088, 089 unchanged, their notes added | they agree on one answer; already one class since #342 |
| 5 | `signAction` | an unknown reference 400 `VALIDATION_ERROR`, as `abortAction`'s | 040 moved; 041 unchanged | they agree: the caller's 400 at both doors |
| 6 | `getVersion` | `bsv-wallet-cli-<major>.<minor>.<patch>` (`bsv-wallet-cli-0.19.0` at 0.19.0-beta.4) | 007, 008 and the consumers' `dolphinmilk/01`, `low-fleet/02` moved | they agree on `[vendor]-[major].[minor].[patch]`, 7 to 30 bytes |
| 7 | `createAction`'s `allowPending` | a value that is not a word 400 `VALIDATION_ERROR` naming the words | 038 moved; 055's audit loses 038's entry | no clause of the rule: neither names `allowPending`; this wallet's extension, decided by its own consistency |

## The beta.6 pin (#386)

The corpus has 179 rows. One row moves and one is new, both named with their
cause in the recording commit; the packages move no other byte. Row 026
(`revealCounterpartyKeyLinkage.badKey`) moves in class and text: the four key
doors (`parse_counterparty`, and the two linkage doors' `counterparty` and
`verifier`) read a caller's key through bsv-rs 0.9.0-beta.6's
`wallet::validate_public_key` (#579), the reference wallet's
`validatePublicKey` (`validationHelpers.ts:185-191` at edf6e03), in place of
#377's host reader, so `02zz` is a `WalletRefusal` of class InvalidParameter,
400 `VALIDATION_ERROR` `The counterparty parameter must be hexadecimal
string.`, where beta.5 answered 400 `ERROR` `invalid hex string: Invalid
character 'z' at position 2`. The rule is stricter than #377's copy: an
uncompressed `04` key is refused and surrounding whitespace is trimmed; no
other fixture names such a key. Row 179 (`createAction.refusedGraph`, new,
last, after #122's 177 and 178) records a refused BEEF graph at the create door: an `inputBEEF` whose
unproven transaction's parent is neither carried nor stored, the toolbox's
`ValidationError` (InvalidParameter through the SDK boundary) served 400
`VALIDATION_ERROR` by its class; since toolbox #546 the text ends at the byte
count, with no dump path and no file written. Every recorded 200 answer of the
wire and consumers corpora (98 wallet calls: 70 wire, among them #122's row
177, and 28 consumers; the getVersion rows read with the package's version
core) passes bsv-rs 0.9.0-beta.6's result rows, none refused; the nine host-route 200 rows
and the eight bodiless preflight answers are not wallet calls. No world
faults a header service at a discover call (toolbox #521) or sends a 429
from one (toolbox #148), and no fixture reaches a script near the door's
100,000,000-byte stack memory figure (toolbox #395).

## The beta.8 pin (#565)

bsv-rs 0.9.0-beta.8, bsv-wallet-toolbox-rs 0.14.0-beta.8 and bsv-tracker
0.9.0-beta.8 move together. The corpus keeps its 221 rows; four rows move,
each in its own recording commit with its cause, and the packages move no
other byte. Row 043 (`internalizeAction.invalidBeef`) keeps 400
`VALIDATION_ERROR` in the toolbox's beta.8 words: a wallet payment's subject
is read before storage reads the BEEF, `Cannot read the BEEF's subject
transaction to check payment outputs: BEEF error: invalid BEEF at byte 0:
BadVersion { word: 4022250974 }` (the toolbox's Hosts section). Row 086
(`createAction.unspendableCoin`) moves from 409 `CONFLICT` to 400 `ERROR`,
`BEEF error: ReaderUint8Array read exceeds available data`: the world's coin
stores four bytes as its creator's transaction, bsv-rs's `merge_raw_tx` now
refuses a raw entry that is not exactly one transaction (bsv-rs #798, #817),
and the toolbox's coin-gap reader classes only a missing transaction as a
gap (`create_action/beef.rs:610`), so the BEEF error reaches the door and is
served by type, as every `BeefError` is (#611). Rows 127 and 128 (the insertion at
the custom-instructions bound) take a payment of their own: the shared
`funded` world had already put the stranger's insertion into basket `wire`,
and the toolbox now refuses a basket insertion that moves an output between
application baskets, as the reference does; 127 answers `{accepted: true}`
and 128 lists the 1,000 bytes as before, under the new outpoint. In the
consumers' corpus, low-fleet 11 and x402-skill 07 take the same internalize
text as row 043, and low-fleet 09 and 10 (`allowPending` passes the gate)
stop at the same BEEF error as row 086 instead of signing (500), their world's
coins storing the same four bytes. Every recorded 200 answer of the wire and
consumers corpora (110 wallet calls: 82 wire and 28 consumers, the
normalized values read as values of their recorded shape) passes bsv-rs
0.9.0-beta.8's result rows, none refused; the fifteen host-route 200 rows are
not wallet calls.

## The stable line's 0.18.4 and 0.18.5, served on beta

The stable line's 0.18.4 and 0.18.5 pin the toolbox's two `internalizeAction` rules (0.13.2: the wallet payment rule; 0.13.3: the basket insertion rules). This line carries both through its own pin, the toolbox 0.14.0-beta.8 (#565, its #819 and #823), whose `### Hosts` lines state each answer as a `WalletRefusal` of class `InvalidParameter`. Each answer the stable line's served tests read (`tests/served_internalize_payment.rs` and `tests/served_internalize_insertion.rs` on `main`) is pinned here by a row of this corpus, rows 222 to 250 recorded from the `internalize` world and 251 to 254 from the `internalize-held` world, all recorded last, or by two tests in `tests/served_beef_doors.rs` where a fixture would not fit (a sweep of 800 inputs, about 120 KB of transaction, is past 120,000 lines in a fixture, which writes `tx` one byte per line). Each refusal row is followed by a listing that shows nothing was stored: 232 (no transaction written after 223 to 231), 236 and 237 (after 235), 246 (after 239 to 241), 249 and 250 (after 248), 254 (after 252). Rows 248 to 254 pin three answers of the toolbox's `### Hosts` that the stable line's tests do not read. Rows 001 to 221 replay byte-identical, and the consumers' corpus is unchanged.

The two lines side by side: the stable line's status and code from its tests, this line's from its rows. Where both lines refuse by the same check, the messages differ by the stable line's `wallet error: ` prefix, except in two rows, each named in its row below: bytes that are not a BEEF, where the reader's text differs too, and the default basket, where the stable line's text opens `wallet error: ` and beta's opens `Validation error: `. Beta's refusals carry the toolbox's words alone.

| answer | stable (0.18.5) | beta (this corpus) | difference and its reason |
|---|---|---|---|
| a wallet payment in bytes that are not a BEEF (#819) | 400 `INVALID_BEEF` | 400 `VALIDATION_ERROR` (043) | beta serves a refusal by its class (#330): invalid BEEF is `InvalidParameter`, and this line has no `INVALID_BEEF` code; the stable line's text arm kept the code 0.18.3 served. The message differs past the prefix: the stable line's ends with its reader's version text (`BEEF error: Invalid BEEF version: 0xEFBEADDE`), beta's with its reader's own text of its type (`BEEF error: invalid BEEF at byte 0: BadVersion { word: 4022250974 }`) |
| an AtomicBEEF naming a subject it does not carry (#819) | 400 `VALIDATION_ERROR` | 400 `VALIDATION_ERROR` (223) | none |
| a wallet payment locked to the P2PKH of another key (#819) | 400 `ERROR` | 400 `VALIDATION_ERROR` (224) | beta serves the toolbox's `InvalidParameter` by class; the stable line's `Derivation mismatch` text has no arm and keeps `ERROR` |
| the same, in a sweep past bsv-rs's link budget (#819) | 400 `ERROR` | 400 `VALIDATION_ERROR` (`served_beef_doors`) | as the row above |
| control: the sweep paying the derived key (#819) | 200, stored | 200, stored (`served_beef_doors`) | none |
| a wallet payment not locked by the derived key's P2PKH, the toolbox's four shapes (#819) | 400 `VALIDATION_ERROR` | 400 `VALIDATION_ERROR` (225 to 228) | none |
| a basket insertion that carries a payment remittance (#819) | 400 `VALIDATION_ERROR` (BRC-29 sentence) or `ERROR` (derivation mismatch) | 400 `VALIDATION_ERROR` (154), `The output remittance parameter must be only insertionRemittance for basket insertion` | the toolbox on this line refuses it at the argument validator, before the rule reads the output (its 0.14.0-beta.8 Fixed: "refused by the argument validator, as before") |
| control: a wallet payment locked by the derived key's P2PKH (#819) | 200, stored | 200 (104, 238), stored as change in `default` (246) | none |
| control: a basket insertion with no payment remittance (#819) | 200, stored | 200 (102, 233), stored in its basket (236) | none in the answer; the stable test's script is a P2PK, these rows' a P2PKH of another key, neither held to the rule |
| a basket insertion naming the default basket (#823) | 400 `VALIDATION_ERROR`, six spellings | 400 `VALIDATION_ERROR` (229 to 231: `default`, `DEFAULT`, ` Default `) | the rows take the spellings the toolbox's 0.14.0-beta.8 names; the stable line needed a text arm for the 0.13.3 text, beta serves the class. The message differs past the prefix: the stable line's is `wallet error: The insertionRemittance.basket parameter must be a non-default basket`, beta's `Validation error: The insertionRemittance.basket parameter must be a non-default basket` |
| a basket insertion over a received payment (#823) | 400 `VALIDATION_ERROR` | 400 `VALIDATION_ERROR` (239) | none |
| a basket insertion over the wallet's own change (#823) | 400 `VALIDATION_ERROR` | 400 `VALIDATION_ERROR` (240) | none in the answer; the stable test makes the change with `Wallet::create_action`, this row names a seeded change coin (`P2PKH`, change, `storage`, `change`, its derivation), since this corpus records no action with a change output (its derivation is drawn at random, above) |
| a basket insertion over a change row with its derivation whose `provided_by` and `purpose` are not `storage` and `change` (#823) | not pinned by the stable line's tests | 400 `VALIDATION_ERROR` (252), `Validation error: The outputs parameter must be output 0 is wallet-managed change and cannot be reclassified as a basket insertion`; the coin kept in `default` (254) | the toolbox's 0.14.0-beta.8 `### Hosts`: wider than the reference's `isManagedChangeOutput`, which also requires `providedBy` `storage` and `purpose` `change`; such a row is refused here where the reference admits it |
| a basket insertion over an output in another application basket (#823) | 400 `VALIDATION_ERROR` | 400 `VALIDATION_ERROR` (235; 236 the output kept, 237 the basket not created) | none |
| a basket insertion naming the output's own basket in another spelling (`Tokens` for `tokens`) (#823) | not pinned by the stable line's tests | 400 `VALIDATION_ERROR` (248), `Validation error: The outputs parameter must be output 0 is already assigned to a different basket and cannot be reclassified as a basket insertion into Tokens`; the output kept in `tokens` (249), `Tokens` not created (250) | the toolbox's 0.14.0-beta.8 `### Hosts`: the basket is matched by its name as given; the reference matches the trimmed, lowercased name, so the request is refused here and admitted there |
| control: a wallet payment carrying a stray insertion remittance naming `default` (#823) | 200, stored as change | 400 `VALIDATION_ERROR` (241), `The output remittance parameter must be only paymentRemittance for wallet payment`, nothing stored (246) | the toolbox on this line refuses an output carrying the other protocol's remittance at its argument validator, as 154 refuses the converse; the stable line's toolbox reads only the protocol's own remittance and stores it |
| control: a basket insertion into a named basket (#823) | 200 | 200 (102, 233) | none |
| control: the same basket named again (#823) | 200 | 200 (234) | none |
| control: an output in no basket, or in `default` and not change, swept into a named basket (#823) | 200 each | 200 (243: an output of the wallet's own action; 244: an earlier insertion into `default`), listed there (247) | none |
| control: a row an earlier release left in `default`, listed and relinquished | 200 | 200 (222 lists it, 245 relinquishes it, 246 no longer lists it) | none |
| what a host holds from before: a payment admitted before the wallet payment rule, change of `default` whose script is not the P2PKH of its derived key, listed and relinquished (#819) | not pinned by the stable line's tests | 200 (251 lists it with its locking script, 253 relinquishes it, 254 no longer lists it) | the toolbox's 0.14.0-beta.8 `### Hosts`: `listOutputs` with `basket: "default"` and the locking scripts lists it, and `relinquishOutput` with `basket: "default"` and its outpoint takes it out of the basket |

No row of this line answers a 500, an `ERROR` where the stable line serves a named class, or a refusal after anything was stored.

## The beta.8.1 pin

bsv-wallet-toolbox-rs moves from 0.14.0-beta.8 to 0.14.0-beta.8.1, the `internalizeAction` door read whole against the reference (the rows its stable line shipped as 0.13.4); bsv-rs 0.9.0-beta.8 and bsv-tracker 0.9.0-beta.8 are unchanged. At the pin commit, before any other change, rows 001 to 254 replay byte-identical (254 replayed, 0 differ), the consumers' 48 replay with no difference and the gate vectors show no divergence: no fixture moved, so no row of this corpus reached a request the release now answers differently. Rows 255 to 266 are new, recorded last from the `internalize` world (`tests/wire_harness/cases_internalize_door.rs`): the door's refusals a caller of this host can reach (257 to 262), the listings before (255, 256) and after (263, 264) that the recorder asserts equal, and the held transaction's accepting side (265, 266). Rows 267 and 268 (toolbox #831) name one `outputIndex` by a payment and an insertion, and by two insertions, on a new transaction, each followed by listings that show nothing stored (269 to 271). Rows 001 to 254 replay byte-identical, and the consumers' corpus is unchanged.

## The stable line's 0.18.6, served on beta

The stable line's 0.18.6 pins the toolbox's 0.13.4, the same door. Each answer the stable line's served test reads (`tests/served_internalize_door.rs` on `main`) is pinned here by a row of this corpus or, where no world of the corpus holds it, by a test of this line's `tests/served_internalize_door.rs` (`served`), through the served router. Each refusal row is followed by a listing that shows nothing was stored (263 and 264 after 257 to 262; 266 after 265; 269 to 271 after 267 and 268). This host configures no permissions manager (`git grep -n -i permissions_manager -- src mcp/src` finds none), so the Hosts rows of a manager (an insertion basket or a label asked, `admin` names refused) are not reachable here and have no row on either line.

The two lines side by side: the stable line's status and code from its tests, this line's from its rows and tests.

| answer | stable (0.18.6) | beta (this corpus) | difference and its reason |
|---|---|---|---|
| no chain tracker (`CHAINTRACKS_URL=off`): every `internalizeAction` | 400 `VALIDATION_ERROR`, `No chain tracker is available to verify the BEEF: ...` | 502 `SERVICE_ERROR`, `wallet error: Service error: ChainTracker verification failed: the merkle proofs of a BEEF the wallet did not sign need headers, and no header service is configured (no chain tracker, no chaintracks_url)`, nothing written or posted (`served`) | this line's toolbox holds with no header source since 0.14.0-beta.6 (#509): a header fault, never a verdict about the payment, served by its family as 502; the stable line's 0.13.4 refuses it as a parameter. A named code on both lines, before any write |
| a BEEF root the tracker refutes | 400 `VALIDATION_ERROR`, `Invalid merkle root <root> at height <height>` | 400 `VALIDATION_ERROR` (257), `Invalid BEEF at byte 41: RootNotCarried` | this line's door verifies the BEEF in one pass against the roots the tracker confirms, so a refuted root is a root the BEEF's proof names and the tracker does not carry, named at its byte; the same code |
| an empty `outputs` | 400 `VALIDATION_ERROR` (its one new `classify` arm) | 400 `VALIDATION_ERROR` (258), `Validation error: The outputs parameter must be at least one output to internalize from the transaction` | beta serves the toolbox's `InvalidParameter` by class, so it needs no arm |
| one `outputIndex` named twice on a new transaction | 400 `VALIDATION_ERROR` | 400 `VALIDATION_ERROR` (259), `Validation error: The outputs parameter must be unique outputIndex values` | none |
| one `outputIndex` named twice on a transaction the wallet holds | 400 `VALIDATION_ERROR`, the row as it was | 400 `VALIDATION_ERROR` (260), the same sentence; the row as it was (264, 266) | none |
| control: the held transaction's output named once | 200, the row unchanged | 200 (265), the row unchanged (266) | none |
| one `outputIndex` named by a wallet payment and a basket insertion on a new transaction | not read by its test | 400 `VALIDATION_ERROR` (267), the same sentence; nothing stored (269 to 271) | the stable line's test names two payments alone |
| one `outputIndex` named by two basket insertions on a new transaction | not read by its test | 400 `VALIDATION_ERROR` (268), the same sentence; nothing stored (269 to 271) | as the row above |
| a wallet payment whose sender is an uncompressed public key | 400 `VALIDATION_ERROR`, `Wallet payment outputs require a sender_identity_key that is empty or a compressed public key` | 400 `VALIDATION_ERROR` (261), `Validation error: The senderIdentityKey parameter must be no more than 66 length.` | this line's argument validator reads the sender as the reference's `validatePublicKey` does (66 hex digits, compressed), ahead of the storage door; the same code |
| a wallet payment whose sender is empty | 200, stored with no sender | 400 `VALIDATION_ERROR` (262), `Validation error: The senderIdentityKey parameter must be hexadecimal string.`, nothing stored | the toolbox's 0.14.0-beta.8.1 `### Hosts`: "An empty `senderIdentityKey`, the stable line's 0.13.3 restore, is refused on this line" |
| control: each of the rows above made well | 200, stored | 200 (238: a proven payment, one output named once, its sender a compressed key) | none |
| the storage door's rows (an empty derivation part, a sender that is not a point), through the wallet door | 400 `ERROR` | 400 `VALIDATION_ERROR` (`served`): `Validation error: The derivationPrefix parameter must be valid base64 string` (and `derivationSuffix`), `Validation error: The senderIdentityKey parameter must be a valid compressed secp256k1 public key`; nothing written or posted | this line's argument validator refuses them as parameters before the storage door; beta serves the class |
| a payment merged over an output the store holds as spent | 200, left spent | 200 (`served`): the output reclassified to a custom row, marked spent, then the payment merged: managed change of `default` (`P2PKH`, `change = 1`), `spendable` and `spent_by` as the row held them, out of the balance; control: the unspent custom row becomes spendable change and the balance counts it | none |
| a spendable row with `spent_by` set, found and relinquished | 200 | 200, counted, found by the Hosts' query, `POST /relinquishOutput` takes it out (`served`) | none |
| a change row with an empty derivation, found and relinquished | 200 | not run here | this host's wallet door never wrote one on either line, and this line's argument validator refuses an empty derivation part (the row above) |
| restore: the wallet's own change, with the wallet's identity key | 200, the spend signs | 200, the spend signs (`served`) | none |
| restore: the wallet's own change, with an empty sender | 200, stored with no sender, the spend signs | 400 `VALIDATION_ERROR`, `hexadecimal string`, nothing stored (`served`) | the toolbox's Hosts on this line, as the empty sender above |
| restore: a received payment with the sender the row held | 200, the spend signs | 200, the spend signs (`served`) | none |
| restore: a received payment with an empty sender | 400 `ERROR`, `Derivation mismatch for output <n>: ...` | 400 `VALIDATION_ERROR`, `hexadecimal string` (`served`) | refused on both lines; this line's validator refuses the empty sender before the derivation is read. Under the wallet's own key: 400 `VALIDATION_ERROR`, `Derivation mismatch for output 0: ...` (`served`), the class served as for 224 |
| restore: a payment this CLI's own flows wrote, with the anyone key | 200, the spend signs | 200, the spend signs (`served`) | none |
| a row holding an empty sender | the spend signs | the spend signs, the row not rewritten (`served`) | none |

No row or test of this line answers a 500, an `ERROR` where the stable line serves a named class, or a refusal after anything was stored. The one status the lines do not share is the no-tracker answer, 502 `SERVICE_ERROR` here and 400 `VALIDATION_ERROR` there, both named and both before any write; and the CLI's own flows meet neither, since `fund`, `receive`, `sync`, `split` and `gift-claim` refuse by name before any network act with no chain tracker (`tests/chaintracks_off.rs`, `src/commands/chaintracks_off_tests.rs`).

## The beta.9 pin (#612)

bsv-rs 0.9.0-beta.9, bsv-wallet-toolbox-rs 0.14.0-beta.9 and bsv-tracker 0.9.0-beta.9 move together. bsv-rs's prerelease carries the fix of GHSA-33fm-jcm5-4m3m, a second, heap, bound on BEEF linking beside its wire bound; the toolbox's carries that pin alone; the tracker's carries it and the room made before its 0.9.0 (five types `#[non_exhaustive]`, a stable word for each `HintStatus` and `Reask` kind, a serde form of `HintStatus`). At the tip, rows 001 to 271 replay byte-identical (271 replayed, 0 differ), the consumers' 48 replay with no difference and the gate vectors show no divergence: nothing served moved.

What moved is inside the tracker's stored row, which no row of this corpus reads. The stored hint is the crate's form, `{source, status, <payload>, observed}` with `status` the crate's word (`HintStatus::label`) and the payload under `height`, `reason`, `competitors` or `courier`: the text the host's hand map wrote before, key for key, so a row already stored reads as it is and is written back identical. The re-ask a row stores and `/tx/:txid` serves is the crate's word (`Reask::label`) with the payload this host names (`reorg@900`, `hint:arcade|seen`); the words equal the strings served before, so `tests/tx_word_route.rs` and `061-tx.pending.json` keep their answers.

Two answers move on purpose; neither is a text this host ever wrote. A stored `mined` hint with no height and a stored `mempool_present` hint with no courier are refused by the crate's reader, where the hand map read both as `unknown` (a stored `rejected` hint with no reason, which the hand map read as an empty reason, is refused as well). A refused hint, or a hints column that does not parse, no longer empties the row's hints: the row is held as stored, never replayed and never written back, and the tracker pass names it among its faults (`<txid>: a stored hint does not read; the row is held as stored, not replayed and not written: <the parse error>`) on every pass that selects it. Its `word`, `height` and `reask` columns stay what the last build that could read it wrote, and `/tx/:txid` serves them as before, with no `tier` (the tier is the hints' replay). The spend door offers nothing on such a row and names the hold as its fault.

## The stable line's 0.18.7 and 0.18.8, served on beta

The stable line's 0.18.7 is a security patch of `bsv-rs` (=0.8.11 to =0.8.12, GHSA-33fm-jcm5-4m3m); this line carries its substance through its own pin of `bsv-rs` =0.9.0-beta.9 (#612, above), so it owes the record alone, and no row moves for it. The stable line's 0.18.8 pins the toolbox's 0.13.5, which brings the received-coin rule to the stable line; this line's toolbox carries the rule since 0.14.0-beta.1 ("a received unproven coin is offered only under `underReceivedRule`"), and its spend gate passes the label under the word. What 0.18.8 found on the stable line, a received coin the gate reads as mined at its depth (the tracker's checked proof) while the toolbox has not yet completed its transaction, offered with no word and no label, was open on this line too (#627): the test that decides it, run at e058eab, answered the gate's 402 for a spend the gate allowed. The gate now passes the toolbox's label alone for such an offer, on both of its roads into the toolbox (the ordinary door and `--from`).

Rows 272 to 281 are recorded from the `received-rule` world, last, so rows 001 to 271 replay byte-identical. The answers that need the tracker's own pass are reached by no served request (no route runs it), so they are tests of `src/spend_gate/tests/received_rule.rs` (`gate`), which take their coins in through the toolbox's own `internalizeAction` and read the listing through the served route's `with_rules`; the restores' spends are `tests/served_internalize_door.rs` (`served`).

The two lines side by side: the stable line's status and code from its tests (`src/spend_gate/tests.rs` and `tests/served_internalize_door.rs` on `main`), this line's from its rows and tests.

| answer | stable (0.18.8) | beta (this corpus) | difference and its reason |
|---|---|---|---|
| under the plain word, a received unproven coin | 200, selected, the spend signs; labels the caller's, `spend-gate:received-pending`, `underReceivedRule` | 200 (280; `gate` `under_the_word_a_received_coin_is_selected_and_the_spend_signs`), the same three labels | the stored label reads `underreceivedrule`: this line's toolbox stores a label trimmed and lowercased ("The stored `underReceivedRule` label also reads back as `underreceivedrule`") |
| under the scoped word, within the named coin | 200, the named coin spent, the other unspent | 200 (278), the other unspent (279; `gate` `under_the_scoped_word_only_the_named_received_coin_is_selected`) | none |
| under the scoped word, past the named coin | 402 `INSUFFICIENT_FUNDS`, the other held `not named under allowPending: received` | 402 `INSUFFICIENT_FUNDS` (277), the same hold | none |
| no word, a received unproven coin | 402 `INSUFFICIENT_FUNDS`, the coins under `held` (`received, no proof; offered under allowPending: received`) | 402 `INSUFFICIENT_FUNDS` (275), the same | none |
| a caller's own `underReceivedRule` at `createAction`, no word | stripped: the no-word refusal, 402 | 402 (276; `gate` `a_callers_own_received_rule_label_does_not_get_the_coin`) | none |
| a caller's own `underReceivedRule` at `internalizeAction` | stripped | 200 (272), stripped (274) | none |
| the spellings the strip reads as the rule | the toolbox's reading (trimmed as JavaScript trims, then lowercased), by the stable line's own copy | the toolbox's own `normalize_label` (`gate` `a_forged_rule_label_is_stripped_and_grants_nothing`, `the_strip_trims_as_the_toolbox_trims_a_label`) | this line's gate calls the toolbox's own function; the stable line's gate keeps its own copy of the reading. The same reading |
| `listActions` with `includeLabels`, a spend under the word | the three labels, `underRule` `received-pending` | the three labels, `underRule` `received-pending` (281) | the stored spelling, as for the plain word |
| no word, a received coin the gate reads as mined while the toolbox has not completed it | 200, the spend signs, `underReceivedRule` alone | signs, `underreceivedrule` alone (`gate` `a_received_coin_the_gate_reads_as_mined_with_no_word`); at e058eab 402 `INSUFFICIENT_FUNDS` | none since #627 |
| the same on the `--from` door | no such door | signs, the label alone (`gate` `the_from_door_carries_the_toolbox_label_alone_for_a_coin_read_as_mined`) | this line's `send --from` and `split --from` hand their offer to the toolbox by their own call |
| its listing with `includeLabels` | the caller's label and `underReceivedRule`, no `underRule` | the same (`gate` `a_served_listing_shows_the_toolbox_label_alone_with_no_word`) | the stored spelling |
| under the word, a received coin read as mined | the label written once | once (`gate` `under_the_word_a_received_coin_read_as_mined_carries_the_label_once`) | none |
| a received coin the toolbox completed | selected, no label | the same (`gate` `a_received_coin_the_toolbox_completed_is_spent_with_no_label`) | none |
| an offer with no received unproven coin | no label | no label (`gate` `an_offer_with_no_received_unproven_coin_carries_no_label`) | none |
| the restores' spends | with the label, signs; without, `wallet error: Insufficient funds: need 305, have 0` | with the label, signs; without, `Insufficient funds: need 1005, have 0` (`served`, the three restores) | the stable line's spend is 300 sats, this line's 1,000; this line's toolbox answers a typed refusal, whose message carries no `wallet error: ` |
| an ancestor carried by txid alone beside a BUMP proving it | 400 `VALIDATION_ERROR`, `... is carried by txid alone` | pinned on this line when its toolbox prerelease lists the rule | this line's toolbox does not yet list it |
| its control, the ancestor whole | 200, stored, posted once | 200 (272, 273; 238) | none |
| an entry by txid alone that no BUMP proves | 400 `VALIDATION_ERROR`, `BEEF structure is invalid` | 400 `VALIDATION_ERROR`, `Invalid BEEF at byte 43: StubNotProven`, nothing written or posted (`served` `a_parent_carried_by_txid_alone_with_no_bump_is_refused`) | this line's BEEF reader names the byte and the kind (as row 257) |

No row or test of this line answers a 500, an `ERROR` where the stable line serves a named class, or a refusal after anything was stored or posted.

## The browser gate (#411) and the switch (#414)

The owner's word of 2026-11-01 on #411 ("yep I like your reccomendations...", through the keeper): the per-origin grant in the store is the shape (#414); step (b), a required `Origin` and an environment allow-list, landed first (PR 419). From #414's L5b (the switch, `docs/design/origin-grants.md` sections (1), (2), (6) and (7), under the owner's rulings W1 (a), W4 (a) and W15 (i) of 2026-11-03) a door admits a page for each power it asks by the owner's grant of that power to its origin, read from the store, and `ALLOWED_ORIGINS` is a seed beside the rows. Every served route but `POST /arc-callback` (Arcade's webhook, server to server, authenticated by the callback token) is gated; the order is the bearer (`AUTH_TOKEN`, when set), then the gate, then the body, then the door's powers.

- **The header.** A call carries `Origin`. The MetaNet Client reads it the same way (bsv-desktop `bcb6003` `src/onWalletReady.ts:45-71`) and answers a call without one `400 {"message": "Origin header is required"}` (`:82-88`); an `Origin` no URL parser reads (`null`, `not-a-url`) is none, as the client's `parseOrigin` returns null for it. The originator a door judges is derived from the `Origin` as the reference derives it (the owner's word of 2026-11-02 on the key): the host, lowercased, with its port only when the header names one, never the scheme (`https://App.Example:8443` is `app.example:8443`; `http://localhost` is `localhost`), the client's `new URL(origin).host` in the form ts-stack `edf6e03` `packages/wallet/wallet-toolbox/src/WalletPermissionsManager.ts` `prepareOriginator` reads (`:5750-5795`). Two places are stricter than the client, by the ruling's rule 1 and the stack's charter section 13 (`lean/BrowserGate.lean` `gate` at bsv-stack-lean `4372ea39`): the legacy `Originator` header never stands in for a missing `Origin` (the client falls back to it, `:59-68`), and an `Originator` header, or a JSON object body's top-level `originator`, naming another host than the `Origin`'s is refused before any row is read or written. The agreement is by hostname scope (#448): both names are compared in the lookup form, the lowercased hostname with the port dropped (`prepareOriginator`'s `normalized`, `:5776`, `:5786`), so `app.example` and `app.example:8080` agree with `https://app.example:8080` (fixture 191) and `localhost` with `http://localhost:5173`, while `evil.example` disagrees with `http://app.example` on any port; a name with a scheme is read as an `Origin`. The body's check fails closed: a second top-level `originator` is a disagreement by itself (fixture 188), and so is an `originator` that is not a string or a body the walk cannot read once it has met one; a fault before any `originator` is the door's own refusal (fixture 032). An `Originator` naming the same originator is the call (fixtures 184, 189): the reference's Node client sends `Origin` and `Originator` with one value (ts-stack `edf6e03` `packages/sdk/src/wallet/substrates/HTTPWalletJSON.ts:314`).
- **The powers.** Each door asks its powers after its arguments parse, one at a time in the reference's order (`src/server/powers.rs`, the design's door-to-kind map): a basket per output or insertion (`insertion`, `listing`, `removal`), a label as the protocol `[false,1,"action label <label>",""]`, a protocol at its level with its counterparty (`signing`, `encrypting`, `hmac`, `publicKey`), the identity key's, the certificate protocols and `proveCertificate`'s certificate, the two linkage protocols at level 2, and `createAction`'s spend when its outputs sum above 0 (W6). Each name passes bsv-rs's validator before a key is built (W17). A live row admits the power; a revoked row refuses it at once, 400 `ERR_PERMISSION_DENIED` `The user has denied the request for permission.` with no pending row (fixture 202, W3); none or an expired row holds the call on the power's pending row (an expired one as a renewal) until the owner answers: `grant` answers the door's own (fixtures 201, 203), `deny` 400 `ERR_PERMISSION_DENIED` `Permission denied.`, the reference's per-request denial (fixture 180). A held call reads its row every 250 ms and wakes at once on the owner's routes' answers. Until L6 a spend is answered `once` or `deny` alone: the answer `grant` on a spending row is refused by name.
- **The open doors and level 0 (W15, ruled (i)).** `isAuthenticated`, `waitForAuthentication`, `getHeight`, `getHeaderForHeight`, `getNetwork`, `getVersion` and any protocol power at security level 0 answer a page with a parsed `Origin` and an agreeing originator with no row (fixtures 204, 205), as the reference's manager gates none of them and BRC-100 line 65 reads level 0 as open access. Each such answer is counted in the probe record (#539), never a grant: one row per (originator, door, level-0 power) with its count and first and last times, at most 32 per originator (the oldest folded into a remainder row), written off the call's path; the owner's listings carry it as `probes` (fixtures 192, 194 with none, 213 with two rows after 210 to 212).
- **The admin-reserved names** (the protocol or label `admin...`, the label `p ...`, the basket `default` or `admin...`, in their validated form) are refused at once in the reference's words, 400 `ERR_PERMISSION_DENIED` `Basket “default” is admin-only.` (fixture 206), with no row.
- **The host routes** (`/permission-audit`, `/permission-audit/reset`, `/chain-feed`, `/tx/{txid}`) answer the owner (no `Origin`, a loopback `Host`, the bearer under `AUTH_TOKEN`) or a seed-listed originator, never a page's grant: no kind names them, so a page is refused 400 `ERR_PERMISSION_DENIED` at once (fixture 208).
- **The seed** (`ALLOWED_ORIGINS`, question 6). Its entries are originators in the reference's form, comma-separated, a host with an optional port, without a scheme, lowercased at read; an entry carrying a scheme, credentials, a path, a query or a fragment, or a name that is neither a DNS hostname, `localhost` nor an IP address, refuses the list at start with the rule it breaks, never stripped to its host. During the seed window a listed originator is admitted at every door and host route as (b) admitted it, with no row read or written, the admin-reserved names included, logged per call (`browser gate: admitted` with the entry); its lookup is the reference's hostname scope, so an entry admits every port and scheme of its host (fixtures 189, 190). At the start of `serve` and `daemon` each entry is logged by name with the seed's notice; unset seeds nothing, and every page is asked per power, the owner's strict default of 2026-11-02. The worlds seed `wire.test` and `app.example`, so rows 001 to 200 replay as before.
- **The held calls.** At most 2 per originator (the hostname) and 4 in all; past either, 429 `ERR_PERMISSION_PENDING_LIMIT` `Too many permission requests from this site are waiting for the owner's answer.` with `Retry-After: 5`, at once, writing no row (fixture 207, W14). A caller that closes its connection while held is denied with every other open row of its originator after the brief reconnect's window (#534, below); the rows are then deleted and keep no trace, and every call held on them is answered `Permission denied.` (W4 (a)); a disconnect writes nothing to its caller, so it has no fixture (the router tests over a real socket and scenes 27 and 34 show it). The server's shutdown answers a held call 503 `WALLET_BRIDGE_UNAVAILABLE` `WALLET_BRIDGE_UNAVAILABLE: HTTP server shutting down`, the client's code and text in the port's carrier (fixture 209, W13), and withdraws its row; the restart sweeps every open row. A pending row's id is never given again (`AUTOINCREMENT`), so an owner answering `#N` from an earlier listing never answers another page's request.
- **The brief reconnect's window (#534, the design's W19).** A disconnect deletes its originator's open spending rows at once and marks its other open rows abandoned for `ABANDON_WINDOW` (10 s, `src/server/powers.rs`); a call of the same originator (any port of its host) asking the same power inside the window is held on the same row, the same `pending_id`, the mark cleared; at the window's end the rows still abandoned are deleted and their calls answered 400 `Permission denied.`. An abandoned row holds a place under the cap. A pending row's JSON in the owner's listings carries `abandonedAt` and `abandonedUntil` (the window's end, `abandonedAt` plus the window in unix seconds, the store's one reading, which the CLI's line and the inspector show) only while the row is abandoned, so no recorded row moves. The owner's answer to an abandoned row is refused 409 `CONFLICT` `pending request #N is waiting for its page to reconnect; answer it when the page asks again, or it is withdrawn at <time>` (fixture 221, `owner.answer.abandoned`, recorded last from the `fresh` world; the owner step's `abandon` opens the row as a held call opens it and marks it as the disconnect does), and no grant row is written. The time is the clock's now plus the window, to the second, and the normalizer writes it `withdrawn at <time>`. Rows 001 to 220 replay byte-identical, and the consumers' corpus is unchanged.
- **The bearer.** `AUTH_TOKEN` is checked ahead of the gate for the owner's non-browser callers and never stands in for a page's grant: a caller with the right bearer and no `Origin` is still the no-origin refusal (fixture 187), and a bearer caller's page still asks its powers.
- **CORS.** Every answer to an `Origin` that parses carries its echo in `access-control-allow-origin` (never `*`), the held call's, the denial's and the 429's included, and every preflight from such an `Origin` is answered (fixture 181), since a page's opening `application/json` call must pass its preflight to reach its prompt; the methods and headers are mirrored as before (#138). A preflight with no `Origin` is refused before the layer answers it. The client answers every origin `*` (`electron/httpServer.ts:40-46` at bsv-desktop `b791cb2`) and gates by its permission prompt, as the rows gate here. The owner's routes answer no CORS and no preflight.
- **The inspector** (`bsv-wallet ui`, its own server) answers a call whose `Origin` is its own (`http://` and the `Host` it was asked by) or has its originator seed-listed, and one exemption: a `GET` or `HEAD` without an `Origin`, the only requests a browser sends without one (its own page's reads); a page of another origin cannot read their answers, as the inspector answers no CORS. It reads no grant row for its own admission: a power over a protocol or a basket is no warrant to read the whole store. Every call must name a loopback `Host`, so a page rebinding its name to the port is refused. Its origins view's writes (`POST /api/origins/answer`, `POST /api/origins/revoke`, #414) admit only its own `Origin`, never a listed or granted one; `GET /api/origins` follows the admission above. Its reads, `/api/query` among them, are on a pool opened read-only, so the database refuses any write whatever the SQL's text (#440).

The corpus under (b): the recorder's `GET` carries the harness's `Origin` (`http://wire.test`, whose originator `wire.test` every world lists, as `app.example` for row 074's `https://app.example`), so the fixtures gained the header through the harness's client, the choice that keeps every recorded answer's body and status byte-identical. 22 `GET` rows (001, 003, 005, 007, 009, 055, 057 to 063, 070 to 073, 085, 091, 092, 117, 118) moved by the request's header and the answer's CORS echo alone; row 028 moved in text to the client's words; rows 180 to 188 were new, each recorded in its own commit with its reason in the manifest. The key's move to the originator (the owner's word of 2026-11-02) moved no recorded answer; rows 189 and 190 record the hostname scope and row 191 the body's originator compared by hostname (#448). The consumers' corpus moved one row under (b): `stack-witness/01-txWord.json`, the stack witness's plain `GET` on loopback, gained `Origin: http://localhost`, the header its caller needed then; the arc-callback rows (064 to 069, 119) carry no `Origin` and are unchanged.

The corpus at the switch (#414 L5b), each moved or new row in its own commit with its reason in the manifest: row 180 moves from `gate.encrypt.notListed` (refused at once, no echo) to `gate.encrypt.heldDenied` (served from `pages`: the unseeded page's protocol call held and denied by the owner, 400 `Permission denied.` echoed); row 181 from `gate.options.notListed` (refused) to `gate.options.unlisted` (answered and echoed); row 196 moves in text to `no pending request #1 (a request is withdrawn when its caller disconnects)`; rows 201 to 209 are new (a held basket call granted, a revoked power, an expired grant held and granted, an open door, a level-0 signature, the admin-reserved basket, the cap's 429, a host route a page asks, the shutdown's 503), recorded last from the `pages` and `shutdown` worlds. 429 `ERR_PERMISSION_PENDING_LIMIT` and 503 `WALLET_BRIDGE_UNAVAILABLE` join `APP_CODES`. Rows 001 to 179, 182 to 195 and 197 to 200 replay byte-identical, and the consumers' corpus is unchanged: its stack witness's `GET /tx/{txid}` with `Origin: http://localhost` is seed-listed in its world; a stack witness job without the seed reaches the host route as the owner, with no `Origin` from loopback.

The corpus at the probe record (#539), each moved or new row in its own commit with its reason in the manifest: rows 192 and 194 (`GET /origins`) gain `"probes": []`; rows 210 to 213 are new, recorded last from the `fresh` world (a page holding no row calls `getVersion` twice and `createSignature` under `[0, "wire probe"]` once, then the owner's listing shows two probe rows, counts 1 and 2). `firstSeen` and `lastSeen` join the volatile keys. Every other row replays byte-identical.

The corpus at the spending power (#414 L6), each new row in its own commit: rows 217 to 220 are new, recorded last from the `spending` world (the funded world's four coins and its local ARC, kept apart so no funded row's coins move). A page the seed does not list spends each coin less the toolbox's fee, so no change output is drawn: held with no spending row and answered `once` (217, the door's 200, echoed); held and answered `grant` with a ceiling of 100,000 (218, 200); past the ceiling (89,960 spent, the call's 29,980) held as a renewal and denied (219, 400 `ERR_PERMISSION_DENIED` `Permission denied.`, echoed); and the renewal as the owner reads it (220: `GET /origins` while that spend is held, the spending row's `spentThisMonth` 89,960 and the open row's `renewal` true, its prompt naming the ceiling and the tally). The owner step gains `amount`, a spending grant's ceiling, and a pending row's `answeredAt` joins the volatile keys. Every other row replays byte-identical. The abort's effect on the tally is the store's (`origin_spends`), so it has no row. Two answers a page can see differ from the reference and are the design's W18, recorded by handler tests rather than rows: a held spend the owner grants below its amount is checked again and held as a renewal, where the reference's `grantPermission` admits the held call on any grant (WPM:1173-1176 at ts-stack `edf6e03d`); and a ceiling of 0 is a live row whose spends are held as renewals, where the reference reads `authorizedAmount` 0 as no token (WPM:1747, `renewal: false`).

The corpus at the grant surfaces (#533), the moved rows in their own commit with their reasons in the manifest: every grant row of the owner's answers (rows 193, 194, 195, 213 and 220) gains `state` (`granted`, `expired` or `revoked`, after `key`) and `ceilingReached` (after `spentThisMonth`: a spending row's tally at or past its ceiling, `false` with no ceiling, `null` for another kind), the store's one reading of the row (`GrantShown`, `src/store/origin_grants/shown.rs`), which the CLI, `GET /origins`, the MCP's `list_origins` and the inspector's `GET /api/origins` read alike. No field is removed. Every other row replays byte-identical.

## The owner's door (#414)

The design's L3 (`docs/design/origin-grants.md` section (3)) adds four host routes over the per-origin grant rows, never BRC-100 doors: `GET /origins`, `POST /origins/answer`, `POST /origins/grant` and `POST /origins/revoke` (`src/server/owner_door.rs`). The router serves them before the doors, which are its fallback, so the browser gate, the CORS layer and the preflight's gate never see them; a call is admitted only with no `Origin`, a loopback `Host`, and then the bearer under `AUTH_TOKEN`. Rows 192 to 200 are new, recorded last from the `fresh` and `auth` worlds: the empty listing (192), a direct grant and its listing (193, 194), the revoke (195), an answer to a row the store does not hold (196, 400 `VALIDATION_ERROR`), a page's call (197) and its preflight (198), a `Host` beyond loopback (199), each 403 `ACCESS_DENIED` naming the rule with no CORS header, and the missing bearer (200, the bearer's 401). Row 197 is the first answer of 403 `ACCESS_DENIED`, which moves from `UNPRODUCED` to `APP_CODES`; a grant row's `grantedAt` and `revokedAt` are the clock's and join `VOLATILE_KEYS`. The answer-once race needs a held call's pending row, which only the gate's switch (L5) writes; the router tests drive it. Rows 001 to 191 replay byte-identical, and the consumers' corpus is unchanged.

The design's L5a (the owner's ruling W1 (a) of 2026-11-03: the reference's four permission kinds, every power one row) moves rows 193 to 195, each re-recorded in its own commit. Row 193's request names one power, `{"originator": "owner.example:8443", "kind": "basket", "key": "[\"todo items\"]"}`, in place of the scope `wallet`; its answer's grant row carries `kind` and `key` where it carried `scope`, and `source` leaves it. Rows 194 (the listing) and 195 (the revoke with no kind, every live row of the host) move by the same two keys in the row they show. Rows 001 to 192 and 196 to 200 replay byte-identical, and the consumers' corpus is unchanged.
