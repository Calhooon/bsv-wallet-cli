# The wire fixtures (#69)

`tests/wire/` holds one JSON file per served call and per refusal body: the request as sent and the response as served (status, the headers that are part of the contract, the body), and `MANIFEST.json`, the list of every fixture the recorder wrote. `cargo test --test wire` replays every file the manifest names against the router and fails on any byte of difference after normalization, printing a line diff. The corpus is the contract of the served shape that MetaNet Client compatibility rests on: a refactor shows zero diff; a behavior change changes a fixture, and a reviewer sees the change in the fixture.

## References, at pins

The field names each shape must keep are BRC-100's (`wallet/0100.md` of bsv-blockchain/BRCs) at the line's pin, `ed1b015` (2026-10-05), the commit the stack's `docs/PROVENANCE.md` names; read it with `git -C ~/bsv/brcs show ed1b015:wallet/0100.md`. Upstream master `5119a9b` (2026-10-19) differs from the pin in two places (`git -C ~/bsv/brcs diff ed1b015 5119a9b -- wallet/0100.md`): in `listOutputs`, `totalOutputs` may be a decimal string and `spendable` may be `false`, for the BRC-112 P-basket balance query and a BRC-207 module inventory view; and in `internalizeAction`, an insertion's `customInstructions` is bounded at 1000 UTF-8 bytes unless a module defines an exception. That is upstream, not on the line: the fixtures record `totalOutputs` as a number and `spendable: true`, as the pin has them, and no fixture sends `customInstructions`. Where this document names a BRC-100 shape, it is the pin's.

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

- **`world`**: the wallet and the server config the request is served from. Each world is built once per run from its name alone (`World::build` in `tests/wire.rs`); a fixture naming any other world fails the run before a world is built, naming the file and the world.

  | world | what it is |
  |---|---|
  | `fresh` | an empty wallet, no bearer, the header stub up (tip 900010) |
  | `auth` | `fresh` behind a bearer token |
  | `callback` | `fresh` with an ARC callback token |
  | `tracked` | the tracker's words for a mined and a pending transaction, and one coin the chain feed froze |
  | `received` | one received coin, its creator mined at 900000 by a stored one-leaf proof, its raw transaction not stored |
  | `headers-down` | `received`, the header service answering 500 |
  | `broken` | `fresh` with the `outputs` table dropped (a store fault) |
  | `broken-host` | `fresh` with the tracker's rows unreadable: a feed status that is not JSON, a states table without its columns |
  | `busy` | a file store whose write lock another connection holds |
  | `funded` | four mined coins the wallet can sign for (50,000, 40,000, 30,000, 20,000 sats, each a one-leaf block of its own that the header stub answers); the mined parents of three payments from a fixed stranger key and of a puzzle coin (`OP_1 OP_EQUAL`), whose blocks the header stub answers too; and a local ARC that takes every post |

