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
  'ARCADE', 'BREAK_GLASS_EXPLORER_HEADERS', 'PUBLIC_CALLBACK_URL',
];

function applyEnv() {
  for (const k of DROPPED) delete process.env[k];
  const forced = {
    CHAINTRACKS_URL: STUB_BASES.chaintracks,
    // Arcade is the wallet's default broadcaster (#300); the stub plays
    // classic ARC only until #299 gives it an Arcade face, so the run names
    // the deprecated classic mode explicitly, never by dropping the mode.
    ARC_MODE: 'classic',
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
    // Node's own fetch and http obey the proxy environment only under this
    // (Node 24, 22.21); read at a process's start, so it binds the children
    // here and the harness itself only when its launcher set it (CI does).
    NODE_USE_ENV_PROXY: '1',
    E2E_RNG_SEED: '42'.repeat(32),
  };
  Object.assign(process.env, forced);
  // The funder's seat and the wallets' scratch root, unless the caller named them.
  process.env.FUNDER_PORT = process.env.FUNDER_PORT || String(HERMETIC_FUNDER_PORT);
  process.env.E2E_WALLET_TMP = process.env.E2E_WALLET_TMP || os.tmpdir();
  if (process.env.FUNDER_PORT === '3320') {
    throw new Error('E2E_HERMETIC=1 never uses the funder on :3320; unset FUNDER_PORT or name another port');
  }
}

/**
 * The harness's own guard: under the flag, a fetch from this process to
 * anything but the loopback is refused before it is made and counted, so
 * the harness is walled on a Node that does not read NODE_USE_ENV_PROXY.
 */
const harnessLeaks = {};
function guardFetch() {
  const real = globalThis.fetch;
  globalThis.fetch = (input, init) => {
    const url = new URL(typeof input === 'string' || input instanceof URL ? input : input.url);
    if (!['127.0.0.1', 'localhost', '[::1]'].includes(url.hostname)) {
      const key = `harness fetch ${url.origin}`;
      harnessLeaks[key] = (harnessLeaks[key] || 0) + 1;
      return Promise.reject(new Error(`hermetic: the harness asked ${url.origin}, outside the stub`));
    }
    return real(input, init);
  };
}

if (enabled) { applyEnv(); guardFetch(); }

/**
 * The walled targets the run tolerates, and only these: the toolbox's own
 * WhatsOnChain and Bitails providers, whose bases `ServicesOptions` does not
 * take (toolbox #98). Their rungs in the toolbox's ladders reach the wall and
 * the ladder moves on to the stub's BananaBlocks (docs/E2E-HERMETIC.md names
 * the ladders). A walled attempt at any other target, or any harness fetch
 * that left the loopback, is a new leak and fails the run. The tolerance is
 * by host, not by caller, and temporary until toolbox #98: the CLI's own
 * sites reach these hosts only through `ExplorerBases`, which the stub
 * moves, and tests/rule_28_sites.rs bans the toolbox's base constants and
 * providers anywhere else in the CLI.
 */
const KNOWN_WALLED = {
  'CONNECT api.whatsonchain.com:443': "the toolbox's WhatsOnChain provider (no base knob, toolbox #98)",
  'CONNECT api.bitails.io:443': "the toolbox's Bitails provider (no base knob, toolbox #98)",
};

/** The stub's walled attempts and the harness's own, split known and new. */
function classifyWalled(stats) {
  const known = {};
  const unknown = {};
  for (const [target, n] of Object.entries(stats.leaks || {})) {
    (KNOWN_WALLED[target] ? known : unknown)[target] = n;
  }
  for (const [target, n] of Object.entries(harnessLeaks)) unknown[target] = n;
  const sum = (o) => Object.values(o).reduce((a, b) => a + b, 0);
  return { known, unknown, knownCount: sum(known), unknownCount: sum(unknown), unserved: stats.unserved || {}, unservedCount: sum(stats.unserved || {}) };
}

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
  // An early exit of the harness (a refused funding, a failed final balance
  // read) still stops the stub.
  process.on('exit', () => { if (proc.exitCode === null) proc.kill('SIGTERM'); });
  return new StubHandle(proc, logPath);
}

/** Stop a hermetic wallet's `serve` when the harness exits, however it exits. */
function stopOnExit(wallet) {
  process.on('exit', () => { if (wallet.proc && wallet.proc.exitCode === null) wallet.proc.kill('SIGTERM'); });
  return wallet;
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
  forge(script, satoshis) { return this.call('POST', '/forge', { script, satoshis }); }
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

/**
 * The negative of `seedFunder`: a coinbase whose BUMP's root is not the
 * stub's header's for that height. The funder must refuse it; a wallet that
 * took it would show the seed's root check never ran. Throws unless refused.
 */
async function seedForgedRefused(stub, funder, satoshis) {
  const { ANYONE_KEY, buildP2PKH } = require('./wallet-client');
  const key = await funder.client.getPublicKey([2, '3241645161d8'], 'SfKxPIJNgdI= NaGLC6fMH50=', ANYONE_KEY, true);
  const forged = await stub.forge(buildP2PKH(key.publicKey), satoshis);
  const before = await funder.client.balance();
  let refusal = null;
  try {
    await funder.client.internalizeAction(
      Array.from(Buffer.from(forged.beef, 'hex')),
      [{
        outputIndex: 0,
        protocol: 'wallet payment',
        paymentRemittance: { derivationPrefix: 'SfKxPIJNgdI=', derivationSuffix: 'NaGLC6fMH50=', senderIdentityKey: ANYONE_KEY },
      }],
      `hermetic forged funding ${satoshis} sats (root ${forged.root.slice(0, 16)}... at ${forged.height})`,
    );
  } catch (e) {
    refusal = e.message;
  }
  if (refusal === null) throw new Error(`the funder took a forged coinbase (root ${forged.root} is not the header's ${forged.headerRoot} at ${forged.height}): the seed's root check did not run`);
  // The root check's own refusal at either side of the beta.1 pin. The
  // streaming verifier reports its typed byte rule; the base names the root.
  const rootRefusals = [`Invalid merkle root ${forged.root} at height ${forged.height}`,
    'Invalid BEEF at byte 41: RootNotCarried'];
  const rootRefusal = rootRefusals.join(' or ');
  if (!rootRefusals.some(word => refusal.includes(word))) throw new Error(`the funder refused the forged coinbase, but not by its root check (wanted "${rootRefusal}"): ${refusal}`);
  const after = await funder.client.balance();
  if (after !== before) throw new Error(`the funder refused the forged coinbase but its balance moved ${before} -> ${after}`);
  return { ...forged, refusal };
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
    for (const [target, n] of leaks) lines.push(`    ${target} x${n}${KNOWN_WALLED[target] ? `: known, ${KNOWN_WALLED[target]}` : ': NEW LEAK'}`);
  } else {
    lines.push('  walled: none (nothing tried to leave the machine)');
  }
  const unserved = Object.entries(stats.unserved || {});
  if (unserved.length) {
    lines.push(`  UNSERVED (${unserved.reduce((s, [, n]) => s + n, 0)} requests for routes the stub does not play, each answered 501):`);
    for (const [route, n] of unserved) lines.push(`    ${route} x${n}`);
  } else {
    lines.push('  unserved: none');
  }
  if (stats.faulted) lines.push(`  injected faults served: ${stats.faulted}`);
  return lines.join('\n');
}

module.exports = { enabled, STUB_PORT, bases: STUB_BASES, HERMETIC_FUNDER_PORT, KNOWN_WALLED, startStub, stopOnExit, seedFunder, seedForgedRefused, classifyWalled, describeStats };
