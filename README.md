# bsv-wallet-cli

A self-hosted BSV command-line wallet and BRC-100 HTTP server, with SQLite
storage, signing, broadcasting and transaction tracking. The release of record
is the tag **v0.18.1**, and crates.io carries the same version, **0.18.1**; the
repository's Releases page holds a Release per tag. It pins
**bsv-wallet-toolbox-rs =0.13.0**, **bsv-rs =0.8.2** and **bsv-tracker =0.8.0**;
[Cargo.lock](Cargo.lock) carries those versions.

## Install and inspect

Install the registry release into an explicit tool directory:

```sh
cargo install bsv-wallet-cli --version 0.18.1 --locked --root ./wallet-tools
./wallet-tools/bin/bsv-wallet --help
./wallet-tools/bin/bsv-wallet respend --help
```

The package includes `bsv-wallet` and the callback receiver `bsv-wallet-relay`.
The MCP companion is a separate workspace member in [mcp](mcp), not a binary
installed by the command above. [Cargo.toml](Cargo.toml) describes the workspace.

## The MCP companion

`bsv-wallet-mcp` (0.3.0, on rmcp 3.5) speaks MCP over stdio and calls the
wallet's HTTP server: the 28 wallet endpoints, `wallet_balance`, and five tools
over the host's own routes, one per route, no route added: `chain_feed_status`
(`GET /chain-feed`), `tx_word` (`GET /tx/{txid}`), and on the callback relay
`health` (`GET /health`), `pull` (`GET /pull`) and `register`
(`POST /register`, with the relay's admin bearer). Each tool's description
says what its word rests on: headers and a proof, or a hint.

It is a workspace member, so build it from the workspace root:

```sh
cargo build --release -p bsv-wallet-mcp
```

The binary lands in the workspace's `target/release/bsv-wallet-mcp` (or under
`$CARGO_TARGET_DIR/release`), not under `mcp/`. A client config, with the
paths and URLs your own:

```json
{
  "mcpServers": {
    "bsv-wallet": {
      "command": "/path/to/bsv-wallet-cli/target/release/bsv-wallet-mcp",
      "env": {
        "WALLET_URL": "http://localhost:3322",
        "WALLET_ORIGIN": "http://localhost",
        "RELAY_URL": "http://localhost:3390",
        "RELAY_ADMIN_TOKEN": "<the relay's admin token, only for register>"
      }
    }
  }
}
```

`RELAY_URL` and `RELAY_ADMIN_TOKEN` are optional; without `RELAY_URL` the
three relay tools answer that no relay is named and ask nothing.

## Use and configuration

The CLI offers initialization, address and balance reads, transaction creation,
internalization, outputs/actions queries and time-locked gifts. `daemon` runs
the monitor with the HTTP server; `serve` runs the server alone. Initialization
creates wallet state, while sends and gift actions can spend funds. Use the
command help for arguments before operating a wallet.

