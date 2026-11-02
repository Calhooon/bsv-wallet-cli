/**
 * E2E.20: UI (Playwright).
 * Tests the database inspector UI via Playwright.
 * Starts the UI server, navigates to it, verifies page renders with wallet data.
 * Cost: 0 sats (read-only).
 */

const { spawn } = require('child_process');
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

      } finally {
        await browser.close();
      }

      return {
        txid: null,
        sats: 0,
        wocConfirmed: 'n/a',
        tables: tables.length,
        balance: stats.total_balance,
        utxos: stats.total_utxo_count,
        playwrightAvailable: true,
        browserChecks,
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
