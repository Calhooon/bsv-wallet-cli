/**
 * E2E.27: the browser gate, a new origin (#414's switch, the design's L5b
 * section (7)). The stack's scenario 2026-11-01-wallet-browser-gate-new-origin
 * (bsv-stack-lean 4372ea39), step by step, per power (W1 (a)), on wallet D
 * with its seed unset:
 *   1. new.example calls listOutputs of the basket `browser gate`: held on
 *      one open row, kind basket, key ["browser gate"], usage listing, call
 *      /listOutputs, asked `list the basket "browser gate"`;
 *   2. a second call: held, joining the same row;
 *   3. the MCP's list_origins and the inspector's /api/origins show the row;
 *      the owner answers grant through the MCP's answer_origin: both held
 *      calls answer 200 with the echo, one grant row (expiry 0), the row
 *      answered grant. The open doors and level 0 (W15, ruled (i)): a page
 *      holding no row reads getVersion and signs under [0, "browser gate"]
 *      at once, with no row;
 *   4. a new call: 200 at once;
 *   5. deny.example calls; the owner denies with `bsv-wallet origin-deny`:
 *      400 ERR_PERMISSION_DENIED "Permission denied." with the echo, no
 *      grant row;
 *   6. gone.example calls and closes its connection once its row appears;
 *      at its close plus the window W and a tenth of it (#534, W read from
 *      the module's constant through the harness) no row of gone.example is
 *      kept, open, abandoned or answered (W4 (a) past the window), no grant
 *      row, serve.log names `browser gate: disconnected` and then `browser
 *      gate: abandoned`; its next call opens a new prompt (a new pending id)
 *      and is held; the owner grants it from the inspector, and it answers
 *      200. The step waits out the window: a call inside it would resume the
 *      same row (scene 34).
 * Cost: 0 sats.
 */
const g = require('../lib/browser-gate');
const { McpClient } = require('../lib/mcp');

