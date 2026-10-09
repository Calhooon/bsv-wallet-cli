# Changelog

Releases before 0.7.0 carry their notes in the release commit's message
(`RELEASING.md`).

## 0.7.0

The 0.4 line: bsv-rs 0.4.1 under the CLI, the toolbox and the tracker, one
copy in the graph, and the posture that a valid BEEF is never refused for its
size or its counts; a BEEF is refused only for invalid bytes, at their offset.

- **Dependencies, moved together.** `bsv-wallet-toolbox-rs` 0.7.0,
  `bsv-tracker` 0.2.0 and the CLI's own `bsv-rs` (`bsv-sdk`) 0.4.1. The
  tracker 0.2.0 takes bsv-rs 0.4's `MerklePath`, so the three move in one
  commit; with any one left behind the graph holds two bsv-rs copies and the
  CLI does not compile.
- **A transaction with no input is invalid bytes** (bsv-rs 0.4.1). A BEEF
  carrying one is refused at the transaction's leading byte (`NoInputs`),
  where 0.6.0 internalized it when no chain tracker was set.
- **The commands' BEEF funnel.** `fund`, `receive` and `gift-claim` hand a
  stranger's BEEF to the wallet through `atomic_beef::ensure_atomic`, which
  now reads it with the streaming reader first: a refusal names the byte in
  the caller's own bytes and the reader's kind (`Invalid BEEF at byte N:
  Kind`). `receive` reads the courier's bytes the same way before it matches
  an output.
- **The served wallet's BEEF doors.** `/internalizeAction` (`tx`) and
  `/createAction` (`inputBEEF`) no longer refuse a body over 50 MiB (0.6.0:
  413 `TOO_LARGE`, a valid BEEF of about 13 MB and more refused for its
  size); every other route keeps the 50 MiB cap. An invalid BEEF is 400
  `INVALID_BEEF`, the message naming the offset and the kind.
- **Auth before the body.** The bearer token is checked before any request
  body is read, so the uncapped doors are open only to the wallet's caller.

### The bounds that stay, named

- The served doors hold the JSON body whole (the BRC-100 JSON wire carries
  the BEEF as one array, about four bytes of body per byte of BEEF): memory
  linear in the BEEF, never a refusal. With no bearer token set (the default,
  bound to 127.0.0.1), any local caller can send a body of any size.
- The commands hold a BEEF whole: a hex argument (`fund`), a courier's
  response body (`receive`), the toolbox's argument (`internalizeAction`'s
  `tx`, one `Vec<u8>`).
- `/arc-callback` and the relay keep a 1,000,000-byte cap; they carry a
  broadcaster's status and a merkle path, never a BEEF.

### Upgrade

The library's public API names the toolbox's and bsv-rs's types
(`server::make_wallet_state` takes the toolbox's `Wallet`; `tracker_host`
binds the tracker's and bsv-rs's), so a host linking `bsv_wallet_cli` moves
to the toolbox 0.7.0 and bsv-rs 0.4.1 with it, and one matching on the
toolbox's `Error` meets `Error::InvalidBeef { offset, kind }`. A payer whose wallet emits a BEEF with an input-less transaction is
refused where 0.6.0 credited it; the node would not mine such a transaction.
