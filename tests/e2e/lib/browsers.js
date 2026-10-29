/** The harness's Chromium folder, pin and scenario 20 launch contract. */
const path = require('path');
const { execFileSync } = require('child_process');

const REASONS = {
  env: 'PLAYWRIGHT_BROWSERS_PATH',
  standing: 'standing checkout',
  fallback: 'worktree fallback, no git',
};

// One folder per host: the caller's export, else the standing checkout's
// .cca/pw-browsers (the parent of the common git dir, so a linked worktree
// finds its host's folder), else this worktree's own when git cannot answer.
function resolve() {
  if (process.env.PLAYWRIGHT_BROWSERS_PATH) {
    return { folder: process.env.PLAYWRIGHT_BROWSERS_PATH, reason: 'env' };
  }
  const root = path.resolve(__dirname, '..', '..', '..');
  try {
    const common = execFileSync('git', ['rev-parse', '--git-common-dir'], {
      cwd: root,
      stdio: ['ignore', 'pipe', 'ignore'],
    }).toString().trim();
    const standing = path.dirname(path.resolve(root, common));
    return { folder: path.join(standing, '.cca', 'pw-browsers'), reason: 'standing' };
  } catch {
    return { folder: path.resolve(__dirname, '..', '.browsers'), reason: 'fallback' };
  }
}

// Playwright reads this at module load. Set it before loading any of its code;
// an empty value would also send Playwright to the shared cache.
const { folder, reason } = resolve();
process.env.PLAYWRIGHT_BROWSERS_PATH = folder;
const why = REASONS[reason];
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
  const result = { folder, reason, revision, executable: executable.executablePath() };
  try {
    executable.executablePathOrDie('javascript');
    return { ...result, ok: true };
  } catch (error) {
    return { ...result, ok: false, error };
  }
}

module.exports = { folder, reason, why, revision, launchOptions, check };