module.exports = {
  name: '2026-11-01-wallet-browser-gate-new-origin',
  description: "a new origin's power is held on one pending row until the owner answers it from any place; a disconnect withdraws it",

  async run(_walletA, _walletB, assert) {
    return g.scene('wallet-D-new-origin', async (wallet) => {
      // 1.
      const first = g.begin(wallet, 'POST', 'listOutputs', { origin: g.NEW, body: g.ARGS.listOutputs });
      const [row] = await g.untilPending(wallet, 'new.example', 1);
      assert(row.kind === 'basket' && row.key === g.POWER.basket && row.usage === 'listing'
        && row.call === '/listOutputs' && row.asked === 'list the basket "browser gate"',
        `1: one open row for the basket's listing (got ${JSON.stringify(row)})`);
      // 2.
      const second = g.begin(wallet, 'POST', 'listOutputs', { origin: g.NEW, body: g.ARGS.listOutputs });
      await g.sleep(500);
      const still = await g.untilPending(wallet, 'new.example', 1);
      assert(still[0].pendingId === row.pendingId, '2: the second call joins the one row');
      // 3. The MCP and the inspector read the row; the owner grants it through the MCP.
      const mcp = new McpClient(wallet.port);
      const ui = await g.inspector(wallet);
      try {
        await mcp.start();
        await mcp.request('initialize', { protocolVersion: '2024-11-05', capabilities: {}, clientInfo: { name: 'e2e-test', version: '1.0' } });
        mcp.notify('notifications/initialized');
        const fromMcp = await mcp.callTool('list_origins', {});
        const fromUi = await (await fetch(`http://localhost:${g.D_UI_PORT}/api/origins`)).json();
        for (const [place, listed] of [['the MCP', fromMcp], ['the inspector', fromUi]]) {
          const open = listed.pending.filter(p => p.answeredAt === null);
          assert(open.length === 1 && open[0].pendingId === row.pendingId && open[0].key === g.POWER.basket,
            `3: ${place} shows the one pending row (got ${JSON.stringify(listed.pending)})`);
        }
        await mcp.callTool('answer_origin', { pendingId: row.pendingId, answer: 'grant' });
        for (const [n, held] of [[1, first], [2, second]]) {
          const r = await held.answer;
          assert(r.status === 200 && Array.isArray(r.body.outputs) && r.allowOrigin === g.NEW,
            `3: held call ${n} answers 200 with the echo once granted (got ${r.status} ${JSON.stringify(r.body)})`);
        }
        const rows = g.origins(wallet, true);
        const grants = rows.grants.filter(r => r.originator === 'new.example');
        assert(grants.length === 1 && grants[0].kind === 'basket' && grants[0].key === g.POWER.basket && grants[0].expiry === 0,
          `3: one grant row, expiry 0 (got ${JSON.stringify(grants)})`);
        assert(rows.pending.find(p => p.pendingId === row.pendingId).answer === 'grant', '3: the row answered grant');
        // W15 (i): the open doors and level 0, with no row.
        const version = await g.call(wallet, 'GET', 'getVersion', { origin: g.NEW });
        assert(version.status === 200 && version.allowOrigin === g.NEW, `3 (W15): getVersion at once (got ${version.status})`);
        const open = await g.call(wallet, 'POST', 'createSignature',
          { origin: g.NEW, body: { ...g.ARGS.createSignature, protocolID: [0, 'browser gate'] } });
        assert(open.status === 200 && Array.isArray(open.body.signature),
          `3 (W15): a level-0 signature at once (got ${open.status} ${JSON.stringify(open.body)})`);
        assert(g.origins(wallet, true).pending.length === 1, '3 (W15): no row for an open door or level 0');
        // 4.
        const four = await g.call(wallet, 'POST', 'listOutputs', { origin: g.NEW, body: g.ARGS.listOutputs });
        assert(four.status === 200 && four.allowOrigin === g.NEW, `4: granted, answered at once (got ${four.status})`);
        // 5.
        const denied = g.begin(wallet, 'POST', 'listOutputs', { origin: g.DENY, body: g.ARGS.listOutputs });
        const [deny] = await g.untilPending(wallet, 'deny.example', 1);
        g.cli(wallet, ['origin-deny', `#${deny.pendingId}`]);
        const five = await denied.answer;
        assert(five.status === 400 && five.body.code === 'ERR_PERMISSION_DENIED' && five.body.message === g.PERMISSION_DENIED
          && five.allowOrigin === g.DENY,
          `5: the owner's deny, 400 "${g.PERMISSION_DENIED}" with the echo (got ${five.status} ${JSON.stringify(five.body)})`);
        assert(!g.origins(wallet, true).grants.some(r => r.originator === 'deny.example'), '5: no grant row');
        // 6.
        const gone = g.begin(wallet, 'POST', 'listOutputs', { origin: g.GONE, body: g.ARGS.listOutputs });
        const [left] = await g.untilPending(wallet, 'gone.example', 1);
        gone.abort();
        const closed = Date.now();
        await g.until(closed + g.W + g.W / 10);
        const kept = g.origins(wallet, true).pending.filter(p => p.originator === 'gone.example');
        assert(kept.length === 0, `6: past the window no row of gone.example, open, abandoned or answered (got ${JSON.stringify(kept)})`);
        assert(!g.origins(wallet, true).grants.some(r => r.originator === 'gone.example'), '6: no grant row');
        const said = g.log(wallet);
        const marked = said.indexOf('browser gate: disconnected');
        assert(marked >= 0 && said.indexOf('browser gate: abandoned', marked) > marked,
          '6: serve.log names browser gate: disconnected and then browser gate: abandoned');
        const again = g.begin(wallet, 'POST', 'listOutputs', { origin: g.GONE, body: g.ARGS.listOutputs });
        const [renewed] = await g.untilPending(wallet, 'gone.example', 1);
        assert(renewed.pendingId !== left.pendingId, '6: the later call opens a new prompt, held');
        // The inspector grants it (its own Origin, the only one its writes admit).
        const fromInspector = await fetch(`http://localhost:${g.D_UI_PORT}/api/origins/answer`, {
          method: 'POST',
          headers: { 'Content-Type': 'application/json', Origin: `http://localhost:${g.D_UI_PORT}` },
          body: JSON.stringify({ pendingId: renewed.pendingId, answer: 'grant' }),
        });
        assert(fromInspector.ok, `6: the inspector grants the row (got ${fromInspector.status})`);
        const six = await again.answer;
        assert(six.status === 200 && six.allowOrigin === g.GONE,
          `6: granted from the inspector, the held call answers 200 (got ${six.status} ${JSON.stringify(six.body)})`);
      } finally {
        mcp.stop();
        ui.kill('SIGTERM');
      }
      return {};
    });
  },
};
