/**
 * The hermetic run (bsv-wallet-cli #74): `E2E_HERMETIC=1`.
 *
 * Nothing leaves the machine. The harness starts the stub
 * (`tests/e2e/stub/server.js`: the header service, the three explorers and
 * a broadcaster over one in-memory chain), points every base URL the binary
 * reads at it, and walls the rest: `HTTPS_PROXY`/`HTTP_PROXY` name the stub,
 * which refuses every CONNECT and counts it under `leaks`, so a request bound
 * for a real host is a 403 the binary sees as a fault and the run sees in
 * its stats, never a packet on the wire. The funder is a third stub-backed
 * wallet this module starts and seeds from a coinbase the stub mines; the
 * funder on :3320 and the registry's ports are never touched.
 *
 * The environment is applied when this module is required, so every child
 * the harness spawns (`serve`, the scenarios' CLI commands, the MCP server)
 * inherits it. Require it before `./setup`, which reads FUNDER_PORT at load.
 */

const { spawn } = require('child_process');
const fs = require('fs');
const os = require('os');
const path = require('path');
const { bases } = require('../stub/server');

const enabled = process.env.E2E_HERMETIC === '1';
const STUB_PORT = parseInt(process.env.E2E_STUB_PORT || '3380');
const STUB_BASES = bases(STUB_PORT);
/** The hermetic funder's seat: never 3320, never a registry port. */
const HERMETIC_FUNDER_PORT = 3373;

/** The keys that would reach a real service, dropped under the flag. */
const DROPPED = [
  'WOC_API_KEY', 'BITAILS_API_KEY', 'TAAL_API_KEY', 'MAIN_TAAL_API_KEY',
  'ARC_MODE', 'ARCADE', 'BREAK_GLASS_EXPLORER_HEADERS', 'PUBLIC_CALLBACK_URL',
];

function applyEnv() {
  for (const k of DROPPED) delete process.env[k];
  const forced = {
    CHAINTRACKS_URL: STUB_BASES.chaintracks,
    ARC_URL: STUB_BASES.arc,
    ARC_GORILLAPOOL_URL: '',
    BANANABLOCKS_BASE: STUB_BASES.bananablocks,
    WOC_BASE: STUB_BASES.woc,
    BITAILS_BASE: STUB_BASES.bitails,
    // The wall: every scheme through the stub, the stub itself direct.
    HTTPS_PROXY: STUB_BASES.root, https_proxy: STUB_BASES.root,
    HTTP_PROXY: STUB_BASES.root, http_proxy: STUB_BASES.root,
    ALL_PROXY: STUB_BASES.root, all_proxy: STUB_BASES.root,
    NO_PROXY: '127.0.0.1,localhost', no_proxy: '127.0.0.1,localhost',
  };
  Object.assign(process.env, forced);
  // The funder's seat and the wallets' scratch root, unless the caller named them.
  process.env.FUNDER_PORT = process.env.FUNDER_PORT || String(HERMETIC_FUNDER_PORT);
  process.env.E2E_WALLET_TMP = process.env.E2E_WALLET_TMP || os.tmpdir();
  if (process.env.FUNDER_PORT === '3320') {
    throw new Error('E2E_HERMETIC=1 never uses the funder on :3320; unset FUNDER_PORT or name another port');
  }
}

if (enabled) applyEnv();

/** Start the stub process; resolves once it answers. */
async function startStub() {
  const script = path.join(__dirname, '..', 'stub', 'server.js');
  const logPath = path.join(process.env.E2E_WALLET_TMP || os.tmpdir(), `bsv-wallet-stub-${STUB_PORT}.log`);
  const taken = await fetch(`${STUB_BASES.stub}/stats`).then(r => r.ok).catch(() => false);
  if (taken) throw new Error(`a stub (or something) already answers on :${STUB_PORT}; stop it or set E2E_STUB_PORT`);
  const logFd = fs.openSync(logPath, 'w');
  // The stub itself must never be proxied by its own wall; it makes no requests, but its env is clean anyway.
  const env = { ...process.env };
  for (const k of Object.keys(env)) if (/_proxy$/i.test(k)) delete env[k];
  const proc = spawn(process.execPath, [script, '--port', String(STUB_PORT)], { stdio: ['ignore', logFd, logFd], env });
  fs.closeSync(logFd);
  let up = false;
  for (let i = 0; i < 40 && !up; i++) {
    if (proc.exitCode !== null) break;
    up = await fetch(`${STUB_BASES.stub}/stats`).then(r => r.ok).catch(() => false);
    if (!up) await new Promise(r => setTimeout(r, 250));
  }
  if (!up) throw new Error(`the stub did not answer on :${STUB_PORT} (its log: ${logPath})`);
  return new StubHandle(proc, logPath);
}

