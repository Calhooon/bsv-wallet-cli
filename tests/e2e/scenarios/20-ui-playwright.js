/**
 * E2E.20: UI (Playwright).
 * Tests the database inspector UI via Playwright.
 * Starts the UI server, navigates to it, verifies page renders with wallet data.
 * The origins view (#414): a pending row seeded with sqlite3 renders at
 * /origins and is granted from the page (the row answered in the store); a
 * listed origin cannot answer it (the write routes admit the inspector's
 * own page alone). Cost: 0 sats.
 */

const { spawn, execFileSync } = require('child_process');
const path = require('path');
const { BSV } = require('../lib/setup');
const { sleep } = require('../lib/woc');
const { launchOptions } = require('../lib/browsers');

module.exports = {
  name: 'ui-playwright',
  description: 'Database inspector UI renders correctly via Playwright (0 sats)',

  async run(walletA, walletB, assert) {
    const UI_PORT = 9321;
    let uiProc = null;

    try {
      // ─── Start UI server in background ───
      uiProc = spawn(BSV, ['--db', 'wallet.db', 'ui', '--ui-port', String(UI_PORT)], {
        cwd: walletA.dir,
        stdio: ['ignore', 'pipe', 'pipe'],
        detached: false,
      });

      // Wait for server to be ready
      let ready = false;
      for (let i = 0; i < 20; i++) {
        try {
          const resp = await fetch(`http://localhost:${UI_PORT}/api/tables`);
          if (resp.ok) { ready = true; break; }
        } catch { /* not ready yet */ }
        await sleep(500);
      }
      assert(ready, 'UI server must start and respond within 10s');

      // ─── Test HTTP API endpoints first (fetch-based) ───

      // /api/tables
      const tablesResp = await fetch(`http://localhost:${UI_PORT}/api/tables`);
      assert(tablesResp.ok, `/api/tables should return 200, got ${tablesResp.status}`);
      const tables = await tablesResp.json();
      assert(Array.isArray(tables), '/api/tables should return array');
      assert(tables.length > 0, '/api/tables should have tables');
      const tableNames = tables.map(t => t.name);
      assert(tableNames.includes('outputs'), 'Tables should include "outputs"');
      assert(tableNames.includes('transactions'), 'Tables should include "transactions"');

      // /api/stats
      const statsResp = await fetch(`http://localhost:${UI_PORT}/api/stats`);
      assert(statsResp.ok, `/api/stats should return 200, got ${statsResp.status}`);
      const stats = await statsResp.json();
      assert(stats.total_balance !== undefined, 'Stats should include total_balance');
      assert(stats.total_utxo_count !== undefined, 'Stats should include total_utxo_count');

      // /api/tables/outputs (data)
      const outputsResp = await fetch(`http://localhost:${UI_PORT}/api/tables/outputs`);
      assert(outputsResp.ok, `/api/tables/outputs should return 200`);
      const outputsData = await outputsResp.json();
      assert(outputsData.columns && outputsData.columns.length > 0,
        'Table data should have columns');
      assert(outputsData.total >= 0, 'Table data should have total count');

      // /api/tables/outputs/schema
      const schemaResp = await fetch(`http://localhost:${UI_PORT}/api/tables/outputs/schema`);
      assert(schemaResp.ok, `/api/tables/outputs/schema should return 200`);

      // POST /api/query (read-only SQL)
      // A POST names its Origin, as the inspector's own page sends it (#411:
      // the inspector admits its own origin; a GET without one is its page's).
      const ownOrigin = `http://localhost:${UI_PORT}`;
      const queryResp = await fetch(`http://localhost:${UI_PORT}/api/query`, {
        method: 'POST',
        headers: { 'Content-Type': 'application/json', 'Origin': ownOrigin },
        body: JSON.stringify({ sql: 'SELECT COUNT(*) as cnt FROM outputs' }),
      });
      assert(queryResp.ok, `/api/query should return 200`);
      const queryResult = await queryResp.json();
      assert(queryResult.rows && queryResult.rows.length > 0,
        'SQL query should return rows');

      // POST /api/query (mutation should be blocked)
      const mutationResp = await fetch(`http://localhost:${UI_PORT}/api/query`, {
        method: 'POST',
        headers: { 'Content-Type': 'application/json', 'Origin': ownOrigin },
        body: JSON.stringify({ sql: 'DROP TABLE outputs' }),
      });
      assert(!mutationResp.ok || (await mutationResp.text()).includes('Only SELECT'),
        'Mutation queries must be blocked');

      // #411: a POST from another origin is refused before the query runs.
      const foreignResp = await fetch(`http://localhost:${UI_PORT}/api/query`, {
        method: 'POST',
        headers: { 'Content-Type': 'application/json', 'Origin': 'http://evil.example' },
        body: JSON.stringify({ sql: 'SELECT COUNT(*) as cnt FROM outputs' }),
      });
      assert(foreignResp.status === 403, `a foreign origin's query is refused 403, got ${foreignResp.status}`);

      // ─── The origins view's API (#414) ───
      // The read creates the module's tables (its `ensure`); the row is then
      // seeded beside the wallet's own writer, as the switch's gate (L5b)
      // will write it: one power, the basket `browser gate` asked by a
      // listing (#414, the four kinds).
      const originsResp = await fetch(`http://localhost:${UI_PORT}/api/origins`);
      assert(originsResp.ok, `/api/origins should return 200, got ${originsResp.status}`);
      const originsBefore = await originsResp.json();
      assert(Array.isArray(originsBefore.grants) && Array.isArray(originsBefore.pending),
        `/api/origins must return {grants, pending}: ${JSON.stringify(originsBefore)}`);
      const db = path.join(walletA.dir, 'wallet.db');
      const sql = query => execFileSync('sqlite3', ['-batch', '-bail', '-cmd', '.timeout 5000', db, query],
        { encoding: 'utf8', timeout: 15000 }).trim();
      const pendingId = Number(sql(
        "INSERT INTO origin_pending (originator, raw_originator, kind, key, usage, call, renewal, first_seen) " +
        "VALUES ('seeded.example', 'seeded.example:5173', 'basket', '[\"browser gate\"]', 'listing', '/listOutputs', 0, strftime('%s','now')); " +
        'SELECT last_insert_rowid();'));
      assert(pendingId > 0, `the seeded pending row has an id, got ${pendingId}`);

      // A listed origin (the harness's ALLOWED_ORIGINS, which the inspector
      // reads from the wallet's .env) cannot answer it: 403 naming the rule.
      const listedAnswer = await fetch(`http://localhost:${UI_PORT}/api/origins/answer`, {
        method: 'POST',
        headers: { 'Content-Type': 'application/json', 'Origin': walletA.client.origin },
        body: JSON.stringify({ pendingId, answer: 'grant' }),
      });
      const listedBody = await listedAnswer.json();
      assert(listedAnswer.status === 403 && /a granted page must not write a grant/.test(listedBody.error),
        `a listed origin's answer is refused 403 by the rule, got ${listedAnswer.status} ${JSON.stringify(listedBody)}`);
      assert(sql(`SELECT answered_at IS NULL FROM origin_pending WHERE pending_id = ${pendingId}`) === '1',
        'the refused answer left the row open');

      // ─── Playwright browser test ───
      const { chromium } = require('playwright');
      let browserChecks = 0;
      const browser = await chromium.launch(launchOptions());
      try {
        const page = await browser.newPage();
        await page.goto(`http://localhost:${UI_PORT}`, { waitUntil: 'networkidle' });

        // Verify page title
        const title = await page.title();
        assert(title.includes('Wallet Inspector'),
          `Page title should contain 'Wallet Inspector', got '${title}'`);
        browserChecks++;

        // Verify header renders
        const headerTitle = await page.textContent('.header-title');
        assert(headerTitle && headerTitle.includes('Wallet Inspector'),
          `Header should show 'Wallet Inspector', got '${headerTitle}'`);
        browserChecks++;

        // Verify logo
        const logo = await page.textContent('.logo');
        assert(logo === 'B', `Logo should be 'B', got '${logo}'`);
        browserChecks++;

        // Verify sidebar has table entries (loaded from /api/tables)
        const sidebarItems = await page.$$('.sidebar-item');
        assert(sidebarItems.length > 0,
          `Sidebar should have table items, got ${sidebarItems.length}`);
        browserChecks++;

        // Verify header stats are populated (loaded from /api/stats)
        const headerStats = await page.textContent('#headerStats');
        assert(headerStats && headerStats.length > 0,
          'Header stats should be populated');
        browserChecks++;

        // Verify navigation buttons exist
        const dashBtn = await page.$('#navDashboard');
        assert(dashBtn, 'Dashboard nav button should exist');
        const schemaBtn = await page.$('#navSchema');
        assert(schemaBtn, 'Schema nav button should exist');
        const sqlBtn = await page.$('#navSql');
        assert(sqlBtn, 'SQL Console nav button should exist');
        browserChecks++;

        // Click a table to load data
        const firstItem = sidebarItems[0];
        if (firstItem) {
          await firstItem.click();
          await page.waitForTimeout(1000);

          // Verify main content area has a table
          const mainContent = await page.textContent('#mainContent');
          assert(mainContent && mainContent.length > 10,
            'Main content should have table data after clicking a table');
          browserChecks++;
        }

        // The origins view: the seeded row renders, and Grant answers it.
        await page.goto(`http://localhost:${UI_PORT}/origins`, { waitUntil: 'networkidle' });
        const row = page.locator(`#originsPending tr[data-pending-id="${pendingId}"]`);
        await row.waitFor({ timeout: 5000 });
        const rowText = await row.textContent();
        assert(rowText.includes('seeded.example:5173') && rowText.includes('/listOutputs') &&
          rowText.includes('basket') && rowText.includes('list the basket "browser gate"'),
          `the pending row shows its originator with its port, its kind, the power it asks and its call, got '${rowText}'`);
        await row.locator('button', { hasText: /^Grant$/ }).click();
        const grantRow = page.locator('#originsGrants tr[data-originator="seeded.example"]');
        await grantRow.waitFor({ timeout: 5000 });
        // #533: the table prints the state word the JSON carries, never its own.
        const state = await grantRow.locator('td.origin-state').textContent();
        assert(state === 'granted' && (await grantRow.getAttribute('data-state')) === 'granted',
          `the grant row reads its state from /api/origins, got '${state}'`);
        browserChecks++;
      } finally {
        await browser.close();
      }

      // The row is answered in the store, and its grant written by the owner.
      const answered = sql(`SELECT answer FROM origin_pending WHERE pending_id = ${pendingId} AND answered_at IS NOT NULL`);
      assert(answered === 'grant', `the seeded row is answered grant in the store, got '${answered}'`);
      const granted = sql("SELECT kind || ' ' || key || ' ' || expiry || ' ' || (revoked_at IS NULL) FROM origin_grants WHERE originator = 'seeded.example'");
      assert(granted === 'basket ["browser gate"] 0 1', `the grant row is the basket power, never expiring, live; got '${granted}'`);

      return {
        txid: null,
        sats: 0,
        wocConfirmed: 'n/a',
        tables: tables.length,
        balance: stats.total_balance,
        utxos: stats.total_utxo_count,
        playwrightAvailable: true,
        browserChecks,
        originGranted: pendingId,
      };

    } finally {
      // ─── Kill UI server ───
      if (uiProc && !uiProc.killed) {
        uiProc.kill('SIGTERM');
        // Wait for process to exit
        await new Promise(resolve => {
          uiProc.on('exit', resolve);
          setTimeout(resolve, 2000);
        });
      }
    }
  },
};
