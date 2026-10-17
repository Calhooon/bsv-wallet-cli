# bsv-wallet-cli

A self-hosted BSV command-line wallet and BRC-100 HTTP server, with SQLite
storage, signing, broadcasting and transaction tracking. **0.9.0** pins
**bsv-wallet-toolbox-rs =0.8.0**, **bsv-rs =0.6.1** and **bsv-tracker =0.4.0**;
[Cargo.lock](Cargo.lock) carries those versions.

## Install and inspect

Install the registry release into an explicit tool directory:

```sh
cargo install bsv-wallet-cli --version 0.9.0 --locked --root ./wallet-tools
./wallet-tools/bin/bsv-wallet --help
./wallet-tools/bin/bsv-wallet respend --help
```

The package includes `bsv-wallet` and the callback receiver `bsv-wallet-relay`.
The MCP companion is a separate workspace member in [mcp](mcp), not a binary
installed by the command above. [Cargo.toml](Cargo.toml) describes the workspace.

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
the headers. The cursor is stored after each delivery, in the same transaction
as the tracker's rows (`tracker_meta`, `feed_cursor`).

A decode fault, a cursor ahead of the journal (409 `cursorAhead`), an
`event: error` from the feed or a refused request stops the loop at its
cursor, with no skip; the fault is in the daemon's status, `GET /chain-feed`
(`status.fault`: its kind, the cursor it stopped at, the offending cursor, the
text). A restart resumes from the stored cursor. A stream that ends without an
error event reconnects: the page from the cursor, then the stream with
`Last-Event-ID`.

Load: one outbound stream per daemon, plus one page request per reconnect
(more only while the journal says `hasMore`). The daemon runs nothing periodic
of its own for the feed; a transport failure is retried after a pause that
doubles from one second to five minutes. Headers are read beyond the feed
only when an envelope names a tracked transaction.

`SPEND_DEPTH` (default 1, and the floor: a smaller number reads as 1) is the
depth the block holding a coin's transaction must reach before the tracker's
spend guard offers the coin; the tip is depth 1 (the stack's wallet charter
R3).

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
