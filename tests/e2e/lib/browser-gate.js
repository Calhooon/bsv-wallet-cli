/**
 * The browser gate's scenes (#414's switch, the design's L5b,
 * docs/design/origin-grants.md section (7)): a wallet of its own on
 * WALLET_D_PORT (default 3374, off every port the registry reserves), with
 * `ALLOWED_ORIGINS` unset but in scene 28's seed step, so every other step
 * is the kinds' path: a door admits a page for each power it asks by the
 * owner's grant of that power to its origin, read from the store. The
 * owner's acts go through the owner's routes (no Origin, a loopback Host,
 * the MCP's path), the CLI (`bsv-wallet origins`, `origin-grant`,
 * `origin-deny`, `origin-revoke`), the MCP and the inspector, named per
 * step. No scene moves a satoshi.
 *
 * The scenes follow the stack's five scenarios step by step
 * (bsv-stack-lean 4372ea39, corpus/scenarios/2026-11-01-wallet-browser-gate-*.md),
 * per power under the owner's ruling W1 (a) of 2026-11-03. The powers the
 * scenes ask: `listOutputs` of the basket `browser gate` (basket
 * ["browser gate"], listing), and `getPublicKey` and `createSignature`
 * under [2, "browser gate"] with the counterparty `self` (protocol
 * [false,2,"browser gate","self"]).
 */
const fs = require('fs');
const path = require('path');
const { spawn, execFileSync } = require('child_process');
const { startWallet, teardownWallet, BSV } = require('./setup');
const { E2E_ORIGIN, E2E_ORIGINATOR } = require('./wallet-client');
const record = require('./record');

const D_PORT = parseInt(process.env.WALLET_D_PORT || '3374');
/** The inspector's port in scene 27 (scenario 20's is 9321). */
const D_UI_PORT = parseInt(process.env.WALLET_D_UI_PORT || '9374');
/** The pages' `Origin` headers, as a browser sends them. */
const APP = 'http://app.example';
const NEW = 'http://new.example';
const DENY = 'http://deny.example';
const GONE = 'http://gone.example';
const EVIL = 'http://evil.example';
const SEEDED = 'http://seeded.example';
const TOKEN = 'e2e-gate-bearer';

/** The client's refusals (bsv-desktop src/onWalletReady.ts; ts-stack edf6e03 WalletPermissionsManager.ts:1199, 1348). */
const NO_ORIGIN = 'Origin header is required';
const DENIED = 'The user has denied the request for permission.';
const PERMISSION_DENIED = 'Permission denied.';

/** The scenes' two powers, in the canonical text `bsv-wallet origins` prints. */
const POWER = {
  basket: '["browser gate"]',
  protocol: '[false,2,"browser gate","self"]',
};

/**
 * The harness's own originator holds the identity key's power, which the
 * start's identity check asks (`startWallet`): the owner's grant, written
 * before the wallet serves.
 */
const HARNESS_GRANTS = [[E2E_ORIGINATOR, '--kind', 'protocol', '--key', '[false,1,"identity key retrieval",""]']];

const sleep = (ms) => new Promise(r => setTimeout(r, ms));

/** One raw call: the scene names every header itself; recorded as the client's are. */
async function call(wallet, method, endpoint, opts = {}) {
  return record.serial(() => callNow(wallet, method, endpoint, opts));
}

async function callNow(wallet, method, endpoint, { origin, originator, bearer, body, signal } = {}) {
  const done = record.httpStart(wallet.name, method, `/${endpoint}`, body);
  const headers = {};
  if (origin) headers.Origin = origin;
  if (originator) headers.Originator = originator;
  if (bearer) headers.Authorization = `Bearer ${bearer}`;
  if (body !== undefined) headers['Content-Type'] = 'application/json';
  const resp = await fetch(`http://localhost:${wallet.port}/${endpoint}`, {
    method, headers, body: body === undefined ? undefined : JSON.stringify(body), signal,
  });
  const text = await resp.text();
  done(resp.status, text);
  let json = {};
  try { json = text ? JSON.parse(text) : {}; } catch { json = { _raw: text }; }
  return { status: resp.status, body: json, allowOrigin: resp.headers.get('access-control-allow-origin') };
}

/**
 * A call that will be held on the owner's answer: sent at once (never
 * queued behind the recorder's serial order, which a held call would
 * block), its answer a promise; `abort` closes its connection, the page's
 * disconnect.
 */
function begin(wallet, method, endpoint, opts = {}) {
  const controller = new AbortController();
  const answer = callNow(wallet, method, endpoint, { ...opts, signal: controller.signal });
  answer.catch(() => {});
  return { answer, abort: () => controller.abort() };
}

/** One of the owner's routes: no Origin, the bearer when the wallet holds one. */
async function owner(wallet, method, route, body, bearer) {
  const headers = {};
  if (bearer) headers.Authorization = `Bearer ${bearer}`;
  if (body !== undefined) headers['Content-Type'] = 'application/json';
  const resp = await fetch(`http://localhost:${wallet.port}/${route}`, {
    method, headers, body: body === undefined ? undefined : JSON.stringify(body),
  });
  const text = await resp.text();
  let json = {};
  try { json = text ? JSON.parse(text) : {}; } catch { json = { _raw: text }; }
  return { status: resp.status, body: json };
}

