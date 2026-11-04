/**
 * E2E.35: the browser gate's grants read alike in every place (#533), on
 * wallet D with its seed unset. The four places read one reading of each
 * grant row (`GrantShown`, src/store/origin_grants/shown.rs): the CLI
 * (`bsv-wallet origins`, text and `--json`), the owner's route
 * (`GET /origins`), the MCP's `list_origins` (the MCP binary over the
 * route) and the inspector's API (`GET /api/origins`). Every step reads
 * the four at once and compares them to one another value for value
 * (state, amount, spentThisMonth, ceilingReached, grantedAt, expiry,
 * revokedAt), and the CLI's text line to the JSON's values:
 *   1. the owner grants app.example the basket ["browser gate"] with
 *      `--expires 2`: `granted` in every place, its expiry its grant's time
 *      plus 2; after 3 s, `expired` in every place (not `revoked`, not
 *      `granted`);
 *   2. the owner grants app.example spending with a ceiling of 1,500
 *      (`origin-grant app.example --kind spending --amount 1500`): 0 spent,
 *      the ceiling not reached;
 *   3. D is funded from A (3,000: A pays D's funding address, the owner's
 *      `bsv-wallet fund`); the page's createAction for 1,000 to A is
 *      admitted and broadcast: 1,000 spent, the ceiling not reached;
 *   4. the page's 500 to A: admitted, 1,500 spent, the ceiling reached
 *      (`at the ceiling` in the text);
 *   5. the page's 1 satoshi: held on a pending row with renewal true, its
 *      prompt naming the ceiling and the tally; the four read the row at the
 *      ceiling and the open row alike; the owner denies it (400
 *      ERR_PERMISSION_DENIED);
 *   6. the rest of D goes back to A under the harness's own spending grant.
 * A internalizes each payment, so the cost is fees only.
 */
const { promisify } = require('util');
const { execFile } = require('child_process');
const g = require('../lib/browser-gate');
const { BSV } = require('../lib/setup');
const { McpClient } = require('../lib/mcp');
const { ANYONE_KEY, buildP2PKH, E2E_ORIGINATOR } = require('../lib/wallet-client');

const run = promisify(execFile);

/** BRC-29's default derivation, the remittance A internalizes with. */
const REMITTANCE = { derivationPrefix: 'SfKxPIJNgdI=', derivationSuffix: 'NaGLC6fMH50=', senderIdentityKey: ANYONE_KEY };

/** The fields every place carries for a grant row. */
const FIELDS = ['originator', 'rawOriginator', 'kind', 'key', 'state', 'amount', 'spentThisMonth',
  'ceilingReached', 'grantedAt', 'expiry', 'revokedAt'];

/** A JSON value with its keys sorted (the MCP re-serializes the route's body). */
const canonical = (v) => JSON.stringify(v, (_, x) => (x && typeof x === 'object' && !Array.isArray(x)
  ? Object.fromEntries(Object.keys(x).sort().map(k => [k, x[k]])) : x));

/** `bsv-wallet origins`'s time, `YYYY-MM-DDTHH:MMZ`. */
const when = (secs) => new Date(secs * 1000).toISOString().slice(0, 16) + 'Z';

/** The JSON at the first `{` of a command's stdout. */
const parsed = (out) => JSON.parse(out.slice(out.indexOf('{')));

