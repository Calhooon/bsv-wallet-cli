#!/usr/bin/env node
/** Install the harness's pinned Chromium without collecting other builds. */
const fs = require('fs');
const path = require('path');
const { spawnSync } = require('child_process');

process.env.PLAYWRIGHT_SKIP_BROWSER_GC = '1';
const cli = path.join(__dirname, 'node_modules', '.bin', 'playwright');
if (!fs.existsSync(cli)) {
  console.error(`ERROR: ${cli} is missing; run npm ci in tests/e2e first`);
  process.exit(1);
}

const { folder, revision } = require('./lib/browsers');
console.log(`Installing E2E Chromium revision ${revision} in ${folder}`);
const child = spawnSync(cli, ['install', 'chromium', ...process.argv.slice(2)], {
  stdio: 'inherit',
  env: process.env,
});
if (child.error) console.error(`ERROR: ${cli}: ${child.error.message}`);
if (child.status === 0) {
  console.log(`Installed E2E Chromium revision ${revision} in ${folder}`);
}
process.exit(child.status ?? 1);
