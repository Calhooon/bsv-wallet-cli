# Quality Loop: E2E Wallet Health Verification

Runnable scenarios for verifying wallet health after UTXO fixes.
All commands assume a funded `bsv-wallet daemon` running on port 3322.
Each call to the wallet sends `Origin: http://localhost`, which the daemon's
`ALLOWED_ORIGINS` must list (the browser gate, #411).

```
WALLET=http://localhost:3322
```

---

## 1. Basic Broadcast

**Description**: Create a transaction with an OP_RETURN output and verify it reaches the network.

**Preconditions**: Daemon running, wallet funded with at least 1000 sats.

**Steps**:
```bash
# Create and broadcast an OP_RETURN transaction
RESULT=$(curl -s -X POST -H 'Origin: http://localhost' $WALLET/createAction \
  -H "Content-Type: application/json" \
  -d '{
    "description": "quality-loop: basic broadcast",
    "outputs": [{
      "lockingScript": "006a0b7175616c6974792d6c6f6f70",
      "satoshis": 0,
      "outputDescription": "OP_RETURN test"
    }]
  }')

TXID=$(echo $RESULT | jq -r '.txid')
echo "txid: $TXID"
```

**Expected Results**:
- Response contains `txid` (64-char hex string)
- No error in response

**Verify**:
```bash
# Check WoC for confirmation (may take a few seconds to propagate)
curl -s "https://api.whatsonchain.com/v1/bsv/main/tx/$TXID" | jq '.txid'
```

---

## 2. Concurrent Spending

**Description**: Fire 5 createAction requests simultaneously. The SpendingLock should serialize them so all succeed without UTXO conflicts.

**Preconditions**: Daemon running, wallet has at least 5 spendable UTXOs (run `bsv-wallet split` first if needed).

**Steps**:
```bash
# Fire 5 concurrent OP_RETURN transactions
for i in $(seq 1 5); do
  curl -s -X POST -H 'Origin: http://localhost' $WALLET/createAction \
    -H "Content-Type: application/json" \
    -d "{
      \"description\": \"quality-loop: concurrent #$i\",
      \"outputs\": [{
        \"lockingScript\": \"006a0e636f6e63757272656e742d$i\",
        \"satoshis\": 0,
        \"outputDescription\": \"concurrent test $i\"
      }]
    }" &
done
wait

# Collect results
echo "All 5 requests completed"
```

**Expected Results**:
- All 5 requests return a `txid`
- No errors about locked UTXOs or database contention
- Each txid is unique

**Verify**:
```bash
# List recent actions and confirm 5 new entries
curl -s -X POST -H 'Origin: http://localhost' $WALLET/listActions \
  -H "Content-Type: application/json" \
  -d '{"labels": [], "includeLabels": true, "limit": 5}' | jq '.totalActions'
```

---

## 3. noSend Round-Trip

**Description**: Create a signed transaction with `noSend: true`, verify the output is NOT spendable, then verify it is NOT picked as an input for a subsequent transaction.

**Preconditions**: Daemon running, wallet funded.

**Steps**:
```bash
# Step 1: Create a noSend transaction
NOSEND=$(curl -s -X POST -H 'Origin: http://localhost' $WALLET/createAction \
  -H "Content-Type: application/json" \
  -d '{
    "description": "quality-loop: noSend test",
    "outputs": [{
      "lockingScript": "006a076e6f2d73656e64",
      "satoshis": 0,
      "outputDescription": "noSend OP_RETURN"
    }],
    "options": { "noSend": true }
  }')

NOSEND_TXID=$(echo $NOSEND | jq -r '.txid')
NOSEND_TX=$(echo $NOSEND | jq -r '.tx')
echo "noSend txid: $NOSEND_TXID"

# Step 2: Verify the tx bytes are returned (number array)
echo $NOSEND | jq '.tx | length'

# Step 3: Create a normal transaction to confirm it picks a different UTXO
NORMAL=$(curl -s -X POST -H 'Origin: http://localhost' $WALLET/createAction \
  -H "Content-Type: application/json" \
  -d '{
    "description": "quality-loop: after noSend",
    "outputs": [{
      "lockingScript": "006a0b61667465722d6e6f73656e64",
      "satoshis": 0,
      "outputDescription": "after noSend"
    }]
  }')

NORMAL_TXID=$(echo $NORMAL | jq -r '.txid')
echo "normal txid: $NORMAL_TXID"
```

**Expected Results**:
- `noSend` response includes `txid` and `tx` (raw transaction bytes as number array)
- The noSend transaction does NOT appear on WoC (not broadcast)
- The subsequent normal transaction succeeds (noSend inputs were not double-allocated)

**Verify**:
```bash
# noSend tx should NOT be on chain
curl -s "https://api.whatsonchain.com/v1/bsv/main/tx/$NOSEND_TXID" | jq '.error // "not found"'

# Normal tx SHOULD be on chain
curl -s "https://api.whatsonchain.com/v1/bsv/main/tx/$NORMAL_TXID" | jq '.txid'
```

---

## 4. Broadcast Failure Recovery (Synthetic)

**Description**: Verify that when a broadcast fails, inputs are restored and the next createAction succeeds with a different UTXO. This requires inspecting the database directly since we cannot easily force a broadcast failure via the API.

**Preconditions**: Access to the wallet SQLite database.

**Steps**:
```bash
# Step 1: Check current balance
BEFORE=$(curl -s -X POST -H 'Origin: http://localhost' $WALLET/wallet_balance \
  -H "Content-Type: application/json" -d '{}')
echo "Balance before: $BEFORE"

# Step 2: Query the DB for any failed transactions with locked outputs
# (These would be left over from real broadcast failures)
sqlite3 ~/.bsv-wallet/wallet.db "
  SELECT t.transaction_id, t.txid, t.status, COUNT(o.output_id) as locked_outputs
  FROM transactions t
  LEFT JOIN outputs o ON o.spent_by = t.transaction_id AND o.spendable = 0
  WHERE t.status = 'failed'
  GROUP BY t.transaction_id
  HAVING locked_outputs > 0;
"

# Step 3: If any locked outputs exist from failed txs, review_status should clean them
# The daemon runs review_status periodically. Force it by restarting or waiting.

# Step 4: Verify balance is restored after cleanup
AFTER=$(curl -s -X POST -H 'Origin: http://localhost' $WALLET/wallet_balance \
  -H "Content-Type: application/json" -d '{}')
echo "Balance after: $AFTER"
```

**Expected Results**:
- No outputs should remain locked by failed transactions (the query in step 2 returns empty)
- If it does return rows, `review_status` will release them on the next monitor cycle
- Balance should not decrease from failed transactions

**Verify**:
```bash
# Confirm no orphaned locks remain
sqlite3 ~/.bsv-wallet/wallet.db "
  SELECT COUNT(*) FROM outputs
  WHERE spent_by IN (SELECT transaction_id FROM transactions WHERE status = 'failed')
    AND spendable = 0;
"
# Expected: 0
```

---

## 5. Balance Accuracy

**Description**: Verify that `wallet_balance` matches the actual sum of spendable outputs from completed/unproven parent transactions.

**Preconditions**: Daemon running.

**Steps**:
```bash
# Step 1: Get reported balance via API
API_BALANCE=$(curl -s -X POST -H 'Origin: http://localhost' $WALLET/wallet_balance \
  -H "Content-Type: application/json" -d '{}' | jq -r '.balance // .total_satoshis')
echo "API balance: $API_BALANCE"

# Step 2: Query DB for ground-truth spendable sum
DB_BALANCE=$(sqlite3 ~/.bsv-wallet/wallet.db "
  SELECT COALESCE(SUM(o.satoshis), 0)
  FROM outputs o
  JOIN output_baskets ob ON o.basket_id = ob.basket_id
  WHERE o.spendable = 1
    AND ob.name = 'default';
")
echo "DB balance: $DB_BALANCE"

# Step 3: Compare
if [ "$API_BALANCE" = "$DB_BALANCE" ]; then
  echo "PASS: balances match"
else
  echo "FAIL: API=$API_BALANCE DB=$DB_BALANCE"
fi
```

**Expected Results**:
- API balance and DB balance match exactly

**Verify**:
- If they differ, check for outputs with `spendable=1` whose parent transaction is in a non-terminal state (unsigned, sending, etc.)

---

## 6. Ghost UTXO Detection

**Description**: List all spendable outputs and spot-check a sample against WoC's `/spent` endpoint to verify none are already spent on-chain.

**Preconditions**: Daemon running, network access to WoC.

**Steps**:
```bash
# Step 1: Pull a sample of spendable outputs from the DB
sqlite3 ~/.bsv-wallet/wallet.db "
  SELECT o.txid, o.vout
  FROM outputs o
  JOIN output_baskets ob ON o.basket_id = ob.basket_id
  WHERE o.spendable = 1 AND ob.name = 'default'
  ORDER BY RANDOM()
  LIMIT 10;
" | while IFS='|' read txid vout; do
  # Step 2: Check if output is spent on-chain
  STATUS=$(curl -s "https://api.whatsonchain.com/v1/bsv/main/tx/$txid/out/$vout/spent")

  if echo "$STATUS" | jq -e '.spent' 2>/dev/null | grep -q true; then
    echo "GHOST: $txid:$vout is spent on-chain but spendable in wallet!"
  else
    echo "OK: $txid:$vout"
  fi
done
```

**Expected Results**:
- All sampled outputs report as unspent on WoC
- No "GHOST" lines appear

**Verify**:
- If ghost UTXOs are found, check the spending txid on WoC and cross-reference with wallet DB
- Ghost UTXOs indicate a failed broadcast that succeeded on-chain, or an external spend the wallet missed
- Fix: mark the spending transaction in the wallet, or manually set `spendable=0` on the ghost output
