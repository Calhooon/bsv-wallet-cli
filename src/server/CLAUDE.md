# server

> Axum HTTP server implementing all 28 BRC WalletInterface endpoints, wire-compatible with the MetaNet Client.

## Files

- **`mod.rs`** (192 lines) — Router setup, CORS/auth/tracing middleware, TLS support, `SpendingLock` type, `ServerConfig`, graceful shutdown. Exports `make_router()`, `run()`, `make_wallet_state()`.
- **`handlers.rs`** (800 lines) — One handler per endpoint. Translates between MetaNet Client JSON and SDK types, calls `wallet.<method>()`, maps errors via `AppError`. Defines `WalletState = Arc<Wallet<StorageSqlx, Services>>`.
- **`types.rs`** (263 lines) — `Mc*` request/response structs for endpoints whose SDK arg types lack serde derives. Endpoints with SDK serde support (batches 4-6) deserialize directly in handlers without wrapper types.

## Endpoint Map

All 28 endpoints, organized by batch:

| Batch | Method | Route | Handler | Mc* types needed? |
|-------|--------|-------|---------|-------------------|
| Core | GET | `/isAuthenticated` | `is_authenticated` | No (hardcoded JSON) |
| Core | POST | `/getPublicKey` | `get_public_key` | Yes (`McGetPublicKeyReq/Res`) |
| Core | POST | `/createSignature` | `create_signature` | Yes (`McCreateSignatureReq/Res`) |
| Core | POST | `/createAction` | `create_action` | Yes (`McCreateActionReq/Res`) |
| Core | POST | `/internalizeAction` | `internalize_action` | Yes (`McInternalizeActionReq/Res`) |
| 1: Status | GET | `/getHeight` | `get_height` | No (SDK result has serde) |
| 1: Status | GET | `/getNetwork` | `get_network` | No |
| 1: Status | GET | `/getVersion` | `get_version` | No |
| 1: Status | GET | `/waitForAuthentication` | `wait_for_authentication` | No |
| 2: Header | POST | `/getHeaderForHeight` | `get_header_for_height` | No (SDK args have serde) |
| 3: Crypto | POST | `/verifySignature` | `verify_signature` | Yes (`McVerifySignatureReq`) |
| 3: Crypto | POST | `/encrypt` | `encrypt` | Yes (`McEncryptReq`) |
| 3: Crypto | POST | `/decrypt` | `decrypt` | Yes (`McDecryptReq`) |
| 3: Crypto | POST | `/createHmac` | `create_hmac` | Yes (`McCreateHmacReq`) |
| 3: Crypto | POST | `/verifyHmac` | `verify_hmac` | Yes (`McVerifyHmacReq`) |
| 4: Tx workflow | POST | `/signAction` | `sign_action` | No (SDK args have serde) |
| 4: Tx workflow | POST | `/abortAction` | `abort_action` | No |
| 4: Tx workflow | POST | `/listActions` | `list_actions` | No |
| 4: Tx workflow | POST | `/listOutputs` | `list_outputs` | No |
| 4: Tx workflow | POST | `/relinquishOutput` | `relinquish_output` | No |
| 5: Certs | POST | `/acquireCertificate` | `acquire_certificate` | No (SDK args have serde) |
| 5: Certs | POST | `/listCertificates` | `list_certificates` | No |
| 5: Certs | POST | `/proveCertificate` | `prove_certificate` | No |
| 5: Certs | POST | `/relinquishCertificate` | `relinquish_certificate` | No |
| 6: Discovery | POST | `/discoverByIdentityKey` | `discover_by_identity_key` | No (SDK args have serde) |
| 6: Discovery | POST | `/discoverByAttributes` | `discover_by_attributes` | No |
| 6: Key linkage | POST | `/revealCounterpartyKeyLinkage` | `reveal_counterparty_key_linkage` | Yes (`McRevealCounterpartyKeyLinkageReq`) |
| 6: Key linkage | POST | `/revealSpecificKeyLinkage` | `reveal_specific_key_linkage` | Yes (`McRevealSpecificKeyLinkageReq`) |

## Decisions

- **Three-file split**: `mod.rs` owns router setup/middleware/TLS, `handlers.rs` translates between MetaNet JSON and SDK types, `types.rs` defines the MetaNet Client JSON shapes. SDK arg types with serde derives are deserialized directly in handlers (batches 4-6); types without serde (crypto, core endpoints, key linkage) need `Mc*` wrapper structs in `types.rs`.
- **SpendingLock (FIFO mutex)**: Only `createAction` acquires the lock. With limited UTXOs, concurrent spending requests would race on SQLite's write lock and get `SQLITE_BUSY_SNAPSHOT`. The lock queues them so each request sees the previous one's change output. All non-spending endpoints (crypto, queries, certs) remain fully concurrent. The lock is injected as an axum `Extension`.
- **Originator from headers**: Every mutating POST endpoint extracts the originator from `Origin` (parsed as URL, host extracted) or `Originator` header. GET status endpoints hardcode `"localhost"`. Missing originator returns 400.
- **Error classification**: `AppError::classify()` maps wallet error message prefixes to HTTP status codes. It strips the `"wallet error: "` prefix the SDK wraps errors with, then matches:
  - 402 — `Insufficient funds:`
  - 401 — `Authentication required`, `Invalid identity key:`
  - 403 — `Access denied:`
  - 404 — `Entity not found:`, `not found:`
  - 409 — `Duplicate entity:`, `Invalid operation:`
  - 400 — `Validation error:`, `Invalid argument:`, `Origin header required`
  - 500 — `Storage error:`, `Database error:`, `Internal error:`
  - 502 — `Service error:`, `Network error:`, `Broadcast failed:`
  - Default: 400
  - Response body: `{"code": "ERROR_CODE", "message": "..."}`. Errors are also logged via `tracing::warn`.
