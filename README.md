# bsv-wallet-cli

A self-hosted BSV command-line wallet and BRC-100 HTTP server, with SQLite
storage, signing, broadcasting and transaction tracking. **0.7.4** pins
**bsv-wallet-toolbox-rs =0.7.5** and **bsv-rs =0.4.5**. Its tracker dependency
has the compatible requirement `0.2.0`; [Cargo.lock](Cargo.lock) resolves
**bsv-tracker 0.2.0**. The published tracker 0.2.1 is compatible with that
requirement, but is not the version in this checkout's lockfile.

## Install and inspect

Install the registry release into an explicit tool directory:

```sh
cargo install bsv-wallet-cli --version 0.7.4 --locked --root ./wallet-tools
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
