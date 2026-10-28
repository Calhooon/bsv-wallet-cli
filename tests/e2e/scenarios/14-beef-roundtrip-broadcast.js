/**
 * E2E.14: BEEF round-trip broadcast.
 *
 * Tests that a transaction created via createAction produces valid BEEF
 * that ARC miners accept. This specifically catches the BEEF serialization
 * round-trip bug where raw tx bytes were lost during parsing, causing
 * truncated BEEF output rejected by miners.
 *
 * Steps:
 *   1. A sends sats to B via HTTP createAction (not CLI) — forces BEEF construction
 *   2. Verify the txid is returned (no broadcast failure)
 *   3. B receives via listOutputs (confirms the UTXO exists)
 *   4. B sends back to A (proves the received UTXO's BEEF ancestry is valid)
 *   5. Verify both txids on WoC
 */
const { verifyOnChain } = require('../lib/woc');
const { ANYONE_KEY, buildP2PKH } = require('../lib/wallet-client');

module.exports = {
  name: 'beef-roundtrip-broadcast',
  description: 'createAction produces valid BEEF that miners accept (round-trip)',

  async run(walletA, walletB, assert) {
    const prefix = Buffer.from('e2e-beef-14').toString('base64');
    const payment = Buffer.from('payment-1').toString('base64');
    const returned = Buffer.from('return-1').toString('base64');
    // Get B's identity key for payment derivation
    const bIdentity = await walletB.client.identityKey();
    assert(bIdentity && bIdentity.length === 66, `B identity key should be 66 hex chars, got ${bIdentity?.length}`);

    // Step 1: A creates a payment to B via HTTP createAction
    // This exercises the full BEEF construction path:
    //   pick input UTXOs → sign → build BEEF with ancestors → broadcast via ARC
    // Use ANYONE_KEY + forSelf=true (same pattern as scenario 01)
    const bPubkey = await walletB.client.getPublicKey(
      [2, '3241645161d8'],       // BRC-29 payment protocol
      `${prefix} ${payment}`,   // unique key_id for this scenario
      ANYONE_KEY,                // counterparty = anyone (standard payment derivation)
      true,                      // forSelf = true (B derives own receiving key)
    );
    const lockingScript = buildP2PKH(bPubkey.publicKey);

    const sendAmount = 5000; // 5000 sats — enough to be meaningful
    const result = await walletA.client.post('createAction', {
      allowPending: walletA.client.allowPending,
      description: 'E2E BEEF round-trip test: A→B',
      outputs: [{
        lockingScript,
        satoshis: sendAmount,
        outputDescription: 'payment to B',
      }],
    });

    assert(result.txid, `createAction should return txid, got: ${JSON.stringify(result).slice(0, 200)}`);
    assert(result.txid.length === 64, `txid should be 64 hex chars, got ${result.txid.length}`);
    console.log(`    A→B txid: ${result.txid}`);

    // Step 2: B internalizes the payment
    // Get the tx bytes and have B internalize
    const txBytes = result.tx; // array of u8
    assert(txBytes && txBytes.length > 0, 'createAction should return tx bytes');

    await walletB.client.post('internalizeAction', {
      tx: txBytes,
      outputs: [{
        outputIndex: 0, // first output is our payment
        protocol: 'wallet payment',
        paymentRemittance: {
          derivationPrefix: prefix,
          derivationSuffix: payment,
          senderIdentityKey: ANYONE_KEY,
        },
      }],
      description: 'E2E BEEF round-trip: internalize from A',
    });

    // Step 3: B sends back to A — this proves B can spend the UTXO
    // (the UTXO's BEEF ancestry must be valid for B's createAction to broadcast)
    const aPubkey = await walletA.client.getPublicKey(
      [2, '3241645161d8'],
      `${prefix} ${returned}`,
      ANYONE_KEY,
      true,
    );
    const aLockingScript = buildP2PKH(aPubkey.publicKey);

    const returnResult = await walletB.client.post('createAction', {
      allowPending: walletB.client.allowPending,
      description: 'E2E BEEF round-trip test: B→A (return)',
      outputs: [{
        lockingScript: aLockingScript,
        satoshis: 2000,
        outputDescription: 'return payment to A',
      }],
    });

    assert(returnResult.txid, `B→A createAction should return txid, got: ${JSON.stringify(returnResult).slice(0, 200)}`);
    assert(returnResult.txid.length === 64, `B→A txid should be 64 hex chars`);
    console.log(`    B→A txid: ${returnResult.txid}`);

    // A internalizes the return payment so the sats are tracked
    await walletA.client.post('internalizeAction', {
      tx: returnResult.tx,
      outputs: [{
        outputIndex: 0,
        protocol: 'wallet payment',
        paymentRemittance: {
          derivationPrefix: prefix,
          derivationSuffix: returned,
          senderIdentityKey: ANYONE_KEY,
        },
      }],
      description: 'E2E BEEF round-trip: internalize return from B',
    });

    // Step 4: Verify both on WoC
    const woc1 = await verifyOnChain(result.txid, { maxRetries: 8, baseDelay: 1500 });
    assert(woc1.confirmed, `A→B tx ${result.txid} must appear on WoC`);

    const woc2 = await verifyOnChain(returnResult.txid, { maxRetries: 8, baseDelay: 1500 });
    assert(woc2.confirmed, `B→A tx ${returnResult.txid} must appear on WoC`);

    return {
      txA2B: result.txid,
      txB2A: returnResult.txid,
      wocConfirmed: true,
    };
  },
};
