/**
 * E2E.23: The scoped word (bsv-wallet-cli #63, 0.18.0).
 *
 * `allowPending: { received: ['txid:vout'] }` offers only the named received
 * coins under R1 (d)'s word; every other received unproven coin is held as
 * without the word, its reason naming the scope. A named coin that is not a
 * received coin the wallet could spend under it (spent, not the wallet's) is
 * the caller's 400 before the door.
 *
 * A third wallet, C, holds exactly two received unproven coins, both paid by
 * A in this scenario (so neither can be mined yet, whatever block came while
 * the earlier scenarios ran):
 *   1. under the scope {P1}, 3,500 sats is refused 402: P1 (3,000) alone does
 *      not cover, P2 is held and listed "not named under allowPending:
 *      received", P1 is not in the listing (it was offered);
 *   2. under the scope {P1}, 2,000 sats goes through on P1 alone: P2 is still
 *      C's, P1 is spent;
 *   3. naming P1 now (spent) is 400 VALIDATION_ERROR "spent by ...";
 *   4. naming a coin that is not C's is 400 "not a coin of this wallet";
 *   5. the plain word ("received") offers P2 and the rest goes back to A.
 * Cost: fees only (A pays C twice, C pays A twice).
 */
const { startWallet, teardownWallet } = require('../lib/setup');
const { ANYONE_KEY, buildP2PKH } = require('../lib/wallet-client');

const C_PORT = parseInt(process.env.WALLET_C_PORT || '3372');

module.exports = {
  name: 'scoped-allow-pending',
  description: 'allowPending {received: [coin]} offers the named received coin only; the other is held and named',

  async run(walletA, walletB, assert) {
    const walletC = await startWallet(C_PORT, 'wallet-C');
    let keep = false;
    try {
      const aKey = await walletA.client.getPublicKey(
        [2, '3241645161d8'], 'SfKxPIJNgdI= NaGLC6fMH50=', ANYONE_KEY, true,
      );
      const payA = (satoshis, outputDescription) => [{
        lockingScript: buildP2PKH(aKey.publicKey), satoshis, outputDescription,
      }];
      const outputsOf = async () =>
        ((await walletC.client.listOutputs('default', { limit: 100, includeEnvelope: false })).outputs || []);
      const refusalOf = async (fn) => {
        try { await fn(); } catch (e) { return e; }
        return null;
      };

      // Two received coins in C, both unproven: A's two payments.
      const p1 = await walletA.client.transferTo(walletC.client, 3_000, 'E2E.23 P1');
      const p2 = await walletA.client.transferTo(walletC.client, 3_000, 'E2E.23 P2');
      const before = await outputsOf();
      const c1 = before.find(o => o.outpoint.startsWith(p1.txid));
      const c2 = before.find(o => o.outpoint.startsWith(p2.txid));
      assert(c1 && c2, `C holds both payments (${before.length} outputs: ${before.map(o => o.outpoint.slice(0, 12)).join(', ')})`);
      assert(before.length === 2, `C holds exactly the two payments, has ${before.length}`);
      const name = (o) => o.outpoint.replace('.', ':');
      const scope = { received: [name(c1)] };

      // 1. Over the scope: P1 alone does not cover 3,500; P2 is held, named.
      const over = await refusalOf(() => walletC.client.createAction(
        payA(3_500, 'E2E.23 over the scope'), 'E2E.23 over the scope', { allowPending: scope },
      ));
      assert(over !== null, 'a spend over the scope is refused');
      assert(over.status === 402, `over the scope: 402, got ${over.status}: ${over.message}`);
      assert(over.body.code === 'INSUFFICIENT_FUNDS', `over the scope: INSUFFICIENT_FUNDS, got ${over.body.code}`);
      const held = over.body.held || [];
      const heldP2 = held.find(h => h.outpoint === c2.outpoint);
      assert(heldP2, `P2 is listed as held (${held.map(h => h.outpoint.slice(0, 12)).join(', ')})`);
      assert(heldP2.reason.includes('not named under allowPending: received'),
        `P2's reason names the scope: ${heldP2.reason}`);
      assert(!held.some(h => h.outpoint === c1.outpoint), 'P1 was offered, not held');
      assert(over.body.offeredSatoshis === 3_000, `P1's 3,000 offered, got ${over.body.offeredSatoshis}`);

      // 2. Within the scope: 2,000 on P1 alone.
      const within = await walletC.client.transferTo(walletA.client, 2_000, 'E2E.23 within the scope', { allowPending: scope });
      assert(within.txid, 'a spend within the scope goes through');
      const after = await outputsOf();
      assert(after.some(o => o.outpoint === c2.outpoint), 'P2 untouched by the scoped spend');
      assert(!after.some(o => o.outpoint === c1.outpoint), 'P1 spent by the scoped spend');

      // 3. P1 is spent now: naming it is the caller's error before the door.
      const spent = await refusalOf(() => walletC.client.createAction(
        payA(1_000, 'E2E.23 naming a spent coin'), 'E2E.23 naming a spent coin', { allowPending: scope },
      ));
      assert(spent !== null && spent.status === 400, `naming a spent coin: 400, got ${spent?.status}: ${spent?.message}`);
      assert(spent.body.code === 'VALIDATION_ERROR', `naming a spent coin: VALIDATION_ERROR, got ${spent.body.code}`);
      assert(spent.body.message.includes(`names ${name(c1)}`) && spent.body.message.includes('spent by'),
        `the refusal names the coin and why: ${spent.body.message}`);

      // 4. A coin that is not C's.
      const foreign = await refusalOf(() => walletC.client.createAction(
        payA(1_000, 'E2E.23 naming a foreign coin'), 'E2E.23 naming a foreign coin',
        { allowPending: { received: [`${'0'.repeat(64)}:0`] } },
      ));
      assert(foreign !== null && foreign.status === 400, `naming a foreign coin: 400, got ${foreign?.status}: ${foreign?.message}`);
      assert(foreign.body.message.includes('not a coin of this wallet'), `why: ${foreign.body.message}`);

      // 5. The plain word: P2 and C's change go back to A.
      const balance = await walletC.client.balance();
      assert(balance >= 3_000, `C holds P2 and its change, has ${balance}`);
      const back = await walletC.client.transferTo(walletA.client, balance - 300, 'E2E.23 back to A', { allowPending: 'received' });
      assert(back.txid, 'the plain word spends the rest back to A');

      return { txid: within.txid, scopedTxid: within.txid, returnTxid: back.txid, heldReason: heldP2.reason };
    } catch (e) {
      keep = true;
      throw e;
    } finally {
      teardownWallet(walletC, { keep });
    }
  },
};