/** `GET /origins`: the owner's listing, the open rows and the last answered. */
async function listing(wallet, bearer) {
  const r = await owner(wallet, 'GET', 'origins', undefined, bearer);
  if (r.status !== 200) throw new Error(`GET /origins answered ${r.status}: ${JSON.stringify(r.body)}`);
  return r.body;
}

/** The owner's command line in the wallet's directory: its stdout. */
function cli(wallet, args) {
  return execFileSync(BSV, args, { cwd: wallet.dir, encoding: 'utf-8', stdio: ['ignore', 'pipe', 'pipe'] });
}

/** A command the CLI refuses: its exit status and stderr. */
function cliRefused(wallet, args) {
  try {
    cli(wallet, args);
  } catch (e) {
    return { status: e.status, stderr: String(e.stderr) };
  }
  throw new Error(`bsv-wallet ${args.join(' ')} was expected to refuse`);
}

/** `bsv-wallet origins --json [--all]`. */
function origins(wallet, all = false) {
  const out = cli(wallet, ['origins', '--json', ...(all ? ['--all'] : [])]);
  return JSON.parse(out.slice(out.indexOf('{')));
}

/** The open rows of `originator` once there are `n`, polled from the owner's route. */
async function untilPending(wallet, originator, n, bearer) {
  for (let i = 0; i < 200; i++) {
    const open = (await listing(wallet, bearer)).pending
      .filter(p => p.originator === originator && p.answeredAt === null);
    if (open.length === n) return open;
    await sleep(50);
  }
  throw new Error(`${originator}: ${n} open rows never appeared on :${wallet.port}`);
}

/** The wallet's `serve.log`. */
function log(wallet) {
  return fs.readFileSync(wallet.logPath, 'utf-8');
}

/**
 * Restart the scene's wallet with `seed` as `ALLOWED_ORIGINS` (none: unset)
 * and, when `token` is given, `AUTH_TOKEN` set: the operator's edit of the
 * wallet's `.env` and its restart. Resolves once the wallet answers an
 * open door.
 */
async function restartGated(wallet, seed, { token } = {}) {
  const envPath = path.join(wallet.dir, '.env');
  const kept = fs.readFileSync(envPath, 'utf-8').split('\n')
    .filter(l => l && !l.startsWith('ALLOWED_ORIGINS=') && !l.startsWith('AUTH_TOKEN='));
  if (seed.length > 0) kept.push(`ALLOWED_ORIGINS='${seed.join(',')}'`);
  if (token) kept.push(`AUTH_TOKEN='${token}'`);
  fs.writeFileSync(envPath, kept.join('\n') + '\n');
  await new Promise((resolve) => {
    if (wallet.proc.exitCode !== null) return resolve();
    wallet.proc.once('exit', resolve);
    wallet.proc.kill('SIGTERM');
  });
  const logFd = fs.openSync(wallet.logPath, 'a');
  const env = { ...process.env };
  delete env.ALLOWED_ORIGINS;
  delete env.AUTH_TOKEN;
  wallet.proc = spawn(BSV, ['--port', String(wallet.port), 'serve'], {
    cwd: wallet.dir, stdio: ['ignore', logFd, logFd], detached: false, env,
  });
  fs.closeSync(logFd);
  for (let i = 0; i < 40; i++) {
    try {
      const r = await call(wallet, 'GET', 'isAuthenticated', { origin: E2E_ORIGIN, bearer: token });
      if (r.status === 200) return wallet;
    } catch { /* not listening yet */ }
    await sleep(250);
  }
  throw new Error(`${wallet.dir}: the restarted wallet on :${wallet.port} did not answer (its log: ${wallet.logPath})`);
}

/** The audit's count, read as the owner (no Origin): what reached a door. */
async function audited(wallet, bearer) {
  const r = await owner(wallet, 'GET', 'permission-audit', undefined, bearer);
  if (r.status !== 200) throw new Error(`/permission-audit answered ${r.status}: ${JSON.stringify(r.body)}`);
  return r.body.count;
}

/** The arguments of the scenes' calls. */
const ARGS = {
  createSignature: { protocolID: [2, 'browser gate'], keyID: '1', counterparty: 'self', data: [1, 2, 3] },
  listOutputs: { basket: 'browser gate', limit: 10 },
  getPublicKey: { protocolID: [2, 'browser gate'], keyID: '1', counterparty: 'self' },
  createAction: {
    description: 'browser gate scene',
    outputs: [{ lockingScript: '6a', satoshis: 0, outputDescription: 'a refused call' }],
    options: { noSend: true },
  },
};

/** A scene's run on wallet D, its seed unset: torn down always, kept when the scene failed. */
async function scene(name, body) {
  const wallet = await startWallet(D_PORT, name, { env: { ALLOWED_ORIGINS: null }, grants: HARNESS_GRANTS });
  wallet.name = name;
  let keep = true;
  try {
    const result = await body(wallet);
    keep = false;
    return result;
  } finally {
    teardownWallet(wallet, { keep });
  }
}

module.exports = {
  APP, NEW, DENY, GONE, EVIL, SEEDED, TOKEN, NO_ORIGIN, DENIED, PERMISSION_DENIED, POWER, ARGS,
  D_PORT, D_UI_PORT,
  call, begin, owner, listing, cli, cliRefused, origins, untilPending, log, restartGated, audited,
  scene, sleep,
};
