/**
 * E2E.34: the browser gate, a brief reconnect (#534, the design's L8,
 * docs/design/origin-grants.md section (7)'s scene 34), on wallet D with its
 * seed unset. W is the window, `ABANDON_WINDOW_SECS` read from the module's
 * constant through the harness (lib/browser-gate.js); every time below is
 * derived from it, and no step holds a literal. t_late and t_back are the
 * times late.example's and back.example's connections close. Every call
 * asks the basket `browser gate` (listing):
 *   1. late.example calls; once its row appears it closes its connection
 *      (t_late); steps 2 to 5 run while its window passes, and the scene
 *      fails if step 5 ends at or after t_late + W;
 *   2. back.example calls; once its row N appears it closes its connection
 *      (t_back): row N shown with abandonedAt A, still one row for
 *      back.example, serve.log names `browser gate: disconnected`; the
 *      owner's grant of N through the MCP's answer_origin is 409 CONFLICT
 *      `pending request #N is waiting for its page to reconnect; answer it
 *      when the page asks again, or it is withdrawn at <A + W>`, and no grant
 *      row is written;
 *   3. at t_back + W/5 other.example calls: held on a row of its own; row N
 *      still abandoned at A;
 *   4. at t_back + 2W/5 back.example:8080 calls: held on N (no new row,
 *      abandonedAt gone), serve.log names `browser gate: pending` with
 *      joined and resumed;
 *   5. the owner grants N with `bsv-wallet origin-grant #N`: step 4's call
 *      answers 200 with the echo, once; one grant row for back.example; row
 *      N answered grant; then denies other.example's row with `bsv-wallet
 *      origin-deny`: 400 ERR_PERMISSION_DENIED "Permission denied." with the
 *      echo, no grant row for other.example;
 *   6. at t_late + W + W/10, past late.example's window: no row of
 *      late.example (open, abandoned or answered), serve.log names `browser
 *      gate: abandoned` for it; its next call opens a new prompt (a new
 *      pending id), held, and the owner's deny answers it 400.
 * Cost: 0 sats.
 */
const g = require('../lib/browser-gate');
const { McpClient } = require('../lib/mcp');

const LATE = 'http://late.example';
const BACK = 'http://back.example';
const BACK_8080 = 'http://back.example:8080';
const OTHER = 'http://other.example';

