/**
 * E2E.29: the browser gate, a revoked origin (#414's switch, the design's
 * L5b section (7)). The stack's scenario
 * 2026-11-01-wallet-browser-gate-revoked-origin (bsv-stack-lean 4372ea39),
 * step by step, per power, on wallet D with its seed unset:
 *   1. app.example holds the protocol row of scene 28 (the owner's
 *      `origin-grant`) and calls getPublicKey: 200;
 *   2. `bsv-wallet origin-revoke app.example --kind protocol --key ...`:
 *      the row stamped;
 *   3. it calls, on its port and on 8080: 400 ERR_PERMISSION_DENIED in the
 *      client's grouped denial at once, echoed, no pending row, no restart;
 *   4. it calls createSignature under [2, "other gate"]: held (a different
 *      power: the revoke reaches only its own row); the owner denies it;
 *   5. D restarts; it calls getPublicKey: still refused; the log names the
 *      revoked row;
 *   6. the owner grants the protocol again with `--expires 2`; after 3 s it
 *      calls: held on a pending row, renewal true; the owner grants the
 *      renewal (the owner's route): 200.
 * Cost: 0 sats.
 */
const g = require('../lib/browser-gate');

module.exports = {
  name: '2026-11-01-wallet-browser-gate-revoked-origin',
  description: 'a revoked power is refused at once on every port, through a restart; its other powers are asked; an expired grant is renewed',

  async run(_walletA, _walletB, assert) {
    return g.scene('wallet-D-revoked-origin', async (wallet) => {
      const grant = ['origin-grant', 'app.example', '--kind', 'protocol', '--key', g.POWER.protocol];
      g.cli(wallet, grant);
      // 1.
      const one = await g.call(wallet, 'POST', 'getPublicKey', { origin: g.APP, body: g.ARGS.getPublicKey });
      assert(one.status === 200, `1: app.example is admitted by its row (got ${one.status} ${JSON.stringify(one.body)})`);
      // 2.
      g.cli(wallet, ['origin-revoke', 'app.example', '--kind', 'protocol', '--key', g.POWER.protocol]);
      const stamped = g.origins(wallet, true).grants.find(r => r.key === g.POWER.protocol);
      assert(stamped && stamped.revokedAt !== null, `2: the row stamped (got ${JSON.stringify(stamped)})`);
      // 3.
      for (const origin of [g.APP, 'http://app.example:8080']) {
        const r = await g.call(wallet, 'POST', 'getPublicKey', { origin, body: g.ARGS.getPublicKey });
        assert(r.status === 400 && r.body.code === 'ERR_PERMISSION_DENIED' && r.body.message === g.DENIED && r.allowOrigin === origin,
          `3: ${origin} refused at once in the client's denial, echoed (got ${r.status} ${JSON.stringify(r.body)})`);
      }
      assert(g.origins(wallet, true).pending.length === 0, '3: no pending row');
      // 4.
      const other = g.begin(wallet, 'POST', 'createSignature',
        { origin: g.APP, body: { ...g.ARGS.createSignature, protocolID: [2, 'other gate'] } });
      const [asked] = await g.untilPending(wallet, 'app.example', 1);
      assert(asked.key === '[false,2,"other gate","self"]', `4: another power is asked (got ${JSON.stringify(asked)})`);
      const deny = await g.owner(wallet, 'POST', 'origins/answer', { pendingId: asked.pendingId, answer: 'deny' });
      assert(deny.status === 200, `4: the owner denies (got ${deny.status})`);
      const four = await other.answer;
      assert(four.status === 400 && four.body.message === g.PERMISSION_DENIED,
        `4: denied (got ${four.status} ${JSON.stringify(four.body)})`);
      // 5.
      await g.restartGated(wallet, []);
      const five = await g.call(wallet, 'POST', 'getPublicKey', { origin: g.APP, body: g.ARGS.getPublicKey });
      assert(five.status === 400 && five.body.message === g.DENIED, `5: still refused after a restart (got ${five.status})`);
      assert(g.log(wallet).includes('browser gate: revoked, refused'), '5: the log names the revoked row');
      // 6.
      g.cli(wallet, [...grant, '--expires', '2']);
      await g.sleep(3000);
      const renewed = g.begin(wallet, 'POST', 'getPublicKey', { origin: g.APP, body: g.ARGS.getPublicKey });
      const [renewal] = await g.untilPending(wallet, 'app.example', 1);
      assert(renewal.renewal === true && renewal.key === g.POWER.protocol,
        `6: held on a renewal (got ${JSON.stringify(renewal)})`);
      const granted = await g.owner(wallet, 'POST', 'origins/answer', { pendingId: renewal.pendingId, answer: 'grant' });
      assert(granted.status === 200, `6: the owner grants the renewal (got ${granted.status})`);
      const six = await renewed.answer;
      assert(six.status === 200 && typeof six.body.publicKey === 'string',
        `6: answered once renewed (got ${six.status} ${JSON.stringify(six.body)})`);
      return {};
    });
  },
};