module.exports = {
  name: '2026-11-03-wallet-browser-gate-grant-surfaces',
  description: "a grant's expiry, state, ceiling and month's spend read alike by the CLI, the owner's route, the MCP and the inspector",

  async run(walletA, _walletB, assert) {
    return g.scene('wallet-D-grant-surfaces', async (wallet) => {
      const mcp = new McpClient(wallet.port);
      const ui = await g.inspector(wallet);
      try {
        await mcp.start();
        await mcp.request('initialize', { protocolVersion: '2024-11-05', capabilities: {}, clientInfo: { name: 'e2e-test', version: '1.0' } });
        mcp.notify('notifications/initialized');
        const cliNow = async (args) => (await run(BSV, args, { cwd: wallet.dir, encoding: 'utf-8' })).stdout;

        /**
         * The five readings at once: the CLI's text and JSON, the route, the
         * MCP and the inspector's API; the app's grant rows of each JSON
         * compared to one another, and each text line to its row's values.
         */
        const readAll = async (step) => {
          const [text, cli, route, fromMcp, fromUi] = await Promise.all([
            cliNow(['origins']),
            cliNow(['--json', 'origins']).then(parsed),
            g.listing(wallet),
            mcp.callTool('list_origins', {}),
            fetch(`http://localhost:${g.D_UI_PORT}/api/origins`).then(r => r.json()),
          ]);
          const places = { cli, route, mcp: fromMcp, inspector: fromUi };
          const rowsOf = (listed) => listed.grants.filter(r => r.originator === 'app.example')
            .map(r => Object.fromEntries(FIELDS.map(f => [f, r[f]])));
          const openOf = (listed) => listed.pending.filter(p => p.originator === 'app.example' && p.answeredAt === null);
          const rows = rowsOf(route);
          for (const [place, listed] of Object.entries(places)) {
            for (const r of listed.grants) {
              assert(FIELDS.every(f => f in r), `${step}: ${place}'s grant row carries every field (got ${JSON.stringify(r)})`);
            }
            assert(canonical(rowsOf(listed)) === canonical(rows),
              `${step}: ${place} reads the route's grant rows value for value (${canonical(rowsOf(listed))} vs ${canonical(rows)})`);
            assert(canonical(openOf(listed)) === canonical(openOf(route)),
              `${step}: ${place} reads the route's open rows (${canonical(openOf(listed))} vs ${canonical(openOf(route))})`);
          }
          const lines = text.split('\n').filter(l => /^(granted|expired|revoked)  app\.example  /.test(l));
          assert(lines.length === rows.length, `${step}: the text has one line a row (got ${JSON.stringify(lines)})`);
          for (const r of rows) {
            const head = `${r.state}  app.example  ${r.kind.padEnd(11)}  ${r.key}  `;
            const line = lines.find(l => l.startsWith(head));
            assert(line, `${step}: the text reads ${head.trim()} (got ${JSON.stringify(lines)})`);
            const expires = r.expiry === 0 ? 'never' : when(r.expiry);
            assert(line.includes(`  since ${when(r.grantedAt)}  expires ${expires}`),
              `${step}: the text's grant time and expiry are the row's (${line})`);
            if (r.kind === 'spending') {
              const month = new Date().toISOString().slice(0, 7);
              const ceiling = r.amount === null ? 'no ceiling' : `${r.amount}/month`;
              const reached = r.ceilingReached ? ', at the ceiling' : '';
              assert(line.includes(`  ${ceiling} (${r.spentThisMonth} spent ${month}${reached})  `),
                `${step}: the text's ceiling, tally and reading are the row's (${line})`);
            }
          }
          return { rows, open: openOf(route), text };
        };
        const row = (rows, kind) => rows.find(r => r.kind === kind);

        // 1. A grant with an expiry, live then expired in every place.
        const granted = g.cli(wallet, ['origin-grant', 'app.example', '--kind', 'basket', '--key', g.POWER.basket, '--expires', '2']);
        assert(granted.startsWith(`granted  app.example  basket       ${g.POWER.basket}  since `), `1: the grant's line (got ${granted})`);
        const live = await readAll('1 live');
        const basket = row(live.rows, 'basket');
        assert(basket && basket.state === 'granted' && basket.expiry === basket.grantedAt + 2
          && basket.revokedAt === null && basket.amount === null && basket.spentThisMonth === null && basket.ceilingReached === null,
          `1: granted, expiring 2 s after its grant (got ${JSON.stringify(basket)})`);
        await g.sleep(3000);
        while (Date.now() / 1000 <= basket.expiry + 1) await g.sleep(100);
        const gone = row((await readAll('1 expired')).rows, 'basket');
        assert(gone.state === 'expired' && gone.revokedAt === null && gone.expiry === basket.expiry,
          `1: expired in every place, not revoked (got ${JSON.stringify(gone)})`);

        // 2. A spending grant with its ceiling.
        g.cli(wallet, ['origin-grant', 'app.example', '--kind', 'spending', '--amount', '1500']);
        const ceiling = row((await readAll('2')).rows, 'spending');
        assert(ceiling.state === 'granted' && ceiling.amount === 1500 && ceiling.spentThisMonth === 0
          && ceiling.ceilingReached === false && ceiling.expiry === 0,
          `2: the ceiling, nothing spent (got ${JSON.stringify(ceiling)})`);

        // 3. D funded from A; the page's admitted spend of 1,000 to A.
        const aKey = await walletA.client.getPublicKey([2, '3241645161d8'], REMITTANCE.derivationPrefix + ' ' + REMITTANCE.derivationSuffix, ANYONE_KEY, true);
        const toA = buildP2PKH(aKey.publicKey);
        const dKey = parsed(g.cli(wallet, ['--json', 'address'])).pubkey;
        const funding = await walletA.client.createAction([{
          lockingScript: buildP2PKH(dKey), satoshis: 3000, outputDescription: 'E2E.35 fund D', tags: ['e2e-test'],
        }], 'E2E.35 fund D');
        const funded = parsed(g.cli(wallet, ['--json', 'fund', Buffer.from(funding.tx).toString('hex'), '--vout', '0']));
        assert(funded.accepted === true, `3: D takes A's 3,000 (got ${JSON.stringify(funded)})`);
        const pageSpend = async (satoshis, step) => {
          const r = await g.call(wallet, 'POST', 'createAction', {
            origin: g.APP,
            body: {
              description: `E2E.35 a page spend ${step}`,
              outputs: [{ lockingScript: toA, satoshis, outputDescription: 'E2E.35 to A' }],
              allowPending: 'received',
              options: { randomizeOutputs: false, acceptDelayedBroadcast: false },
            },
          });
          assert(r.status === 200 && r.body.txid && Array.isArray(r.body.tx) && r.allowOrigin === g.APP,
            `${step}: the page's ${satoshis} admitted and sent (got ${r.status} ${JSON.stringify(r.body).slice(0, 300)})`);
          await walletA.client.internalizeAction(r.body.tx,
            [{ outputIndex: 0, protocol: 'wallet payment', paymentRemittance: REMITTANCE }], `E2E.35 from D ${step}`);
          return r.body.txid;
        };
        const firstTxid = await pageSpend(1000, '3');
        const tallied = row((await readAll('3')).rows, 'spending');
        assert(tallied.spentThisMonth === 1000 && tallied.ceilingReached === false && tallied.state === 'granted',
          `3: 1,000 spent under the ceiling (got ${JSON.stringify(tallied)})`);

        // 4. The page's 500: the month reaches the ceiling.
        await pageSpend(500, '4');
        const full = await readAll('4');
        const reached = row(full.rows, 'spending');
        assert(reached.spentThisMonth === 1500 && reached.ceilingReached === true && reached.state === 'granted',
          `4: 1,500 spent, the ceiling reached (got ${JSON.stringify(reached)})`);
        assert(full.text.includes('1500/month (1500 spent ') && full.text.includes(', at the ceiling)'),
          `4: the text says at the ceiling (got ${full.text})`);

        // 5. A call at the ceiling: held as a renewal, read alike, denied.
        const past = g.begin(wallet, 'POST', 'createAction', {
          origin: g.APP,
          body: {
            description: 'E2E.35 a page spend at the ceiling',
            outputs: [{ lockingScript: toA, satoshis: 1, outputDescription: 'E2E.35 to A' }],
            allowPending: 'received',
          },
        });
        const [renewal] = await g.untilPending(wallet, 'app.example', 1);
        assert(renewal.kind === 'spending' && renewal.renewal === true && renewal.amount === 1
          && renewal.asked === 'spend 1 satoshis past this month\'s ceiling of 1500 (1500 spent)',
          `5: held as a renewal at the ceiling (got ${JSON.stringify(renewal)})`);
        const held = await readAll('5');
        assert(held.open.length === 1 && held.open[0].pendingId === renewal.pendingId, `5: the one open row in every place`);
        const atCeiling = row(held.rows, 'spending');
        assert(atCeiling.ceilingReached === true && atCeiling.spentThisMonth === 1500,
          `5: the row reads at the ceiling while the call is held (got ${JSON.stringify(atCeiling)})`);
        assert(held.text.split('\n').some(l => l.startsWith(`pending  #${renewal.pendingId}  app.example  spending  `) && l.includes('  renewal  ')),
          `5: the text's open row is a renewal (got ${held.text})`);
        g.cli(wallet, ['origin-deny', `#${renewal.pendingId}`]);
        const five = await past.answer;
        assert(five.status === 400 && five.body.code === 'ERR_PERMISSION_DENIED' && five.allowOrigin === g.APP,
          `5: denied (got ${five.status} ${JSON.stringify(five.body)})`);
        const after = await readAll('5 denied');
        assert(after.open.length === 0 && row(after.rows, 'spending').spentThisMonth === 1500,
          `5: no open row, the tally unmoved by a denied call (got ${JSON.stringify(after)})`);

        // 6. The rest goes back to A under the harness's own spending grant.
        g.cli(wallet, ['origin-grant', E2E_ORIGINATOR, '--kind', 'spending', '--amount', '100000']);
        const rest = parsed(g.cli(wallet, ['--json', 'balance'])).satoshis;
        assert(rest > 600, `6: D holds its change (${rest})`);
        const back = await wallet.client.createAction([{ lockingScript: toA, satoshis: rest - 300, outputDescription: 'E2E.35 back to A' }],
          'E2E.35 back to A', { allowPending: 'received' });
        await walletA.client.internalizeAction(back.tx,
          [{ outputIndex: 0, protocol: 'wallet payment', paymentRemittance: REMITTANCE }], 'E2E.35 the rest from D');
        return { txid: firstTxid, returnTxid: back.txid };
      } finally {
        mcp.stop();
        ui.kill('SIGTERM');
      }
    });
  },
};
