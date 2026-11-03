/**
 * E2E.28: the browser gate, a granted origin (#414's switch, the design's
 * L5b section (7)). The stack's scenario
 * 2026-11-01-wallet-browser-gate-granted-origin (bsv-stack-lean 4372ea39),
 * step by step, per power, on wallet D with its seed unset but in step 5:
 *   1. the owner grants app.example the protocol power
 *      [false,2,"browser gate","self"] (`origin-grant`); app.example calls
 *      getPublicKey on its default port, on 8080 and with an agreeing
 *      Originator: 200 each, echoed (the grant is the hostname's);
 *   2. `bsv-wallet origins --json`: one row, kind protocol, that key, no
 *      spending row;
 *   3. D restarts on the same store with AUTH_TOKEN set; app.example calls
 *      without the bearer: 401 UNAUTHORIZED ahead of the gate, no pending
 *      row, the grant row unchanged;
 *   4. with the bearer: 200, echoed; the bearer narrows, never widens;
 *   5. D restarts with ALLOWED_ORIGINS=seeded.example: seeded.example calls
 *      listOutputs of the basket `default`: 200, echoed, the seed's log line
 *      names it, no row written; the admin-reserved basket is not refused
 *      to a seed-listed caller (question 6).
 * Cost: 0 sats.
 */
const g = require('../lib/browser-gate');

module.exports = {
  name: '2026-11-01-wallet-browser-gate-granted-origin',
  description: "a granted power admits its page on every port; the bearer narrows and never widens; the seed admits by name",

  async run(_walletA, _walletB, assert) {
    return g.scene('wallet-D-granted-origin', async (wallet) => {
      const answered = (r, step, origin = g.APP) => {
        assert(r.status === 200 && typeof r.body.publicKey === 'string' && r.body.publicKey.length === 66,
          `${step}: app.example is admitted by its grant (got ${r.status} ${JSON.stringify(r.body)})`);
        assert(r.allowOrigin === origin, `${step}: the echo of ${origin}`);
      };
      // 1.
      g.cli(wallet, ['origin-grant', 'app.example', '--kind', 'protocol', '--key', g.POWER.protocol]);
      answered(await g.call(wallet, 'POST', 'getPublicKey', { origin: g.APP, body: g.ARGS.getPublicKey }), 1);
      answered(await g.call(wallet, 'POST', 'getPublicKey',
        { origin: 'http://app.example:8080', body: g.ARGS.getPublicKey }), '1 (port 8080)', 'http://app.example:8080');
      answered(await g.call(wallet, 'POST', 'getPublicKey',
        { origin: g.APP, originator: 'app.example', body: g.ARGS.getPublicKey }), '1 (Originator)');
      // 2.
      const rows = g.origins(wallet, true);
      const app = rows.grants.filter(r => r.originator === 'app.example');
      assert(app.length === 1 && app[0].kind === 'protocol' && app[0].key === g.POWER.protocol
        && !rows.grants.some(r => r.kind === 'spending'),
        `2: app.example holds one protocol row, no spending row (got ${JSON.stringify(rows.grants)})`);
      // 3.
      await g.restartGated(wallet, [], { token: g.TOKEN });
      const three = await g.call(wallet, 'POST', 'getPublicKey', { origin: g.APP, body: g.ARGS.getPublicKey });
      assert(three.status === 401 && three.body.code === 'UNAUTHORIZED',
        `3: without the bearer, refused ahead of the gate (got ${three.status} ${JSON.stringify(three.body)})`);
      const after = g.origins(wallet, true);
      assert(after.pending.length === 0, '3: no pending row');
      assert(JSON.stringify(after.grants) === JSON.stringify(rows.grants), '3: the grant row unchanged');
      // 4.
      answered(await g.call(wallet, 'POST', 'getPublicKey',
        { origin: g.APP, bearer: g.TOKEN, body: g.ARGS.getPublicKey }), 4);
      // 5.
      await g.restartGated(wallet, ['seeded.example']);
      const five = await g.call(wallet, 'POST', 'listOutputs',
        { origin: g.SEEDED, body: { basket: 'default', limit: 10 } });
      assert(five.status === 200 && Array.isArray(five.body.outputs) && five.allowOrigin === g.SEEDED,
        `5: the seed admits seeded.example, the basket default included (got ${five.status} ${JSON.stringify(five.body)})`);
      assert(g.log(wallet).includes('ALLOWED_ORIGINS lists seeded.example'), "5: the seed's log line names seeded.example");
      const seeded = g.origins(wallet, true);
      assert(!seeded.grants.some(r => r.originator === 'seeded.example') && seeded.pending.length === 0,
        `5: the seed writes no row (got ${JSON.stringify(seeded)})`);
      return {};
    });
  },
};
