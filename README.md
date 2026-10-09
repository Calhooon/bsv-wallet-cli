# bsv-wallet-cli

[![CI](https://github.com/Calhooon/bsv-wallet-cli/actions/workflows/ci.yml/badge.svg)](https://github.com/Calhooon/bsv-wallet-cli/actions/workflows/ci.yml)
[![License: MIT](https://img.shields.io/badge/License-MIT-blue.svg)](LICENSE)

**Your agent's favorite wallet.**

A self-hosted BRC-100 wallet server and CLI. Single Rust binary. Runs as a command-line wallet, an HTTP server with all 28 WalletInterface endpoints, or both. Wire-compatible with MetaNet Client.

Built for AI agents, autonomous software, and developers who want self-hosted BSV infrastructure.

## Install

```bash
curl -sSf https://raw.githubusercontent.com/Calhooon/bsv-wallet-cli/main/install.sh | sh
```

Or build from source:

```bash
cargo install --git https://github.com/Calhooon/bsv-wallet-cli.git
```

## Quick Start

```bash
# Initialize wallet (generates identity key, creates SQLite database)
bsv-wallet init

# Show your funding address
bsv-wallet address

# Check balance
bsv-wallet balance

# Run the HTTP server (default port 3322)
bsv-wallet daemon
```

## Use Cases

### Local development wallet
Run `bsv-wallet daemon` on your machine. Build apps against the 28 BRC-100 endpoints at `localhost:3322`. Wire-compatible with MetaNet Client -- any tool built for it works without modification.

### Deployable wallet server
Host on your own infrastructure. Any app that speaks BRC-100 connects by changing one URL:
```bash
# On your server
bsv-wallet init
bsv-wallet daemon

# From any client (bsv-worm, your app, etc.)
WORM_WALLET_URL=https://wallet.myserver.com:3322
```

### Shared wallet for multi-agent fleets
Multiple AI agents sharing one wallet, one funding source, one UTXO pool:
```bash
# One wallet server
bsv-wallet daemon

# N agents pointing at it
WORM_WALLET_URL=http://wallet-internal:3322 bsv-worm serve --port 8080
WORM_WALLET_URL=http://wallet-internal:3322 bsv-worm serve --port 8081
```

See [bsv-worm](https://github.com/Calhooon/rust-bsv-worm) for an autonomous agent that uses this wallet.

### Custody separation
Keep keys on a hardened server, run agents on throwaway VMs. Compromise the agent, keys are safe. The wallet never exposes private key material -- clients send `protocol_id`, `key_id`, `counterparty` and the wallet derives keys internally via BRC-42.

### Fork and customize
Swap storage backends, add HSM key management, put it behind your corporate proxy for audit logging, add OAuth/mTLS -- the BRC-100 interface stays identical. Anything built for MetaNet Client works unmodified against your fork.

## Why bsv-wallet-cli?

- **No GUI required** -- Runs headless on servers, VMs, CI pipelines. The only BRC-100 wallet that doesn't need a desktop or browser.
- **Non-custodial** -- Your keys stay on your machine. No cloud service, no account, no third party.
- **Wire-compatible** -- Any tool built for MetaNet Client works without modification. Same endpoints, same JSON format.
- **Built for agents** -- AI agents, autonomous software, multi-wallet fleets. One wallet server, N agents pointing at it.

## CLI Commands

| Command | Description |
|---------|-------------|
| `init` | Generate identity key and create wallet database |
| `identity` | Show public key and identity info |
| `balance` | Show spendable balance |
| `address` | Show BRC-29 funding address |
| `send <address> <sats>` | Send BSV to a P2PKH address |
| `fund <beef_hex>` | Internalize a BEEF transaction (receive funds) |
| `outputs` | List unspent outputs |
| `actions` | List transaction history |
| `split --count N` | Split UTXOs for concurrent spending |
| `gift-send <pubkey> <sats> --unlock <date>` | Send a time-locked BSV gift |
| `gift-inspect <txid>` | See a gift, prove it's yours + when it unlocks |
| `gift-claim <txid>` | Claim a time-locked gift after its unlock time |
| `tracker-tick` | Run one pass of the transaction tracker (see [The transaction tracker](#the-transaction-tracker)) |
| `daemon` | Run monitor + HTTP server (production) |
| `serve` | Run HTTP server only (dev mode) |
| `services` | Show blockchain service status |

### Time-locked gifts

Pay someone such that they own the key immediately but **can't spend until a date
you set** (`--unlock +3mo`, `2026-12-25`, or a unix timestamp). See
**[GIFT.md](GIFT.md)** for the full flow. Note: unlock dates are supported through
**Jan 2038**; later dates use a code path not yet mainnet-proven — `gift-send`
warns and you should pick an earlier date for real value.

## HTTP Server

All 28 BRC WalletInterface endpoints on `http://127.0.0.1:3322`, wire-compatible with MetaNet Client.

### Endpoints

**Status** (GET)
- `/isAuthenticated` -- health check
- `/getHeight` -- current chain height
- `/getNetwork` -- `mainnet` or `testnet`
- `/getVersion` -- wallet version
- `/waitForAuthentication` -- auth status

**Crypto** (POST, requires `Origin` header)
- `/getPublicKey` -- identity or derived public key
- `/createSignature` / `/verifySignature` -- ECDSA signing
- `/encrypt` / `/decrypt` -- symmetric encryption via BRC-42
- `/createHmac` / `/verifyHmac` -- HMAC operations
- `/getHeaderForHeight` -- block header lookup

**Transactions** (POST, requires `Origin` header)
- `/createAction` -- build, sign, and broadcast transactions
- `/signAction` -- sign a deferred (unsigned) transaction
- `/abortAction` -- cancel a deferred transaction and release UTXOs; a broadcast transaction the wallet holds no chain evidence for is aborted too (inputs released, its unproven descendants retired), one the chain vouches for is refused
- `/internalizeAction` -- accept incoming payments
- `/listActions` -- transaction history
- `/listOutputs` -- UTXO listing
- `/relinquishOutput` -- release an output

**Certificates** (POST, requires `Origin` header)
- `/acquireCertificate` -- store a certificate
- `/listCertificates` -- query certificates
- `/proveCertificate` -- prove certificate ownership
- `/relinquishCertificate` -- delete a certificate

**Discovery** (POST, requires `Origin` header)
- `/discoverByIdentityKey` / `/discoverByAttributes` -- certificate discovery
- `/revealCounterpartyKeyLinkage` / `/revealSpecificKeyLinkage` -- key linkage revelation

## Architecture

```
bsv-wallet-cli          (this repo -- CLI + HTTP server)
  |-- rust-wallet-toolbox  (wallet engine -- storage, signing, broadcasting)
  +-- bsv-sdk             (WalletInterface trait, types, crypto primitives)
```

- **Storage**: SQLite via sqlx (single file, portable)
- **Concurrency**: Spending operations queue via FIFO lock. All other endpoints (crypto, queries, status) are fully concurrent.
- **Blockchain**: the header service (`CHAINTRACKS_URL`) answers every header, root, tip and height question; explorers (WhatsOnChain, Bitails) are asked only for the break-glass reads listed under [Explorer calls](#explorer-calls)

## Configuration

All configuration is via environment variables:

| Variable | Required | Description |
|----------|----------|-------------|
| `ROOT_KEY` | Yes | Wallet root private key (hex). Set by `bsv-wallet init`. |
| `CHAINTRACKS_URL` | Yes | Header service that validates merkle proofs: your own `chaintracks-cloudflare` or rust-chaintracks deployment. There is no default; a command that builds the wallet's services refuses to run without it (`CHAINTRACKS_URL is not set`). `off` runs with no header service: every merkle proof is refused and nothing is marked proven, and `tick` and `reproof --execute` exit non-zero |
| `BREAK_GLASS_EXPLORER_HEADERS` | No | Break-glass, off by default. `1`, `true` or `yes` lets the proof path ask WhatsOnChain (then Bitails) for block headers when the header service gives no answer; every such call is logged at warn level. An explorer never overrules the header service's own answer |
| `AUTH_TOKEN` | No | Bearer token for HTTP auth (localhost-only, optional) |
| `TLS_CERT_PATH` | No | TLS certificate path (requires `--features tls`) |
| `TLS_KEY_PATH` | No | TLS private key path (requires `--features tls`) |
| `MIN_UTXOS` | No | Low UTXO warning threshold in daemon mode (default: 3) |
| `ARC_URL` | No | Override the broadcaster URL (classic ARC, or the Arcade endpoint in Arcade mode) |
| `ARC_MODE=arcade` / `ARCADE=1` | No | Arcade V2 mode: EF-only submit, SSE push statuses, push proofs |
| `CALLBACK_TOKEN` | No | Override the per-wallet callback token (auto-generated + persisted as `<db>.callback-token` otherwise) |
| `PUBLIC_CALLBACK_URL` | No | Public HTTPS URL for Arcade status webhooks (`X-CallbackUrl`), pointing at this daemon's `/arc-callback` |
| `BIND_ADDR` | No | HTTP server bind address (default `127.0.0.1`; set `0.0.0.0` for the tunnel/webhook case) |
| `RELAY_URL` / `RELAY_POLL_SECS` | No | Drain a `bsv-wallet-relay` queue (see proof-delivery ladder) |
| `TAAL_API_KEY` / `MAIN_TAAL_API_KEY` | No | TAAL ARC auth (Bearer / raw `Authorization` respectively) |
| `TRACKER_AGE_SECS` | No | Seconds without a new word from a broadcaster before the tracker asks for a transaction's merkle proof (default 600); the pause between two asks for the same transaction doubles from there |
| `TRACKER_MAX_ASKS` | No | Proof requests one tracker pass runs (default 20) |

Port is set via `--port` CLI flag (default: 3322), not an environment variable.

## Explorer calls

The rule (0.5.0): an explorer is asked only where no header, no merkle proof and no row
of the wallet's own answers the question, and then in one shape: two providers where a
second exists, a rotating start, a negative only when both agree, and "could not look"
kept apart from "nothing there". Each call says why at its site in the source, and
`tests/rule_28_sites.rs` holds the list.

| Question | Asked when | Where |
|----------|-----------|-------|
| Has the network seen a transaction we just sent | after a send, by `reconcile-broadcasts` (by hand), by `cleanup-abandoned`; never by `serve`'s loop (0.6.0) | the broadcaster first, then WhatsOnChain and Bitails as chain indexes: present from either, absent only from both |
| Who spent an output, and is it unspent | `cleanup-abandoned`, `reconcile-outputs`, `sync --reconcile-spent`, the daemon's sweep | WhatsOnChain names a spender; the named transaction's own bytes must hold the outpoint among their inputs (0.6.0), and the spend counts only with a merkle proof of it; "unspent" is the toolbox's two-explorer read |
| The BEEF of a payment nobody handed us | `receive` | WhatsOnChain's BEEF route, then the toolbox's `get_beef`; the BEEF is checked against the header service like any other |
| Which outputs pay our deposit address | `sync` (an operator's command, never a daemon path) | WhatsOnChain; a chain scan. The routine way to receive is `fund` with the BEEF the payer hands over |
| A stranger's transaction bytes | `gift-inspect`, `gift-claim` | the toolbox's `get_raw_tx` (two explorers, the bytes hashed to the txid) |

Nothing else asks an explorer: `export-beefs` builds from the wallet's own storage, the
tip height and median time past come from the header service, and `gift-claim`
broadcasts through the wallet's own broadcasters.

## The transaction tracker

The CLI hosts [`bsv-tracker`](https://github.com/Calhooon/bsv-tracker) (0.6.0). Each
of the wallet's own unproven transactions holds one word: `built`, `announced`, `seen`,
`mined`. A broadcaster's word (an acceptance, a SEEN, a MINED with no path) is a hint:
it can move a word among the first three and it can ask for a proof, and it never
writes `mined`. Only a merkle path checked against the header service's header does.

- `bsv-wallet tracker-tick` runs one pass; `serve` runs the same pass every 60 s in
  place of the chain-index poll it ran through 0.5.0.
- A pass names transactions. It asks for one proof for one transaction when a hint
  disagrees with the word or when `TRACKER_AGE_SECS` passed with no new hint, and it
  pauses twice as long before each next ask for that transaction. It asks no chain
  index and walks no chain.
- The state is in the wallet's own database (`tracker_states`): the hints heard and
  the raw merkle path, never a trusted word. A stored path is checked again against
  the active header every time its row is read.
- A mined row is read again only when a header at or below its height moved, or when
  the spend guard is run for it (`tracker_host::spend_guard`, the tracker README's
  wallet sketch run by this host; `send` does not call it yet).
- What the loop no longer does by itself: retire a transaction because the chain
  indexes do not know it. `bsv-wallet reconcile-broadcasts` does that, by hand; a
  pass lists every transaction it asked a proof for and got none.

## Broadcasting: Arcade V2 mode

By default the wallet broadcasts through classic ARC (TAAL → GorillaPool →
Bitails → WoC failover) and the Monitor discovers merkle proofs by polling.
Setting `ARC_MODE=arcade` (or `ARCADE=1`) switches the primary broadcaster to
**Arcade V2**, the Teranode-native broadcaster
(`https://arcade-v2-us-1.bsvblockchain.tech` unless `ARC_URL` overrides):

- **EF-only submit** — Arcade rejects BEEF; every unmined ancestor in a
  transaction's BEEF is converted to Extended Format (BRC-30) and batch-posted
  as binary concat to `POST /txs` (Arcade dedupes re-submitted ancestors).
- **Always-async statuses** — submit returns `202`; the Monitor's
  `arcade_events` task subscribes **outbound** to the per-wallet SSE stream
  (`GET /events?callbackToken=…`) and gates spendability on `SEEN_ON_NETWORK`
  (~3s, reliable). Fatal verdicts (`REJECTED`, `DOUBLE_SPEND_ATTEMPTED`) mark
  the transaction so it is never re-broadcast.
- **Failover preserved** — classic ARC providers stay behind Arcade in the
  postBeef chain, so a transient Arcade outage never blocks a broadcast.

Each wallet db gets its own callback token (auto-generated, persisted as
`<db>.callback-token`, never logged), so any number of concurrent wallets
(e.g. `:3322`, `:3382`) get independent status streams.

## Proof delivery: the acquisition ladder

Merkle proofs (BUMPs) complete a transaction (`unproven → completed`). The
wallet acquires them by the best available rung — god-tier when possible,
never blocked:

**(a) Direct webhook — god-tier, zero polling.** When the daemon can present
public HTTPS, Arcade pushes the `MINED` webhook — which carries the
`merklePath` — straight into wallet storage via the daemon's own
`POST /arc-callback` route. Two ways to be publicly reachable:

```bash
# TLS directly (public IP + cert):
TLS_CERT_PATH=/path/cert.pem TLS_KEY_PATH=/path/key.pem \
PUBLIC_CALLBACK_URL=https://wallet.example.com:3322/arc-callback \
ARC_MODE=arcade bsv-wallet daemon        # built with --features tls

# Or a tunnel (cloudflared shown; tailscale funnel works the same way):
cloudflared tunnel --url http://localhost:3322   # gives https://<name>.trycloudflare.com
PUBLIC_CALLBACK_URL=https://<name>.trycloudflare.com/arc-callback \
ARC_MODE=arcade bsv-wallet daemon
```

`/arc-callback` is authenticated by the per-wallet callback token
(`Authorization: Bearer <token>` or `X-CallbackToken`) and is **exempt** from
the wallet `AUTH_TOKEN` bearer — the broadcaster doesn't know your wallet
token. The body goes whole to the toolbox's one Arcade judgment
(`ArcadeEventsTask::apply_event`, the same as an SSE frame): the word decides,
not the presence of a path. A `MINED`/`IMMUTABLE` path (a `reorg_reanchor`
included) is checked against the ChainTracker and stored, or rejected and
never stored; `reorg_unmined` changes nothing and answers `Reask`; `REJECTED`
466 is a double spend, 476 is retryable and not applied; a word Arcade does
not define answers `Refused(..)`. Note Arcade's webhook target is SSRF-guarded: it must be public
HTTPS — plain localhost can never receive it, which is why the SSE rung below
is the default.

**(b) SSE-triggered fetch — the always-works default.** On a `MINED` SSE event
the Monitor immediately fetches the BUMP through its existing services stack
(WhatsOnChain/Bitails/ARC) and ingests it through the same validated path.
Event-driven, no schedule polling, fully local — works from a laptop behind
NAT with zero setup. (The 2-hourly polling sync remains as the final safety
net, exactly as before.)

**(c) `bsv-wallet-relay` — one public receiver for fleets.** A small
store-and-forward companion binary (ships in this repo): Arcade posts
callbacks to the relay; wallet daemons poll their queue **outbound** by
callback token and ingest through the same path — connect/disconnect at will.

```bash
# On the public box:
RELAY_PORT=3390 bsv-wallet-relay          # sqlite queue in relay.db

# Each wallet daemon (can be localhost-only):
ARC_MODE=arcade \
PUBLIC_CALLBACK_URL=https://relay.example.com/arc-callback \
RELAY_URL=https://relay.example.com \
bsv-wallet daemon
```

Relay design: a token is auto-registered on first `/pull` (wallets poll from
boot, before any submit); callbacks with unknown tokens are rejected, and
tokens are high-entropy so a guessed token reads nothing. Set
`RELAY_ADMIN_TOKEN` + `RELAY_REQUIRE_REGISTER=1` to require explicit
`POST /register` instead.

## MCP Server

A separate binary exposes all 28 endpoints as MCP tools for AI agents (Claude Code, Codex, etc.):

```bash
# Start the wallet daemon first
bsv-wallet daemon

# In another terminal -- start MCP server (stdio transport, 29 tools)
bsv-wallet-mcp
```

Add to your Claude Code MCP config:
```json
{
  "mcpServers": {
    "bsv-wallet": {
      "command": "bsv-wallet-mcp",
      "env": { "WALLET_URL": "http://localhost:3322" }
    }
  }
}
```

## Building

```bash
# Standard build
cargo build --release

# With TLS support
cargo build --release --features tls
```

## Testing

```bash
# Run all synthetic tests (no server needed, 41 tests)
cargo test --test integration

# Run e2e tests against a live wallet
bsv-wallet daemon &
WALLET_URL=http://localhost:3322 cargo test --test integration e2e_ -- --test-threads=1
```

## License

[MIT](LICENSE)
