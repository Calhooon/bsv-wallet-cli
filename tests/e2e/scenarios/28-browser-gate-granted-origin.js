/**
 * E2E.28: the browser gate, a granted origin (#411, step (b)). The stack's
 * scenario 2026-11-01-wallet-browser-gate-granted-origin (bsv-stack-lean
 * 76004514), step by step; under (b) the grant is app.example's line in
 * ALLOWED_ORIGINS:
 *   1. app.example calls getPublicKey, the host holding no token: admitted;
 *   2. the same naming app.example in the Originator header: admitted;
 *   3. the host holding a token, without the bearer: refused ahead of the
 *      gate, 401 UNAUTHORIZED;
 *   4. with the bearer: admitted (the bearer narrows, never widens).
 * Cost: 0 sats.
 */
const g = require('../lib/browser-gate');

module.exports = {
  name: '2026-11-01-wallet-browser-gate-granted-origin',
  description: 'a listed origin is admitted; the bearer narrows and never widens',

  async run(_walletA, _walletB, assert) {
    return g.scene('wallet-D-granted-origin', [g.APP], async (wallet) => {
      const answered = (r, step) => {
        assert(r.status === 200 && typeof r.body.publicKey === 'string' && r.body.publicKey.length === 66,
          `${step}: app.example is admitted (got ${r.status} ${JSON.stringify(r.body)})`);
        assert(r.allowOrigin === g.APP, `${step}: the CORS layer echoes app.example`);
      };
      answered(await g.call(wallet, 'POST', 'getPublicKey', { origin: g.APP, body: g.ARGS.getPublicKey }), 1);
      answered(await g.call(wallet, 'POST', 'getPublicKey',
        { origin: g.APP, originator: 'app.example', body: g.ARGS.getPublicKey }), 2);
      await g.restartGated(wallet, [g.APP], { token: g.TOKEN });
      const three = await g.call(wallet, 'POST', 'getPublicKey', { origin: g.APP, body: g.ARGS.getPublicKey });
      assert(three.status === 401 && three.body.code === 'UNAUTHORIZED',
        `3: without the bearer, refused ahead of the gate (got ${three.status} ${JSON.stringify(three.body)})`);
      answered(await g.call(wallet, 'POST', 'getPublicKey',
        { origin: g.APP, bearer: g.TOKEN, body: g.ARGS.getPublicKey }), 4);
      return {};
    });
  },
};
