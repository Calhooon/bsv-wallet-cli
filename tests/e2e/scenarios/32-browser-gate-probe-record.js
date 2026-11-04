/**
 * E2E.32: the probe record (#539, one of the three refinements of the
 * owner's W15 (i) ruling of 2026-11-03; docs/design/origin-grants.md
 * section "The probe record"), on wallet D with its seed unset:
 *   1. probe.example, holding no row, calls getVersion twice and
 *      createSignature under the level-0 protocol [0, "browser gate"] once:
 *      each 200 at once with the echo;
 *   2. `bsv-wallet origins --json` shows its probe rows: getVersion count 2,
 *      createSignature with the power [false,0,"browser gate",""] count 1,
 *      their first and last times within the scene's window; no grant row
 *      and no pending row of probe.example; `bsv-wallet origins` prints the
 *      line `probe  probe.example  getVersion  2 times  first .. last ..`;
 *   3. the owner's GET /origins and the inspector's /api/origins list the
 *      same rows;
 *   4. `bsv-wallet origin-revoke probe.example` refuses (no grant row) and
 *      leaves the record;
 *   5. busy.example signs under 40 level-0 protocols: its rows stop at the
 *      cap (32), the ones seen longest ago folded into one remainder row
 *      (door `*`, count 9), every answer counted. The level-0 powers answer
 *      a site 60 a minute with a burst of 10 (#541): a call past it is
 *      answered 429 with Retry-After and sent again after the wait, and a
 *      refused call is never counted;
 *   6. probe.example's GET at the POST door getHeaderForHeight is the
 *      router's 405 and is not counted (#556): the record read before it,
 *      then after a getVersion sent behind it (the writer keeps the order),
 *      moved by that getVersion alone, with no getHeaderForHeight row.
 * Cost: 0 sats.
 */
const g = require('../lib/browser-gate');

const PROBE = 'http://probe.example';
const BUSY = 'http://busy.example';
/** The rows an originator keeps, its remainder among them (probes::PER_ORIGINATOR). */
const CAP = 32;
const LEVEL_ZERO = '[false,0,"browser gate",""]';

/** `originator`'s probe rows in `listed`. */
const of = (listed, originator) => (listed.probes || []).filter(p => p.originator === originator);

/** `originator`'s rows from the CLI once their counts sum to `n` (the record is written off the call's path). */
async function untilCounted(wallet, originator, n) {
  let rows = [];
  for (let i = 0; i < 100; i++) {
    rows = of(g.origins(wallet, true), originator);
    if (rows.reduce((sum, p) => sum + p.count, 0) === n) return rows;
    await g.sleep(100);
  }
  throw new Error(`${originator}: ${n} answers never counted (got ${JSON.stringify(rows)})`);
}

