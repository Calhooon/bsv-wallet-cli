# The dependents of bsv-wallet-cli (#110)

Who consumes this binary, what each one calls, the version it runs, who answers for it, and the order in which a release reaches it. The dependents take the wire, not the crate (dolphinmilk is the one exception, and it reaches the wire through the crate too). A change one of them would feel fails `cargo test --test wire_consumers` and names the consumer: `tests/wire_consumers/<consumer>/` holds one recorded exchange per call shape each consumer sends, replayed against the router on every CI run (the `Test` job runs every test).

`[X]` read by this lane at the path given (2026-10-23); `[D]` told, not checked. Versions move: the keeper's versions page (`bsv-stack-lean/docs/p0/versions-2026-10-10.md`) is the record, and this table is brought to it with each fixture change (`RELEASING.md` 1b).

## The table

| consumer | what it calls | version today | captain | canary | fixtures |
|---|---|---|---|---|---|
| **LOW's fleet** (`~/bsv/bsv-low`; the daemons on four hosts and the e2e seats) | HTTP from the app's `WalletClient` over @bsv/sdk 2.1.4 `HTTPWalletJSON` (POST for every call, `Origin`/`Originator` from the page, no bearer): `isAuthenticated`, `waitForAuthentication`, `getVersion`, `getNetwork`, `getPublicKey` (identity; BRC-29 `[2,"3241645161d8"]` `forSelf`), `createSignature` (`hashToDirectlySign`, `[2,"low escrow settle"]` or `[0,"low escrow"]`, `[0,"low seat"]`), `createHmac` (`[2,"low hand secret"]`), `listActions` (`includeLabels`, pages of 10,000), `createAction` (`randomizeOutputs: false`, `acceptDelayedBroadcast: false`), `internalizeAction` (`wallet payment`) [X]. The e2e route adds `allowPending: "received"` to a seat's `/createAction` in transit (`app/e2e/lib/allowPendingRoute.ts`) [X]; the scoped object `{"received": ["txid:vout"]}` is #63's ask, not yet sent by LOW code [X: no sender found]. The harness's fetch sends the bearer when `AUTH_TOKEN` is set (`app/e2e/lib/cli.ts`) [X]. The funder and the fleet drive the CLI: `--json address`, `balance`, `outputs`, `identity`, `send <addr> <sats> [--allow-pending received]` (reads `txid`, `beef`), `split --count N [--allow-pending received]` (reads `txid`), `receive <txid> [--vout N]` (reads `txid`, `vout`, `accepted`), and `fund`, `sync`, `tick`, `init`, `daemon --db --port` [D: the sweep's reading of `app/e2e/lib/funder.mjs`, `walletFleet.mjs`]; it reads `wallet.db` read-only through sqlite3 (`custodyRead` in the pin file) [X] | 0.17.1 (`e2e/wallet-cli.pin.json`, the one pin; toolbox 0.12.0, tracker 0.8.0, bsv-rs 0.8.1) [X] | CAP (`.cca/project.toml`) [X] | canary: stays on the previous version until a release has soaked a day; a pin move runs the custody and spend-guard canaries on every host before it moves (the pin file) [X] | `low-fleet/`, 12 |
| **The stack's witness spend wallet** (`bsv-stack-lean`, the daily job) | The CLI only: `--db wallet.db address`, `--json split --count 2` (reads `txid` from stdout and `broadcast accepted name=` from stderr), `balance`, under `SPEND_DEPTH=1` and a header-service URL; its daemon `--db wallet.db daemon --port <n>`; the one HTTP read is `GET /tx/{txid}` on loopback [D: the sweep's reading of the job script and `docs/p0/witness-spend.md`] | 0.17.2 (the binary answers `bsv-wallet 0.17.2`) [X]; skips 0.18.0 by the stack's ruling [D] | the keeper | canary: stays on the previous version until a release has soaked a day | `stack-witness/`, 1 |
| **The x402 skill** (calgooon-x402 1.1.4 at `dc00cde`, upstream `main` on 2026-10-19; read 2026-10-23) | `lib/metanet.py`'s `_call`, byte-identical from 1.1.1 (`2c23b25`) to 1.1.4 [X: `git diff 2c23b25 dc00cde -- skills/x402/lib/metanet.py` is empty]: POST `{X402_WALLET_URL}/{method}`, `Origin: http://localhost`, JSON, no bearer: `getPublicKey` (identity; BRC-29 `[2,"3241645161d8"]` for the server, `forSelf: false`), `createSignature` (`[2,"auth message signature"]`, `data` as numbers), `createAction` (payment: `options` `acceptDelayedBroadcast`, `randomizeOutputs` both false; `execute-action`: the caller's outputs and options, `labels: ["app:x402"]`), `internalizeAction` (refund, `wallet payment`), and since `223bed3` (2026-10-11, after 1.1.1) `verifySignature` from `lib/auth_request.py` `verify_response` (the server's signed response: `data` and `signature` as numbers, `[2,"auth message signature"]`, keyID `"<server nonce> <client nonce>"`, the server as counterparty; reads `valid`, and any fault as "BAD SIGNATURE"); `SKILL.md`'s health check `GET /isAuthenticated`. 1.1.4's other changes (Retry-After on a signed 503, bounded 503 retries, saved-payment recovery) are between the skill and the paid server, not calls to this wallet [X: `git grep _call dc00cde -- skills/x402`]. Reads `publicKey`, `signature` (numbers), `txid`, `tx` (numbers, or an object of numeric keys), `accepted`; a non-2xx is read as `message` or `error` [X] | the wallet behind `X402_WALLET_URL` (default :3321, the MetaNet Client) [X] | the owner [D] | after the canary day | `x402-skill/`, 8 |
| **The MCP companion's clients** (`mcp/`, bsv-wallet-mcp 0.3.0, in this repository) | 34 tools: the 28 BRC-100 calls as typed parameters posted in struct order with `Origin` from `WALLET_ORIGIN` (default `http://localhost`), status calls on GET; `wallet_balance` pages `listOutputs` (`basket: default`, `include: locking scripts`, 100 at a time); `create_action`'s `allowPending` is a string only (the scoped object cannot be sent through it); `relinquish_output` sends `{basket, output: {txid, vout}}`; `chain_feed_status` and `tx_word` GET the host routes; `health`, `pull`, `register` go to the relay, not this server [X] | 0.3.0 with 0.19.0-beta.4 | this repository's captain | ships with the binary | `mcp/`, 11 |
| **The apps' x402 payers** (`~/bsv/agents`, `~/bsv/zanaadu`, `~/bsv/teragunv2`) | agents: `tools/fleet/proof.py` through the x402 helper (the skill's shapes), plus `createAction` with `noSend: true` and `abortAction {reference}` [D: the sweep]. Zanaadu: `WalletClient` over @bsv/sdk 1.10.4 `HTTPWalletJSON` when `VITE_WALLET_URL` is set: `getPublicKey [2,"zanaadu identity"] "1"`, `createSignature`, `createAction` (`randomizeOutputs`, `acceptDelayedBroadcast` false); its e2e runs `daemon`, `serve`, `--json address` [D: the sweep]. teragunv2: @bsv/sdk 3.2.1 `WalletClient` (auto substrate, aimed at :3321): BRC-29 under `[2,"wallet payment"]` (not `3241645161d8`), `createAction` with `acceptDelayedBroadcast: false` only [X] | agents: 0.7.3, the proof wallet parked [D]; Zanaadu: 0.7.3 (`e2e/fleet-line.json`) [X]; teragunv2: none recorded [X] | agents captain [X]; Zanaadu: CAP (`.cca/project.toml`) [X]; teragunv2: none recorded [X] | after the canary day | `app-payers/`, 6 |
| **dolphinmilk** (`~/bsv/dolphinmilk`; also `~/bsv/rust-bsv-worm`) | The crate: `bsv-wallet-cli = "0.1.23"` behind its default `embedded-wallet` feature [X]; `server::make_wallet_state_from_arc`, `ServerConfig::default()`, `server::run` in-process [D: the sweep's reading of `src/main.rs`]; then HTTP from `src/wallet/http.rs`: `Origin` from its config, no bearer; GET `isAuthenticated`, `getHeight`, `getVersion`; POST `getPublicKey`, `createSignature`, `createAction` (`acceptDelayedBroadcast`, `randomizeOutputs` false), `internalizeAction`, `listOutputs` (the MCP's balance pages) [D: the sweep] | 0.1.23 (`Cargo.toml`) [X]; last commit 2026-04-17 [D] | none recorded [D] | not on the line; a crate API change reaches it only on its own re-pin | `dolphinmilk/`, 3 |
| **The e2e harness** (`tests/e2e/lib/wallet-client.js`, this repository) | POST with `Origin: http://e2e-test`: its own key spellings `protocolId` and `keyId`, and the BRC certificate `type`; `createAction` with `allowPending` (`"received"` or the scoped object, scenario 23) and `options` `signAndProcess`, `acceptDelayedBroadcast`, `randomizeOutputs`, `noSend`; `listOutputs` with `includeEnvelope: false`, `limit: 10000`; `relinquishOutput {basket, output: "txid.vout"}` (#119) [X] | 0.19.0-beta.4 | this repository's captain | ships with the binary | `e2e-harness/`, 7 |

## The canary order

1. The stack's witness spend wallet and LOW's fleet stay on the previous version until a release has soaked a day on a `vX.Y.Z-beta.N` tag (#106, the lines). A beta reaches them only after that day, on their captains' word.
2. The x402 skill, the apps' payers and dolphinmilk move on their captains' word after the canary day.
3. The MCP companion and the e2e harness live in this repository and move with it.

Rollback is the target symlink and the tag (#106).

## What the fixtures show

Every fixture is a consumer's request as its code builds it and the router's answer, normalized by the wire oracle's rules (#69, `docs/WIRE.md` on `lane/oracle-fixtures`: a fresh ciphertext, a held coin's age). The worlds are three: `fresh` (an empty wallet), `auth` (behind a bearer) and `pending` (two received coins, not yet mined, as a funder's sweeps leave them). Spends answer before a broadcast: 402 in an empty wallet; in `pending` without the word, 402 with both coins `held`; with the word or the scope, the gate offers the coin and the build stops at signing (the seeded script is deliberately not the coin's key, so the answer is byte-stable). That answer, a 400 `ERROR` carrying the toolbox's signing message, is a known wrong shape pinned as today's: typed errors (#81) and the fixtures oracle (#69) fix it, and `low-fleet/09` and `10` change with that fix in its own commit. The list of fixtures is `CONSUMERS` in `tests/wire_consumers.rs`: a fixture deleted or renamed, or a file no case names, fails the replay naming the file. So a renamed request field, a moved word, a changed refusal body or a changed key derivation each fail with the consumer's name.

What no fixture covers: a successful spend's body (`txid`, `tx`, `noSendChange`), whose txid moves with the change's fresh derivation; and the CLI under `--json`, which LOW's funder and the stack's witness wallet drive. Both are owed: a spend's body needs a normalizer rule for its own txid, the CLI a recorder of its own (the commands live in the binary, not the library, so the replay cannot drive one in-process; a recorder would spawn `CARGO_BIN_EXE_bsv-wallet` as `tests/json_refusal.rs` does, with a fixture shape of its own: argv, environment, stdout, stderr, exit code). Also owed: a `verifySignature` that verifies (`x402-skill/08` is the refusal; a valid one needs the server's signature made in the test).

## Found while recording (not fixed here)

- `relinquishOutput` answers `{relinquished: true}` for an outpoint the wallet does not hold (`e2e-harness/06-relinquishOutput.json`, `mcp/09-relinquish_output.json`): the toolbox's update matches no row and the answer does not read the count, and the update does not match on the named basket. The harness's 422 that this bullet named is fixed (#119).
- The SDK's `HTTPWalletJSON` reads a refusal as 400 with `isError` and a numeric `code`; this server answers `{code: "<WORD>", message}` with the status by class (402, 409, 503, ...). LOW wraps the substrate (`AnsweringHTTPWalletJSON`, `app/src/lib/walletRefusal.ts`) [X]; Zanaadu and teragunv2 use it bare and see a generic error.
- teragunv2 derives its payment key under `[2,"wallet payment"]`, not BRC-29's `[2,"3241645161d8"]` [X]: a payee that derives by BRC-29 will not find the output. It is the app's choice to make.
- The MCP's `create_action` cannot send the scoped `allowPending` (its parameter is a string).
- The keeper's latest versions-page commit names "the CLI's 0.18.1"; no `v0.18.1` tag exists at origin today [X: `git ls-remote --tags`].

The beta.1 pin replays every consumer request unchanged. Wire fixture 103
now records the invalid-spend refusal and 106 no longer lists that rejected
action; 124 records abort's accepting status. All 14 recorded BEEFs retain
the same transaction sets and proofs, with no byte re-record required.
Version fixtures 007 and 008 retain `bsv-wallet-toolbox-0.1.0`. The directly
served SDK JSON types and the stored `InternalizeActionArgs` follow-up keep
their beta.1 serde shape at that release. This branch now pins bsv-rs
=0.9.0-beta.4, bsv-wallet-toolbox-rs =0.14.0-beta.4 and bsv-tracker
=0.9.0-beta.4 (#347). Ordinary refusals retain their reference classes (#330);
the toolbox's faults are typed and served by family since beta.4 (#342), so
LOW's two recorded `allowPending` creates (`low-fleet/09`, `10`), whose
answer is a signing fault, move from 400 ERROR to 500 INTERNAL_ERROR with the
same words; the generic untyped fault stays ERROR. The CLI-only --from
wrapper uses the must-spend list (#313). Discovery reads `TRUST_SETTINGS`
(#331, toolbox #388). The `v0.19.0-beta.3` announcement carries the consumer fixture
diff since `v0.19.0-beta.1`. The beta.2 replay moved no consumer fixture
([`926107c`](https://github.com/Calhooon/bsv-wallet-cli/commit/926107ce8473decd7136cc873b350059ec399dec),
with the corrected wire recording in
[`6ea286c`](https://github.com/Calhooon/bsv-wallet-cli/commit/6ea286c389afd829a8fb19f8ca6cb3881c9d9c1e)).
The beta.3 replay moved exactly three refusals in
[`d633a98`](https://github.com/Calhooon/bsv-wallet-cli/commit/d633a98315bb7a1d39899bf390c7692e8e41d6d2):
`app-payers/02-agents.abortAction.json` changes an unknown reference from
404 `NOT_FOUND` to 400 `VALIDATION_ERROR`;
`low-fleet/11-internalizeAction.potPayout.json` and
`x402-skill/07-internalizeAction.refund.json` change `INVALID_BEEF` to
`VALIDATION_ERROR`, both still 400. All three lose the SDK's former
`wallet error: ` prefix. Their requests and the other 45 consumer fixtures
are byte-identical across that replay. Separately, since beta.1,
[`4a8b4e1`](https://github.com/Calhooon/bsv-wallet-cli/commit/4a8b4e1f5bf1b74a9e66a6caf053974896c52d4e)
records the e2e harness's corrected `relinquishOutput` request and its 200
answer (#119). The captains' reads of 2026-10-28 [D: via the keeper]: LOW
has no matcher, its funder reads the class; bsv-dd has no matcher, its gift
sender is on 0.18.2; the x402 skill's captain is the owner.
The `v0.19.0-beta.4` announcement carries the consumer fixture diff since
`v0.19.0-beta.3`. The beta.4 replay moved exactly two refusals in
[`9361897`](https://github.com/Calhooon/bsv-wallet-cli/commit/936189722848a6fac1b19e86513131bd93a6c0bf):
`low-fleet/09-createAction.allowPending.json` and
`low-fleet/10-createAction.scoped.json` change 400 `ERROR` to 500
`INTERNAL_ERROR`, the words unchanged (#342, the toolbox's typed fault by
family). Their requests and the other 46 consumer fixtures are
byte-identical across that replay. The wire corpus grows from 174 to 176
fixtures: 175 and 176 are new under the trust settings (#331).
The #122 divergence rows (items 3 to 7, after `v0.19.0-beta.4`) move
exactly two consumer fixtures: `dolphinmilk/01-getVersion.json` and
`low-fleet/02-getVersion.json` answer `bsv-wallet-cli-<major>.<minor>.<patch>`
(the corpus's shape of `bsv-wallet-cli-0.19.0`) where they answered
`bsv-wallet-toolbox-0.1.0`. LOW's gate logs the string as a reachability
probe and parses nothing (`app/src/components/ConnectGate.tsx:376-382`) [X];
dolphinmilk's GET reads it [D: the sweep; its checkout is not on this host]. Their requests and the other 46 consumer
fixtures are byte-identical. What no consumer fixture records moves too, and
its captains should know it: the framework's rejections (a body that does
not parse, a missing or mistyped field) are `{code, message}` at 400
`VALIDATION_ERROR` where they were text at 400 and 422; a JSON body under a
content type that is not JSON is the call where it was a 415 in text, and a
request with no content type is the call with `{}`; `signAction`'s unknown reference is 400 `VALIDATION_ERROR` where it
was 501 `NOT_IMPLEMENTED`; and an `allowPending` value other than
`"received"` or the scoped object is 400 `VALIDATION_ERROR` where it was
quietly off (the MCP's string parameter reaches the same door; the CLI's
flag never took another word).
External consumers stay on their recorded
versions until their captains move them.
