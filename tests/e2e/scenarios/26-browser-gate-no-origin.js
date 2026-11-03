/**
 * E2E.26: the browser gate, no Origin (#414's switch, the design's L5b
 * section (7)). The stack's scenario 2026-11-01-wallet-browser-gate-no-origin
 * (bsv-stack-lean 4372ea39), step by step, on wallet D with its seed unset:
 *   1. no Origin, `Originator: app.example`, createSignature, no bearer:
 *      refused before any power, 400 "Origin header is required", the
 *      Originator header not read;
 *   2. the same with the bearer, the host holding a token: the same
 *      refusal; the bearer changes no word;
 *   3. `bsv-wallet origins --json`: no pending row; nothing reached a door
 *      (the permission audit, read as the owner, is unchanged).
 * The control: the owner grants app.example the protocol power
 * [false,2,"browser gate","self"] (`origin-grant`), and the page's call
 * with the bearer reaches the door (audited).
 * Cost: 0 sats.
 */
const g = require('../lib/browser-gate');

module.exports = {
  name: '2026-11-01-wallet-browser-gate-no-origin',
  description: 'no Origin is refused before any power, with or without the bearer; no row is written',

  async run(_walletA, _walletB, assert) {
    return g.scene('wallet-D-no-origin', async (wallet) => {
      const before = await g.audited(wallet);
      // 1.
      const one = await g.call(wallet, 'POST', 'createSignature',
        { originator: 'app.example', body: g.ARGS.createSignature });
      assert(one.status === 400 && one.body.code === 'VALIDATION_ERROR' && one.body.message === g.NO_ORIGIN,
        `1: no Origin is the client's refusal, 400 "${g.NO_ORIGIN}" (got ${one.status} ${JSON.stringify(one.body)})`);
      assert(one.allowOrigin === null, '1: no CORS header on the refusal');
      assert(await g.audited(wallet) === before, '1: nothing reached a door (the audit is unchanged)');
      // 2.
      await g.restartGated(wallet, [], { token: g.TOKEN });
      const two = await g.call(wallet, 'POST', 'createSignature',
        { originator: 'app.example', bearer: g.TOKEN, body: g.ARGS.createSignature });
      assert(two.status === 400 && two.body.message === g.NO_ORIGIN,
        `2: with the bearer, still refused before any power (got ${two.status} ${JSON.stringify(two.body)})`);
      // 3.
      const rows = g.origins(wallet, true);
      assert(rows.pending.length === 0, `3: no pending row (got ${JSON.stringify(rows.pending)})`);
      assert(await g.audited(wallet, g.TOKEN) === 0, '3: nothing reached a door since the restart');
      // The control: the owner's grant of the power, then the page with the bearer reaches the door.
      g.cli(wallet, ['origin-grant', 'app.example', '--kind', 'protocol', '--key', g.POWER.protocol]);
      const control = await g.call(wallet, 'POST', 'createSignature',
        { origin: g.APP, bearer: g.TOKEN, body: g.ARGS.createSignature });
      assert(control.status === 200 && Array.isArray(control.body.signature) && control.allowOrigin === g.APP,
        `control: the granted page with the bearer is answered, echoed (got ${control.status} ${JSON.stringify(control.body)})`);
      assert(await g.audited(wallet, g.TOKEN) === 1, 'control: the answered call is audited, so the audit reads a door');
      return {};
    });
  },
};
