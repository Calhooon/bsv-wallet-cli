# commands

> One file per CLI subcommand, each exposing a single `pub async fn run(...)`. Plus `ui.rs`, which is a standalone web-based wallet inspector.

## Files

| File | Signature | Purpose |
|------|-----------|---------|
| `init.rs` | `run(db_path, key?)` | Generate or import root key, create SQLite DB, write `.env` |
| `daemon.rs` | `run(&Cli)` | Monitor + Wallet + HTTP server (production mode) |
| `serve.rs` | `run(WalletContext, port)` | HTTP server only, no monitor (dev mode) |
| `send.rs` | `run(&WalletContext, address, satoshis)` | P2PKH send, outputs AtomicBEEF hex; a self-payment is kept (#38) |
| `fund.rs` | `run(&WalletContext, beef_hex, vout)` | Internalize external BEEF (faucet/exchange funding) |
| `split.rs` | `run(&WalletContext, count)` | Split all UTXOs into N equal outputs |
| `balance.rs` | `run(&WalletContext)` | Sum all spendable sats (paginates through all outputs) |
| `address.rs` | `run(&WalletContext)` | Derive and display BRC-29 receiving address |
| `identity.rs` | `run(&WalletContext)` | Show identity key, address, chain |
| `actions.rs` | `run(&WalletContext, label?)` | List transaction actions, optional label filter |
| `outputs.rs` | `run(&WalletContext, basket?, tag?)` | List UTXOs, optional basket/tag filters |
| `services.rs` | `run(&Cli)` | Check chain services health (block height) |
| `compact.rs` | `run(&WalletContext)` | Debug BEEF compaction on largest stored BEEF blob |
| `ui.rs` | `run(db_path, port)` | Web-based wallet inspector (standalone Axum server) |

## Decisions

- **Three initialization patterns**: (1) Most commands take `&WalletContext`. (2) `init` takes raw `db_path` + optional key (no context exists yet). (3) `daemon`, `services`, and `ui` take `&Cli` or raw args because they manage their own storage/services.
- **`daemon` vs `serve`**: `daemon` creates its own `Arc<Storage>` + `Arc<Services>` for the Monitor (background sync tasks) + Wallet + HTTP server, with a periodic UTXO count check (every 5 min, threshold via `MIN_UTXOS` env var, default 3). `serve` takes an owned `WalletContext` and runs the HTTP server only — no monitor, no UTXO warnings. Use `serve` for dev, `daemon` for production.
- **TLS support**: Both `daemon` and `serve` read `TLS_CERT_PATH` and `TLS_KEY_PATH` env vars. If both are set, the server binds with TLS. Otherwise plain HTTP.
- **`send` output tagging**: Send outputs are tagged `"relinquish"` to mark them as belonging to the recipient, not this wallet. This prevents the wallet from counting sent funds in its own balance. The result also builds AtomicBEEF hex from the returned BEEF for wallet-to-wallet transfers. A send to the wallet's own deposit address (its locking script is `brc29::deposit_script`; the identity or deposit key in hex is taken as that address) is a self-payment (#38): untagged, then internalized as a wallet payment with the deposit remittance (`fund`'s prefix, suffix and anyone sender), so the balance moves by the fee alone. It needs the header service and is refused before anything is built without one.
- **`AUTH_TOKEN` env var**: Both `daemon` and `serve` pass `AUTH_TOKEN` to `ServerConfig`. When set, the HTTP server requires bearer auth on all endpoints.
- **JSON output mode**: All `WalletContext`-based commands check `ctx.json_output` and emit structured JSON when `--json` is passed. Commands using `&Cli` check `cli.json`.
- **`ui` is a separate web server**: It opens its own SQLite connection (read-only queries) and serves an embedded HTML dashboard at a configurable port. It does NOT go through the wallet engine — it queries the database directly via sqlx.

## Gotchas

- **Duplicated BRC-29 constants**: `DEFAULT_DERIVATION_PREFIX`, `DEFAULT_DERIVATION_SUFFIX`, and `BRC29_PROTOCOL` are defined in `address.rs` and `split.rs`. `fund.rs` shares the two derivation constants but not `BRC29_PROTOCOL`. If you change one, update all.
- **`fund` uses "anyone" as sender**: `internalizeAction` requires a `sender_identity_key`. For external funding (e.g., from a faucet or exchange), there's no real sender, so `fund` uses `KeyDeriver::anyone_key()` as the counterparty. This must match the derivation the wallet used when generating its address.
- **`split` reserves 200 sats for fees**: The fee reserve is hardcoded. With the default fee rate of 101 sat/KB, this is sufficient for typical split transactions but may be tight for very large split counts. Also enforces minimum count of 2.
- **`init` writes `.env` to CWD**: The root key is saved to `./.env` in the current working directory, not a global config. The wallet database path comes from the `--db` CLI flag.
- **`balance` paginates**: Unlike other list commands, `balance` loops through all pages (limit 100 per page) to sum the total. `outputs` and `actions` only fetch the first page (limit 100).
- **`daemon` opens the wallet file once**: `store::open_shared` opens one `StorageSqlx`, runs `make_available()`, and answers two handles of it: the store itself for the Wallet (`Wallet::new` takes it by value) and `Arc::new(store.clone())` for the Monitor and the chain feed. A clone shares the reader, the one-connection writer and the in-flight reservations (toolbox 0.11.0), so every writer in the process queues on one fair writer pool. Never open the file a second time in the same process.
- **`compact` operates on one BEEF only**: It finds the single largest completed BEEF blob (>50KB), upgrades unproven txids using merkle paths from `proven_txs`, trims proven ancestors, and writes back if smaller. This is a debug/diagnostic tool, not the batch compaction run by the monitor.
- **`ui` query safety**: The custom SQL endpoint only allows `SELECT`, `PRAGMA`, and `EXPLAIN` statements. Blob columns are displayed as truncated hex. Filter searches cast all columns to text and use `LIKE`.
- **`ui` stats query**: The `SPENDABLE_WHERE` constant filters for `spendable=1 AND status IN ('completed','unproven','nosend','sending')`, matching the SDK's definition of spendable outputs. Balance is shown per-basket.

## Related

- [Root CLAUDE.md](../../CLAUDE.md) — project architecture and conventions
- `../context.rs` — `WalletContext` struct that most commands depend on
- `../server/` — HTTP server that `daemon` and `serve` start
- `../ui/index.html` — embedded HTML dashboard served by `ui.rs`
