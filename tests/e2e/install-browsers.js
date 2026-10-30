#!/usr/bin/env node
/** Install the harness's pinned Chromium without collecting other builds. */
const browsers = require('./lib/browsers');
if (!browsers.node.ok) {
  const range = require('./package.json').engines.node;
  let playwrightVersion;
  try {
    playwrightVersion = require('playwright/package.json').version;
  } catch {
    playwrightVersion = require('./package-lock.json').packages['node_modules/playwright'].version;
  }
  console.error(`ERROR: Node v${browsers.node.version} is outside the harness's proven range (${range}): Playwright ${playwrightVersion}'s zip extractor never finishes under Node 26 (#366); run the step under one of them (nvm use 22)`);
  process.exit(1);
}

const fs = require('fs');
const path = require('path');
const { spawnSync } = require('child_process');

process.env.PLAYWRIGHT_SKIP_BROWSER_GC = '1';
const cli = path.join(__dirname, 'node_modules', '.bin', 'playwright');
if (!fs.existsSync(cli)) {
  console.error(`ERROR: ${cli} is missing; run npm ci in tests/e2e first`);
  process.exit(1);
}

const { folder, why, revision } = browsers;
console.log(`Installing E2E Chromium revision ${revision} in ${folder} (${why})`);
const child = spawnSync(cli, ['install', 'chromium', ...process.argv.slice(2)], {
  stdio: 'inherit',
  env: process.env,
});
if (child.error) console.error(`ERROR: ${cli}: ${child.error.message}`);
if (child.status === 0) {
  console.log(`Installed E2E Chromium revision ${revision} in ${folder} (${why})`);
}
process.exit(child.status ?? 1);
