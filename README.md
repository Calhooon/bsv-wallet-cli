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
- `/abortAction` -- cancel a deferred transaction and release UTXOs
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
- **Blockchain**: Chaintracks (primary) with WoC/BHS/Bitails failover

## Configuration

All configuration is via environment variables:

| Variable | Required | Description |
|----------|----------|-------------|
| `ROOT_KEY` | Yes | Wallet root private key (hex). Set by `bsv-wallet init`. |
| `CHAINTRACKS_URL` | No | Chaintracks server URL for chain tracking |
| `AUTH_TOKEN` | No | Bearer token for HTTP auth (localhost-only, optional) |
| `TLS_CERT_PATH` | No | TLS certificate path (requires `--features tls`) |
| `TLS_KEY_PATH` | No | TLS private key path (requires `--features tls`) |
| `MIN_UTXOS` | No | Low UTXO warning threshold in daemon mode (default: 3) |

Port is set via `--port` CLI flag (default: 3322), not an environment variable.

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
