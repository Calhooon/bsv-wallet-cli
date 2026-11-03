/**
 * E2E.31: the browser gate, the spending power (#414 L6, the design's
 * section (7) scene 31), on wallet D with its seed unset:
 *   1. a page with no spending row (app.example) calls createAction for
 *      2500 satoshis: held on a row of kind spending, key [], amount 2500,
 *      renewal false; the owner answers once (the owner's route): no grant
 *      row is written, and the call reaches its door;
 *   2. it calls again: held on a new row; the owner answers grant with a
 *      ceiling of 60000 (`bsv-wallet origin-grant #id --amount 60000`): the
 *      row `spending [] 60000`, and the call reaches its door;
 *   3. a call past the ceiling (60001): held with renewal true, its prompt
 *      naming the ceiling and the month's tally; the owner denies
 *      (`bsv-wallet origin-deny`): 400 ERR_PERMISSION_DENIED
 *      `Permission denied.`, echoed;
 *   4. `bsv-wallet origins --json`: the spending row with its ceiling, no
 *      open row.
 * Wallet D holds no coin and the stub funds nothing: a call the gate
 * admits answers the toolbox's 402 INSUFFICIENT_FUNDS after the gate (the
 * gate's answer precedes the toolbox's), so no spend is tallied and step 3
 * passes the ceiling by its own amount. Cost: 0 sats.
 */
const g = require('../lib/browser-gate');

const P2PKH = '76a914dbc0a7c84983c5bf199b7b2d41b3acf0408ee5aa88ac';

/** The app's rows of a listing (the harness holds its own grant beside them). */
const app = (rows) => rows.filter(r => r.originator === 'app.example');

function spend(satoshis) {
  return {
    description: 'browser gate spend',
    outputs: [{ lockingScript: P2PKH, satoshis, outputDescription: 'a spend' }],
    options: { noSend: true },
  };
}

module.exports = {
  name: '2026-11-03-wallet-browser-gate-spending',
  description: 'a page\'s spend is held on the spending power: answered once, granted with a ceiling, past the ceiling held as a renewal and denied',

  async run(_walletA, _walletB, assert) {
    return g.scene('wallet-D-spending', async (wallet) => {
      const reached = (r, step) => {
        assert(r.status === 402 && r.body.code === 'INSUFFICIENT_FUNDS' && r.allowOrigin === g.APP,
          `${step}: admitted by the gate, the door's own answer on an unfunded wallet (got ${r.status} ${JSON.stringify(r.body)})`);
      };
      // 1.
      const first = g.begin(wallet, 'POST', 'createAction', { origin: g.APP, body: spend(2500) });
      const [once] = await g.untilPending(wallet, 'app.example', 1);
      assert(once.kind === 'spending' && once.key === '[]' && once.amount === 2500 && once.renewal === false
        && once.call === '/createAction' && once.asked === 'spend 2500 satoshis',
        `1: held on a spending row (got ${JSON.stringify(once)})`);
      const answered = await g.owner(wallet, 'POST', 'origins/answer', { pendingId: once.pendingId, answer: 'once' });
      assert(answered.status === 200, `1: the owner answers once (got ${answered.status} ${JSON.stringify(answered.body)})`);
      reached(await first.answer, 1);
      assert(app(g.origins(wallet).grants).length === 0, '1: once writes no grant row');
      // 2.
      const second = g.begin(wallet, 'POST', 'createAction', { origin: g.APP, body: spend(2500) });
      const [asked] = await g.untilPending(wallet, 'app.example', 1);
      assert(asked.pendingId !== once.pendingId && asked.renewal === false,
        `2: once wrote nothing, so the call is held again (got ${JSON.stringify(asked)})`);
      g.cli(wallet, ['origin-grant', `#${asked.pendingId}`, '--amount', '60000']);
      reached(await second.answer, 2);
      const granted = app(g.origins(wallet).grants);
      assert(granted.length === 1 && granted[0].kind === 'spending' && granted[0].key === '[]'
        && granted[0].amount === 60000 && granted[0].expiry === 0,
        `2: the spending row with its ceiling (got ${JSON.stringify(granted)})`);
      // 3.
      const past = g.begin(wallet, 'POST', 'createAction', { origin: g.APP, body: spend(60001) });
      const [renewal] = await g.untilPending(wallet, 'app.example', 1);
      assert(renewal.renewal === true && renewal.amount === 60001
        && renewal.asked === 'spend 60001 satoshis past this month\'s ceiling of 60000 (0 spent)',
        `3: held as a renewal past the ceiling (got ${JSON.stringify(renewal)})`);
      g.cli(wallet, ['origin-deny', `#${renewal.pendingId}`]);
      const three = await past.answer;
      assert(three.status === 400 && three.body.code === 'ERR_PERMISSION_DENIED'
        && three.body.message === g.PERMISSION_DENIED && three.allowOrigin === g.APP,
        `3: denied (got ${three.status} ${JSON.stringify(three.body)})`);
      // 4.
      const listed = g.origins(wallet, true);
      const [grants, pending] = [app(listed.grants), app(listed.pending)];
      assert(pending.every(p => p.answeredAt !== null), '4: no open row');
      assert(grants.length === 1 && grants[0].amount === 60000 && grants[0].spentThisMonth === 0,
        `4: the ceiling, nothing tallied on an unfunded wallet (got ${JSON.stringify(grants)})`);
      assert(pending.map(p => p.answer).join(',') === 'once,grant,deny',
        `4: the owner's three answers kept (got ${JSON.stringify(pending)})`);
      return {};
    });
  },
};
