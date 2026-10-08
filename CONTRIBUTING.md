# Contributing to bsv-wallet-cli

Contributions are welcome. This guide covers the setup, constraints, and process.

## Dev Environment

**Prerequisites:** Rust stable toolchain (1.75+), SQLite3.

```bash
git clone https://github.com/Calhooon/bsv-wallet-cli.git
cd bsv-wallet-cli
cargo build
```

All dependencies (`bsv-wallet-toolbox-rs`, `bsv-rs`) are published on crates.io -- no sibling repos needed.

To run the wallet locally, initialize and start the daemon:

```bash
bsv-wallet init          # generates ROOT_KEY, creates wallet.db
bsv-wallet daemon        # starts HTTP server on port 3322
```

Environment variables: `ROOT_KEY` (required, set by `init`), `CHAINTRACKS_URL` (required: your header service, or `off`), `AUTH_TOKEN` (optional bearer auth), `RUST_LOG` (tracing filter). Port is set via `--port` flag (default 3322), not an env var.

## Running Tests

```bash
# 53 synthetic tests -- no running server, no funded wallet, no network
cargo test --test integration
```

The synthetic tests spin up an in-process Axum server per test and exercise all 28 endpoints against it. This is the baseline that must pass for every PR.

E2E tests require a running daemon with a funded wallet and are not expected to pass in CI:

```bash
bsv-wallet daemon &
WALLET_URL=http://localhost:3322 cargo test --test integration e2e_ -- --test-threads=1
```

## Code Style

Standard `rustfmt`, no custom configuration. Run `cargo fmt` before committing.

## The Wire-Compatibility Constraint

This is the single most important design constraint in the project.

The HTTP server must produce **identical JSON** to the MetaNet Client. Existing tools (x402 payments, BRC-31 auth, browser extensions) connect to `localhost:3322` expecting MetaNet Client responses. If the JSON shape changes, those tools break silently.

This means you cannot change HTTP response shapes, field names, or serialization formats without verifying compatibility against the MetaNet Client wire format.

## The Translation Layer

The MetaNet Client and the Rust SDK use different JSON representations. `src/server/types.rs` bridges the gap. Key differences:

- **`protocolID`**: MetaNet sends `[2, "name"]` (mixed array). SDK uses `Protocol { security_level, protocol_name }`.
- **`counterparty`**: Hex string over the wire, `Counterparty::Other(PublicKey)` in SDK.
- **Binary fields** (`tx`, `signature`, `data`): JSON number arrays `[0,1,2,...]` over the wire, not hex strings.
- **Field casing**: MetaNet uses `protocolID` / `keyID` (capital D). Serde's `rename_all = "camelCase"` produces lowercase `d`, so every affected field needs `#[serde(alias = "protocolID")]`.

If you add or modify an endpoint handler, check that the request deserialization and response serialization match the MetaNet Client format exactly.

## PR Process

1. Fork the repo and create a feature branch off `main`.
2. Make your changes. Keep commits focused.
3. Ensure `cargo test --test integration` passes (all 53 tests).
4. Ensure `cargo fmt` and `cargo clippy` produce no warnings.
5. Open a PR against `main` with a clear description of what changed and why.

## Finding Work

Check the [issue tracker](https://github.com/Calhooon/bsv-wallet-cli/issues) for issues labeled **"good first issue"**. These are scoped tasks that don't require deep knowledge of the wallet engine or SDK internals.

## Project Structure

```
src/
  main.rs        -- CLI entrypoint, clap subcommand dispatch
  context.rs     -- WalletContext: ROOT_KEY, SQLite, services
  cli.rs         -- Clap argument definitions
  commands/      -- One file per CLI command
  server/
    mod.rs       -- Axum router, middleware, SpendingLock
    handlers.rs  -- 28 endpoint handlers (MetaNet JSON <-> SDK translation)
    types.rs     -- MetaNet Client JSON request/response structs
```

## License

By contributing, you agree that your contributions will be licensed under the [MIT License](LICENSE).
