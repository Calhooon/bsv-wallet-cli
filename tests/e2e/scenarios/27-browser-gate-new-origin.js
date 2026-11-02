/**
 * E2E.27: the browser gate, a new origin (#411, step (b)). The stack's
 * scenario 2026-11-01-wallet-browser-gate-new-origin (bsv-stack-lean
 * 76004514), step by step, under (b):
 *   1. a page from new.example calls listOutputs: refused in the client's
 *      denial (400 ERR_PERMISSION_DENIED); under (b) no pending row is
 *      written (the scenario's pending row is #414's);
 *   2. again: refused, still nothing written;
 *   3. the owner grants new.example: under (b), its line in
 *      ALLOWED_ORIGINS and a restart;
 *   4. the page calls again: admitted.
 * Cost: 0 sats.
 */
const g = require('../lib/browser-gate');

module.exports = {
  name: '2026-11-01-wallet-browser-gate-new-origin (b: refused, no pending row)',
  description: 'an origin ALLOWED_ORIGINS does not name is refused, no row written, until its line exists',

  async run(_walletA, _walletB, assert) {
    return g.scene('wallet-D-new-origin', [g.APP], async (wallet) => {
      for (const step of [1, 2]) {
        const r = await g.call(wallet, 'POST', 'listOutputs', { origin: g.NEW, body: g.ARGS.listOutputs });
        assert(r.status === 400 && r.body.code === 'ERR_PERMISSION_DENIED' && r.body.message === g.DENIED,
          `${step}: new.example is refused in the client's denial (got ${r.status} ${JSON.stringify(r.body)})`);
        assert(r.allowOrigin === null, `${step}: no CORS header, so the page cannot read it`);
      }
      // 3.
      await g.restartGated(wallet, [g.APP, g.NEW]);
      // 4.
      const four = await g.call(wallet, 'POST', 'listOutputs', { origin: g.NEW, body: g.ARGS.listOutputs });
      assert(four.status === 200 && Array.isArray(four.body.outputs),
        `4: listed, new.example is admitted (got ${four.status} ${JSON.stringify(four.body)})`);
      assert(four.allowOrigin === g.NEW, '4: the CORS layer echoes the listed origin');
      return {};
    });
  },
};
