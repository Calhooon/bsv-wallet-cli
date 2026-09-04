# src/

> Binary entrypoint, CLI definitions, and shared wallet context — glue that connects CLI commands and the HTTP server to the wallet engine.

## Files

### `main.rs` (entrypoint)

Tokio async main. Loads `.env` via `dotenvy`, parses CLI args with `clap`, initializes `tracing_subscriber`, then dispatches to the appropriate command module.

Four commands bypass `WalletContext` and receive raw CLI args instead:
- `init` — takes `&cli.db` and optional key hex (must work before `ROOT_KEY` exists)
- `daemon` — takes `&cli` (constructs its own context with the monitor)
- `services` — takes `&cli` (only needs services config, not a full wallet)
- `ui` — takes `&cli.db` and `ui_port` (database inspector, no wallet operations)

All other commands (10 of 14) follow the pattern:
```rust
let ctx = context::WalletContext::load(&cli).await?;
commands::foo::run(&ctx, ...).await?;
```

### `cli.rs` (argument definitions)

Clap `Parser` + `Subcommand` definitions. All global flags are `#[arg(global = true)]`.

**Global flags:**

| Flag | Type | Default | Purpose |
|------|------|---------|---------|
| `--testnet` | bool | false | Use `Chain::Test` instead of `Chain::Main` |
| `--db` | String | `"wallet.db"` | SQLite database path |
| `--port` | u16 | 3322 | HTTP server port (only used by `serve` and `daemon`) |
| `--json` | bool | false | Output JSON instead of tables (stored in `WalletContext.json_output`) |
| `-v, --verbose` | bool | false | Override tracing to `debug` for all crates |

**14 subcommands:**

| Command | Args | Context | Description |
|---------|------|---------|-------------|
| `init` | `--key <hex>` (optional) | `&cli.db` | Generate or import root key, create wallet database |
| `identity` | — | WalletContext | Show public key and address |
| `balance` | — | WalletContext | Show spendable balance |
| `address` | — | WalletContext | Show BRC-29 funding address |
| `send` | `<address> <satoshis>` | WalletContext | Send BSV to a P2PKH address |
| `fund` | `<beef_hex> --vout N` | WalletContext | Internalize a BEEF transaction (receive funds) |
| `outputs` | `--basket <name> --tag <tag>` | WalletContext | List unspent outputs with optional filters |
| `actions` | `--label <label>` | WalletContext | List transaction history with optional filter |
| `daemon` | — | `&cli` | Run monitor (13 background tasks) + HTTP server |
| `serve` | — | WalletContext | Run HTTP server only (no monitor, dev mode) |
| `split` | `--count N` (default 3) | WalletContext | Split UTXOs into multiple outputs for concurrency |
| `services` | — | `&cli` | Show blockchain service status |
| `ui` | `--ui-port N` (default 9321) | `&cli.db` | Open database inspector UI in browser |
| `compact` | — | WalletContext | Compact stored BEEF blobs to reduce proof sizes |

### `context.rs` (shared wallet state)

`WalletContext` is the shared state object loaded by most commands. It owns the full wallet stack.

**Fields:**

| Field | Type | Source |
|-------|------|--------|
| `wallet` | `Wallet<StorageSqlx, Services>` | Constructed from `ROOT_KEY` + `--db` + services |
| `identity_key` | `String` | Hex-encoded public key derived from `root_key` |
| `root_key` | `PrivateKey` | Parsed from `ROOT_KEY` env var |
| `chain` | `Chain` | `Chain::Test` if `--testnet`, else `Chain::Main` |
| `json_output` | `bool` | From `--json` flag |

**`WalletContext::load(&cli)` sequence:**

1. Read `ROOT_KEY` from environment (fails with clear error if missing)
2. Parse `PrivateKey` from hex, derive `identity_key` (public key hex)
3. Determine `chain` from `--testnet` flag
4. Open SQLite storage at `cli.db` path
5. Run `make_available()` (SQLite migrations)
6. Configure services: `mainnet()` or `testnet()`, optionally with `CHAINTRACKS_URL`
7. Construct `Wallet::new(root_key, storage, services)`

**Environment variables consumed:**
- `ROOT_KEY` (required) — hex-encoded private key
- `CHAINTRACKS_URL` (optional) — header service for proof validation; unset = the public Babbage chaintracks for the chain (`services_env::chaintracks_url_for`), `off` = no validation

### `lib.rs` (test support)

Single line: `pub mod server;`. Exists solely so integration tests can import the server module — Rust test binaries can only import from `lib.rs`, not `main.rs`.

## Decisions

- **`WalletContext` as the shared state object**: Every command except `init`, `daemon`, `services`, and `ui` loads a `WalletContext`, which owns the `Wallet<StorageSqlx, Services>` instance. This avoids re-initializing storage/services in each command module.
- **`init` receives `&cli.db` not a full context**: `init` must work before `ROOT_KEY` exists (it creates it), so it can't go through `WalletContext::load`. It takes only the database path and an optional key to import.
- **`lib.rs` only re-exports `server`**: The binary's main logic lives in `main.rs`. `lib.rs` exists solely so integration tests can import the server module (Rust test binaries can't import from a `main.rs`).
- **Tracing defaults**: Without `RUST_LOG`, tracing filters to `bsv_wallet=info,tower_http=info`. The `--verbose` flag overrides to `debug` for all crates. `dotenvy` loads `.env` before anything else.
- **Chain selection via `--testnet` flag**: Defaults to mainnet. The flag propagates through `WalletContext.chain` to both `StorageSqlx` service configuration and `Services` construction. `ServicesOptions::mainnet()` vs `ServicesOptions::testnet()` selects the appropriate blockchain endpoints.
- **Chaintracks integration at context level**: If `CHAINTRACKS_URL` is set, it's applied via `ServicesOptions.with_chaintracks_url()` during context construction. This puts the user's Chaintracks instance at highest priority in the chain tracker failover chain.

## Gotchas

- **`ROOT_KEY` must be set before any command except `init`, `daemon`, `services`, and `ui`**: `WalletContext::load` returns an error with a clear message if it's missing. After `init`, the key is printed to stdout for the user to export — it's not persisted in the database.
- **`StorageSqlx::make_available()`**: Must be called after `open()` and before any wallet operations. It runs SQLite migrations. Forgetting it causes "no such table" errors.
- **`--db` default is `wallet.db` (relative path)**: The database file lands in the current working directory. When running as a daemon/service, ensure the working directory is consistent or pass an absolute path.
- **`--port` is global but only used by `serve` and `daemon`**: Clap defines it globally for simplicity, but other commands ignore it. Similarly `--json` is global but only checked by commands that produce tabular output.
- **`daemon` vs `serve` context construction**: `serve` uses `WalletContext::load()` and passes the context to the server. `daemon` constructs its own context internally because it also needs to set up the monitor's 13 background tasks (proof checking, BEEF compaction, etc.) which require different initialization.
- **`identity_key` is derived, not stored**: It's computed from `root_key.public_key().to_hex()` at load time. There's no separate identity key configuration.

## Related

- [commands/](commands/) — One file per CLI subcommand (14 files + mod.rs)
- [server/](server/) — Axum HTTP server, handlers, and MetaNet Client translation types
- [../CLAUDE.md](../CLAUDE.md) — Root project docs with full architecture and conventions
