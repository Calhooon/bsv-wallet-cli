/**
 * E2E.30: the browser gate, a disagreeing originator (#411, step (b)). The
 * stack's scenario 2026-11-01-wallet-browser-gate-disagreeing-originator
 * (bsv-stack-lean e4383662, the scenarios' re-pin; steps 1 to 3 unchanged
 * since 4372ea39), step by step:
 *   1. a page from evil.example calls createAction naming app.example in
 *      the Originator header: refused disagrees, nothing written;
 *   2. a page from app.example naming evil.example: refused disagrees;
 *      and (the ruling's rule 1, beyond the scenario's steps) the same page
 *      naming evil.example in the body's originator: refused;
 *   3. the host's rows: under (b) none; nothing reached a door (the
 *      permission audit is unchanged);
 *   4. a page from app.example:8080 naming app.example in the Originator
 *      header: admitted (#448), the agreement compared in the lookup form,
 *      the hostname, as the key scopes a grant to every port of a host. The
 *      call is a key read (getPublicKey), so it moves no satoshi.
 * Cost: 0 sats (steps 1 to 3 are refused before the door; step 4 reads a key).
 */
const g = require('../lib/browser-gate');

module.exports = {
  name: '2026-11-01-wallet-browser-gate-disagreeing-originator',
  description: 'an Originator header or a body originator naming another host than the Origin is refused; a port beside the name agrees',

  async run(_walletA, _walletB, assert) {
    return g.scene('wallet-D-disagreeing-originator', [g.APP_GRANT], async (wallet) => {
      const before = await g.audited(wallet);
      const disagrees = (r, step) => assert(
        r.status === 400 && r.body.code === 'VALIDATION_ERROR' && /disagrees with the Origin header/.test(r.body.message),
        `${step}: refused disagrees (got ${r.status} ${JSON.stringify(r.body)})`);
      const one = await g.call(wallet, 'POST', 'createAction',
        { origin: g.EVIL, originator: 'app.example', body: g.ARGS.createAction });
      disagrees(one, 1);
      assert(one.allowOrigin === null, '1: evil.example is not listed: no CORS header');
      const two = await g.call(wallet, 'POST', 'createAction',
        { origin: g.APP, originator: 'evil.example', body: g.ARGS.createAction });
      disagrees(two, 2);
      assert(two.allowOrigin === g.APP, '2: the listed page reads its refusal');
      const inBody = await g.call(wallet, 'POST', 'createAction',
        { origin: g.APP, body: { ...g.ARGS.createAction, originator: 'evil.example' } });
      disagrees(inBody, '2 (the body)');
      assert(await g.audited(wallet) === before, '3: under (b) no pending row; nothing reached a door');
      const PORTED = 'http://app.example:8080';
      const four = await g.call(wallet, 'POST', 'getPublicKey',
        { origin: PORTED, originator: 'app.example', body: g.ARGS.getPublicKey });
      assert(four.status === 200 && typeof four.body.publicKey === 'string' && four.body.publicKey.length === 66,
        `4: app.example agrees with app.example:8080 by hostname, admitted (got ${four.status} ${JSON.stringify(four.body)})`);
      assert(four.allowOrigin === PORTED, '4: the CORS layer echoes the Origin as sent');
      return {};
    });
  },
};
