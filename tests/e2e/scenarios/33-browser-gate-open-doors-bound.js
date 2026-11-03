/**
 * E2E.33: the open doors' bound (#541, one of the three refinements of the
 * owner's W15 (i) ruling of 2026-11-03; docs/design/origin-grants.md
 * section "The open doors' bound"), on wallet D with its seed unset:
 *   1. bound.example, holding no row, calls getHeight until it is refused:
 *      the burst's 10 answered 200 (one more for each second the calls
 *      took), then 429 ERR_RATE_LIMITED naming /getHeight and the chain
 *      doors' window, Retry-After 1, the CORS echo with retry-after
 *      exposed; serve.log names `browser gate: rate limited`;
 *   2. other.example's getHeight at once: 200, its own bound;
 *   3. after the Retry-After, bound.example's getHeight: 200;
 *   4. no grant row and no pending row of bound.example, and its probe
 *      record (#539) counts the answered calls alone, never the refused one;
 *   5. a presence door answers once: isAuthenticated 200, then 429 with
 *      Retry-After of at most 60; waitForAuthentication answers its own
 *      once; a level-0 signature past the burst is 429 on its own bound;
 *   6. the owner's GET /origins, twenty times at once: every one 200.
 * Cost: 0 sats.
 */
const g = require('../lib/browser-gate');

const BOUND = 'http://bound.example';
const OTHER = 'http://other.example';
const CHAIN_WINDOW = 'the chain doors (getHeight, getHeaderForHeight, getNetwork, getVersion) answer 60 a minute per site, 10 at once';

/** `originator`'s probe counts from the CLI once they sum to `n` (written off the call's path). */
async function untilCounted(wallet, originator, n) {
  let sum = -1;
  for (let i = 0; i < 100; i++) {
    sum = (g.origins(wallet, true).probes || [])
      .filter(p => p.originator === originator).reduce((s, p) => s + p.count, 0);
    if (sum === n) return sum;
    await g.sleep(100);
  }
  return sum;
}

/** Call until the first refusal: the answered count, the refusal, and the seconds the calls took. */
async function untilRefused(call, most) {
  const start = Date.now();
  let answered = 0;
  for (let i = 0; i < most; i++) {
    const r = await call();
    if (r.status !== 200) return { answered, refused: r, secs: (Date.now() - start) / 1000 };
    answered += 1;
  }
  throw new Error(`no refusal in ${most} calls`);
}

module.exports = {
  name: '2026-11-03-wallet-browser-gate-open-doors-bound',
  description: 'the open doors and level 0 answer a page within a bound per origin; past it 429 with Retry-After, no row',

  async run(_walletA, _walletB, assert) {
    return g.scene('wallet-D-open-doors-bound', async (wallet) => {
      // 1.
      const height = (origin) => g.call(wallet, 'GET', 'getHeight', { origin });
      const chain = await untilRefused(() => height(BOUND), 40);
      const { refused } = chain;
      assert(chain.answered >= 10 && chain.answered <= 10 + Math.ceil(chain.secs),
        `1: the burst of 10 answered (one more a second), then refused (got ${chain.answered} in ${chain.secs} s)`);
      assert(refused.status === 429 && refused.body.code === 'ERR_RATE_LIMITED'
        && refused.body.message === `Too many calls to /getHeight from this site: ${CHAIN_WINDOW}; retry after 1 s.`,
        `1: 429 ERR_RATE_LIMITED naming the door and the window (got ${refused.status} ${JSON.stringify(refused.body)})`);
      assert(refused.retryAfter === '1' && refused.allowOrigin === BOUND && refused.exposed === 'retry-after',
        `1: Retry-After 1, the echo, retry-after exposed (got ${refused.retryAfter} ${refused.allowOrigin} ${refused.exposed})`);
      assert(g.log(wallet).includes('browser gate: rate limited'), '1: serve.log names browser gate: rate limited');
      // 2.
      const other = await height(OTHER);
      assert(other.status === 200 && other.allowOrigin === OTHER, `2: another origin's own bound (got ${other.status})`);
      // 3.
      await g.sleep(Number(refused.retryAfter) * 1000 + 100);
      const after = await height(BOUND);
      assert(after.status === 200 && Number.isInteger(after.body.height),
        `3: answered after the window (got ${after.status} ${JSON.stringify(after.body)})`);
      // 4.
      const all = g.origins(wallet, true);
      assert(!all.grants.some(r => r.originator === 'bound.example') && !all.pending.some(r => r.originator === 'bound.example'),
        `4: no grant row and no pending row (got ${JSON.stringify(all)})`);
      const counted = await untilCounted(wallet, 'bound.example', chain.answered + 1);
      assert(counted === chain.answered + 1,
        `4: the probe record counts the ${chain.answered + 1} answered calls alone (got ${counted})`);
      // 5.
      const present = await g.call(wallet, 'GET', 'isAuthenticated', { origin: BOUND });
      assert(present.status === 200 && present.body.authenticated === true, `5: isAuthenticated once (got ${present.status})`);
      const again = await g.call(wallet, 'GET', 'isAuthenticated', { origin: BOUND });
      assert(again.status === 429 && again.body.code === 'ERR_RATE_LIMITED'
        && again.body.message.includes('a presence door answers once a minute per site')
        && Number(again.retryAfter) >= 1 && Number(again.retryAfter) <= 60 && again.allowOrigin === BOUND,
        `5: the second isAuthenticated 429 within the minute (got ${again.status} ${again.retryAfter} ${JSON.stringify(again.body)})`);
      const waited = await g.call(wallet, 'GET', 'waitForAuthentication', { origin: BOUND });
      assert(waited.status === 200, `5: waitForAuthentication answers its own once (got ${waited.status})`);
      const sign = () => g.call(wallet, 'POST', 'createSignature',
        { origin: BOUND, body: { ...g.ARGS.createSignature, protocolID: [0, 'browser gate'] } });
      const zero = await untilRefused(sign, 40);
      assert(zero.answered >= 10 && zero.answered <= 10 + Math.ceil(zero.secs)
        && zero.refused.status === 429 && zero.refused.retryAfter === '1'
        && zero.refused.body.message.startsWith('Too many level-0 calls to /createSignature from this site'),
        `5: a level-0 power on its own bound (got ${zero.answered} in ${zero.secs} s, ${zero.refused.status} ${JSON.stringify(zero.refused.body)})`);
      // 6.
      const owner = await Promise.all(Array.from({ length: 20 }, () => g.owner(wallet, 'GET', 'origins')));
      assert(owner.every(r => r.status === 200), `6: the owner's routes are outside the bound (got ${owner.map(r => r.status)})`);
      return {};
    });
  },
};
