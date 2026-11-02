/**
 * E2E.26: the browser gate, no Origin (#411, step (b)). The stack's
 * scenario 2026-11-01-wallet-browser-gate-no-origin (bsv-stack-lean
 * 76004514), step by step:
 *   1. no Origin, `Originator: app.example`, createSignature, no bearer (the
 *      host holds none): refused noOrigin, the Originator header not read;
 *   2. the same with the bearer, the host holding a token: refused noOrigin,
 *      the bearer changes no word;
 *   3. the host's rows: under (b) there are none; nothing reached a door
 *      (the permission audit is unchanged).
 * The refusal is the MetaNet Client's, 400 "Origin header is required".
 * Cost: 0 sats.
 */
const g = require('../lib/browser-gate');

module.exports = {
  name: '2026-11-01-wallet-browser-gate-no-origin',
  description: 'no Origin is refused whatever the Originator header names, with or without the bearer',

  async run(_walletA, _walletB, assert) {
    return g.scene('wallet-D-no-origin', [g.APP], async (wallet) => {
      const before = await g.audited(wallet);
      // 1.
      const one = await g.call(wallet, 'POST', 'createSignature',
        { originator: 'app.example', body: g.ARGS.createSignature });
      assert(one.status === 400 && one.body.message === g.NO_ORIGIN,
        `1: no Origin is the client's refusal, 400 "${g.NO_ORIGIN}" (got ${one.status} ${JSON.stringify(one.body)})`);
      assert(one.allowOrigin === null, '1: no CORS header on the refusal');
      assert(await g.audited(wallet) === before, '1: nothing reached a door (the audit is unchanged)');
      // 2.
      await g.restartGated(wallet, [g.APP], { token: g.TOKEN });
      const two = await g.call(wallet, 'POST', 'createSignature',
        { originator: 'app.example', bearer: g.TOKEN, body: g.ARGS.createSignature });
      assert(two.status === 400 && two.body.message === g.NO_ORIGIN,
        `2: with the bearer, still refused noOrigin (got ${two.status} ${JSON.stringify(two.body)})`);
      // 3.
      assert(await g.audited(wallet, g.TOKEN) === 0,
        '3: under (b) no pending row exists; nothing reached a door since the restart');
      // The control: the same call from the listed page with the bearer reaches the door.
      const control = await g.call(wallet, 'POST', 'createSignature',
        { origin: g.APP, bearer: g.TOKEN, body: g.ARGS.createSignature });
      assert(control.status === 200 && Array.isArray(control.body.signature),
        `control: the listed origin with the bearer is answered (got ${control.status})`);
      assert(await g.audited(wallet, g.TOKEN) === 1, 'control: the answered call is audited, so the audit reads a door');
      return {};
    });
  },
};
