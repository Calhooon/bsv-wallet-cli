# Changelog

Releases before 0.7.0 carry their notes in the release commit's message
(`RELEASING.md`).

## 0.7.3

The toolbox 0.7.4 (bsv-stack-lean #66: every broadcaster word is a hint; a
refusal on the immediate post is not an error), and the host's side of it:
no path of the CLI retires a transaction a broadcaster took, the served
doors answer a refused post with the tracker's word, and a coin whose
transaction drew no accepting word is not spent.

- **Dependencies.** `bsv-wallet-toolbox-rs` 0.7.4 (was 0.7.3). `fastrand`
  2.5.0 and `spin` 0.9.9 (were 2.4.0 and 0.9.8, both yanked, in the lock
  since 0.7.1): a lock-only move.
- **The vector.** `tests/vectors/arcade_status_verdicts.json` is the toolbox
  0.7.4's file, byte-identical (`cmp` against the crate's file); four
  `toolbox_push` values move to `none` (`push_rejected_467`,
  `push_rejected_466_conflict`, `push_double_spend_attempted`,
  `push_rejected_lower_case`): a pushed REJECTED or DOUBLE_SPEND_ATTEMPTED
  writes no word.
- **No retire of a transaction a broadcaster took** (the tracker's
  `abandoned` is the host's act on a template never announced; #24). An
  accepting word in the wallet's records (the proof request past the post,
  an `accepted`/`seen`/`mined` memory row, the tracker's word past `built`)
  keeps a transaction from every retire path: the served sweep (which
  retired any unproven transaction with a `rejected` memory row, now a
  hint), `reconcile-broadcasts`, `cleanup-abandoned` and the daemon's
  ticker (which abandoned one the broadcaster held past
  `BROADCAST_ABSENCE_MINUTES`, or found nowhere), the after-send follow-up,
  and `/abortAction` (409). It stays, re-asked, until a proof, a node
  verdict or a competitor's checked proof. A transaction no broadcaster took
  is retired as before; a confirmed competitor's spend still abandons.
- **The doors' word for a refused post.** "Transaction broadcast failed" no
  longer reaches the CLI, and the `BROADCAST_REJECTED` mapping that promised
  "the tx is already failed and its inputs released" is gone.
  `/createAction` and `/signAction` answering `sending`, and
  `/internalizeAction`, carry `broadcast: {word: "built", request, hint,
  inputs: "locked"}` (the request's status, the toolbox's
  `immediateBroadcastHint`); the MCP tools say the same. The inline
  follow-up's 502 promises a release only where the chain vouches the coin.
- **The spend guard.** A coin of a transaction whose immediate post drew no
  accepting word, with no status source holding it, is held (not
  spendable, recorded in `spend_guard_holds`) before every spend and on each
  served pass, and released once the toolbox promotes the request
  (`unmined`) or a proof completes it; a retire drops the hold.

### Upgrade

The wallet database gains `spend_guard_holds`; no existing table changes. A
held coin reads `spendable = 0` until released; the toolbox's balance does
not count it meanwhile. A client of the served doors that read 502
`BROADCAST_REJECTED` on a refused post now receives 200 with `sending` and
`broadcast.word = "built"`, and reads the refusal there or from the
monitor. A daemon that relied on the absence threshold, or on
`/abortAction`, to free the inputs of a transaction a broadcaster accepted
keeps them locked. The library's report structs (`ReconcileBroadcastsReport`,
`SweepReport`, `ServedPassReport`) and the doors' response types gain a
field each. Rollback: pin 0.7.2 (with the toolbox 0.7.3); `spend_guard_holds`
is left unread, and a coin it held stays `spendable = 0` until set back by
hand (`UPDATE outputs SET spendable = 1 WHERE output_id IN (SELECT
output_id FROM spend_guard_holds)`).

## 0.7.2

The toolbox 0.7.3 and bsv-rs 0.4.3, moved together; no line of the CLI's
code changed, one test added.

- **Dependencies.** `bsv-wallet-toolbox-rs` 0.7.3 (was 0.7.0) and the CLI's
  own `bsv-rs` (`bsv-sdk`) 0.4.3 (was 0.4.1), in one commit, so the graph
  holds one bsv-rs; `bsv-tracker` 0.2.0 resolves the same 0.4.3.
- **A transaction with no output is invalid bytes** (bsv-rs 0.4.3,
  bsv-stack-lean #59; the node's rule, bsv-script-lean@87f0461
  `lean/BsvScript/TxRules.lean:85-86`), as one with no input has been since
  0.7.0. The CLI's reader of a stranger's BEEF (`atomic_beef`, the
  toolbox's `refuse_invalid_beef_bytes`, which the served doors also call)
  refuses one at the transaction's offset, kind `NoOutputs`, under a BUMP
  too; the witness `a_transaction_with_no_output_is_refused_at_its_offset`
  fails on 0.7.1's dependencies (the BEEF taken) and passes here. No fixture
  of the CLI carries a transaction with no output, and no code of the CLI
  matches on bsv-rs's `Kind` or `Reason`, so the new variant breaks no match
  here.
- **What the toolbox 0.7.1 to 0.7.3 brings to the daemon.** A txid-only
  entry is valid only when the BEEF proves it, except that `createAction`
  with `trustSelf: 'known'` (the wallet's default) resolves an entry the
  wallet's own storage holds before it verifies. ARC is posted the plain BEEF
  as written, and ARC's 400 is a request fault that schedules a re-ask, not
  the transaction's rejection. The monitor retires no announced transaction
  on transient words or by age: a `sending` transaction stays `sending`, its
  inputs locked, re-asked on a 1, 2, 4 ... 64 minute cadence, until a
  definitive word, a proof, or the host's explicit retire.

### Upgrade

Nothing stored is migrated. A daemon that relied on the toolbox failing a
transaction stuck in `sending` (after seven attempts, or five minutes) now
keeps it announced and its inputs locked; the CLI's own retire paths
(`broadcast_reconcile`'s absence threshold, `/abortAction`, the release rule
on a definitive rejection) are unchanged. Rollback: pin 0.7.1.

## 0.7.1

- **No open bind without a token.** `serve`, `daemon` and `serve-fleet`
  refuse at startup, before the wallet is opened or a socket bound, to bind
  an address beyond loopback (`BIND_ADDR`, IPv4 or IPv6) when no bearer token
  (`AUTH_TOKEN`, an empty value counting as none) is set. The error names the
  one flag, `--allow-no-token`, that permits the open bind deliberately. A
  loopback bind without a token is unchanged.
- **The risk it closes.** Since 0.7.0 the served `/internalizeAction` and
  `/createAction` doors take a body of any size, auth before the body; a
  daemon bound to `0.0.0.0` with no token was a wallet any host on the
  network could drive, and made to hold any body it sent.

### Upgrade

A daemon started with `BIND_ADDR` beyond loopback and no `AUTH_TOKEN` now
exits non-zero at startup: set `AUTH_TOKEN` (and send it as
`Authorization: Bearer`), or add `--allow-no-token` to keep the open bind.
The library gains `server::refuse_open_bind`,
`server::refuse_open_bind_from_env` and `server::bind_addr_from_env`;
`server::run` itself does not check, so a host calling it directly checks
first.

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