The server implements the wallet interface and serializes spending operations.
`serve`, `daemon` and `serve-fleet` refuse a non-loopback bind without
`AUTH_TOKEN`, unless the operator explicitly supplies `--allow-no-token`.
`CHAINTRACKS_URL` selects the header service. Broadcaster callbacks and status
feeds are hints until their evidence is checked. Configuration and defaults
are defined in [src/config.rs](src/config.rs): every variable is read once at
start, and a malformed number, address or URL is refused there, naming the
variable; every refused variable is named at once, `init` runs regardless,
and `bsv-wallet config` prints the table with the refused rows marked and
exits 1. `RELAY_POLL_SECS` (the daemon's poll of `RELAY_URL`, default 10)
is at least 1 second: 0 is refused at start, naming it.
`bsv-wallet config` prints every setting with its effective value
and its source (env, flag, default), keys and tokens as `<set>` (`empty` when
set to nothing), a URL's userinfo and query as `<set>`, and `BANANABLOCKS_BASE`
as the base in effect (the chain's default when unset); `--json` gives the same
as an array of `{name, value, source}`, `value` `null` for no value (`-`,
`unset`, `off`, `empty` in the table), `true` or `false` for a switch, a string
otherwise (`CHAINTRACKS_URL=off` is a value, the header service set off: the
string `off`; unset or blank it is `null`); `daemon` and `serve` log the same list as one line at start. [GIFT.md](GIFT.md) documents gift flows and limits.

The explorers ([src/config.rs](src/config.rs) reads them, [src/services_env.rs](src/services_env.rs) applies them; 0.17.0):

| variable | effect |
|---|---|
| `WOC_API_KEY` | WhatsOnChain key, sent as `woc-api-key` and `Authorization: Bearer`; optional |
| `BITAILS_API_KEY` | Bitails key; optional |
| `BANANABLOCKS_BASE` | BananaBlocks API base: unset is the chain's default (`https://bananablocks.com/api/v1`, `https://test.bananablocks.com/api/v1`), empty turns the rung off, a URL moves it; BananaBlocks takes no key and paces itself at 60 requests a minute |
| `WOC_BASE` | the base of the CLI's own WhatsOnChain rungs: unset or blank is the chain's default (`https://api.whatsonchain.com/v1/bsv/main`, `.../test`), a URL moves it (the hermetic e2e run points it at its stub); the toolbox's own WhatsOnChain provider takes no base |
| `BITAILS_BASE` | the base of the CLI's own Bitails rung: unset or blank is the chain's default (`https://api.bitails.io`, `https://test-api.bitails.io`), a URL moves it; the toolbox's own Bitails provider takes no base |
| `ARC_GORILLAPOOL_URL` | the GorillaPool ARC's base: unset is the toolbox's default (`https://arc.gorillapool.io` on mainnet, none on testnet), empty or blank leaves GorillaPool out, a URL moves it |

A set value of `BANANABLOCKS_BASE`, `WOC_BASE`, `BITAILS_BASE` or
`ARC_GORILLAPOOL_URL` that is not blank and not an `http` or `https` URL is
refused at start, naming the variable.

## The chain-event feed

`daemon` holds the header service's chain-event feed
([src/chain_feed.rs](src/chain_feed.rs)); the source is `CHAINTRACKS_URL`
(required, no default; `off` has no feed). On start it replays from its stored
cursor (0 at birth) through `GET /events?since=<cursor>` until `hasMore` is
false, then holds `GET /events/stream?since=<cursor>`. Every envelope goes
through the tracker's decoder and `Tracker::on_chain` with the new branch's
headers. A reorg names the transactions mined at or above its fork height, and
the tracker rechecks each by its stored proof's root against the active header
at its height (bsv-tracker 0.7.0, the stack's ruling (5) of 2026-10-20): the
same root, the word stands and nothing is fetched; a root that is gone, the
word is `stale` with the re-ask `reorg@<forkHeight>`, the only word the pass
asks a proof for; a height where the header service still answers a header
the reorg deactivated has no answer, so the word stands suspect with the
re-ask `recheck` until the next tip. A fork or a split marks them suspect and
moves no word; a tip or a resolved split rechecks the marked ones against the
headers; a freeze writes its outpoint to the frozen record,
`frozen_outpoints` (txid, vout, the envelope's cursor, when), which the spend
guard reads (never offered; nothing here clears a row, the charter's R6). The
cursor is stored after each delivery, in the same transaction as the tracker's
rows (`tracker_meta`, `feed_cursor`). The status object `GET /chain-feed`
serves (`status`: its `cursor`, `delivered`, `updatedAt`) is rewritten after
every delivery (N = 1), while replaying and while following, so `status.cursor`
equals the top-level `cursor` once the delivery's write lands.

A decode fault, a cursor ahead of the journal (409 `cursorAhead`), an
`event: error` from the feed or a refused request stops the loop at its
cursor, with no skip; the fault is in the daemon's status, `GET /chain-feed`
(`status.fault`: its kind, the cursor it stopped at, the offending cursor, the
text), with `frozenCount`, the outpoints in the frozen record. A restart resumes from the stored cursor. A stream that ends without an
error event reconnects: the page from the cursor, then the stream with
`Last-Event-ID`.

Load: one outbound stream per daemon, plus one page request per reconnect
(more only while the journal says `hasMore`). The daemon runs nothing periodic
of its own for the feed; a transport failure is retried after a pause that
doubles from one second to five minutes. Headers are read beyond the feed
only when an envelope names a tracked transaction.

## The tracker pass

`serve`, `daemon` (and each tenant of `serve-fleet`, with or without
`--daemon`) run the same served pass
([src/broadcast_reconcile.rs](src/broadcast_reconcile.rs),
`run_served_pass`) every `BROADCAST_RECONCILE_INTERVAL_SECS` (default 60, at least 5;
`BROADCAST_RECONCILE=0` turns it off): the tracker's pass, the poisoned-chain
sweep and the spend guard. Under `serve` it drains the wallet's Arcade SSE
stream before the pass; under `daemon` the monitor holds that stream and the pass drains
none. Its load: it reads the store and asks the header service for the tip and
the header at each height a stored path names (one read per height per pass).
Under `daemon`, which holds the feed, the pass reads no mined row again: the
recheck of a mined word is the tracker's own step on each chain event. Under
`serve`, which holds no feed, it keeps the last twelve tips and reads the mined
rows at or above a header that moved. Beyond that it asks only when a row is
due: a merkle path for a re-ask that is due (at most `TRACKER_MAX_ASKS`,
default 20, per pass, the words whose root moved ahead of the rest and at most
four of them, through the toolbox's couriers, below), a re-announce to the
broadcasters at a lock time, the follow-up of a `stale` word (below), and the
sweep's re-check of an input held locked past its due time.

A reorg is absorbed below the user (the stack's ruling 7 of 2026-10-20,
`docs/DECISIONS-2026-10-19.md` section 8c). Once per `stale` episode one
follow-up reads the transaction's status at the broadcasters (Arcade `GET /tx/{txid}`,
classic ARC `GET /v1/tx/{txid}`). A broadcaster that knows it ends the matter,
its word kept as a hint; when every broadcaster that answered does not know
it, the transaction is announced once (its stored BEEF: the transaction as
bytes, its parents with their paths), never a second time in the same episode;
a refusal the node gave writes the node's verdict (`rejected`, or `conflicted`
with the competitors it names); no answer at all announces nothing. Load: one
status read and at most one announce per `stale` word, at most four words a
pass. Under `daemon` the follow-up is the toolbox's monitor's (its reorg and
review tasks demote the proof and re-ask the courier once, toolbox 0.10.0
`reask_lost_proof`) and the pass marks the episode `monitor` and makes no read
of its own; under `serve` and `tracker-tick`, which run no monitor, it is the
pass's. A coin whose creator is `stale`, and the status of the action that
spent into it, are the toolbox's door: at toolbox 0.10.0 a proof whose root
moved is demoted, the action is `unproven` again and `actions` prints it as
"confirming", its inputs stay allocated to it, its change stays spendable as an
unproven creator's is, and nothing is released or failed.

Every creator of a coin we hold gets a row (the stack's wallet charter,
section 0: "About a transaction it believes only the tracker's word: each
action hosts one tracker state"). An `unproven` or `sending` transaction of
the wallet is adopted at `built`. A `completed` transaction with a stored proof
(`proven_tx_id` set; the toolbox's monitor proved it before the tracker did) that creates a coin
we hold and has no row is adopted by replaying its stored path against the
headers of the moment, as the spend guard replays it: the row's word is
`mined(h)` with that path as its evidence when the header at h carries the
root, else the row has no word and the pending re-ask `proofFailed`, run by the
next pass. A header the service could not give adopts nothing that pass; the
toolbox's rows are never written; at most 100 such creators per pass. A creator
the toolbox moves to `completed` while a row exists keeps its row. So
`GET /tx/{txid}` answers for every coin the wallet holds.

## The spend guard on coin selection

Every coin the binary offers to `createAction` (the served `/createAction`,
`send`, `drain`, `split`, `gift-send`) passes the spend guard
([src/spend_gate.rs](src/spend_gate.rs)). The guard offers: it names the coins
it offers and the toolbox allocates from those only, in one call
(`Wallet::create_action_offering`, bsv-wallet-toolbox-rs 0.11.0). The toolbox
picks within the offer and locks within it inside its one `BEGIN IMMEDIATE`
transaction, so a coin outside the offer is never allocated, concurrent calls
included, and the guard writes nothing around the call. (Through 0.15.0 the
guard held every coin it did not offer by its `change` flag for the call and
restored it after, a write after the broadcast; a `daemon` or `serve` start
makes change again any coin a 0.15 process left held in `spend_gate_holds`.)
Two settings:

- `SPEND_DEPTH` (default 1, and the floor: a smaller number reads as 1): the
  depth the block holding a coin's transaction must reach, checked against the
  header service's headers of the moment; the tip is depth 1. The stack's
  wallet charter **R3 (b)**: the spend depth is the host's parameter, 1 the
  floor.
- `SPEND_OWN_CHANGE` (default `1`; `0` turns it off): offer our own change
  (`made`: a transaction this wallet created and signed) whose transaction has
  no block yet. The charter's **R1 (b)**: the one rule a host may name for a
  coin whose creator is not yet mined is for its own change. A received coin
  (`internalizeAction`, any counterparty's payment) with no proof is never
  offered under any host setting. Here the guard departs from the reference
  toolbox, which spends unproven coins whoever made them.

One word per action, never a setting: `allowPending: "received"` in the
served `/createAction` body (beside `options`; absent or any other value is
off), or `--allow-pending received` on `send`, `drain`, `split` and
`gift-send`. The charter's **R1 (d)** (the owner's ruling of 2026-10-18):
under it, and only for that action, a received coin whose transaction has no
block yet is offered; its body was checked at internalize (the toolbox refuses
an incomplete ancestry and checks every proof it carries against the header
service), so the one word trusted is the sender's broadcast. Its cost: if the
sender double-spends, this action becomes `conflicted` and the coins of ours
it spent are lost with it, though nothing of ours was ever lost in fact. The
action carries the word as its label, `spend-gate:received-pending`, which the
toolbox writes with the action in its one transaction (the gate writes nothing
after the call), and `actions` and `/listActions` show it as `underRule:
"received-pending"`. A template made for deferred signing (`signAndProcess:
false`) carries the label from its creation through `signAction`; an aborted
template keeps it. The label is the gate's alone: a caller's own
`spend-gate:received-pending` (in any case) is stripped from `createAction`'s
and `internalizeAction`'s labels, so it grants nothing. The records a 0.15
store made (`spend_gate_actions`) keep answering. There is no environment
variable for it.

The word may name the coins it reaches (0.18.0, #63, from LOW's fleet: a
funder that just paid a seat can vouch for that coin and not for a payout
the seat received from elsewhere): `allowPending: {"received": ["txid:vout",
...]}` in the served body, or `--allow-pending received --coin txid:vout`
(repeatable; `txid.vout` as the listings print it is taken too) on the
commands. Only the named received coins are offered under the word; every
other received unproven coin is held as without it, with the reason
`received, no proof; not named under allowPending: received`; the action
carries the same label. A named coin that is not a received coin this wallet
could spend under the word (not one of its coins, its own change, a coin
another action consumed, a `nosend`) refuses the call before anything is
read, `400 VALIDATION_ERROR` naming the coin and why; a named coin already
mined is offered as any mined coin (the word is moot for it, not wrong). An
empty list is off; an object of another shape is the caller's 400; the
string form `"received"` stays every received coin. The scope narrows what
the word reaches and never widens it. The MCP tool `create_action` carries
the string form only.

A coin whose transaction is `rejected`, `conflicted` or `abandoned` is never
offered; nor, under any rule or word and mined included, is our own coin
whose transaction's lineage is dead: an input of it, or of an ancestor on
the walk, was created by an action of this wallet, made or received, whose
stored word is `conflicted`, `rejected` or `abandoned` or whose status is
`failed`, unless a proof held for that action checks against the headers,
which reads it mined for the walk as at the door: the gate reads such an
action once per call, with the creators it judges and in their one retry
(the owner's lineage ruling of 2026-10-22; `lean/Wallet.lean` `State.skel`
reads the word the door reads, `lean/Tracker.lean` `onProof_of_rootChecks`
puts a checked proof above every word; #163); a coin whose transaction is `stale` (its proof's block left the
chain, the transaction confirming again) ranks as an unproven one, offered
under the same rule (our own change under `SPEND_OWN_CHANGE`, a received
coin under the action's word) and never with no rule (the owner's ruling 7a
of 2026-10-20); a coin whose creator is `mined` at or above the
`forkHeight` of a `split` the feed delivered is held until that split's
`splitResolved` (the open splits are kept in `tracker_meta`, `open_splits`,
written in the delivery's transaction; the scenario
`tracker-split-observation-writes-no-word.md` column 4), with the reason
`suspect: a fork or an unresolved split at or below its height`; a frozen coin is never offered, and a coin a live action of ours
consumed is spent: an input of that action, never a hold that clears. Coins a caller names (`createAction`'s `inputs`,
`respend`'s exact inputs) pass the same judgment; a named coin the guard would
hold refuses the action, `403 INPUT_NOT_OFFERED` with the same `held` list (an
outpoint that is not one of the wallet's coins is left to the toolbox).
When no offered coin covers the action, the refusal lists every coin held
back: its outpoint, the word, the origin (`made` or `received`), its age in
minutes since the wallet learned of it, and why (`depth 1: 0 blocks`,
`received, no proof; offered under allowPending: received`,
`own change, SPEND_OWN_CHANGE=0`,
`stale: its block left the chain; received, confirming again; offered under allowPending: received`, `dead lineage: ancestor <txid> conflicted`, `spent by <txid> (unproven)`, `frozen: never offered`).
`spent by <txid> (unproven)` names an input the live action `<txid>` consumed,
spent for good; the coin leaves the listing when that spender is completed (its
proof lands) or failed.
The route answers `402 INSUFFICIENT_FUNDS` with the list under `held`; the
commands print it, and under `--json` print the route's body as one JSON object
on stdout, `{"code":"INSUFFICIENT_FUNDS","message","offeredSatoshis","held":[...]}`
(`INPUT_NOT_OFFERED` for a named coin), exit 1. Nothing is fetched for it but the header service's tip and
headers: no chain scan, no explorer.

A coin whose creator the guard could not check because the header service
did not answer (its tip, or a header the check needs: a transport fault, an
outage) is held `headers unavailable: <the service's own error>` and marked
as held on the service, no verdict on the coin (0.18.0, #59, seen on the
0.17.1 gate and on LOW's funder the same day: a two-second blink held every
coin and the caller read `INSUFFICIENT_FUNDS` as a balance). The guard reads
those creators once more after a two-second pause, once per call, and when
the coins still held on the fault could have covered the action (with what
was offered, the outputs' sum at least), or every named input is held on it,
the answer is "could not verify now": `503 HEADERS_UNAVAILABLE`, header
`Retry-After: 2`, the body the refusal's (`message` naming the service's
error, `offeredSatoshis`, `held`); the commands print the same body under
`--json`, exit 1. Nothing was written, built, locked or posted: ask again
after the pause. A shortfall the faulted coins could not have covered is the
funds' refusal whatever the service said. A header the snapshot simply does
not hold (a block above its tip) is not the service's fault and is not
retried.

## The served answers

A refused call answers `{"code", "message"}` (the gate's refusals add
`offeredSatoshis` and `held`), with the status by the error's class:

| status | `code` | when |
|---|---|---|
| 400 | `VALIDATION_ERROR`, `INVALID_BEEF`, `ERROR` | the caller's request (`Validation error:`, `Invalid argument:`; `Invalid BEEF at byte N`) |
| 401 | `AUTHENTICATION_REQUIRED` | `Authentication required`, `Invalid identity key:` |
| 402 | `INSUFFICIENT_FUNDS` | no offered coin covers the action; `held` lists every coin held back |
| 403 | `ACCESS_DENIED`, `INPUT_NOT_OFFERED` | `Access denied:`; a named input the spend guard would hold |
| 404 | `NOT_FOUND` | `Entity not found:` |
| 409 | `CONFLICT` | `Duplicate entity:`, `Invalid operation:` |
| 409 | `UNSPENDABLE_COIN` | the wallet's own coin covered the action but its stored ancestry is incomplete (`Unspendable coin: <txid.vout>: ...`); the body names it under `coin`. Not the caller's fault and not a shortfall |
| 500 | `INTERNAL_ERROR` | `Storage error:` (the spend gate's and guard's own faults: `Storage error: spend gate: ...`), `Database error:`, `Internal error:` |
| 501 | `PRIVILEGED_NOT_SUPPORTED` | `privileged: true` on `getPublicKey`, the six crypto calls (`encrypt`, `decrypt`, `createHmac`, `verifyHmac`, `createSignature`, `verifySignature`) or the two key-linkage calls: this wallet holds no privileged keyring, and a privileged request is never answered with the everyday key (#145) |
| 502 | `SERVICE_ERROR`, `BROADCAST_REJECTED` | `Service error:`, `Network error:`, `Broadcast failed:`; a broadcast no broadcaster took, retired |
| 503 | `STORE_BUSY` | the store busy (#44, beside bsv-low #828): body `{"code":"STORE_BUSY","message":"<the error>"}`, header `Retry-After: 1` |
| 503 | `HEADERS_UNAVAILABLE` | "could not verify now" (#59): the header service did not answer, twice, for coins that could have covered the action (or for every named input); body the refusal's (`message` naming the service's error, `offeredSatoshis`, `held`), header `Retry-After: 2`. Nothing written, built, locked or posted |

`503 STORE_BUSY` is answered by the error's type, never its words: the
toolbox's `WalletBusy` from a wallet door, or a read or write of the spend
gate or guard before the door that hit SQLite's busy (code 5, 517 included) or
the writer's acquire ceiling (a reader's pool timeout is a plain storage
fault, 500). It is answered only when nothing was written, built, locked or
posted by the call: repeat the same call after `Retry-After`. The other half
of the rule: once a call has posted its transaction, a store fault after the
post is never the call's error. The answer is the post's truth with its txid,
and the fault is logged with a marker: the served `createAction`'s follow-up
writes (`post_broadcast_follow_up_write_failed`), `respend`'s status write,
guard and request read (`post_broadcast_status_write_failed`,
`post_broadcast_guard_failed`, `post_broadcast_request_read_failed`), and
`split`'s self-internalize (`post_broadcast_split_internalize_failed`, which
adds `"selfInternalized": false` under `--json`). The word `STORE_BUSY` is in
no 4xx body. Under
`--json`, `send`, `drain`, `split` and `gift-send` print the same body and
exit 1. CORS: `Access-Control-Expose-Headers` carries `Retry-After`, so a
browser caller can read it.

## One transaction's word

`GET /tx/{txid}` (behind the same bearer as `/chain-feed`) serves the
tracker's stored word for one transaction: `{txid, word, height, reask,
evidence}`, where `evidence` is, for `mined`, the stored proof's `root` and
`height` (the toolbox's `proven_txs`), else `null`; 404 when the tracker holds
no word for it. A `stale` word adds `tier`, the word its hints give
(`announced` or `seen`: its status before the proof, which an app reads as
"confirming"), and its `reask` is the one the reorg left (`reorg@900`). A view; nothing is decided from it. The MCP tool `tx_word`
reads it.

## Explorer calls (rule 28)

Every explorer call is deleted, because a header, a proof or our own index
already answers, or it is a break-glass read that says why at its site
([tests/rule_28_sites.rs](tests/rule_28_sites.rs) holds the CLI's own list).

Since 0.17.0 (#50, the owner's word of 2026-10-22: each scanner has its own
rate limits, so when one fails we go to the next) every chain question asks
WhatsOnChain, BananaBlocks and Bitails as each other's fallbacks: a fault (a
429, a timeout, a 5xx, a malformed body) moves to the next rung at once and is
never read as "unspent" or "absent"; a negative needs a second provider's
clean answer; no rung sleeps on a 429, and a rate-limited rung is skipped
without a request until its Retry-After passes. Every explorer call is logged
at info with marker `explorer_call`, `provider`, `call`, `status` and `ms`.

The CLI's sites:

| site | what it asks, and when |
|---|---|
| `src/broadcast_verify.rs` (C1) | WhatsOnChain `GET /tx/hash/{txid}`, Bitails `GET /tx/{txid}`, BananaBlocks `GET /tx/{txid}` (through the toolbox's provider; its `mined` is chain evidence, its `known` a presence word read as a broadcaster's `SEEN`, never a proof or chain evidence, and its 404 a gap, never an absence vote): after a send, whether the network holds what a broadcaster took. The start rotates; a `mined` from any index outranks BananaBlocks' `known`; absence is WhatsOnChain's and Bitails' clean 404s both, which a `known` does not stand against; the report names the index that answered (`chain_index_by`) |
| `src/explorers.rs` (C2) | `receive`, a payment's BEEF no store of ours holds: WhatsOnChain `GET /tx/{txid}/beef`, then BananaBlocks `GET /tx/{txid}/hex` (bytes that hash to the txid) with the proof the wallet's services checked against the header service, then the toolbox's `get_beef`. A fault (a 429, a closed rung, bytes that do not hash, no checked proof) moves to the next courier at once. The WhatsOnChain rung passes the toolbox WhatsOnChain provider's own gate and the BananaBlocks rung is the toolbox's provider, so the CLI and the toolbox's ladders spend one budget per host and a 429 either meets closes that host for both until Retry-After passes |
| `src/commands/sync.rs` (C4) | a chain scan for payments to our deposit address nobody handed us, through the toolbox's `get_utxo_leads` with the deposit script itself (WhatsOnChain and Bitails by the script's hash, BananaBlocks by the deposit address); `sync` only, by hand. A lead is discovery, not a verdict: one explorer's list is enough to look, the union of every answering explorer's list is the fullest look, and each lead is received only after its bytes hash to its txid and its proof meets the header service. An empty set is every answering explorer's empty list; no answer at all is "could not look", an error. Each explorer's list is read page by page, at most 10 pages of 1000 (the toolbox's `LEAD_PAGES_MAX`); a list cut short (the cap, or a later page it could not read) is still a list, its leads received as any other, and the sync names it (`unspent_partial`, "N explorers answered (M cut short)"). When every answering list was cut short the summary says the sync was not a full look: an output past the cap is reached by a later sync only if the explorers' ordering brings it into the pages read, which nothing promises. An outpoint of ours missing from the leads, a cut-short list's included, is only a question for the spend probe, never "spent" |
| `src/explorers.rs` (C7) | who spent an outpoint (no header or proof says an output is spent by someone else): WhatsOnChain `GET /tx/{txid}/{vout}/spent` and BananaBlocks `GET /txo/{txid}/{vout}` (its `script_pubkey` must be the asked script's), a rotating start; a name from either is bound by the spender's own bytes and proven by its merkle path; then the toolbox's `is_utxo` (three explorers, a rotating start, asked until two agree: an unspent and a not-listed each need two explorers' clean answers, and one answer with the others faulting is "could not look", `Unknown`, never released on). Both rungs pass the same per-host gates as the toolbox's ladders. Asked by `cleanup-abandoned`, `reconcile-outputs`, the daemon's sweep and `sync --reconcile-spent` |
| the toolbox's monitor, `check_for_proofs` (`daemon` only) | the monitor's own proof backstop, not the tracker's path: on each new header (and a two-hour fallback) it asks the status providers about every unproven transaction, then a merkle path for each one called mined, from Arcade when configured, then WhatsOnChain, then Bitails; the path is checked against the header service before it is stored. Each proof it stores is logged at info with the provider that served it: marker `proof_backstop_provider`, `provider`, `txid`, `height` ([src/backstop_log.rs](src/backstop_log.rs)); `status-answer` when the status answer carried the proof. No setting turns its explorer providers off (toolbox 0.9.0 `ServicesOptions` has none; `BREAK_GLASS_EXPLORER_HEADERS` governs headers only) |
| the tracker's re-ask (`Courier`, every served pass) | a merkle path for one named transaction whose re-ask is due, through the same toolbox couriers (Arcade, then WhatsOnChain, then Bitails), checked by the tracker against the header service |

## Deliberate respend

`respend` is the host's explicit choice to build and post a competing spend
from a previously announced transaction's reserved inputs. It does not retire
the original transaction or make its inputs spendable. The original remains
tracked until the competitor's checked proof establishes the conflict.

[src/respend.rs](src/respend.rs) refuses when a current status source still
holds the original, when a configured or previously accepting source cannot
answer, when no broadcaster ever took it, or when it already has proof or a
terminal word. It also refuses missing or invalid ancestry, inputs that are
not the wallet's reserved P2PKH outputs, changed reservations, insufficient
fee coverage, and an already tracked replacement. Silence must be established;
an unavailable source is not silence. A refused post of the replacement is
still a retryable hint.

## The repair passes' report

`reconcile-outputs` and `cleanup-abandoned` print a measured balance line, the
default basket's spendable sum (the rows `balance` counts) read before the pass
and after it: `Balance (default basket): B1 -> B2 (delta)`. Below it, one line
per basket for the rows the pass turned off (`spendable` 1 to 0, counted from
the updates that changed a row): `Turned off: N rows, S sats in <basket>`;
`cleanup-abandoned` adds `Turned on: ...` for the inputs it restored. A protocol
output in another basket turns off without the balance moving. In
`reconcile-outputs`, phase (a) counts only rows that were spendable; a row it
finds already off is linked to its spender and named on its own line, `already
off: N rows, S sats`. A dry run prints the balance once and the `Would turn
off` lines. The passes write the store under a running daemon; restart it to
refresh its in-memory view.

## Build and gate

```sh
cargo build --locked
cargo fmt --all -- --check
```

[.github/workflows/ci.yml](.github/workflows/ci.yml) runs workspace tests,
formatting, clippy, release builds on Linux and macOS, and the status/covenant
vector checks on every PR and main push. [tests](tests) contains the HTTP and
tracker integration witnesses; local fixture tests are not live-wallet proof.
Dependabot moves the stack's crates daily at 18:00Z under
[.github/dependabot.yml](.github/dependabot.yml).

## Records and releases

[RELEASING.md](RELEASING.md) defines the machine gate and release publication.
[CHANGELOG.md](CHANGELOG.md) records changes and
[docs/reading-a-balance.md](docs/reading-a-balance.md) explains balance evidence.
Licensed under [MIT](LICENSE).
