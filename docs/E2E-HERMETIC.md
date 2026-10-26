# The hermetic e2e run (#74)

`E2E_HERMETIC=1 node tests/e2e/run.js` runs the 24 e2e scenarios with nothing
leaving the machine and no sat moving. The mainnet run (`node tests/e2e/run.js`
against the funder daemon on :3320) is unchanged and stays the release gate;
the hermetic run is the gate of every PR (the `e2e-hermetic` job of
`.github/workflows/ci.yml`).

The stack's rule 10 binds it: no network except reads, oracles run locally.
The node's harness is the pattern (`bsv-script-lean/harness`): the evaluator is
the oracle, the corpus is the judge. Here the stub is the oracle of inclusion
(what the chain holds, which block, which proof) and the scenarios are the
judge; the wallet binary is what is judged.

## The pieces

**The stub** (`tests/e2e/stub/`, Node, no package beyond Node's own) is one
HTTP process, `node tests/e2e/stub/server.js --port 3380`, that plays every
service the binary and the auditor leave the machine for, over one in-memory
chain:

| prefix | plays | pointed at by |
|---|---|---|
| `/chaintracks` | the header service | `CHAINTRACKS_URL` |
| `/woc` | WhatsOnChain's shape | `WOC_BASE` (the CLI's own rungs), the auditor |
| `/bitails` | Bitails' shape | `BITAILS_BASE` (the CLI's own rung), the auditor |
| `/bananablocks` | BananaBlocks' shape | `BANANABLOCKS_BASE` (the toolbox's provider and the CLI's rung) |
| `/arc` | a classic ARC broadcaster | `ARC_URL`; `ARC_GORILLAPOOL_URL=` (empty) leaves the second ARC out |
| `/_stub` | the harness's own routes | `tests/e2e/lib/hermetic.js` |

The chain starts from twelve recorded, linked mainnet headers
(`tests/e2e/stub/fixtures/headers.json`, read from the header service on
2026-10-23, heights 972370 to 972381, each checked to hash to its `hash` at
load). The stub mines on top of them: a block on demand (`POST /_stub/mine`)
takes the mempool behind a coinbase, links to the tip, and carries the merkle
root of its transactions; a header the wallet asks for by height or hash is
answered from this chain. No proof of work is checked by anything, and none is
done.

The mempool takes every well-formed transaction (raw, Extended Format, or the
subject and ancestors of a BEEF) whose inputs no other held transaction spends.
A second spend of an input is answered as ARC answers it
(`DOUBLE_SPEND_ATTEMPTED` with `competingTxs`), not taken, so a wallet that
spends a coin twice is seen. No signature or script is checked: the wallet is
the signer and the stub is the oracle of inclusion, not of validity.

**The wall.** The toolbox's own WhatsOnChain and Bitails providers do not take a
base URL (`ServicesOptions` has `bananablocks_base` alone), and a fault in a
rung the stub serves would send the binary to the next real host. So the run
sets `HTTPS_PROXY`, `HTTP_PROXY` and `ALL_PROXY` to the stub and `NO_PROXY` to
`127.0.0.1,localhost`, and every request from the binary and the harness
becomes a `CONNECT host:443` at the stub (or, for plain HTTP, an absolute-URI
request), which the stub refuses with 403 and counts under `leaks`:

