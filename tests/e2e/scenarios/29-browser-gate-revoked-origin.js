/**
 * E2E.29: the browser gate, a revoked origin (#411, step (b)). The stack's
 * scenario 2026-11-01-wallet-browser-gate-revoked-origin (bsv-stack-lean
 * 76004514), step by step; under (b) the revocation is (c)'s stand-in:
 * app.example's line removed from ALLOWED_ORIGINS and the wallet restarted.
 *   1. app.example calls: admitted;
 *   2. the owner revokes app.example: its line removed, the wallet restarted;
 *   3. the page calls: refused (under (b) the not-listed denial; no pending
 *      row);
 *   4. a second grant with an expiry, called past it: not expressible under
 *      (b), an env line has no expiry; #414's (named, not run).
 * Cost: 0 sats.
 */
const g = require('../lib/browser-gate');

module.exports = {
  name: '2026-11-01-wallet-browser-gate-revoked-origin (b: delisted and restarted)',
  description: "a revoked origin (under (b): removed from ALLOWED_ORIGINS, restarted) is refused",

  async run(_walletA, _walletB, assert) {
    return g.scene('wallet-D-revoked-origin', [g.APP], async (wallet) => {
      const one = await g.call(wallet, 'POST', 'getPublicKey', { origin: g.APP, body: g.ARGS.getPublicKey });
      assert(one.status === 200, `1: app.example is admitted (got ${one.status} ${JSON.stringify(one.body)})`);
      await g.restartGated(wallet, []);
      const three = await g.call(wallet, 'POST', 'getPublicKey', { origin: g.APP, body: g.ARGS.getPublicKey });
      assert(three.status === 400 && three.body.code === 'ERR_PERMISSION_DENIED' && three.body.message === g.DENIED,
        `3: revoked, app.example is refused in the client's denial (got ${three.status} ${JSON.stringify(three.body)})`);
      assert(three.allowOrigin === null, '3: no CORS header, so the page cannot read it');
      console.log('       4: an expired grant: not expressible under (b) (an ALLOWED_ORIGINS line has no expiry); #414');
      return {};
    });
  },
};
