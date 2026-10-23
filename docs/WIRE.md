# The wire fixtures (#69)

`tests/wire/` holds one JSON file per served call and per refusal body: the request as sent and the response as served (status, the headers that are part of the contract, the body). `cargo test --test wire` replays every file against the router and fails on any byte of difference after normalization, printing a line diff. The corpus is the contract of the served shape that MetaNet Client compatibility rests on: a refactor shows zero diff; a behavior change changes a fixture, and a reviewer sees the change in the fixture.

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
    "json": { "ciphertext": "<ciphertext>" }
  }
}
```

- **`world`**: the wallet and the server config the request is served from. Each world is built once per run from its name alone (`World::build` in `tests/wire.rs`):

  | world | what it is |
  |---|---|
  | `fresh` | an empty wallet, no bearer, the header stub up (tip 900010) |
  | `auth` | `fresh` behind a bearer token |
  | `callback` | `fresh` with an ARC callback token |
  | `tracked` | the tracker's words for a mined and a pending transaction, and one coin the chain feed froze |
  | `received` | one received coin, its creator mined at 900000 by a stored one-leaf proof, its raw transaction not stored |
  | `headers-down` | `received`, the header service answering 500 |
  | `broken` | `fresh` with the `outputs` table dropped (a store fault) |
  | `busy` | a file store whose write lock another connection holds |

- **Order**: files run in name order, and the fixtures of one world share it, so a later file may read what an earlier one left (a certificate acquired, then listed). The permission audit is the process's, so `/permission-audit` lists every audited call of the files before it, in every world.
- **`request.json`** is sent as its compact bytes with `content-type: application/json`; **`request.raw`** is sent byte for byte (a malformed body).
- **`response.json`** is a JSON body after normalization; **`response.text`** is a body that is not JSON (the framework's own rejections, an unknown route's empty 404).
- **`response.headers`** keeps `content-type`, `retry-after` and the CORS headers. The rest is the transport's.
- The recorded JSON keeps the body's own key order: a field moved is a byte changed. Each served JSON body is also checked to be compact JSON in that order, so the comparison is of the body's bytes after normalization.

## What the run judges

Beyond the byte comparison, the replay fails when the corpus stops covering:

- each of the 28 BRC-100 calls the router serves,
- the five host routes (`/permission-audit`, `/permission-audit/reset`, `/chain-feed`, `/tx/{txid}`, `/arc-callback`),
- each refusal: 400 `VALIDATION_ERROR`, 401 `UNAUTHORIZED`, 402 `INSUFFICIENT_FUNDS` (with `held`), 403 `INPUT_NOT_OFFERED`, 404 `NOT_FOUND`, 409 `UNSPENDABLE_COIN` (with `coin`), 500 `INTERNAL_ERROR`, 502 `SERVICE_ERROR`, 503 `STORE_BUSY` (`Retry-After: 1`), 503 `HEADERS_UNAVAILABLE` (`Retry-After: 2`).

The corpus also records the 400 `ERROR` and `INVALID_BEEF` codes, the 404 `CALLBACK_DISABLED`, the callback route's 400 `BAD_JSON`, and the framework's 400 and 422 text bodies.

## Changing a fixture on purpose

A PR that changes what the server answers changes its fixture in the same PR, in a commit of its own:

1. Make the change in `src/` (one commit).
2. `WIRE_RECORD=1 cargo test --locked --test wire` re-records every fixture from the cases in `tests/wire.rs`. Nothing else re-records: without `WIRE_RECORD=1` the run only replays, and CI's `wire` job also refuses a run that left a fixture changed.
3. `git diff tests/wire` shows exactly the bytes the change moved. Commit them alone, with a subject that says which answer changed and why (`wire: /listActions carries underRule`).

A new served call or refusal gets a case in `record_all` (`tests/wire.rs`) and, if it is a route or a code the judge must keep, an entry in `ENDPOINTS`, `HOST_ROUTES` or `CODES`. Re-recording writes the files with fresh sequence numbers, so a case added in the middle renames the files after it; the diff shows them as moved, their bodies unchanged.

Two recordings in a row are byte-identical: a re-record with no change in `src/` leaves `git status` clean.

## What the normalizer hides, and why

One function, `normalize` in `tests/wire.rs`, and only values that differ from run to run with no change to the code:

| what | where | placeholder | why it differs |
|---|---|---|---|
| `ciphertext` | `/encrypt` | `<ciphertext>` | AES-GCM under a fresh random IV each call |
| `encryptedLinkage`, `encryptedLinkageProof` | the two key-linkage calls | `<ciphertext>` | encrypted for the verifier the same way |
| `revelationTime` | `/revealCounterpartyKeyLinkage` | `<timestamp>` | the clock |
| `ageMinutes` | a refusal's `held` coins | `<minutes>` | the clock, from the row's fixed time |
| `<n> min old` | inside a refusal's message | `<minutes> min old` | the same age, in words |
| `127.0.0.1:<n>` | inside any string | `127.0.0.1:<port>` | the local stub's port |

A placeholder replaces a value, never a key: the field's presence and place stay judged. A `null` is kept as `null`.

What is not hidden, because it does not differ: the identity key, every derived key and every signature. The worlds use one fixed root key, so the identity key, the BRC-42 derivations and the RFC 6979 signatures are the same on every run, and a change to derivation or signing fails the replay. The txids are the seeded rows' fixed values. The `/decrypt` request carries a fixed ciphertext of `wire` under that key (`DECRYPT_CIPHERTEXT`), so a recording does not move it.

## Nothing leaves the machine

The router is driven in-process (`tower::ServiceExt::oneshot`). The header service is a local stub on `127.0.0.1`; the broadcaster is a closed local port; `HTTPS_PROXY` and `HTTP_PROXY` point at a closed local port with `NO_PROXY=127.0.0.1`, so an explorer request fails at once instead of reaching the network. The operator's settings (`SPEND_DEPTH`, `ARC_*`, `CHAINTRACKS_URL`, the explorer keys and the rest) are removed from the run's environment before any world is built.

The worlds use the toolbox's real `Services`, pointed at the stubs, not its `MockWalletServices`: the router's state is `Arc<Wallet<StorageSqlx, Services>>`, a concrete type, and the brief allows no change under `src/` for it.
