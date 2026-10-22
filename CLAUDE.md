# bsv-wallet-cli

> Single Rust binary providing both a CLI wallet and an HTTP server that implements all 28 BRC WalletInterface endpoints, wire-compatible with the MetaNet Client.

## Architecture

The binary has two modes: **CLI commands** for interactive wallet use, and an **HTTP server** (port 3322) that is wire-compatible with MetaNet Client.

```
src/
  main.rs          — CLI entrypoint, routes clap subcommands
  context.rs       — WalletContext: loads ROOT_KEY, opens SQLite, configures services
  cli.rs           — Clap argument definitions
  commands/        — One file per CLI command (init, send, daemon, serve, split, etc.)
  server/
    mod.rs         — Axum router setup, CORS, auth middleware, SpendingLock
    handlers.rs    — All 28 endpoint handlers, each translating MetaNet JSON ↔ SDK types
    types.rs       — MetaNet Client JSON request/response structs (the translation layer)
```

**Dependency chain**: `bsv-wallet-cli` → `bsv-wallet-toolbox-rs` (wallet engine, SQLite storage, signing, broadcasting) → `bsv-rs` (WalletInterface trait, crypto primitives). Both are published on crates.io.

## Conventions

- **MetaNet Client wire compatibility** is the top constraint. The HTTP server must produce identical JSON to the MetaNet Client so existing tools (x402 skill, BRC-31 auth) work without modification.
- **Translation layer** (`server/types.rs`): MetaNet Client uses different JSON shapes than the Rust SDK. Key differences:
  - `protocolID`: mixed array `[2, "name"]` vs SDK's `Protocol { security_level, protocol_name }`
  - `counterparty`: hex string vs `Counterparty::Other(PublicKey)`
  - Binary data (`tx`, `signature`): JSON number arrays `[0,1,2,...]` vs hex strings
  - Field names: MetaNet Client uses `protocolID` / `keyID` (capital D). Use `#[serde(alias = "protocolID")]` since `rename_all = "camelCase"` produces lowercase `d`.
