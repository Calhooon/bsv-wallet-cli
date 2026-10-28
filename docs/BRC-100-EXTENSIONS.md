# BRC-100 extensions: every field we serve beyond the BRC (#100)

The reference is BRC-100 (`wallet/0100.md` of bsv-blockchain/BRCs) at the line's pin `5119a9b` (2026-10-19), extracted call by call into `docs/brc100/fields.json`: each request and response field the pin's TypeScript interface names, with its line, and the uniform error structure (lines 291 to 296: `status`, `code`, `description`, `context`). The served shapes are the wire fixtures, `tests/wire/*.json` (`docs/WIRE.md`). This document lists every key a fixture serves or sends that `fields.json` does not name, each with its reason and the release or issue that added it; the fields the BRC names that we do not serve; the fields the source can serve that no fixture shows yet; and the host's own routes.

`cargo test --test brc100` (`tests/brc100.rs`, in the ordinary `cargo test` run) holds this document true in both directions. It reads every fixture of the 28 calls, collects the request keys sent and the response keys served (a 2xx body is the `response` side, any other body the `refusal` side) and fails naming the key when a key is neither in `fields.json` nor a row of the extensions table below. It fails on a dead row too: an extension no fixture serves, a gap the corpus does serve, an unrecorded field a fixture now serves (move it to the extensions table), a host route no fixture calls. It also fails on a fixture whose path is neither one of the 28 calls nor a host route below, unless the fixture is the router's 404 for an unknown route.

## The table format is strict

The test parses the four tables below by their header rows, which must stay exactly as they are. Each row has every cell; `endpoint` is one of the 28 calls (as `fields.json` spells it) or `*` for every call, in backticks; `side` is `request`, `response` or `refusal`; `field` is the key's dotted path in backticks. A path follows `fields.json`'s notation: an array is transparent (`held.outpoint` is the `outpoint` of each held coin) and a key of a BRC `Record` (`fields`, `spends`, `keyringForSubject` and the others marked `record`) is `*`. Each table's `##` heading ends with its row count (`: N rows`), which the test checks against the rows it reads, and a table ends only at its section's end: a `|` line after a blank line, before the next `##`, is a malformed row. A row the test cannot read fails it by its line. Prose outside the tables is not read.