- the binary's clients are reqwest's, which read the proxy environment
  (hyper-util's system matcher);
- the harness's Node reads it under `NODE_USE_ENV_PROXY=1` (Node 24, 22.21),
  which `lib/hermetic.js` sets for every child and CI sets for the harness
  itself; since a Node reads it at start, the harness also refuses its own
  fetch to anything but the loopback before it is made and counts it as a
  leak, on any Node;
- Playwright's Chromium (scenario 20) reads no proxy environment and is given
  `--proxy-server` in its launch args under the flag; it bypasses the loopback
  itself, so the inspector on localhost is reached direct. (Playwright's
  `proxy` launch option is not used: it adds `<-loopback>` and sends
  localhost to the wall.)

The binary sees a fault ("could not look", never a verdict) and the run sees
the host in its stats. A walled attempt at any target other than the two the
toolbox gives no knob for (below) fails the run, as does a stats read that
fails and any route the stub does not play.

What the wall is not: an OS-level block. The runner's egress is open; a client
that ignores the proxy environment (a raw socket, a library with its own
resolver) would reach the network unseen. Blocking egress at the OS
(`unshare -n` for a loopback-only namespace, or a firewall rule) changes how
the job runs its step and is its own concern: #136.

**The funder** is a third stub-backed wallet the harness starts on :3373
(`FUNDER_PORT` moves it; 3320 is refused) in a scratch directory. The stub mines
a coinbase paying the funder's BRC-29 address (`POST /_stub/fund`), advances 99 more stub blocks for its next-block
coinbase maturity, and the funder internalizes its AtomicBEEF: the wallet checks the BUMP's root against the
stub's header for that height (`/findHeaderHexForHeight`), as it would against
the real header service. The check is shown to run: the harness then offers a
forged coinbase (`POST /_stub/forge`: a BUMP claiming it alone in the recorded
block at the last fixture height, whose root is not that header's) and the
run is fatal unless the funder refuses it and its balance does not move. The proof itself is not stored: the toolbox's proof
gate (`max_acceptable_proof_height`, written by the monitor) is closed under
`serve`, so the coin is a received unproven coin and the funder spends it under
`allowPending: received`, exactly as every coin of the mainnet run is spent.
`bsv-wallet tick` in the funder's directory would open the gate; the chaos suite
(#97) may want that.

## The routes the stub serves

Each answers in the shape the toolbox's provider or the CLI's rung parses
(`bsv-wallet-toolbox-rs` 0.14.0-beta.1 `src/services/providers/*.rs`,
`src/explorers.rs`, `src/broadcast_verify.rs`). A fault injected for a provider
(`POST /_stub/fault`) is served before any of its routes.

`/chaintracks` (`{"status":"success","value":...}` frames; a header is
`{version, previousHash, merkleRoot, time, bits, nonce, height, hash}`):
`GET /getChain` (main), `GET /getHeaders?height=START&count=N` (concatenated
80-byte headers, including the eleven ending at the tip),
`GET /getPresentHeight`, `GET /findChainTipHeaderHex`,
`GET /findHeaderHexForHeight?height=N` (404 and an error frame off the chain),
`GET /findHeaderHexForBlockHash?hash=H`. The chain-feed SSE is not served:
`serve` does not subscribe (only `daemon`'s tenant loop does), and the harness
runs `serve`.

`/woc`: `GET /tx/{txid}` and `GET /tx/hash/{txid}` (the auditor's and the
verifier's chain index; `confirmations` 0 in the mempool), `GET /tx/{txid}/hex`,
`GET /tx/{txid}/beef` (a BEEF V1 of the transaction, its unmined ancestors and
one BUMP per mined block), `GET /tx/{txid}/proof/tsc` (a one-element TSC array
with `target` the block hash; an empty 200 while unmined),
`GET /tx/{txid}/{vout}/spent` (`{txid, vin}` or 404), `POST /tx/raw`
(`{"txhex"}`), `POST /txs/status` (`{"txids"}`),
`GET /script/{hash}/unspent/all`, `GET /script/{hash}/confirmed/history`,
`GET /script/{hash}/unconfirmed/history`, `GET /block/{hash}/header`,
`GET /exchangerate` (a fixed 50 USD).

`/bitails`: `GET /tx/{txid}` (`blockHash`, `blockHeight`), `GET /tx/{txid}/hex`,
`GET /tx/{txid}/proof/tsc` (one TSC object), `POST /tx/broadcast/multi`
(`{"raws"}`), `GET /block/{hash}` (`{height, header}`, the 80 bytes hex),
`GET /scripthash/{hash}/unspent` (`?limit&from`),
`GET /address/scripthash/{hash}/history`.

`/bananablocks`: `GET /tx/{txid}` (`block_hash`, `block_height`, null while
unmined), `GET /tx/{txid}/hex`, `GET /tx/{txid}/proof` (`targetType:
"merkleRoot"`), `GET /txo/{txid}/{vout}` (`script_pubkey`, `spent`,
`spent_txid`), `GET /address/{address}/utxos`.

`/arc`: `POST /v1/tx` (bytes as `application/octet-stream`, BEEF, EF or raw
told apart by their leading bytes; or JSON `{"rawTx": hex}`; answers
`txStatus: SEEN_ON_NETWORK`, `MINED` with `merklePath` once mined,
`DOUBLE_SPEND_ATTEMPTED` with `competingTxs`, or a 400 problem document for
bytes that do not parse), `POST /v1/txs` (a JSON array of `{rawTx}`, one answer
each), `GET /v1/tx/{txid}` (the status, 404 when unknown), `GET /v1/policy`.

`/_stub`: `GET /stats` (calls per provider and route, `leaks`, `unserved`, the chain),
`GET /chain`, `POST /mine` (`{txids?}`), `POST /fund` (`{script, satoshis}`:
a coinbase mined alone, its AtomicBEEF returned), `POST /fault`
(`{provider, status, retryAfter?, count?, body?}`: the next `count` requests to
that provider answer `status`, with `Retry-After` when given), `DELETE /fault`,
`POST /forge` (`{script, satoshis}`: the forged negative of `/fund`, nothing
held), `GET /beef/{txid}` (the AtomicBEEF of anything held), `GET /tx/{txid}` (the
stub's own view: block, outputs, spenders).

The TSC proof of a transaction mined alone has no nodes, which the toolbox's
converter refuses; every block the stub mines with the mempool has a coinbase,
so a wallet transaction's proof always has a node. The lone coinbase of a
funding block is proven by the BUMP inside its BEEF instead.

## Adding a route when the binary learns a new one

1. Find the parser: the toolbox provider's `format!("{}/...", base_url)` or the
   CLI's route, and the struct or `serde_json::Value` fields it reads.
2. Add the route to that provider's method in `tests/e2e/stub/server.js`
   (`woc`, `bitails`, `bananablocks`, `arc`, `chaintracks`), answering from
   `Chain` (`tests/e2e/stub/chain.js`); add to `Chain` what the route needs.
3. A new provider is a new prefix: add it to `PROVIDERS`, `bases()`, and the
   environment `lib/hermetic.js` forces; if the binary has no knob for its base,
   add one in `src/services_env.rs` (the brief's one allowed `src/` change) or
   let the wall count it and say so in the report.
4. Run `E2E_HERMETIC=1 node tests/e2e/run.js`; the stats at the end list every
   route each provider answered, every walled attempt and every unserved
   route. A route the binary asked that the stub lacks is answered 501
   (`stub: unserved route <method> <path>`), never a 404 the binary would read
   as a clean absent, and is counted under `unserved`; the run fails on any.
5. List it here.

## What is not hermetic, and why

- **The toolbox's WhatsOnChain and Bitails providers** keep their default bases
  (`api.whatsonchain.com`, `api.bitails.io`): `ServicesOptions` takes no
  `whatsonchain_base` or `bitails_base` (toolbox #98 asks for the knobs). So
  in the toolbox's ladders (`bsv-wallet-toolbox-rs` 0.14.0-beta.1,
  `Services::with_options`) these rungs never run hermetically today; each
  reaches the wall, is refused, and the ladder moves on (to BananaBlocks,
  which the stub serves, or to the stub's ARC):
  `getMerklePath` (WhatsOnChain, Bitails), `getRawTx` (WhatsOnChain, Bitails),
  `postBeef` (the Bitails and WhatsOnChain broadcasters behind the ARCs),
  `getUtxoStatus` (WhatsOnChain, Bitails), `getStatusForTxids` (WhatsOnChain,
  Bitails), `getScriptHashHistory` (WhatsOnChain, Bitails), the BSV exchange
  rate (WhatsOnChain), and the break-glass header fallback by hash
  (WhatsOnChain, Bitails; off unless `BREAK_GLASS_EXPLORER_HEADERS`, which the
  run drops). The harness names these two walled targets,
  `CONNECT api.whatsonchain.com:443` and `CONNECT api.bitails.io:443`
  (`KNOWN_WALLED` in `lib/hermetic.js`), and tolerates walled attempts at them
  only; a walled attempt at any other target fails the run, so a new leak
  elsewhere is seen. The 24-scenario run under `serve` makes none
  (walled 0): the CLI's own rungs, which the stub serves, answered ahead of them. When
  #98 lands, `services_env.rs` passes `WOC_BASE` and `BITAILS_BASE` through
  and `KNOWN_WALLED` empties.
- **Overlay discovery** (`discoverByIdentityKey`, `discoverByAttributes`): the
  toolbox builds `HttpLookupResolver::mainnet()` inline, whose SLAP tracker
  hosts are bsv-rs defaults, and the wallet takes no resolver of the caller's.
  Scenario 06 skips its discovery step under the flag and prints why; the rest
  of the certificate lifecycle runs. The stack keeper's item: a resolver or
  tracker list the wallet takes (`HttpLookupResolver::with_endpoints` exists),
  then the stub plays a SLAP tracker and `ls_identity` host.
- **The chain-feed SSE** (`daemon`'s tenant loop): not subscribed by `serve`,
  not served.
- **Arcade V2** (`ARC_MODE=arcade`): the harness drops the mode; the stub plays
  classic ARC only. Arcade's `/tx/{txid}` status route and SSE stream are not
  served.
- **Proof of work and signatures**: not checked by the stub, not needed by the
  scenarios (the wallet checks a BUMP's root against the header, which the stub
  honours; the pinned toolbox runs bsv-rs's script verifier at internalize;
  the stub itself remains an inclusion oracle).
- **Scenario 20's browser checks** need Playwright's Chromium; without it the
  scenario's API checks run and the browser checks are skipped by the scenario
  itself. CI installs Chromium; under the flag it runs behind the wall by its
  launch args (above).
- **A scenario that cannot be hermetic** says `hermetic: false` with a
  `hermeticReason` on its module; the runner skips it under the flag, prints
  the reason, and counts it as run. As of this writing no scenario needs it.

## Environment the hermetic run forces

`CHAINTRACKS_URL`, `ARC_URL`, `BANANABLOCKS_BASE`, `WOC_BASE`, `BITAILS_BASE`
at the stub; `ARC_GORILLAPOOL_URL=` (out); `HTTPS_PROXY`, `HTTP_PROXY`,
`ALL_PROXY` at the stub and `NO_PROXY=127.0.0.1,localhost` (both letter cases),
`NODE_USE_ENV_PROXY=1`; `E2E_RNG_SEED` is the same 32-byte seed for every
wallet and storage stream, refused by the binary without this complete wall;
`FUNDER_PORT` 3373 unless set (3320 refused); `E2E_WALLET_TMP` the system
temporary directory unless set (the wallets' directories, the stub's log and
CI's artifacts). Dropped: `WOC_API_KEY`, `BITAILS_API_KEY`, `TAAL_API_KEY`,
`MAIN_TAAL_API_KEY`, `ARC_MODE`, `ARCADE`, `BREAK_GLASS_EXPLORER_HEADERS`,
`PUBLIC_CALLBACK_URL`. `E2E_STUB_PORT` moves the stub (default 3380);
`BSV_WALLET_BIN` and `BSV_WALLET_MCP_BIN` name the binaries when they are not
under the repository's `target/release`.

## Busy-store coverage

Scenario 24 (`tests/e2e/scenarios/24-busy-store.js`, #204) requires the
`sqlite3` command-line tool on `PATH`. Missing it fails the scenario with the
tool's name. CI's `ubuntu-latest` image supplies it. The holder is a child
process with stdin kept open, so this works on Node 20 as well as CI's Node 24
without `node:sqlite`.

The served store is already open in WAL mode. An attempted
`PRAGMA locking_mode=EXCLUSIVE; BEGIN IMMEDIATE` returns SQLite code 5 before
it can hold that store. The scenario proves and logs this, then holds the WAL
writer with `locking_mode=NORMAL; BEGIN IMMEDIATE`, creates its own
`e2e_busy` table and inserts one row. It waits for the child's acknowledgement
after the write before asking any door. SQLite's [WAL locking
rules](https://www.sqlite.org/wal.html#use_of_wal_without_shared_memory)
allow a switch to exclusive mode when only one connection uses the shared
index; [normal WAL readers coexist with a
writer](https://www.sqlite.org/wal.html#concurrency).

| Door, in request order | While the writer holds | After release |
|---|---|---|
| `listOutputs` | 200 | 200 |
| `createAction`, unsigned | 503 `STORE_BUSY`, `Retry-After: 1` | 200, new template aborted |
| `abortAction`, kept reference | 503 `STORE_BUSY`, `Retry-After: 1` from the toolbox's abort write | 200, `aborted: true` |
| `GET /chain-feed` | 200 | 200 |
| `GET /tx/{txid}` | 200 | 200 |

The read doors' 503 is reachable in the unit and wire tests' `unix-dotfile`
VFS world, which opens the served store itself with different locking; this
scenario does not claim to reach it from outside an already-open WAL store.
The abort's #168 pre-check is a read and passes under this WAL writer; its
end-to-end busy refusal remains covered by the unit test and wire fixture 116.
The two busy answers each take about the toolbox's five-second busy ceiling.
The scenario adds about 12 seconds to `e2e-hermetic`, including the exclusive
attempt, rather than the roughly 25 seconds five blocked doors would need.

Before the lock, A builds an unsigned template paying its own address, with
an external input from the funder's existing change and its BEEF. It is never
signed or broadcast. Using an external input leaves A's sole coin available
for the spend-door check when scenario 24 runs alone. In a live run, the
funder read lists at most 50 coins and requests the BEEF for only the chosen
coin of at least 2,000 sats. The hermetic read is unchanged. A temporary local
`unknown` tracker row, only if absent, lets the transaction view read an
existing txid; the row is removed afterward without asserting a proof or a
mined word. Under the hermetic flag the stub mines that existing transaction;
the live run does not ask a miner or run a tracker pass. Both templates are
aborted and A's balance is unchanged: 0 sats.

The log names the holder's PID, file, SQL, acquisition time, held duration and
every answer's status, retry header and elapsed time. A `finally` rolls back,
closes stdin, awaits the child and asserts exit 0. A second `sqlite3` process
then probes the write side with `PRAGMA busy_timeout=2000; BEGIN IMMEDIATE;
ROLLBACK;`, bounded by five seconds. The two-second busy timeout tolerates
the server's own brief writes. A missing exit or failed probe fails the
scenario naming the outlived lock and holder PID, and kills a lingering
holder before cleanup or any later door. The log says "released" only after
the holder has exited and the write probe has succeeded. The table and its
row roll back with the transaction. The shared GET keeps beta's contract:
it returns the parsed body even for a non-2xx response. Scenario 24 alone
uses `getResponse` to read
`{ status, retryAfter, body }`; POST errors retain all three fields;
other scenarios and the response bodies are unchanged.

## The six tests

T1 (scale with our use): the stub is in-memory and local; a run costs nothing
and can run on every PR. T2 (headers and a proof): the wallet verifies the
funding BUMP against a header the stub serves, the same check as on mainnet.
T3 (fast and cheap): no sats, no explorer rate limits, no network latency. T4
(the rule in script or math): the merkle root, the BUMP and the TSC fold are
computed, not recorded. T5 (fewer trusted words): a walled attempt outside the
two named targets, an unserved route or unread stats fails the run, and the
seed's root check is shown to refuse a forged root; the stub's double-spend answer is derived from its own
spends, not asserted. T6 (executable): `E2E_HERMETIC=1 node tests/e2e/run.js`.

At beta.1 the verifier asks the header service for its network and eleven
linked headers to compute median time past. The initial range is drawn from
the twelve real headers already recorded above; subsequent ranges include
the stub's mined headers. `PromiseAll` records every named broadcaster, so
WhatsOnChain and Bitails' unconfigurable broadcast bases can reach the
known proxy wall even after ARC accepts. Their counts remain in the
differential recordings; no explorer rung moves.

Each CLI invocation gets a distinct seed derived from the harness seed and
its command index. Reopening a store with the server's initial stream would
repeat action references; paired runs replay the same per-command sequence.

The BEEF V2 decoder reads the format-1 proof index before the transaction
(BRC-96). `node --test tests/e2e/stub/tx.test.js` checks it against the same
transactions and proof as a recorded V1 body. This keeps signAction's
broadcast readable by the inclusion oracle without changing its bytes.
