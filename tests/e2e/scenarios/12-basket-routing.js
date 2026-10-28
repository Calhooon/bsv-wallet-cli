/**
 * E2E.12: Custom basket routing.
 * Tests that outputs internalized with basket insertion protocol
 * land in the correct basket and can be queried via listOutputs.
 *
 * Known limitation: The HTTP API doesn't support the `inputs` field in
 * createAction (hardcoded to empty), so custom basket outputs can't be
 * explicitly spent back. The 1K sats in the custom basket are a known
 * per-run cost of testing basket routing.
 *
 * Cost: ~1000 sats (stuck in custom basket) + fee
 */
const { ANYONE_KEY, buildP2PKH } = require('../lib/wallet-client');

module.exports = {
  name: 'basket-routing',
  description: 'Output basket assignment persists and is queryable via listOutputs',

  async run(walletA, walletB, assert) {
    const amount = 1_000;
    const basketName = 'e2e-test-basket';
    const suffix = Buffer.from('e2eBasket12').toString('base64');

    // Step 1: Verify basket starts empty on B
    const before = await walletB.client.listOutputs(basketName);
    assert(
      before.outputs.length === 0,
      `basket "${basketName}" should start empty on B`,
    );

    // Step 2: Get B's derived key for P2PKH
    const bKey = await walletB.client.getPublicKey(
      [2, '3241645161d8'], `SfKxPIJNgdI= ${suffix}`, ANYONE_KEY, true,
    );
    const lockingScript = buildP2PKH(bKey.publicKey);

    // Step 3: A creates and broadcasts action sending to B
    const result = await walletA.client.createAction(
      [{
        lockingScript,
        satoshis: amount,
        outputDescription: 'basket routing test',
        tags: ['e2e-basket-test'],
      }],
      'basket routing scenario',
    );
    assert(result.txid, 'createAction must return txid');
    assert(Array.isArray(result.tx), 'createAction must return tx bytes');

    // Step 4: B internalizes using "basket insertion" protocol into custom basket
    const internalized = await walletB.client.post('internalizeAction', {
      tx: result.tx,
      outputs: [{
        outputIndex: 0,
        protocol: 'basket insertion',
        insertionRemittance: {
          basket: basketName,
          tags: ['e2e-basket-test'],
        },
      }],
      description: 'basket insertion test',
    });
    assert(internalized.accepted === true, 'B must accept the tx');

    // Step 5: Query the custom basket on B — output should be there
    const after = await walletB.client.listOutputs(basketName);
    assert(
      after.outputs.length > 0,
      `basket "${basketName}" should have outputs after internalization, got ${after.outputs.length}`,
    );

    // Step 6: Verify the output has correct satoshis
    const found = after.outputs.find(o => o.satoshis === amount);
    assert(found, `should find output with ${amount} sats in basket "${basketName}"`);

    // Step 7: Try to recover — internalize same tx as wallet payment into default basket.
    // This may fail (wallet may reject duplicate internalization), which is acceptable.
    try {
      await walletB.client.internalizeAction(result.tx, [{
        outputIndex: 0,
        protocol: 'wallet payment',
        paymentRemittance: {
          derivationPrefix: 'SfKxPIJNgdI=',
          derivationSuffix: suffix,
          senderIdentityKey: ANYONE_KEY,
        },
      }], 'recover basket output to default');
      console.log(`       basket recovery: OK (output also in default basket)`);
    } catch (e) {
      console.log(`       basket recovery: skipped (${e.message.slice(0, 50)}) — ${amount} sats stuck in custom basket`);
    }

    return { txid: result.txid, wocConfirmed: 'n/a', sats: amount,
      knownLoss: `${amount} sats in custom basket (no HTTP inputs support)` };
  },
};
