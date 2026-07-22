# Reading a wallet balance — the `spendable` / basket trap

> **The one rule:** a wallet's usable balance is the sum of `spendable` outputs **in the
> change/`default` basket** — *never* a raw `SUM(satoshis) WHERE spendable = 1`.
> `spendable = 1` alone does **not** mean "money this wallet can spend."

If you only remember one line, remember that one. The rest of this document explains why
the naive query is wrong, what each output field actually means, and the exact right/wrong
queries — so this class of mistake stops recurring.

---

## The trap (a real case study)

A funding wallet that had paid out many times over a long session was inspected two ways:

```sql
-- ❌ THE NAIVE QUERY (WRONG — overcounts by millions)
SELECT SUM(satoshis) FROM outputs WHERE spendable = 1 AND spent_by IS NULL;
--> 8,262,180 sats across 158 rows
```

```
-- ✅ THE API (RIGHT)
bsv-wallet balance            # or  GET /funder/status .floatSats
--> 2,582,846 sats across 6 coins
```

The naive query overcounted by **5.6M sats**. Someone then "discovered" 152 phantom
"spendable" coins, invented a corruption theory, and nearly wrote a database-surgery tool to
"clean them up" — all to fix a number that was never wrong. The wallet's own API had reported
the correct usable balance the entire time.

**Nothing was corrupted. The query was wrong.** The 152 extra rows are *payment outputs* —
money this wallet **sent to other keys** — which are recorded but are not, and never were,
this wallet's spendable coins.

---

## Why a payment output has `spendable = 1`

This surprises people, so state it plainly: **`createAction` records every output it builds —
including payments to third parties — and the created-output default for `spendable` is `true`.**
This is intentional and matches the reference implementation
(`ts-stack/packages/wallet/wallet-toolbox/src/storage/methods/createAction.ts`:
`makeDefaultOutput` defaults `spendable: true`; the user-output branch does not override it).
The Rust storage layer mirrors this byte-for-semantics on purpose — divergence here would be a
parity bug, **not** a fix.

So `spendable` is **not** "this wallet controls the key and can spend it." What actually gates
whether an output is a usable coin is the **basket**:

| field | meaning | usable-coin? |
|---|---|---|
| `basket_id` = the change/`default` basket, `change = 1`, `provided_by = 'storage'` | a change output this wallet owns and can sign | **YES** — this is your float |
| `basket_id` = a named basket you asked to track | a tracked output (token, custom) | yes, within that basket |
| `basket_id` = `NULL`, `change = 0`, `provided_by = 'you'` | a **payment** you sent to someone else's key | **NO** — recorded for history; you cannot sign it |

**Coin selection is basket-scoped.** `createAction` allocates change inputs only from the
change basket (`count_change_inputs(user_id, change_basket.basket_id, …)`). A `basket_id = NULL`
payment output is *never* selected, can never cause a double-spend, and never blocks funding.
That is precisely why the naive `SUM(spendable=1)` is harmless-looking but meaningless: it sums
a superset that includes coins the selector correctly ignores.

---

## The correct way to read a balance

Always go through the API. It is basket-scoped and correct:

- **CLI:** `bsv-wallet balance` (sums `spendable` in the `default` basket; see
  `src/commands/balance.rs`).
- **Daemon / service:** whatever your service exposes on top of `listOutputs({ basket: 'default' })`
  (e.g. a `/status` endpoint's usable-float field). Never re-derive it from raw rows.
- **Never** `SUM(spendable=1)` over the whole `outputs` table.

If you *must* query SQL directly (debugging), scope it:

```sql
-- ✅ usable float, direct SQL (equivalent to `bsv-wallet balance`)
SELECT COALESCE(SUM(o.satoshis), 0)
FROM outputs o
JOIN output_baskets b ON b.basket_id = o.basket_id
WHERE o.spendable = 1 AND o.spent_by IS NULL AND b.name = 'default';
```

```sql
-- 🔎 to SEE the payment outputs the naive query wrongly includes:
SELECT COUNT(*), SUM(satoshis) FROM outputs
WHERE spendable = 1 AND spent_by IS NULL AND basket_id IS NULL;   -- payments, NOT your float
```

---

## Debugging checklist (paste this before you conclude "the wallet is corrupted")

1. Does `bsv-wallet balance` (or the service's usable-float field) match what you expect?
   If **yes**, the wallet is fine — your other query is the problem. Stop here.
2. If a "phantom spendable" row bothers you, check its `basket_id`, `change`, `provided_by`.
   `NULL / 0 / you` = it's a **payment you made**, not your coin. Working as designed.
3. Only if a **change-basket** coin (`basket_id = default, change = 1`) is wrong should you
   suspect real drift — and even then, reconcile against chain before touching the DB, and
   never demote a coin without a positive chain-spent proof.

**A wallet's balance is what its API says, not what a hand-rolled `SUM` says.**