class StubHandle {
  constructor(proc, logPath) { this.proc = proc; this.logPath = logPath; this.bases = STUB_BASES; }

  async call(method, route, body) {
    const resp = await fetch(`${STUB_BASES.stub}${route}`, {
      method,
      headers: body === undefined ? {} : { 'Content-Type': 'application/json' },
      body: body === undefined ? undefined : JSON.stringify(body),
    });
    const text = await resp.text();
    let data; try { data = JSON.parse(text); } catch { data = text; }
    if (!resp.ok) throw new Error(`stub ${method} ${route} ${resp.status}: ${typeof data === 'string' ? data : JSON.stringify(data)}`);
    return data;
  }

  stats() { return this.call('GET', '/stats'); }
  mine(txids) { return this.call('POST', '/mine', txids ? { txids } : {}); }
  fund(script, satoshis) { return this.call('POST', '/fund', { script, satoshis }); }
  fault(provider, status, { retryAfter, count = 1, body } = {}) { return this.call('POST', '/fault', { provider, status, retryAfter, count, body }); }
  clearFaults() { return this.call('DELETE', '/fault'); }

  stop() {
    if (this.proc && this.proc.exitCode === null) this.proc.kill('SIGTERM');
  }
}

/**
 * Seed the hermetic funder: the stub mines a coinbase paying the funder's
 * BRC-29 address (the same derivation every transfer of this harness uses),
 * and the funder internalizes its AtomicBEEF. The BEEF's root is checked by
 * the wallet against the stub's header; the proof itself is held back by the
 * toolbox's proof gate (closed under `serve`, no monitor), so the coin is a
 * received unproven coin, spent under `allowPending: received` as every coin
 * of the mainnet run is.
 */
async function seedFunder(stub, funder, satoshis) {
  const { ANYONE_KEY, buildP2PKH } = require('./wallet-client');
  const key = await funder.client.getPublicKey([2, '3241645161d8'], 'SfKxPIJNgdI= NaGLC6fMH50=', ANYONE_KEY, true);
  const script = buildP2PKH(key.publicKey);
  const coin = await stub.fund(script, satoshis);
  await funder.client.internalizeAction(
    Array.from(Buffer.from(coin.beef, 'hex')),
    [{
      outputIndex: 0,
      protocol: 'wallet payment',
      paymentRemittance: { derivationPrefix: 'SfKxPIJNgdI=', derivationSuffix: 'NaGLC6fMH50=', senderIdentityKey: ANYONE_KEY },
    }],
    `hermetic funding ${satoshis} sats (coinbase at ${coin.height})`,
  );
  return coin;
}

/** One line per provider: route counts; and the walled attempts. */
function describeStats(stats) {
  const lines = [];
  for (const [provider, routes] of Object.entries(stats.calls || {})) {
    const total = Object.values(routes).reduce((s, n) => s + n, 0);
    const detail = Object.entries(routes).map(([r, n]) => `${r} x${n}`).join(', ');
    lines.push(`  ${provider.padEnd(13)} ${String(total).padStart(4)}  ${detail}`);
  }
  const leaks = Object.entries(stats.leaks || {});
  if (leaks.length) {
    lines.push(`  WALLED (${leaks.reduce((s, [, n]) => s + n, 0)} attempts to leave the machine, each refused 403):`);
    for (const [target, n] of leaks) lines.push(`    ${target} x${n}`);
  } else {
    lines.push('  walled: none (nothing tried to leave the machine)');
  }
  if (stats.faulted) lines.push(`  injected faults served: ${stats.faulted}`);
  return lines.join('\n');
}

module.exports = { enabled, STUB_PORT, bases: STUB_BASES, HERMETIC_FUNDER_PORT, startStub, seedFunder, describeStats };
