/**
 * E2E.30: the browser gate, a disagreeing originator (#414's switch, the
 * design's L5b section (7)). The stack's scenario
 * 2026-11-01-wallet-browser-gate-disagreeing-originator (bsv-stack-lean
 * e4383662, the scenarios' re-pin; steps 1 to 3 unchanged since 4372ea39),
 * step by step, on wallet D with its seed unset:
 *   1. a page from evil.example (no row) calls createAction naming
 *      app.example in the Originator header: 400 VALIDATION_ERROR naming
 *      both, echoed; the agreement runs before any row is read or written;
 *   2. app.example naming evil.example in the Originator header, then in
 *      the body's originator: 400 naming both;
 *   3. nothing reached a door (the audit, read as the owner, is unchanged);
 *   4. app.example with an agreeing Originator: createAction of a 0-satoshi
 *      output asks no power here (W6: its amount is its outputs' sum, 0;
 *      the reference would hold it on a spending prompt for the fee), so the
 *      door answers at once, never held: wallet D holds no coin, so the
 *      door's answer is the toolbox's 402 INSUFFICIENT_FUNDS (no scene
 *      moves a satoshi), audited. And (#448) app.example:8080 naming
 *      app.example agrees by hostname: a level-0 getPublicKey (W15) at once;
 *   5. `bsv-wallet origins --json`: no pending row.
 * Cost: 0 sats.
 */
const g = require('../lib/browser-gate');

module.exports = {
  name: '2026-11-01-wallet-browser-gate-disagreeing-originator',
  description: 'an Originator header or a body originator naming another host than the Origin is refused before any power; a port beside the name agrees',

  async run(_walletA, _walletB, assert) {
    return g.scene('wallet-D-disagreeing-originator', async (wallet) => {
      const before = await g.audited(wallet);
      const disagrees = (r, step, origin) => {
        assert(r.status === 400 && r.body.code === 'VALIDATION_ERROR' && /disagrees with the Origin header/.test(r.body.message),
          `${step}: refused disagrees (got ${r.status} ${JSON.stringify(r.body)})`);
        assert(r.allowOrigin === origin, `${step}: every answer to a parsed Origin carries its echo`);
      };
      // 1.
      disagrees(await g.call(wallet, 'POST', 'createAction',
        { origin: g.EVIL, originator: 'app.example', body: g.ARGS.createAction }), 1, g.EVIL);
      // 2.
      disagrees(await g.call(wallet, 'POST', 'createAction',
        { origin: g.APP, originator: 'evil.example', body: g.ARGS.createAction }), 2, g.APP);
      disagrees(await g.call(wallet, 'POST', 'createAction',
        { origin: g.APP, body: { ...g.ARGS.createAction, originator: 'evil.example' } }), '2 (the body)', g.APP);
      // 3.
      assert(await g.audited(wallet) === before, '3: nothing reached a door');
      // 4.
      const four = await g.call(wallet, 'POST', 'createAction',
        { origin: g.APP, originator: 'app.example', body: g.ARGS.createAction });
      assert(four.status === 402 && four.body.code === 'INSUFFICIENT_FUNDS' && four.allowOrigin === g.APP,
        `4: no power asked (W6), the door's own answer at once on an unfunded wallet (got ${four.status} ${JSON.stringify(four.body)})`);
      assert(await g.audited(wallet) === before + 1, '4: the createAction reached its door');
      const PORTED = 'http://app.example:8080';
      const ported = await g.call(wallet, 'POST', 'getPublicKey',
        { origin: PORTED, originator: 'app.example', body: { ...g.ARGS.getPublicKey, protocolID: [0, 'browser gate'] } });
      assert(ported.status === 200 && typeof ported.body.publicKey === 'string' && ported.allowOrigin === PORTED,
        `4 (#448): app.example agrees with app.example:8080 by hostname (got ${ported.status} ${JSON.stringify(ported.body)})`);
      // 5.
      assert(g.origins(wallet, true).pending.length === 0, '5: no pending row');
      return {};
    });
  },
};
