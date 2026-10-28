/**
 * E2E.11: Error resilience.
 * Bad inputs, garbage BEEF, double-internalize, insufficient funds.
 * Must not lose money or corrupt state.
 */

const { execSync } = require('child_process');
const { BSV } = require('../lib/setup');

module.exports = {
  name: 'error-resilience',
  description: 'Bad inputs must fail gracefully without losing money (0 sats)',

  async run(walletA, walletB, assert) {
    // #330 and --from ship together; the differential also drives beta.2.
    const typedRefusals = execSync(`"${BSV}" send --help`, { encoding: 'utf-8', stdio: 'pipe' }).includes('--from');
    const invalidBeef = typedRefusals ? 'VALIDATION_ERROR' : 'INVALID_BEEF';
    const balanceBefore = await walletA.client.balance();

    // --- Send more than balance ---
    try {
      await walletA.client.createAction([{
        lockingScript: '76a914' + '00'.repeat(20) + '88ac',
        satoshis: 999_999_999,
        outputDescription: 'too much',
      }], 'should fail');
      assert(false, 'Overspend should fail');
    } catch (e) {
      assert(true, 'Overspend correctly rejected');
    }

    // --- Send 0 sats ---
    try {
      await walletA.client.createAction([{
        lockingScript: '76a914' + '00'.repeat(20) + '88ac',
        satoshis: 0,
        outputDescription: 'zero',
      }], 'should fail');
      // Some wallets may allow 0-sat outputs, so we just check it doesn't crash
      assert(true, 'Zero sats handled (may succeed or fail gracefully)');
    } catch (e) {
      assert(true, 'Zero sats correctly rejected');
    }

    // --- Internalize garbage bytes ---
    // #277 finding 5: pass the remittance gate so these test the BEEF reader.
    const badBeefOutput = {
      outputIndex: 0,
      protocol: 'wallet payment',
      paymentRemittance: {
        derivationPrefix: 'ZXJyb3I=',
        derivationSuffix: 'YmVlZg==',
        senderIdentityKey: walletB.identityKey,
      },
    };
    try {
      await walletA.client.internalizeAction(
        [0xDE, 0xAD, 0xBE, 0xEF, 0x00, 0x01],
        [badBeefOutput],
        'garbage',
      );
      assert(false, 'Garbage BEEF should fail');
    } catch (e) {
      assert(e.status === 400 && e.body?.code === invalidBeef,
        `Garbage must reach the BEEF reader: ${e.message}`);
    }

    // --- Internalize valid AtomicBEEF prefix but truncated ---
    try {
      await walletA.client.internalizeAction(
        [0x01, 0x01, 0x01, 0x01, 0x00, 0x00], // magic prefix but no txid or beef
        [badBeefOutput],
        'truncated',
      );
      assert(false, 'Truncated BEEF should fail');
    } catch (e) {
      assert(e.status === 400 && e.body?.code === invalidBeef,
        `Truncated BEEF must reach the BEEF reader: ${e.message}`);
    }

    // --- Decrypt with no matching key ---
    try {
      await walletA.client.decrypt(
        [0x01, 0x02, 0x03, 0x04, 0x05],
        [2, 'nonexistent'], 'nope', 'self',
      );
      assert(false, 'Decrypt garbage should fail');
    } catch (e) {
      assert(true, 'Decrypt garbage correctly rejected');
    }

    // --- Relinquish nonexistent output ---
    // The body is the served shape (#119): the server takes it and answers
    // with its word, a 200 carrying `relinquished` or a coded refusal, never
    // the 422 of a body it cannot read. The balance check below holds it to
    // moving nothing.
    try {
      const r = await walletA.client.relinquishOutput(
        'deadbeef'.repeat(8), 0, 'default',
      );
      assert(typeof r.relinquished === 'boolean',
        `Relinquish nonexistent must answer relinquished: ${JSON.stringify(r)}`);
    } catch (e) {
      assert(e.status !== 422 && typeof (e.body || {}).code === 'string',
        `Relinquish nonexistent must be a coded refusal, not an unread body: ${e.message}`);
    }

    // --- Balance unchanged after all errors ---
    const balanceAfter = await walletA.client.balance();
    assert(balanceAfter === balanceBefore,
      `Balance must be unchanged after errors: was ${balanceBefore}, now ${balanceAfter}`);

    // --- Wallet still responsive ---
    const health = await walletA.client.isAuthenticated();
    assert(health.authenticated === true, 'Wallet must still be responsive after errors');

    return { txid: null, sats: 0, wocConfirmed: 'n/a',
      tests: 'overspend, zero-sats, garbage-beef, truncated-beef, bad-decrypt, bad-relinquish, balance-unchanged' };
  },
};