- **Order**: files run in name order, and the fixtures of one world share it, so a later file may read what an earlier one left (a certificate acquired, then listed; a template built, then signed or aborted). The permission audit is the process's, so `/permission-audit` lists every audited call of the files before it, in every world.
- **`request.json`** is sent as its compact bytes with `content-type: application/json`; **`request.raw`** is sent byte for byte (a malformed body); **`request.fill`** is a body of that many spaces (an oversized one; the file carries the count, not the megabyte).
- **`request.bind`**: a value of `request.json` that comes from an earlier fixture's served body in the same run. The toolbox draws a signable transaction's `reference` at random, so the fixture that signs or aborts it cannot carry the reference: its `json` carries `<bound>` there, and `bind` maps the JSON pointer in the request (`/reference`) to the file and the pointer it is read from (`096-createAction.templateToAbort.json`, `/signableTransaction/reference`). The recorder and the replay resolve it the same way, from the body that file was served; a bind to a file not served yet, or to a pointer its body lacks, is a failure naming both.
- **`response.json`** is a JSON body after normalization; **`response.text`** is a body that is not JSON (the framework's own rejections, an unknown route's empty 404, a preflight's empty 200).
- **`response.headers`** keeps `content-type`, `retry-after` and every CORS header: `access-control-allow-origin`, `-allow-credentials`, `-expose-headers`, `vary`, and the preflight's `-allow-methods`, `-allow-headers` and `-max-age` (the server sets no max-age; the fixture shows its absence). The rest is the transport's.
- The recorded JSON keeps the body's own key order: a field moved is a byte changed. Each served JSON body is also checked to be compact JSON in that order, so the comparison is of the body's bytes after normalization.

## What the run judges

The manifest, before any fixture is served: every file `MANIFEST.json` lists must be on disk, every file on disk must be listed, and nothing but `.json` fixtures may sit beside the manifest. A fixture deleted, a fixture added by hand, or a stray file is a failure naming it, not a silent pass.

Beyond the byte comparison, the replay fails when the corpus stops covering:

- each of the 28 BRC-100 calls the router serves,
- the five host routes (`/permission-audit`, `/permission-audit/reset`, `/chain-feed`, `/tx/{txid}`, `/arc-callback`),
- every world named above (a world no fixture is served from is a gap),
- every code the server answers with, at its status, at least once. `AppError`'s (`src/server/handlers.rs`, `APP_CODES`): 400 `VALIDATION_ERROR`, 400 `INVALID_BEEF`, 400 `ERROR`, 402 `INSUFFICIENT_FUNDS` (with `held`), 403 `INPUT_NOT_OFFERED`, 404 `NOT_FOUND`, 409 `UNSPENDABLE_COIN` (with `coin`), 409 `CONFLICT`, 500 `INTERNAL_ERROR`, 502 `SERVICE_ERROR`, 503 `STORE_BUSY` (`Retry-After: 1`), 503 `HEADERS_UNAVAILABLE` (`Retry-After: 2`). The router's own (`src/server/mod.rs`, `ROUTER_CODES`): 401 `UNAUTHORIZED`, 404 `CALLBACK_DISABLED`, 400 `BAD_JSON`, 400 `BAD_PAYLOAD`, 413 `TOO_LARGE`, 500 `ERR_CHAIN_FEED`, 500 `ERR_TX_WORD`. A fixture answering a code this list does not judge is a gap too: the list and the corpus agree in both directions.

`APP_CODES` is typed by hand, but it is checked against the source: `every_app_error_code_is_judged` reads every `(StatusCode::NAME, "CODE")` of `AppError::classify` and every `status:`/`code:` pair of a built `AppError` in `src/server/handlers.rs` (a code held in a constant read from its module), and fails unless `APP_CODES` and `UNPRODUCED` together are exactly that set, at the same statuses. A code added to the source with no fixture fails it by name, and so does one gone from the source.

Three `AppError` codes no in-process world produces, so they are not judged (`UNPRODUCED` in `tests/wire.rs`; a fixture that answers one moves it to `APP_CODES`):

| code | why no world produces it |
|---|---|
| 401 `AUTHENTICATION_REQUIRED` | classified from a toolbox message beginning `Authentication required` or `Invalid identity key:`; the router serves one wallet with its key loaded, and no door the toolbox exposes asks it to authenticate another identity |
| 403 `ACCESS_DENIED` | classified from `Access denied:`, the toolbox's permission refusal; this server grants every originator (the permission audit records, it does not refuse) |
| 502 `BROADCAST_REJECTED` | the host's verdict after the full post-broadcast probe window: no broadcaster took the transaction and an independent chain index says it is absent. The probe asks explorers; the run's proxy refuses every request, which the ladder reads as "could not look", never as absent, so the verdict is not reached in-process |

The corpus also records the framework's 400 and 422 text bodies.

## Known server defects, recorded as served

A fixture records what the server answers, defect included; the fix changes the fixture in its own commit, and the reviewer sees the answer move.

| fixture | what is served | why it is a defect | issue |
|---|---|---|---|
| `082-options.auth.createAction.json` | a preflight in the `auth` world answers 401 `UNAUTHORIZED` with no CORS headers | the bearer is checked before CORS answers, so a browser's `OPTIONS` request, which carries no credentials, is refused and the browser never sends the real call; the CORS layer must answer a preflight before the bearer is asked | the bearer-before-CORS issue (#138) (number to come from the captain) |
| `103-internalizeAction.badUnlock.json` | `accepted: true` for a payment whose only input's unlocking script is `OP_0` against a P2PKH lock (its spend fails, `OP_EQUALVERIFY`) | neither the server nor the toolbox (=0.13.0) runs the payment's scripts: `internalizeAction` checks the BEEF's proofs, not that the transaction spends what it names. Here the broadcaster takes every post; a real one would refuse it, and the toolbox reads a refusal on its post as a hint, not a refusal of the payment | to be filed (the unverified-spend issue (#140)) |

## Changing a fixture on purpose

A PR that changes what the server answers changes its fixture in the same PR, in a commit of its own:

1. Make the change in `src/` (one commit).
2. `WIRE_RECORD=1 cargo test --locked --test wire` re-records every fixture and the manifest from the cases in `tests/wire.rs`. A recording never passes: the run ends with the word that a recording is not a replay and must be replayed by a second run. Nothing else re-records: unset, the run only replays, and any other value (`0`, `true`, an empty string) fails the run naming it.
3. `cargo test --locked --test wire` replays what was recorded.
4. `git diff tests/wire` shows exactly the bytes the change moved. Commit them alone, with a subject that says which answer changed and why (`wire: /listActions carries underRule`).

CI's `wire` job replays only: it fails before cargo runs if `WIRE_RECORD` is set to anything, the empty string included, and it refuses a run that left a fixture changed.

A new served call or refusal gets a case in one of the `record_*` functions `record_all` calls in order (`tests/wire.rs`) and, if it is a route or a code the judge must keep, an entry in `ENDPOINTS`, `HOST_ROUTES`, `APP_CODES` or `ROUTER_CODES` (a new `AppError` code with no entry fails `every_app_error_code_is_judged`). Re-recording writes the files with fresh sequence numbers, so a case added in the middle renames the files after it; the diff shows them as moved, their bodies unchanged.

Two recordings in a row are byte-identical: a re-record with no change in `src/` leaves `git status` clean. A bound request value is written as `<bound>`, never as the value sent, for that reason.

## What the normalizer hides, and why

One normalizer, `NORMALIZER` in `tests/wire.rs` (its rules; the walk and the capture it drives are `Normalizer` in `tests/wire_support/mod.rs`, the replay machinery `tests/wire_consumers.rs` shares), and only values that differ from run to run with no change to the code. A placeholder replaces a value, never a key, and it keeps the value's shape: its JSON type, a byte array's length, a string's kind and length, a number's sign and digit count. A field that changes from a byte array to a hex string, or grows by a byte, fails the replay.

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

Inside any string: `127.0.0.1:<n>` becomes `127.0.0.1:<port>` (the local stub's port), `<n> min old` becomes `<minutes> min old` (the same age, in words), and `os error <n>` becomes `os error <errno>` (the platform's number for a refused connection; macOS and Linux differ).

What is not hidden, because it does not differ: the identity key, every derived key, every signature, every txid, every built transaction. The worlds use one fixed root key, so the identity key, the BRC-42 derivations and the RFC 6979 signatures are the same on every run, and a change to derivation or signing fails the replay. The seeded txids are fixed values or the ids of fixed transactions. The `funded` world's actions each ask for exactly their coin less the toolbox's fee for one input and one output (20 sats at its rate for a P2PKH input, 9 for the puzzle's one-byte unlock), and the basket wants no more coins than it has, so the toolbox builds no change output; the template, the signed transaction, their txids and the AtomicBEEFs are the same bytes on every run. The stranger's payments are signed by a fixed key (RFC 6979), and the recorder checks each before it is offered: the two valid spends verify (their scripts run true, their parents' proofs at their heights), the bad one does not.

Not recorded, for that reason: an action with a change output, and so a non-empty `noSendChange`. The toolbox draws a change output's derivation suffix from `rand::thread_rng` (`random_derivation` in its `create_action.rs`), so the change's locking script, the transaction's bytes and its txid move on every run, and hiding them would hide the shape the fixture is for. It needs a seeded derivation in the toolbox, or a normalizer rule scoped to one fixture's txid and bytes; neither is this corpus's to add. The `/decrypt` request carries a fixed ciphertext of `wire` under that key (`DECRYPT_CIPHERTEXT`), so a recording does not move it.

## Nothing leaves the machine

The router is driven in-process (`tower::ServiceExt::oneshot`). The header service is a local stub on `127.0.0.1`; the broadcaster is a closed local port, except in `funded`, where it is a local ARC (`arc_stub`) answering `SEEN_ON_NETWORK` for every posted transaction; `HTTPS_PROXY` and `HTTP_PROXY` point at a closed local port with `NO_PROXY=127.0.0.1`, so an explorer request fails at once instead of reaching the network. The operator's settings (`SPEND_DEPTH`, `ARC_*`, `CHAINTRACKS_URL`, the explorer keys and the rest) are removed from the run's environment before any world is built. In `funded`, `createAction` in its default mode (`acceptDelayedBroadcast: false`, as x402 sends it) and each `internalizeAction` post to that stub, which takes them: the action answers `sendWithResults` `unproven`, and the toolbox posts to no other broadcaster after an acceptance.

The worlds use the toolbox's real `Services`, pointed at the stubs, not its `MockWalletServices` (whose `post_beef_success` the ARC stub stands in for): the router's state is `Arc<Wallet<StorageSqlx, Services>>`, a concrete type, and the brief allows no change under `src/` for it.

## The judge, judged

`tests/wire.rs` carries planted cases beside the replay: `AppError`'s codes in the source and the judged and unproduced lists agree in both directions; a recording run into a temporary directory ends with the word to replay and never passes, a second run replays it clean, and every file it wrote is the committed one byte for byte; a `WIRE_RECORD` other than `1` is refused by name; a manifest with a listed file missing, a file on disk unlisted, and a stray non-JSON file names all three, and the replay refuses that set naming them; a fixture of an unknown world fails the replay naming the file and the world; the normalizer keeps each placeholder's shape and leaves a non-volatile key whole. The runs share one lock, since the permission audit is the process's.