- **Layer ordering**: the last layer added runs first: the body limit, tracing, the extensions, auth, then the JSON body is read (`lenient_json_body`), then CORS. Auth runs before any body is read. The body cap is 50 MiB (`BODY_CAP`) on every route but the two BEEF doors (`BEEF_DOORS`: `/createAction`, `/internalizeAction`), which have none: a valid BEEF is never refused for its size, the body is held whole, and the toolbox refuses invalid bytes only (`INVALID_BEEF`, 400, the offset and the kind). `ServerConfig` and `SpendingLock` are injected via axum `Extension`, not per-handler.
- **AtomicBEEF in createAction response**: The SDK returns standard BEEF with ancestors. The handler converts it to AtomicBEEF via `Beef::from_binary()` + `to_binary_atomic(&txid_hex)`. Falls back to raw tx bytes if conversion fails. Clients (x402, worm) expect AtomicBEEF format in the `tx` response field.
- **Graceful shutdown**: `run()` registers a ctrl-c signal handler via `tokio::signal::ctrl_c()` and uses `axum::serve`'s `with_graceful_shutdown()` to drain in-flight requests.
- **ServerConfig**: Holds optional `auth_token` (bearer auth) and optional `TlsConfig` (cert + key paths). Auth middleware checks bearer token only when configured; when `None`, all requests pass.

## Gotchas

- **`protocolID` capital D**: Serde's `rename_all = "camelCase"` produces `protocolId` (lowercase d), but the MetaNet Client sends `protocolID`. Every struct with this field needs `#[serde(alias = "protocolID")]`. Same for `keyID`. Missing this alias silently drops the field.
- **Binary data as JSON number arrays**: The MetaNet Client sends `data`, `signature`, `tx`, `ciphertext`, `hmac` as `[0,1,2,...]` JSON arrays, not hex strings. Serde deserializes `Vec<u8>` from number arrays by default, which happens to match — but if you ever switch to hex encoding you'll break wire compatibility.
- **signableTransaction reference double-encoding**: The SDK stores the reference as `Vec<u8>` from `String.into_bytes()`, then hex serde would double-encode it. The handler reverses this with `String::from_utf8(st.reference)` to recover the original cache key. Without this, `signAction`/`abortAction` can't find the pending transaction.
- **`inputs: Some(vec![])`**: `createAction` passes an empty inputs vec, not `None`. This signals to the SDK "I have no specific inputs, auto-select UTXOs" vs `None` which may have different behavior.
- **TLS is compile-time gated**: `--features tls` is required at build time. Requesting TLS without the feature compiled in returns a runtime error, not a compile error.
- **`/isAuthenticated` is always true**: It's a health check, not real auth verification. The actual auth gate is the bearer token middleware.
- **`parse_counterparty` special values**: Accepts `"self"` → `Counterparty::Self_`, `"anyone"` → `Counterparty::Anyone`, or a hex pubkey → `Counterparty::Other(PublicKey)`. Crypto endpoints pass counterparty as `Option<String>`, not required.
- **`parse_protocol_id` security levels**: Maps `0` → `Silent`, `1` → `App`, `2` → `Counterparty`. Any other value returns an error.
- **HMAC response is `.to_vec()`**: The `createHmac` handler returns `hmac.to_vec()` (the SDK returns `[u8; 32]`). This serializes as a JSON number array to match the MetaNet Client wire format.
- **`McCreateActionOutput` includes `basket`, `custom_instructions`, `tags`**: These optional fields are wired through to `CreateActionOutput`. Previously they were missing, which silently dropped basket assignments from HTTP callers like rust-bsv-worm.
- **`locking_script` is hex in JSON, bytes in SDK**: The `createAction` handler calls `hex::decode(&o.locking_script)` on each output. The MetaNet Client sends locking scripts as hex strings, but the SDK expects `Vec<u8>`.
- **`createAction` logs after dropping the spending lock**: The handler drops `_guard` explicitly before logging, so the tracing call doesn't hold the lock.
- **`no_send_change` and `send_with_results` use `serde_json::to_value`**: These fields are serialized to `serde_json::Value` to pass through whatever the SDK returns without needing concrete types.

## Related

- [Root CLAUDE.md](../../CLAUDE.md) — project architecture and conventions
- [Commands](../commands/CLAUDE.md) — CLI commands including `daemon` and `serve` that start this server