Only JSON keys are judged. The transport (HTTP methods, headers, status codes, the framework's text bodies) is described under "The transport" and not tested here; the status codes and refusal codes are judged by `tests/wire.rs`.

The `internalizeAction` basket insertion's `customInstructions` bound is the reference's rule, not an extension: at `edf6e03`, `validateInternalizeActionArgs` limits it to 1,000 UTF-8 bytes and throws `WERR_INVALID_PARAMETER`. Since 0.18.2, [fixture 123](../tests/wire/123-internalizeAction.customInstructionsBound.json) records 1,001 bytes refused as HTTP 400 `VALIDATION_ERROR`, with `customInstructions: no more than 1000 length.` and the measured byte count. The served-handler tests also check that nothing is stored. `createAction` output instructions remain unbounded. No table row changes.

The five paging calls' `limit` bounds are BRC-100's own: 1 to 10000, with 10
when absent (`wallet/0100.md:277,808,925,1291,1386,1429` at `5119a9b`). The
JSON door checks supplied values before the toolbox (#198), returning 400
`VALIDATION_ERROR` with the reference validator's minimum or maximum wording
and the supplied value. The four unsigned offsets already have their type's
0 to 4294967295 bound (`:274,809,1292,1387,1430`); absent paging fields still
reach the toolbox unchanged. `signAction.spends.*.sequenceNumber` is already
bounded by its `u32` type (`:728,744`), so its handler is unchanged. These
constraints add no JSON key and no extension-table row. Negative
`listOutputs.offset` values pass unchanged for BRC-100's newest-first paging
(`wallet/0100.md:911,926`): `-1` starts at the newest output.

## Extensions: 19 rows

| endpoint | side | field | reason | added |
|---|---|---|---|---|
| `*` | refusal | `message` | The refusal's human-readable text; the BRC's error structure names it `description` (line 295). BRC-100 leaves the error's realisation to the transport (line 298); the body is `{code, message}` with `code` the BRC's (line 294), against the BRC's `{status, code, description}` (#149). | the initial release, feb0dd0 (2026-03-25) |
| `createAction` | request | `allowPending` | R1 (d), the action's word at the door, beside `options` (the toolbox's `CreateActionOptions` is the reference's shape and stays as it is): `"received"` offers a received coin whose creator is not yet mined; absent or any other word is off (#122 item 7). | 0.11.0 (#29; the owner's ruling of 2026-10-18) |
| `createAction` | request | `allowPending.received` | The scoped form `{"received": ["txid:vout", ...]}`: only the named received coins are offered; a named coin that is not a received coin of this wallet is 400. | 0.18.0 (#63) |
| `createAction` | refusal | `offeredSatoshis` | The satoshis the spend gate offered the toolbox, beside the coins it held back. | 0.10.0 (e69291b) |
| `createAction` | refusal | `held` | Every coin the spend gate held back, on 402 `INSUFFICIENT_FUNDS`, 403 `INPUT_NOT_OFFERED` (0.11.0) and 503 `HEADERS_UNAVAILABLE` (0.18.0, #59). | 0.10.0 (e69291b) |
| `createAction` | refusal | `held.outpoint` | The held coin. | 0.10.0 (e69291b) |
| `createAction` | refusal | `held.satoshis` | Its value. | 0.10.0 (e69291b) |
| `createAction` | refusal | `held.word` | The tracker's word for its creator. | 0.10.0 (e69291b) |
| `createAction` | refusal | `held.origin` | `received` or our own change. | 0.10.0 (e69291b) |
| `createAction` | refusal | `held.ageMinutes` | The coin's age in minutes. | 0.10.0 (e69291b) |
| `createAction` | refusal | `held.reason` | The rule that holds it. | 0.10.0 (e69291b) |
| `createAction` | refusal | `coin` | The coin a 409 `UNSPENDABLE_COIN` names (its stored ancestry is incomplete). | 0.16.0 (#44) |
| `createAction` | refusal | `isError` | The typed review envelope marker. HTTP 400, numeric code 5 under the pinned #140 host map. | #277, bsv-rs #339 |
| `createAction` | refusal | `reviewActionResults` | The reference review evidence, retained from the typed carrier. HTTP 400, numeric code 5 under the pinned #140 host map. | #277, bsv-rs #339 |
| `createAction` | refusal | `reviewActionResults.txid` | The transaction under review. HTTP 400, numeric code 5 under the pinned #140 host map. | #277, bsv-rs #339 |
| `createAction` | refusal | `reviewActionResults.status` | The review outcome. HTTP 400, numeric code 5 under the pinned #140 host map. | #277, bsv-rs #339 |
| `createAction` | refusal | `reviewActionResults.competingTxs` | The competing transaction IDs. HTTP 400, numeric code 5 under the pinned #140 host map. | #277, bsv-rs #339 |
| `createAction` | refusal | `sendWithResults` | The action send results, including an empty list. HTTP 400, numeric code 5 under the pinned #140 host map. | #277, bsv-rs #339 |
| `listCertificates` | response | `certificates.verifier` | bsv-rs's `CertificateResult` carries the verifier beside the certificate. | the initial release, feb0dd0 |

19 extensions.

## Gaps: 49 rows

Fields the BRC names that we do not read or do not serve. A request gap is a field the handler's type does not read (serde drops it) or refuses; a response gap is a field the BRC's shape has that the served body does not. An optional response field we never set (`context` of an error, `BEEF` of a `listOutputs` asked for no transactions) is not a gap.

The test checks served keys only. A response or refusal gap fails it the day a fixture serves the field; a request gap is held true by this document's word alone: the test checks that it names a field of BRC-100, not that the handler drops it (the fixtures send what the handler reads). The cold review of PR 143 checked every request gap by hand against `src/server/types.rs`.

| endpoint | side | field | what is served instead | issue |
|---|---|---|---|---|
| `*` | refusal | `status` | no `status: "error"` key; the HTTP status carries the failure | #149 |
| `*` | refusal | `description` | `message` | #149 |
| `createAction` | request | `version` | not read; the toolbox's default version | #147 |
| `createAction` | request | `options.knownTxids` | not read | #147 |
| `createAction` | request | `options.returnTXIDOnly` | not read | #147 |
| `createAction` | request | `options.noSendChange` | not read (a chained `noSend` batch cannot name its change) | #147 |
| `createAction` | request | `options.sendWith` | not read (a `noSend` batch cannot be sent through `createAction`) | #147 |
| `internalizeAction` | request | `seekPermission` | not read | #145 (held unread: every originator is granted) |
| `getPublicKey` | request | `seekPermission` | not read | #145 (held unread: every originator is granted) |
| `encrypt` | request | `seekPermission` | not read | #145 (held unread: every originator is granted) |
| `decrypt` | request | `seekPermission` | not read | #145 (held unread: every originator is granted) |
| `createHmac` | request | `seekPermission` | not read | #145 (held unread: every originator is granted) |
| `verifyHmac` | request | `seekPermission` | not read | #145 (held unread: every originator is granted) |
| `createSignature` | request | `counterparty` | required, where the BRC has it optional (line 1186); a request without it is a 422 | #148 |
| `createSignature` | request | `seekPermission` | not read | #145 (held unread: every originator is granted) |
| `verifySignature` | request | `hashToDirectlyVerify` | not read: the handler passes the toolbox no hash (`hash_to_directly_verify: None`) | #146 |
| `verifySignature` | request | `seekPermission` | not read | #145 (held unread: every originator is granted) |
| `acquireCertificate` | request | `privileged` | carried unread; the everyday key | #194 |
| `acquireCertificate` | request | `privilegedReason` | carried unread | #194 |
| `proveCertificate` | request | `privileged` | read for the permission check only; the verifier's keyring is built with the everyday key | #194 |
| `proveCertificate` | request | `privilegedReason` | carried unread | #194 |
| `discoverByIdentityKey` | response | `certificates.type` | no entries: toolbox #388 (CLI #331); the beta.2 type has this BRC field | #331 |
| `discoverByIdentityKey` | response | `certificates.subject` | no entries: toolbox #388 (CLI #331); the beta.2 type has this BRC field | #331 |
| `discoverByIdentityKey` | response | `certificates.serialNumber` | no entries: toolbox #388 (CLI #331); the beta.2 type has this BRC field | #331 |
| `discoverByIdentityKey` | response | `certificates.certifier` | no entries: toolbox #388 (CLI #331); the beta.2 type has this BRC field | #331 |
| `discoverByIdentityKey` | response | `certificates.revocationOutpoint` | no entries: toolbox #388 (CLI #331); the beta.2 type has this BRC field | #331 |
| `discoverByIdentityKey` | response | `certificates.signature` | no entries: toolbox #388 (CLI #331); the beta.2 type has this BRC field | #331 |
| `discoverByIdentityKey` | response | `certificates.fields` | no entries: toolbox #388 (CLI #331); the beta.2 type has this BRC field | #331 |
| `discoverByIdentityKey` | response | `certificates.certifierInfo` | no entries: toolbox #388 (CLI #331); the beta.2 type has this BRC field | #331 |
| `discoverByIdentityKey` | response | `certificates.certifierInfo.name` | no entries: toolbox #388 (CLI #331); the beta.2 type has this BRC field | #331 |
| `discoverByIdentityKey` | response | `certificates.certifierInfo.iconUrl` | no entries: toolbox #388 (CLI #331); the beta.2 type has this BRC field | #331 |
| `discoverByIdentityKey` | response | `certificates.certifierInfo.description` | no entries: toolbox #388 (CLI #331); the beta.2 type has this BRC field | #331 |
| `discoverByIdentityKey` | response | `certificates.certifierInfo.trust` | no entries: toolbox #388 (CLI #331); the beta.2 type has this BRC field | #331 |
| `discoverByIdentityKey` | response | `certificates.publiclyRevealedKeyring` | no entries: toolbox #388 (CLI #331); the beta.2 type has this BRC field | #331 |
| `discoverByIdentityKey` | response | `certificates.decryptedFields` | no entries: toolbox #388 (CLI #331); the beta.2 type has this BRC field | #331 |
| `discoverByAttributes` | response | `certificates.type` | no entries: toolbox #388 (CLI #331); the beta.2 type has this BRC field | #331 |
| `discoverByAttributes` | response | `certificates.subject` | no entries: toolbox #388 (CLI #331); the beta.2 type has this BRC field | #331 |
| `discoverByAttributes` | response | `certificates.serialNumber` | no entries: toolbox #388 (CLI #331); the beta.2 type has this BRC field | #331 |
| `discoverByAttributes` | response | `certificates.certifier` | no entries: toolbox #388 (CLI #331); the beta.2 type has this BRC field | #331 |
| `discoverByAttributes` | response | `certificates.revocationOutpoint` | no entries: toolbox #388 (CLI #331); the beta.2 type has this BRC field | #331 |
| `discoverByAttributes` | response | `certificates.signature` | no entries: toolbox #388 (CLI #331); the beta.2 type has this BRC field | #331 |
| `discoverByAttributes` | response | `certificates.fields` | no entries: toolbox #388 (CLI #331); the beta.2 type has this BRC field | #331 |
| `discoverByAttributes` | response | `certificates.certifierInfo` | no entries: toolbox #388 (CLI #331); the beta.2 type has this BRC field | #331 |
| `discoverByAttributes` | response | `certificates.certifierInfo.name` | no entries: toolbox #388 (CLI #331); the beta.2 type has this BRC field | #331 |
| `discoverByAttributes` | response | `certificates.certifierInfo.iconUrl` | no entries: toolbox #388 (CLI #331); the beta.2 type has this BRC field | #331 |
| `discoverByAttributes` | response | `certificates.certifierInfo.description` | no entries: toolbox #388 (CLI #331); the beta.2 type has this BRC field | #331 |
| `discoverByAttributes` | response | `certificates.certifierInfo.trust` | no entries: toolbox #388 (CLI #331); the beta.2 type has this BRC field | #331 |
| `discoverByAttributes` | response | `certificates.publiclyRevealedKeyring` | no entries: toolbox #388 (CLI #331); the beta.2 type has this BRC field | #331 |
| `discoverByAttributes` | response | `certificates.decryptedFields` | no entries: toolbox #388 (CLI #331); the beta.2 type has this BRC field | #331 |

The certificate type and flat list-entry gaps close at beta.2 (bsv-rs #105,
#268). Discovery still returns no entries because published toolbox
`certifier_info` always returns `None`: toolbox #388 (CLI #331). The discovery
rows record this missing behavior; no trust setting is added. Key-linkage results are also flat at beta.2.
Privileged-request and refusal-envelope gaps retain their existing issues.
`getVersion` still answers the toolbox's own version (#122 item 6).

## Served by the source, not in the corpus: 18 rows

Fields the server's types can serve that no fixture shows. Each is an extension the day a fixture serves it: the test then fails until the row moves to the extensions table.

| endpoint | side | field | why no fixture serves it | added |
|---|---|---|---|---|
| `listActions` | response | `actions.underRule` | The rule an action was created under (`spend-gate:received-pending`, from `allowPending`); no fixture lists an action created under the word. | 0.11.0 (#29) |
| `createAction` | response | `broadcast` | The doors' word when the immediate post drew no accepting word (`src/server/post_word.rs`); the `funded` world's ARC stub takes every post. | 43376b8 (#66, 2026-10-10) |
| `createAction` | response | `broadcast.word` | `built`. | 43376b8 (#66) |
| `createAction` | response | `broadcast.request` | The proof request's status. | 43376b8 (#66) |
| `createAction` | response | `broadcast.hint` | The toolbox's last immediate-post hint, an open value: the note the toolbox (=0.12.1) appends to the proof request's history (`record_immediate_broadcast_hint`), served whole by `last_note` (`src/server/post_word.rs`); its sub-keys are the next six rows. | 43376b8 (#66) |
| `createAction` | response | `broadcast.hint.when` | The note's time, RFC 3339. | 43376b8 (#66) |
| `createAction` | response | `broadcast.hint.what` | `immediateBroadcastHint`, the note's kind. | 43376b8 (#66) |
| `createAction` | response | `broadcast.hint.outcome` | The post's outcome: `serviceError`, `orphanMempool`, `invalidTx` or `doubleSpend`. | 43376b8 (#66) |
| `createAction` | response | `broadcast.hint.attempts` | The request's attempts, this post counted. | 43376b8 (#66) |
| `createAction` | response | `broadcast.hint.words` | The broadcasters' words, a list of strings. | 43376b8 (#66) |
| `createAction` | response | `broadcast.hint.nextReaskMinutes` | Minutes to the re-ask on the cadence. | 43376b8 (#66) |
| `createAction` | response | `broadcast.inputs` | `locked`. | 43376b8 (#66) |
| `signAction` | response | `broadcast` | The same word on the signed template's post, with `createAction`'s sub-keys. | 43376b8 (#66) |
| `internalizeAction` | response | `broadcast` | The same word on the payment's post, with `createAction`'s sub-keys. | 43376b8 (#66) |
| `internalizeAction` | response | `broadcast.hint.words.error` | When the post itself errs (`internalize_action.rs`'s `serviceError` on a services fault), the toolbox writes the words as `[{"error": ...}]`, the fault's text. | 43376b8 (#66) |
| `listCertificates` | response | `certificates.keyring` | bsv-rs's `CertificateResult` carries it when set; no fixture's certificate has one. | the initial release, feb0dd0 |
| `proveCertificate` | response | `certificate` | bsv-rs's `ProveCertificateResult` carries it when set; fixture 159 proves an empty disclosure, but the toolbox leaves this optional member absent (049 still refuses a missing master keyring). | the initial release, feb0dd0 |
| `proveCertificate` | response | `verifier` | As `certificate`. | the initial release, feb0dd0 |

## Certificate disclosure at beta.2

| endpoint | request | served result | reading |
|---|---|---|---|
| `proveCertificate` | `fieldsToReveal: []` | 200 with `keyringForVerifier: {}`, even with a stored field and master key | Toolbox #240 closes the prior disclosure of every field; fixture 159 records the empty keyring, with no field recovery. |
| `proveCertificate` | a serial that decodes to 31 bytes | 400 `VALIDATION_ERROR` before field recovery | Toolbox #240 requires exactly 32 bytes; the served #311 argument validator enforces that bound earlier, recorded in fixture 160. Direct acquisition uses the same bound. |

## Host routes: 5 rows

The router's own routes beyond BRC-100 (`src/server/mod.rs`), not endpoint extensions: no BRC call is reached through them, and a BRC-100 client never needs them. Their bodies are the host's and are not judged against `fields.json`; their shapes are the wire fixtures'. The MCP server's `chain_feed_status` and `tx_word` tools read `/chain-feed` and `/tx/{txid}`.

| route | method | what it answers | added |
|---|---|---|---|
| `/permission-audit` | GET | What a wallet with a human behind it would have prompted for: `count`, `entries` (method, originator, protocol, counterparty, satoshis). This server grants every originator; the audit records, it does not refuse. | d42ee8d (2026-08-14) |
| `/permission-audit/reset` | POST | Clears the audit: `ok`. | d42ee8d (2026-08-14) |
| `/chain-feed` | GET | The daemon's chain-event feed: `status`, `cursor`, `frozenCount` (0.11.0); any store fault from those reads is 500 `ERR_CHAIN_FEED` with its message unless busy by type, which is 503 `STORE_BUSY` with `Retry-After: 1` ([#203](https://github.com/Calhooon/bsv-wallet-cli/issues/203), [#227](https://github.com/Calhooon/bsv-wallet-cli/issues/227)). | 0.9.0 (#26) |
| `/tx/{txid}` | GET | The tracker's stored word for one transaction: `txid`, `word`, `height`, `reask`, `evidence`; 404 for an unknown txid; 500 `ERR_TX_WORD`; 503 `STORE_BUSY` with `Retry-After: 1` when the store is busy (#203). | 0.11.0 (e891c4b) |
| `/arc-callback` | POST | The ARC/Arcade proof-delivery webhook, behind the callback token, not the wallet's bearer: `ok`, `action`; 404 `CALLBACK_DISABLED` without a token configured; 503 `STORE_BUSY` with `Retry-After: 1` for a typed store busy (#226); other payload faults keep 400 `BAD_PAYLOAD` and their messages. | 9b0d436 (#6, 2026-07-14) |

The callback relay is a binary of its own (`bsv-wallet-relay`, `src/relay.rs`): `GET /health`, `POST /arc-callback`, `GET /pull`, `POST /register`. The MCP server's `health`, `pull` and `register` tools reach them on `RELAY_URL`. They are not the wallet's routes and no wire fixture calls them.

## Privileged requests: refused (#145)

BRC-100 names `privileged` and `privilegedReason` on `getPublicKey`, `encrypt`, `decrypt`, `createHmac`, `verifyHmac`, `createSignature`, `verifySignature`, the two key-linkage calls and the certificate calls: `privileged: true` asks for the wallet's secondary, privileged keyring (line 42; line 325: "when implemented, a secondary and more secure set of keys is used instead of the primary ones"), and `privilegedReason` is the justification that accompanies it (lines 970 and 971: "required if this is a privileged operation").

This wallet holds one keyring and no privileged key. The field is read, never ignored: on the seven calls above and the two key-linkage calls, `privileged: true` is refused with 501 `PRIVILEGED_NOT_SUPPORTED`, body `{"code": "PRIVILEGED_NOT_SUPPORTED", "message": "<call>: privileged is not supported: this wallet holds no privileged keyring, and a privileged request is never answered with the everyday key"}`, before any key is derived and before the call is recorded in the permission audit. Through 0.18.1 the seven calls dropped the field and the two key-linkage calls passed it to the toolbox, which drops it too, so a privileged request was answered with the everyday key. BRC-100 has no refusal code for a capability the wallet does not implement (its vocabulary at lines 315 to 317 is `ERR_MISSING_PARAMETER`, `ERR_INVALID_PARAMETER_VALUE`, `ERR_CONFLICTING_PARAMETERS`), so the code is named in the style of this server's others; 501 is HTTP's "does not support the functionality required". `privileged: false`, `null` or absent is the call as before, byte for byte. A non-boolean `privileged` (`"true"`, `1`) is a 422, the framework's text naming the field's path (`privileged: invalid type: ...`), on all nine calls; through 0.18.1 the seven calls dropped it and answered 200 with the everyday key. `privilegedReason` without `privileged: true` is read and ignored, as the BRC reads it: it accompanies a privileged request and changes nothing without one. The refusal stands until this wallet holds a privileged key. The certificate calls take bsv-rs's own argument types and are not refused: `proveCertificate` reads `privileged`, and the toolbox at 0.13.0 passes it to the permission check only (and to its certificate lookup, which drops it), then builds the verifier's keyring with the everyday key (`wallet.rs:2511`, `create_keyring_for_verifier`); `acquireCertificate` carries it unread. Their four rows are gaps, bsv-wallet-cli #194 (the toolbox's half is bsv-wallet-toolbox-rs #142).

`seekPermission` stays unread on every call that names it: this server grants every originator (the permission audit records what a wallet with a human behind it would have prompted for; it does not refuse), so there is no permission to seek.

## The transport

What BRC-100 leaves to the realisation (line 298; its ABI, lines 1533 on, is a binary framing this server does not serve) and this server answers over HTTP:

- Each call is `POST /<call>` with the args as the JSON body; the five calls with no args (`isAuthenticated`, `waitForAuthentication`, `getHeight`, `getNetwork`, `getVersion`) also answer `GET`, for curl and the CLI.
- The BRC's `originator` parameter is the `Origin` header (its host) or the `Originator` header; a call that needs it and has neither is a 400, mirroring the reference's Node HTTP substrate ([keeper reading #298](https://github.com/Calhooon/bsv-wallet-cli/issues/298)). Each served wallet call passes that value as `Some(&originator)` at beta.2.
- An optional bearer (`AUTH_TOKEN`) guards every route but `/arc-callback`: 401 `UNAUTHORIZED`.
- `Retry-After` on a 503: `1` for `STORE_BUSY`, `2` for `HEADERS_UNAVAILABLE` (the header service's fault, 0.18.0, #59); CORS exposes it.
- The refusal codes and their statuses are listed in `docs/WIRE.md` and judged by `tests/wire.rs`.

## Deviations on the owner's word

None.

The rule: we stay with BRC-100 at the pin. A change to a BRC-100 shape (a field renamed, moved, retyped or dropped, beyond adding a listed extension) is proposed as a named deviation with its measurement (what it saves or fixes, measured) and lands only on the owner's word, recorded here with the date of the word, the measurement and the PR. An entry above in "Extensions" or "Gaps" that is a disagreement with the BRC is not a deviation: it is a defect or a choice awaiting its decision (#122), recorded as served.