- **SpendingLock**: A FIFO mutex serializes `createAction` calls. With limited UTXOs, concurrent spending races on SQLite's write lock. Non-spending endpoints (crypto, queries) run fully concurrent.
- **Daemon vs Serve**: `daemon` = Monitor (background sync tasks) + HTTP server. `serve` = HTTP server only (dev mode, no background sync).
- **The explorer ladder (0.17.0, #50; the owner's word of 2026-10-22)**: each chain scanner has its own rate limits, so when one fails we go to the next. Every chain question that leaves the wallet asks WhatsOnChain, BananaBlocks and Bitails as each other's fallbacks: the toolbox's ladders (=0.12.0: `get_raw_tx`, `get_merkle_path`, `get_utxo_status`, `get_status_for_txids`, `is_utxo`; `get_utxo_leads` for `sync`'s discovery, where one explorer's list is a lead and each lead is verified before it is received) and the CLI's own rungs in `src/explorers.rs` (the spend probe's "who spent it": WoC `/spent` and BananaBlocks `/txo/{txid}/{vout}`, whose `script_pubkey` must hash to the asked script's, a rotating start; `receive`'s couriers: WoC `/beef`, then BananaBlocks `/hex` with the services' checked proof, then `get_beef`) and `broadcast_verify`'s chain indexes (WoC, Bitails, BananaBlocks). A fault (a 429, a timeout, a 5xx, a malformed body, bytes that do not hash to the txid) is "could not look": it moves to the next rung at once and is never "unspent" or "absent". A negative needs a second provider's clean answer (BananaBlocks' 404 is a gap, never a vote). No rung sleeps on a 429: the rung is skipped without a request until its Retry-After passes (the toolbox's `RateGate`). Every explorer call logs `explorer_call` at info with `provider`, `call`, `status`, `ms`. The spend gate and the tracker read the wallet's own proof records, never an explorer.

## Development

```bash
# Build
cargo build --release

# Run daemon (default port 3322)
bsv-wallet daemon

# Run daemon on a custom port
bsv-wallet daemon --port 3322

# Run HTTP server only (no background sync)
bsv-wallet serve --port 3322

# Run tests (41 synthetic tests, no server needed)
cargo test --test integration

# E2E tests (Node.js, requires funder wallet on :3320)
# See "E2E Testing" section below for full setup.
node tests/e2e/run.js                   # all 22 scenarios
node tests/e2e/run.js --scenario 16     # single scenario
node tests/e2e/run.js --dry-run         # setup only, no scenarios
node tests/e2e/run.js --skip-woc        # skip WoC verification (faster)
```

Environment variables: `ROOT_KEY` (required, set by `bsv-wallet init`), `SPEND_DEPTH` (default 1, the floor), `SPEND_OWN_CHANGE` (default 1; 0 holds our own unmined change), `CHAINTRACKS_URL` (required: your header service, or `off`; no default), `BREAK_GLASS_EXPLORER_HEADERS` (break-glass, off by default), `WOC_API_KEY` and `BITAILS_API_KEY` (optional explorer keys), `BANANABLOCKS_BASE` (unset = the chain's default, empty = off), `AUTH_TOKEN` (optional, bearer auth), `RUST_LOG` for tracing. Note: port is set via `--port` CLI flag, not an environment variable.

## E2E Testing

The E2E test suite (`tests/e2e/`) is a Node.js harness that spawns fresh wallets and runs 22 scenarios covering the HTTP API, CLI commands, MCP server, and database inspector UI.

### E2E Funder Wallet

A dedicated funder wallet on port 3320 provides initial funds and receives sweep-back after each run.

- **Location**: `e2e-funder/` directory (gitignored — contains wallet.db and .env)
- **Port**: 3320 (configurable via `FUNDER_PORT` env var)
- **Default funding**: 100,000 sats per run (configurable via `FUND_AMOUNT` env var; scenario 22 needs 50,000 in wallet A at its start and the scenarios before it move about 40,000 out of A). The run refuses at start when the funder holds less than the funding plus 1,000.
- **The spend gate's word**: a test wallet's coins are received payments not yet mined, which the gate holds from a plain spend (R1 (d), 0.15.0). The harness names `allowPending: received` on every spend a test wallet makes (the HTTP client's default; `--allow-pending received` on the CLI scenarios' `send` and `split`), and on the funder's funding transfer (its coins after a run are the sweeps, received and unmined). The funder's client never passes it otherwise.
- **Wallet logs**: each test wallet's `serve` log goes to `serve.log` in its temp directory; a failed run keeps both directories (keys and logs) and names them.
- **SwiftBar**: Blue "E2E" icon in menu bar (`~/bsv-wallet-cli-runner/e2e-funder.30s.sh`)

**Setup (first time only):**
```bash
mkdir -p e2e-funder
cd e2e-funder && ../target/release/bsv-wallet init
# Fund the funder wallet's address, then:
../target/release/bsv-wallet split --count 5
```

**Before each run:**
```bash
cd e2e-funder && ../target/release/bsv-wallet daemon --port 3320 &
```

### Scenario Coverage (22 scenarios)

| # | Name | Tests | Cost |
|---|------|-------|------|
| 01-15 | HTTP API scenarios | All 28 endpoints, crypto, certs, baskets, concurrency | ~3K sats |
| 16 | CLI parity | identity, balance, outputs, actions match HTTP | 0 sats |
| 17 | CLI split + spend | Split UTXOs, spend from split output | fees only |
| 18 | CLI compact | BEEF compaction, balance unchanged | 0 sats |
| 19 | CLI services | Chain/height via services command | 0 sats |
| 20 | UI (Playwright) | Database inspector UI renders, API + browser | 0 sats |
| 21 | MCP server parity | The 34 MCP tools vs HTTP API: 29 wallet tools and the five host-route tools (which `mcp/src/tests.rs` drives against a stub) | 0 sats |
| 22 | Rapid internalize + spend | 10 cycles A→B, B internalizes and returns; no phantom UTXOs (needs 50,000 in A at its start) | fees only |

### Dependencies

Scenario 20 (UI) requires Playwright:
```bash
cd tests/e2e && npm install
npx playwright install chromium
```

### Budget

Net cost per run: ~3,000 sats (fees). Hard fail if net cost exceeds 5,000 sats.

## Decisions

- **Axum over JSON-RPC**: The MetaNet Client uses REST-style POST endpoints, not JSON-RPC. Matching its URL structure (`/createAction`, `/getPublicKey`, etc.) ensures wire compatibility.
- **SQLite via sqlx**: Single-file database for portability. No external database process needed.
- **Port 3322**: Default HTTP server port. Override with `--port`.
- **crates.io deps**: `bsv-wallet-toolbox-rs` and `bsv-rs` are published on crates.io. No sibling directories needed — `cargo build` works from a fresh clone.
- **BEEF → AtomicBEEF conversion**: WoC returns standard BEEF (`0x0100beef`), but `internalizeAction` requires AtomicBEEF (`0x01010101`). Conversion prepends the AtomicBEEF header + reversed txid before the standard BEEF payload.
- **createAction signing modes**: `sign_and_process: true` + `noSend: false` = signed and broadcast (default). `noSend: true` = signed but not broadcast. `sign_and_process: false` = unsigned template for deferred signing via `signAction`. These are distinct flows — `noSend` is not a dry run.