/** serve.log without its colour codes, so a field reads `name=value`. */
function plainLog(wallet) {
  return g.log(wallet).replace(/\x1b\[[0-9;]*m/g, '');
}

/** The owner's rows of `originator`, every state, from the owner's route. */
async function rowsOf(wallet, originator) {
  return (await g.listing(wallet)).pending.filter(p => p.originator === originator);
}

/** Poll the owner's route until `test` holds of `originator`'s rows: those rows. */
async function untilRows(wallet, originator, test, what) {
  let rows = [];
  for (let i = 0; i < 200; i++) {
    rows = await rowsOf(wallet, originator);
    if (test(rows)) return rows;
    await g.sleep(25);
  }
  throw new Error(`${originator}: ${what} never held (rows ${JSON.stringify(rows)})`);
}

module.exports = {
  name: '2026-11-04-wallet-browser-gate-brief-reconnect',
  description: "a page that reconnects inside the window resumes its prompt; the owner's answer is refused meanwhile; past the window the disconnect's deny",

  async run(_walletA, _walletB, assert) {
    return g.scene('wallet-D-brief-reconnect', async (wallet) => {
      const W = g.W;
      const listing = { body: g.ARGS.listOutputs };
      const mcp = new McpClient(wallet.port);
      try {
        await mcp.start();
        await mcp.request('initialize', { protocolVersion: '2024-11-05', capabilities: {}, clientInfo: { name: 'e2e-test', version: '1.0' } });
        mcp.notify('notifications/initialized');

        // 1.
        const late = g.begin(wallet, 'POST', 'listOutputs', { origin: LATE, ...listing });
        const [lateRow] = await g.untilPending(wallet, 'late.example', 1);
        late.abort();
        const tLate = Date.now();

        // 2.
        const back = g.begin(wallet, 'POST', 'listOutputs', { origin: BACK, ...listing });
        const [row] = await g.untilPending(wallet, 'back.example', 1);
        const N = row.pendingId;
        back.abort();
        const tBack = Date.now();
        const marked = await untilRows(wallet, 'back.example', rs => rs.length === 1 && rs[0].abandonedAt !== undefined,
          'one row with abandonedAt');
        assert(marked[0].pendingId === N && marked[0].answeredAt === null,
          `2: row N shown abandoned, still one row for back.example (got ${JSON.stringify(marked)})`);
        const A = marked[0].abandonedAt;
        assert(marked[0].abandonedUntil === A + W / 1000,
          `2: abandonedUntil is the window's end, abandonedAt + W (got ${JSON.stringify(marked[0])})`);
        assert(g.log(wallet).includes('browser gate: disconnected'), '2: serve.log names browser gate: disconnected');
        const words = `pending request #${N} is waiting for its page to reconnect; answer it when the page asks again, or it is withdrawn at ${g.exact(A + W / 1000)}`;
        let refused = null;
        try {
          await mcp.callTool('answer_origin', { pendingId: N, answer: 'grant' });
        } catch (e) {
          refused = String(e.message || e);
        }
        assert(refused !== null && refused.includes('HTTP 409') && refused.includes(words),
          `2: the owner's answer is 409 CONFLICT "${words}" (got ${refused})`);
        assert(!g.origins(wallet, true).grants.some(r => r.originator === 'back.example'), '2: no grant row');

        // 3.
        await g.until(tBack + W / 5);
        const other = g.begin(wallet, 'POST', 'listOutputs', { origin: OTHER, ...listing });
        const [otherRow] = await g.untilPending(wallet, 'other.example', 1);
        assert(otherRow.pendingId !== N, `3: other.example is held on a row of its own (got #${otherRow.pendingId})`);
        const still = await rowsOf(wallet, 'back.example');
        assert(still.length === 1 && still[0].pendingId === N && still[0].abandonedAt === A,
          `3: row N still abandoned at A, unchanged (got ${JSON.stringify(still)})`);

        // 4.
        await g.until(tBack + (2 * W) / 5);
        const rejoined = g.begin(wallet, 'POST', 'listOutputs', { origin: BACK_8080, ...listing });
        const resumed = await untilRows(wallet, 'back.example', rs => rs.length === 1 && rs[0].abandonedAt === undefined,
          'row N resumed');
        assert(resumed[0].pendingId === N && resumed[0].answeredAt === null && resumed[0].abandonedUntil === undefined,
          `4: held on N, no new row, abandonedAt and abandonedUntil gone (got ${JSON.stringify(resumed)})`);
        assert(new RegExp(`browser gate: pending.*pending_id=${N} joined=true resumed=true`).test(plainLog(wallet)),
          '4: serve.log names browser gate: pending with joined and resumed');

        // 5.
        g.cli(wallet, ['origin-grant', `#${N}`]);
        const four = await rejoined.answer;
        assert(four.status === 200 && Array.isArray(four.body.outputs) && four.allowOrigin === BACK_8080,
          `5: step 4's call answers 200 with the echo (got ${four.status} ${JSON.stringify(four.body)})`);
        const rows = g.origins(wallet, true);
        const granted = rows.grants.filter(r => r.originator === 'back.example');
        assert(granted.length === 1 && granted[0].kind === 'basket' && granted[0].key === g.POWER.basket,
          `5: one grant row for back.example (got ${JSON.stringify(granted)})`);
        assert(rows.pending.find(p => p.pendingId === N).answer === 'grant', '5: row N answered grant');
        g.cli(wallet, ['origin-deny', `#${otherRow.pendingId}`]);
        const denied = await other.answer;
        assert(denied.status === 400 && denied.body.code === 'ERR_PERMISSION_DENIED'
          && denied.body.message === g.PERMISSION_DENIED && denied.allowOrigin === OTHER,
          `5: other.example's call answers 400 "${g.PERMISSION_DENIED}" with the echo (got ${denied.status} ${JSON.stringify(denied.body)})`);
        assert(!g.origins(wallet, true).grants.some(r => r.originator === 'other.example'), '5: no grant row for other.example');
        const stepFive = Date.now();
        assert(stepFive < tLate + W,
          `5: steps 2 to 5 ran inside late.example's window (ended ${stepFive - tLate} ms after t_late, W ${W} ms)`);

        // 6.
        await g.until(tLate + W + W / 10);
        const gone = g.origins(wallet, true).pending.filter(p => p.originator === 'late.example');
        assert(gone.length === 0, `6: no row of late.example, open, abandoned or answered (got ${JSON.stringify(gone)})`);
        assert(/browser gate: abandoned.*originator=late\.example/.test(plainLog(wallet)),
          '6: serve.log names browser gate: abandoned for late.example');
        const again = g.begin(wallet, 'POST', 'listOutputs', { origin: LATE, ...listing });
        const [renewed] = await g.untilPending(wallet, 'late.example', 1);
        assert(renewed.pendingId !== lateRow.pendingId, '6: the later call opens a new prompt, held');
        g.cli(wallet, ['origin-deny', `#${renewed.pendingId}`]);
        const six = await again.answer;
        assert(six.status === 400 && six.body.message === g.PERMISSION_DENIED,
          `6: the owner's deny answers it 400 "${g.PERMISSION_DENIED}" (got ${six.status} ${JSON.stringify(six.body)})`);
      } finally {
        mcp.stop();
      }
      return {};
    });
  },
};
