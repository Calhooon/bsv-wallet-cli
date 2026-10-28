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

- **Order**: files run in name order, and the fixtures of one world share it, so a later file may read what an earlier one left (a certificate acquired, then listed; a template built, then signed or aborted). The permission audit is the process's, so `/permission-audit` lists every audited call of the files before it, in every world.
- **`request.json`** is sent as its compact bytes with `content-type: application/json`; **`request.raw`** is sent byte for byte (a malformed body); **`request.fill`** is a body of that many spaces (an oversized one; the file carries the count, not the megabyte).
- **`request.bind`**: a value of `request.json` that comes from an earlier fixture's served body in the same run. The toolbox draws a signable transaction's `reference` at random, so the fixture that signs or aborts it cannot carry the reference: its `json` carries `<bound>` there, and `bind` maps the JSON pointer in the request (`/reference`) to the file and the pointer it is read from (`096-createAction.templateToAbort.json`, `/signableTransaction/reference`). The recorder and the replay resolve it the same way, from the body that file was served; a bind to a file not served yet, or to a pointer its body lacks, is a failure naming both.
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
- the five host routes (`/permission-audit`, `/permission-audit/reset`, `/chain-feed`, `/tx/{txid}`, `/arc-callback`),
- every world named above (a world no fixture is served from is a gap),
- every code the server answers with, at its status, at least once. `AppError`'s (`src/server/error.rs`, `APP_CODES`): 400 numeric `5` (`REVIEW_ACTIONS`), 400 `VALIDATION_ERROR`, 400 `ERROR`, 402 `INSUFFICIENT_FUNDS` (with `held`), 403 `INPUT_NOT_OFFERED`, 409 `CONFLICT`, 500 `INTERNAL_ERROR`, 501 `NOT_IMPLEMENTED`, 501 `PRIVILEGED_NOT_SUPPORTED` (`privileged: true` on the nine calls that name it, #145), 502 `SERVICE_ERROR`, 503 `STORE_BUSY` (`Retry-After: 1`), 503 `HEADERS_UNAVAILABLE` (`Retry-After: 2`). The router's own (`src/server/mod.rs`, `ROUTER_CODES`): 401 `UNAUTHORIZED`, 404 `CALLBACK_DISABLED`, 404 `NOT_FOUND` (the host's missing transaction word), 400 `BAD_JSON`, 400 `BAD_PAYLOAD`, 413 `TOO_LARGE`, 500 `ERR_CHAIN_FEED`, 500 `ERR_TX_WORD`. A fixture answering a code this list does not judge is a gap too: the list and the corpus agree in both directions.

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
busy promise. The missing typed fault family and abort-acceptance detail are
recorded in CLI #342 and bsv-rs #439. Invalid BEEF folds into VALIDATION_ERROR,
and incomplete ancestry into CONFLICT without the former coin member. Prove
and abort use InvalidParameter; missing/expired sign sessions use
NotImplemented. The published relinquish paths still return success for a
missing row (#344), so no BadRequest is invented.

Two `AppError` codes no in-process world produces, so they are not judged (`UNPRODUCED` in `tests/wire_harness/judge.rs`; a fixture that answers one moves it to `APP_CODES`):

| code | why no world produces it |
|---|---|
| 403 `ACCESS_DENIED` | the reference's `Unauthorized` class, including the toolbox's permission refusal; this server grants every originator (the permission audit records, it does not refuse) |
| 502 `BROADCAST_REJECTED` | the host's verdict after the full post-broadcast probe window: no broadcaster took the transaction and an independent chain index says it is absent. The probe asks explorers; the run's proxy refuses every request, which the ladder reads as "could not look", never as absent, so the verdict is not reached in-process |

The corpus also records the framework's 400 and 422 text bodies.

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
Fixture 141, `signAction.sequenceNumberAboveType`, records axum's existing
422 text for `4294967296`, before the handler: the `u32` type already enforces
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

CI's `wire` job replays only: it fails before cargo runs if `WIRE_RECORD` is set to anything, the empty string included, and it refuses a run that left a fixture changed.

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
| each `keyringForVerifier` value | `/proveCertificate` | freshly encrypted for the verifier; every key, its order and each value's shape/length remain strict |

Inside any string: `127.0.0.1:<n>` becomes `127.0.0.1:<port>` (the local stub's port), `<n> min old` becomes `<minutes> min old` (the same age, in words), and `os error <n>` becomes `os error <errno>` (the platform's number for a refused connection; macOS and Linux differ).

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

Discovery rows 053, 054 and 137 to 140 retain their bytes and carry the reason
"toolbox #388 (CLI #331)" in the manifest. Both successful discovery calls
return empty arrays on this package; above-bound requests stop at the host's
page guard. No trust configuration is added.

## The beta.3 pin (#338)

The corpus has 174 rows. All moved and new files are named with their cause
in the single recording commit. Toolbox #404 removes the SDK Display's generic
wallet-error prefix from ordinary refusals, with statuses and codes from the
class table above. Toolbox #374 requires valid signed, encrypted certificate
material, including the disclosure fixtures; #343 sources the 64 KiB
decrypted-field bound in bsv-rs 0.9.0-beta.3
`src/auth/certificates/bounds.rs:26` (`MAX_DECRYPTED_FIELD_BYTES = 64 * 1024`),
applied at `master.rs:325-329` by toolbox 0.14.0-beta.3
`src/wallet/direct_acquire.rs:188-197`. Row 155 passes construction and
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
