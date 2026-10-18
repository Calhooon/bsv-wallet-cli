# bsv-wallet-cli

A self-hosted BSV command-line wallet and BRC-100 HTTP server, with SQLite
storage, signing, broadcasting and transaction tracking. **0.9.0** pins
**bsv-wallet-toolbox-rs =0.8.0**, **bsv-rs =0.6.1** and **bsv-tracker =0.4.0**;
[Cargo.lock](Cargo.lock) carries those versions.

## Install and inspect

Install the registry release into an explicit tool directory:

```sh
cargo install bsv-wallet-cli --version 0.10.0 --locked --root ./wallet-tools
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
are defined in [src](src); [GIFT.md](GIFT.md) documents gift flows and limits.

## The chain-event feed

`daemon` holds the header service's chain-event feed
([src/chain_feed.rs](src/chain_feed.rs)); the source is `CHAINTRACKS_URL`
(required, no default; `off` has no feed). On start it replays from its stored
cursor (0 at birth) through `GET /events?since=<cursor>` until `hasMore` is
false, then holds `GET /events/stream?since=<cursor>`. Every envelope goes
through the tracker's decoder and `Tracker::on_chain` with the daemon's
headers: a reorg turns the words of the transactions mined at or above its
height `stale` and asks for a new proof; a fork or a split marks them suspect
and moves no word; a tip or a resolved split rechecks the marked ones against
the headers; a freeze writes its outpoint to the frozen record,
`frozen_outpoints` (txid, vout, the envelope's cursor, when), which the spend
guard reads (never offered; nothing here clears a row, the charter's R6). The
cursor is stored after each delivery, in the same transaction as the tracker's
rows (`tracker_meta`, `feed_cursor`).

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

## The spend guard on coin selection

Every coin the binary offers to `createAction` (the served `/createAction`,
`send`, `drain`, `split`, `gift-send`) passes the spend guard
([src/spend_gate.rs](src/spend_gate.rs)). The toolbox picks the coins inside
its own query, so the guard holds every coin it does not offer for the length
of the call (the coin's `change` flag, recorded in `spend_gate_holds`) and
restores exactly those after. Two settings:

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
action records the word (`spend_gate_actions`), and `actions` and
`/listActions` show it as `underRule: "received-pending"`. A template made
for deferred signing (`signAndProcess: false`) records it by the action's
`reference` and gains its txid at `signAction`; an aborted template keeps the
record. There is no environment variable for it.

A coin whose transaction is `stale`, `rejected`, `conflicted` or `abandoned`
is never offered, a frozen coin is never offered, and a coin an action of ours
holds is the toolbox's lock. Coins a caller names (`createAction`'s `inputs`,
`respend`'s exact inputs) pass the same judgment; a named coin the guard would
hold refuses the action, `403 INPUT_NOT_OFFERED` with the same `held` list (an
outpoint that is not one of the wallet's coins is left to the toolbox).
When no offered coin covers the action, the refusal lists every coin held
back: its outpoint, the word, the origin (`made` or `received`), its age in
minutes since the wallet learned of it, and why (`depth 1: 0 blocks`,
`received, no proof; offered under allowPending: received`,
`own change, SPEND_OWN_CHANGE=0`, `locked by <txid>`, `frozen: never offered`).
The route answers `402 INSUFFICIENT_FUNDS` with the list under `held`; the
commands print it. Nothing is fetched for it but the header service's tip and
headers: no chain scan, no explorer.

## One transaction's word

`GET /tx/{txid}` (behind the same bearer as `/chain-feed`) serves the
tracker's stored word for one transaction: `{txid, word, height, reask,
evidence}`, where `evidence` is, for `mined`, the stored proof's `root` and
`height` (the toolbox's `proven_txs`), else `null`; 404 when the tracker holds
no word for it. A view; nothing is decided from it. The MCP tool `tx_word`
reads it.

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

## Build and gate

```sh
cargo build --locked
cargo fmt --all -- --check
```

[.github/workflows/ci.yml](.github/workflows/ci.yml) runs workspace tests,
formatting, clippy, release builds on Linux and macOS, and the status/covenant
vector checks on every PR and main push. [tests](tests) contains the HTTP and
tracker integration witnesses; local fixture tests are not live-wallet proof.

## Records and releases

[RELEASING.md](RELEASING.md) defines the machine gate and release publication.
[CHANGELOG.md](CHANGELOG.md) records changes and
[docs/reading-a-balance.md](docs/reading-a-balance.md) explains balance evidence.
Licensed under [MIT](LICENSE).