module.exports = {
  name: '2026-11-03-wallet-browser-gate-probe-record',
  description: "a page's answers with no row are counted per door and level-0 power, never a grant, bounded per origin",

  async run(_walletA, _walletB, assert) {
    return g.scene('wallet-D-probe-record', async (wallet) => {
      const since = Math.floor(Date.now() / 1000);
      // 1.
      for (const n of [1, 2]) {
        const r = await g.call(wallet, 'GET', 'getVersion', { origin: PROBE });
        assert(r.status === 200 && r.allowOrigin === PROBE, `1: getVersion ${n} at once (got ${r.status})`);
      }
      const signed = await g.call(wallet, 'POST', 'createSignature',
        { origin: PROBE, body: { ...g.ARGS.createSignature, protocolID: [0, 'browser gate'] } });
      assert(signed.status === 200 && Array.isArray(signed.body.signature) && signed.allowOrigin === PROBE,
        `1: a level-0 signature at once (got ${signed.status} ${JSON.stringify(signed.body)})`);
      // 2.
      const rows = await untilCounted(wallet, 'probe.example', 3);
      const until = Math.floor(Date.now() / 1000);
      const door = (d) => rows.find(p => p.door === d) || {};
      const version = door('getVersion');
      const signature = door('createSignature');
      assert(rows.length === 2 && version.count === 2 && version.power === null
        && signature.count === 1 && signature.power === LEVEL_ZERO,
        `2: two rows, getVersion 2 and the level-0 power 1 (got ${JSON.stringify(rows)})`);
      for (const p of rows) {
        assert(p.rawOriginator === 'probe.example' && since <= p.firstSeen && p.firstSeen <= p.lastSeen && p.lastSeen <= until,
          `2: ${p.door}'s times within the scene (${since}..${until}, got ${JSON.stringify(p)})`);
      }
      const all = g.origins(wallet, true);
      assert(!all.grants.some(r => r.originator === 'probe.example') && !all.pending.some(r => r.originator === 'probe.example'),
        `2: no grant row and no pending row (got ${JSON.stringify(all)})`);
      const line = g.cli(wallet, ['origins']).split('\n').find(l => l.startsWith('probe  probe.example  getVersion  '));
      assert(/^probe {2}probe\.example {2}getVersion {2}2 times {2}first \S+ {2}last \S+$/.test(line || ''),
        `2: the probe line (got ${line})`);
      // 3.
      const ui = await g.inspector(wallet);
      try {
        const fromUi = await (await fetch(`http://localhost:${g.D_UI_PORT}/api/origins`)).json();
        const fromOwner = await g.listing(wallet);
        for (const [place, listed] of [['the inspector', fromUi], ["the owner's route", fromOwner]]) {
          assert(JSON.stringify(of(listed, 'probe.example')) === JSON.stringify(rows),
            `3: ${place} lists the same rows (got ${JSON.stringify(listed.probes)})`);
        }
      } finally {
        ui.kill('SIGTERM');
      }
      // 4.
      const refused = g.cliRefused(wallet, ['origin-revoke', 'probe.example']);
      assert(refused.status === 1 && refused.stderr.includes('holds no live grant row'),
        `4: no grant to revoke (got ${refused.status} ${refused.stderr})`);
      assert(JSON.stringify(of(g.origins(wallet, true), 'probe.example')) === JSON.stringify(rows),
        '4: the revoke leaves the record');
      // 5.
      const calls = 40;
      for (let i = 1; i <= calls; i++) {
        const sign = () => g.call(wallet, 'POST', 'createSignature',
          { origin: BUSY, body: { ...g.ARGS.createSignature, protocolID: [0, `probe number ${i}`] } });
        let r = await sign();
        for (let tries = 0; r.status === 429 && tries < 5; tries++) {
          await g.sleep(Number(r.retryAfter) * 1000 + 50);
          r = await sign();
        }
        assert(r.status === 200, `5: level-0 call ${i} answered (got ${r.status})`);
      }
      const busy = await untilCounted(wallet, 'busy.example', calls);
      const remainder = busy[busy.length - 1];
      assert(busy.length === CAP && remainder.door === '*' && remainder.power === null
        && remainder.count === calls - (CAP - 1),
        `5: ${CAP} rows, the remainder last with ${calls - (CAP - 1)} (got ${busy.length} rows, ${JSON.stringify(remainder)})`);
      assert(of(g.origins(wallet, true), 'probe.example').length === 2, "5: another origin's rows are its own");
      // 6.
      const before = of(g.origins(wallet, true), 'probe.example');
      const wrong = await g.call(wallet, 'GET', 'getHeaderForHeight', { origin: PROBE });
      assert(wrong.status === 405, `6: a GET at a POST door is 405 (got ${wrong.status})`);
      const fence = await g.call(wallet, 'GET', 'getVersion', { origin: PROBE });
      assert(fence.status === 200, `6: the getVersion behind it answered (got ${fence.status})`);
      const after = await untilCounted(wallet, 'probe.example', 4);
      const moved = before.map(p => (p.door === 'getVersion' ? { ...p, count: p.count + 1 } : p));
      const shape = rows => rows.map(p => [p.door, p.power, p.count]).sort((a, b) => (a[0] < b[0] ? -1 : 1));
      assert(JSON.stringify(shape(after)) === JSON.stringify(shape(moved))
        && !after.some(p => p.door === 'getHeaderForHeight'),
        `6: the 405 is not counted (before ${JSON.stringify(before)}, after ${JSON.stringify(after)})`);
      return {};
    });
  },
};
