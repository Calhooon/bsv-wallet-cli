# Time-locked BSV gifts

Pay someone in BSV such that **they fully own the key from day one** — they can
back it up and email it to themselves — but **the network itself won't let them
spend the coins until an unlock date you choose.** After the date, it's one
command to claim, and the money lands as normal, spendable wallet balance.

It's a Bitcoin **covenant**: the locking script reads the spending transaction's
own `nLockTime`/`nSequence` and refuses to validate unless the unlock time has
passed (`OP_CHECKLOCKTIMEVERIFY` is a no-op on BSV post-Genesis, so this is the
correct mechanism). Consensus then refuses to mine any early spend.

---

## Install (recipient)

```bash
# 1. Rust toolchain (once)
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh

# 2. bsv-wallet-cli
cargo install --git https://github.com/Calhooon/bsv-wallet-cli.git
#   …or:  curl -sSf https://raw.githubusercontent.com/Calhooon/bsv-wallet-cli/main/install.sh | sh
```

## Receiving a gift

```bash
bsv-wallet init                 # generate your wallet (a fresh, never-spent key)
bsv-wallet backup               # tar your .env + wallet.db; email it to yourself,
                                # and write down the printed SHA256 as your record
bsv-wallet address              # prints your address AND your gift pubkey:
#   1ABC...                       <- normal funding address
#   pubkey (for receiving gifts): 03d68a1f...   <- SEND THIS to the giver
```

Send the **pubkey** line to whoever's gifting you. That's public — safe to text
or email. Your private key never leaves your machine.

> You don't pay anything, sign anything, or run anything to *receive* a gift.

## Sending a gift (giver)

A gift is **not** a normal "send BSV to an address." `gift-send` builds a
**covenant** — a custom script that bakes in the recipient's pubkey *and* the
unlock date — which is the whole reason the coins can't be spent early. You can
only create it with this tool; sending plain BSV to someone's address gives them
coins they can spend immediately (no time-lock).

**Step 1 — fund your own wallet** (this part you can do from *any* wallet —
exchange, HandCash, hardware, MetaNet):

```bash
bsv-wallet address     # your funding address — send BSV here from anywhere
bsv-wallet sync        # pull the received coins into your spendable balance
bsv-wallet balance     # confirm it arrived
```

**Step 2 — send the gift** (must be this tool — it builds the covenant):

```bash
bsv-wallet gift-send <recipient-pubkey> <sats> --unlock 2026-12-25
#   <recipient-pubkey>  the recipient's `pubkey (for receiving gifts)` line —
#                       a 33-byte compressed pubkey, NOT an address
#   --unlock  accepts a date (YYYY-MM-DD), an RFC-3339 timestamp, a unix time,
#             or a relative offset like +3mo / +90d / +1y
#   --fee-utxo N  (default 1000, minimum 500) bundles a small recipient-owned
#             UTXO so the recipient can pay the ~155-sat claim fee with no funds
#             of their own. Keep the default unless you have a reason not to.
```
This prints the gift transaction id. Give it to the recipient. (Always confirm it
landed: `https://whatsonchain.com/tx/<txid>`.)

## Seeing your gift (recipient) — no claiming

```bash
bsv-wallet gift-inspect <gift-txid>
```
Shows that it's locked to **your** key, proves your signature can open it, the
unlock time, and an **estimated claimable block height** — without broadcasting
anything.

> You do **not** need to "receive", import, or internalize the gift before
> inspecting or claiming it. `gift-inspect` and `gift-claim` read the gift
> straight from the chain by its txid. The only "internalize" step is
> `bsv-wallet sync` *after* you claim — that's what turns the freed coins into
> normal spendable balance.

## Claiming after unlock

```bash
bsv-wallet gift-claim <gift-txid>
```
Before the unlock time it politely refuses. After, it builds + broadcasts the
claim; the coins land at your wallet's deposit address. Then:

```bash
bsv-wallet sync                 # pull the claimed coins into your balance
bsv-wallet balance              # there it is
bsv-wallet send <addr> <sats>   # spend it like any other balance
```

## Timing note (important)

BSV decides "is this final yet?" using **median-time-past** — the median
timestamp of the last 11 blocks — which trails real-world time by roughly an
hour. So a gift that unlocks at 3:00 PM becomes claimable a bit *after* 3:00 PM
(typically within an hour or two). `gift-inspect` shows the estimate. It's exact
to the block, not to the minute — fine for a gift, just don't promise a precise
clock time.

## Supported unlock dates (read this)

Unlock times are stored as a 4-byte on-chain number, which covers every date up
to **2038-01-19 03:14:07 UTC**. Any normal gift — days, months, or years out —
is well within range and is the path that's been **proven end-to-end on mainnet**
(deposit → locked → released after unlock → claimed → spent).

Dates **past January 2038** would use a wider 5-byte encoding whose *claim* path
has only been unit-tested, **not yet exercised with real coins on mainnet**.
`gift-send` will warn you if you pick such a date. Until that path is mainnet-
proven, **don't lock real value past Jan 2038** — pick an earlier unlock.

## What's guaranteed

- The recipient **cannot get a confirmed spend before the unlock time** — proven
  on mainnet: any transaction crafted to be mineable early is rejected by the
  network at script evaluation.
- The recipient **fully owns the key** the whole time and can back it up.
- After unlock it's a normal, spendable coin — no middleman, no custodian.
