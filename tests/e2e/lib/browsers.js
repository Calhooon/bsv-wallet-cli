/** The harness's Chromium folder, pin and scenario 20 launch contract. */
const path = require('path');

// Playwright reads this at module load. Set it before loading any of its code;
// an empty value would also send Playwright to the shared cache.
if (!process.env.PLAYWRIGHT_BROWSERS_PATH) {
  process.env.PLAYWRIGHT_BROWSERS_PATH = path.resolve(__dirname, '..', '.browsers');
}
const folder = process.env.PLAYWRIGHT_BROWSERS_PATH;
const core = path.dirname(require.resolve('playwright-core/package.json'));
const { revision } = require(path.join(core, 'browsers.json')).browsers
  .find(browser => browser.name === 'chromium');

function launchOptions() {
  const options = { headless: true };
  if (process.env.E2E_HERMETIC === '1') {
    // Chromium ignores the proxy environment. Its own loopback bypass keeps
    // the inspector direct; Playwright's proxy option would disable that.
    options.args = [`--proxy-server=${process.env.HTTPS_PROXY}`];
  }
  return options;
}

function check() {
  // Use the same selector and registry check as Chromium.launch, including
  // the headless shell and platform-specific path. executablePath() on the
  // public chromium object names the full browser, which headless does not use.
  const { Chromium } = require(path.join(core, 'lib/server/chromium/chromium.js'));
  const { registry } = require(path.join(core, 'lib/server/registry/index.js'));
  const name = Chromium.prototype.getExecutableName(launchOptions());
  const executable = registry.findExecutable(name);
  const result = { folder, revision, executable: executable.executablePath() };
  try {
    executable.executablePathOrDie('javascript');
    return { ...result, ok: true };
  } catch (error) {
    return { ...result, ok: false, error };
  }
}

module.exports = { folder, revision, launchOptions, check };
