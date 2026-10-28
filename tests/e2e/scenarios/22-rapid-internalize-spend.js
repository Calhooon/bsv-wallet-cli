/**
 * E2E.22: Rapid internalize-then-spend (phantom UTXO prevention).
 *
 * This scenario specifically tests the fix for the double-spend bug where:
 * 1. Wallet A sends to B (B internalizes)
 * 2. B immediately spends the internalized output
 * 3. Repeat 10 times in rapid succession
 *
 * Before the fix, some of B's internalized outputs were "phantom" —
 * they existed in the DB but their parent tx was never broadcast.
 * When B tried to spend them, the broadcast failed with "Missing inputs".
 *
 * The fix ensures:
 * - Internalized txs get proven_tx_req status 'unsent' (monitor retries)
 * - No courtesy broadcast (avoids phantom UTXOs)
 * - Full BEEF sent when ancestors are unproven (not EF)
 *
 * Cost: 10 × 5000 sats + fees ≈ 51,000 sats (recoverable via sweep)
 */
const { ANYONE_KEY, buildP2PKH } = require('../lib/wallet-client');

module.exports = {
  name: 'rapid-internalize-spend',
  description: '10 rapid internalize+spend cycles — no phantom UTXOs',

  async run(walletA, walletB, assert) {
    const cycles = 10;
    const sendAmount = 5_000;
    const returnAmount = 2_000;
    const results = [];

    const aBalanceBefore = await walletA.client.balance();
    assert(aBalanceBefore >= sendAmount * cycles,
      `A needs at least ${sendAmount * cycles} sats, has ${aBalanceBefore}`);

    for (let i = 0; i < cycles; i++) {
      const tag = Buffer.from(`e2e22-cycle${i}`).toString('base64');
      const fwd = Buffer.from('fwd').toString('base64');
      const ret = Buffer.from('ret').toString('base64');

      // A sends to B
      const bPubkey = await walletB.client.getPublicKey(
        [2, '3241645161d8'], `${tag} ${fwd}`, ANYONE_KEY, true,
      );

      const fwdResult = await walletA.client.createAction([{
        lockingScript: buildP2PKH(bPubkey.publicKey),
        satoshis: sendAmount,
        outputDescription: `cycle ${i + 1} forward`,
      }], `E2E.22 cycle ${i + 1}/${cycles}: A→B`);
      assert(fwdResult.txid, `Cycle ${i + 1} A→B should succeed, got: ${JSON.stringify(fwdResult).slice(0, 100)}`);

      // B internalizes immediately
      await walletB.client.internalizeAction(fwdResult.tx, [{
        outputIndex: 0,
        protocol: 'wallet payment',
        paymentRemittance: {
          derivationPrefix: tag,
          derivationSuffix: fwd,
          senderIdentityKey: ANYONE_KEY,
        },
      }], `E2E.22 cycle ${i + 1}: internalize from A`);

      // B immediately spends the internalized output back to A
      const aPubkey = await walletA.client.getPublicKey(
        [2, '3241645161d8'], `${tag} ${ret}`, ANYONE_KEY, true,
      );

      const retResult = await walletB.client.createAction([{
        lockingScript: buildP2PKH(aPubkey.publicKey),
        satoshis: returnAmount,
        outputDescription: `cycle ${i + 1} return`,
      }], `E2E.22 cycle ${i + 1}/${cycles}: B→A`);
      assert(retResult.txid, `Cycle ${i + 1} B→A should succeed, got: ${JSON.stringify(retResult).slice(0, 100)}`);

      // A internalizes the return
      await walletA.client.internalizeAction(retResult.tx, [{
        outputIndex: 0,
        protocol: 'wallet payment',
        paymentRemittance: {
          derivationPrefix: tag,
          derivationSuffix: ret,
          senderIdentityKey: ANYONE_KEY,
        },
      }], `E2E.22 cycle ${i + 1}: internalize return from B`);

      results.push({
        cycle: i + 1,
        fwdTxid: fwdResult.txid.slice(0, 12),
        retTxid: retResult.txid.slice(0, 12),
      });

      console.log(`       Cycle ${i + 1}/${cycles}: A→B=${fwdResult.txid.slice(0, 8)}... B→A=${retResult.txid.slice(0, 8)}...`);
    }

    // All cycles must have succeeded (no Missing inputs / double spend)
    assert(results.length === cycles, `All ${cycles} cycles should complete`);

    // Verify balances changed appropriately
    const aBalanceAfter = await walletA.client.balance();
    const bBalance = await walletB.client.balance();
    console.log(`       A: ${aBalanceBefore} → ${aBalanceAfter} (Δ=${aBalanceAfter - aBalanceBefore})`);
    console.log(`       B balance: ${bBalance}`);

    return {
      cycles,
      succeeded: results.length,
      firstFwd: results[0]?.fwdTxid,
      lastRet: results[results.length - 1]?.retTxid,
    };
  },
};
