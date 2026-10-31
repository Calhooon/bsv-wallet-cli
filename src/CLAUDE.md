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
| `config` | `Arc<Config>` | The configuration `main` read once at start (`config.rs`) |

**`WalletContext::load(&config, json)` sequence:**

1. Take `ROOT_KEY` from the configuration (fails with clear error if missing)
2. Parse `PrivateKey` from hex, derive `identity_key` (public key hex)
3. Determine `chain` from `--testnet` flag
4. Open SQLite storage at the `--db` path (`open_store`)
5. Run `make_available()` (SQLite migrations)
6. Configure services from the configuration's `ServicesSettings` (`services_env::services_options`), with `CHAINTRACKS_URL`
7. Construct `Wallet::new(root_key, storage, services)`

### `config.rs` (the configuration, #80)

`Config::load(&Flags)` runs once in `main`, before any command: the environment is read once as a snapshot (the crate's one read of the environment; the arch rule A7 refuses any other), parsed into its group (`services`, `host`, `reconcile`, `verify`, `server`, `daemon`, `wallet`), and a malformed number, address or URL is a `ConfigError` naming the variable and its shape; every refusal is returned together (`Refused`, with the configuration at the refused settings' defaults), so `config` prints the table with the refused rows marked and exits 1, `init` runs, and every other command refuses at start naming them all. Keys and tokens are `Secret`s (`<set>` in `Debug` and `Display`). `bsv-wallet config` prints `Config::knobs()` (value and source: env, flag, default); `daemon` and `serve` log `Config::log_line()` at start. The server builds `GateOptions` from it once (`make_router`); nothing on the request path reads the environment. `RelaySettings::load()` is `bsv-wallet-relay`'s.

**Environment variables consumed (read by `config.rs`):**
- `ROOT_KEY` (required) — hex-encoded private key
- `CHAINTRACKS_URL` (required) — header service for proof validation; unset or empty is an error, there is no default (`ServicesSettings::header_service`, `services_env::chaintracks_url_for`, P0-1c: no explorer in the proof path); `off` = no header service: every proof is refused (nothing marked proven) and `tick` / `reproof --execute` exit non-zero; the wallet's own spends of its proven coins still reach allocation (the latest era), an incoming or caller-supplied BEEF with proofs is refused for want of headers, and discovery answers no overlay certificate (toolbox 0.14.0-beta.5, #372)
- `BREAK_GLASS_EXPLORER_HEADERS` (break-glass, off by default) — `1`/`true`/`yes` lets the toolbox ask WhatsOnChain, then Bitails, for headers when the header service gives no answer, each call logged at warn
- `WOC_API_KEY`, `BITAILS_API_KEY` (optional) — explorer keys passed to `ServicesOptions` (`WOC_API_KEY` also to the CLI's own WhatsOnChain rung in `explorers.rs`)
- `BANANABLOCKS_BASE` — BananaBlocks API base: unset = the chain's default, empty = off (`services_env::ExplorerSettings`, the toolbox's `ServicesOptions::bananablocks_base`)
- `WOC_BASE`, `BITAILS_BASE`, `ARC_GORILLAPOOL_URL` — the bases of the CLI's own WhatsOnChain and Bitails rungs and of the GorillaPool ARC the broadcast verifier asks, never a broadcaster (`services_env::ExplorerBases`; #74, #300, the hermetic e2e run points them at its stub): unset = the chain's default; `ARC_GORILLAPOOL_URL` empty leaves GorillaPool out. The toolbox's own WhatsOnChain and Bitails providers take no base yet
- `ARC_MODE` (or the older `ARCADE` when it is unset) — the broadcaster mode, `ServicesSettings::arc_mode` (`ArcMode`): Arcade by default (#300); `classic` is deprecated, the hermetic stub's until #299; any other value is refused at start by name. `ARC_URL` moves the broadcaster (no testnet Arcade default: refused where services are built). `TAAL_API_KEY` / `MAIN_TAAL_API_KEY` are classic mode only, deprecated

### `lib.rs` (test support)

Single line: `pub mod server;`. Exists solely so integration tests can import the server module — Rust test binaries can only import from `lib.rs`, not `main.rs`.

## Decisions

- **`WalletContext` as the shared state object**: Every command except `init`, `daemon`, `services`, and `ui` loads a `WalletContext`, which owns the `Wallet<StorageSqlx, Services>` instance. This avoids re-initializing storage/services in each command module.
- **`init` receives `&cli.db` not a full context**: `init` must work before `ROOT_KEY` exists (it creates it), so it can't go through `WalletContext::load`. It takes only the database path and an optional key to import.
- **`lib.rs` only re-exports `server`**: The binary's main logic lives in `main.rs`. `lib.rs` exists solely so integration tests can import the server module (Rust test binaries can't import from a `main.rs`).
- **Tracing defaults**: Without `RUST_LOG`, tracing filters to `bsv_wallet=info,tower_http=info`. The `--verbose` flag overrides to `debug` for all crates. `dotenvy` loads `.env` before anything else.
- **Chain selection via `--testnet` flag**: Defaults to mainnet. The flag propagates through `WalletContext.chain` to both `StorageSqlx` service configuration and `Services` construction. `ServicesOptions::mainnet()` vs `ServicesOptions::testnet()` selects the appropriate blockchain endpoints.
- **Chaintracks integration at context level**: `CHAINTRACKS_URL` is applied via `ServicesOptions.with_chaintracks_url()` during context construction and is the only header source the chain tracker asks (no explorer unless `BREAK_GLASS_EXPLORER_HEADERS` is on).

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
