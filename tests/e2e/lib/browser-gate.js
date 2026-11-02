/**
 * The browser gate's scenes (#411, step (b) of the owner's word of
 * 2026-11-01): a wallet of its own on WALLET_D_PORT (default 3374, off every
 * port the registry reserves), whose `ALLOWED_ORIGINS` and `AUTH_TOKEN` a
 * scene sets and changes by restarting it, as an operator would. Under (b)
 * the owner's grant is a line of `ALLOWED_ORIGINS` and a revocation is its
 * removal and a restart: the stand-ins for #414's grant rows. No scene moves
 * a satoshi: every call is refused before a door or is a key read.
 *
 * The scenes follow the stack's five scenarios step by step
 * (bsv-stack-lean 76004514, corpus/scenarios/2026-11-01-wallet-browser-gate-*.md),
 * their origins `app.example`, `new.example` and `evil.example` read as the
 * pages `http://app.example` and so on.
 */
const fs = require('fs');
const path = require('path');
const { spawn } = require('child_process');
const { startWallet, teardownWallet, BSV } = require('./setup');
const { WalletClient, E2E_ORIGIN } = require('./wallet-client');
const record = require('./record');

const D_PORT = parseInt(process.env.WALLET_D_PORT || '3374');
const APP = 'http://app.example';
const NEW = 'http://new.example';
const EVIL = 'http://evil.example';
const TOKEN = 'e2e-gate-bearer';

/** The client's two refusals (bsv-desktop bcb6003 src/onWalletReady.ts:82-88; ts-stack edf6e03 WalletPermissionsManager.ts:1348). */
const NO_ORIGIN = 'Origin header is required';
const DENIED = 'The user has denied the request for permission.';

const sleep = (ms) => new Promise(r => setTimeout(r, ms));

/** One raw call: the scene names every header itself; recorded as the client's are. */
async function call(wallet, method, endpoint, opts = {}) {
  return record.serial(() => callNow(wallet, method, endpoint, opts));
}

async function callNow(wallet, method, endpoint, { origin, originator, bearer, body } = {}) {
  const done = record.httpStart(wallet.name, method, `/${endpoint}`, body);
  const headers = {};
  if (origin) headers.Origin = origin;
  if (originator) headers.Originator = originator;
  if (bearer) headers.Authorization = `Bearer ${bearer}`;
  if (body !== undefined) headers['Content-Type'] = 'application/json';
  const resp = await fetch(`http://localhost:${wallet.port}/${endpoint}`, {
    method, headers, body: body === undefined ? undefined : JSON.stringify(body),
  });
  const text = await resp.text();
  done(resp.status, text);
  let json = {};
  try { json = text ? JSON.parse(text) : {}; } catch { json = { _raw: text }; }
  return { status: resp.status, body: json, allowOrigin: resp.headers.get('access-control-allow-origin') };
}

/** Start the scene's wallet with `allowed` listed (the harness's origin too). */
async function startGated(name, allowed) {
  const wallet = await startWallet(D_PORT, name, { env: { ALLOWED_ORIGINS: [E2E_ORIGIN, ...allowed].join(',') } });
  wallet.name = name;
  return wallet;
}

/**
 * Restart the scene's wallet with `allowed` listed and, when `token` is
 * given, `AUTH_TOKEN` set: the operator's edit of the wallet's `.env` and
 * its restart. Resolves once the wallet answers the harness's origin.
 */
async function restartGated(wallet, allowed, { token } = {}) {
  const envPath = path.join(wallet.dir, '.env');
  const kept = fs.readFileSync(envPath, 'utf-8').split('\n')
    .filter(l => l && !l.startsWith('ALLOWED_ORIGINS=') && !l.startsWith('AUTH_TOKEN='));
  kept.push(`ALLOWED_ORIGINS='${[E2E_ORIGIN, ...allowed].join(',')}'`);
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

/** The audit's count: what a call that reached a door recorded (`/permission-audit`). */
async function audited(wallet, bearer) {
  const r = await call(wallet, 'GET', 'permission-audit', { origin: E2E_ORIGIN, bearer });
  if (r.status !== 200) throw new Error(`/permission-audit answered ${r.status}: ${JSON.stringify(r.body)}`);
  return r.body.count;
}

/** The arguments of the scenes' calls. */
const ARGS = {
  createSignature: { protocolID: [2, 'browser gate'], keyID: '1', counterparty: 'self', data: [1, 2, 3] },
  listOutputs: { basket: 'default', limit: 10 },
  getPublicKey: { protocolID: [2, 'browser gate'], keyID: '1', counterparty: 'self' },
  createAction: {
    description: 'browser gate scene',
    outputs: [{ lockingScript: '6a', satoshis: 0, outputDescription: 'a refused call' }],
    options: { noSend: true },
  },
};

/** A scene's run: its wallet always torn down, kept when the scene failed. */
async function scene(name, allowed, body) {
  const wallet = await startGated(name, allowed);
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
  APP, NEW, EVIL, TOKEN, NO_ORIGIN, DENIED, ARGS, D_PORT,
  call, restartGated, audited, scene, WalletClient,
};
