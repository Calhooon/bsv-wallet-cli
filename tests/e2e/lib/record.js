/**
 * The differential recording (bsv-wallet-cli #70): `DIFF_RECORD=<file>`.
 *
 * Under the variable the harness writes one JSON line per event to the file:
 * every HTTP call a `WalletClient` makes (its seat, method, path, request
 * body, status, response body) and every CLI command a scenario runs through
 * `execSync` (its seat, argv, exit code, stdout, stderr), each with the phase
 * it ran in (`setup`, a scenario's name, `sweep`) and a sequence number taken
 * when the call starts, so concurrent calls keep their order of issue. The
 * wallets' directories and ports are written once, as `wallet` events, so
 * tools/differential can name them by seat. Unset, nothing is recorded and
 * nothing is wrapped.
 *
 * `DIFF_ROOT_KEYS=<A>,<B>,<C>[,<funder>]` (hex) gives the fresh wallets the
 * same keys in two runs; setup.js passes a seat's key to `init --key`. It is
 * refused outside `E2E_HERMETIC=1`: a known key must never hold mainnet sats.
 * The recording never holds a key: every line written, the argv included
 * (`--key <root-key:A>`), has each key replaced by its seat's name, and so
 * has the error a failed command throws.
 *
 * Require this module before ./setup and the scenarios: `install()` replaces
 * `child_process.execSync`, which they take by destructuring at load.
 */

const childProcess = require('child_process');
const fs = require('fs');

const FILE = process.env.DIFF_RECORD || '';
const enabled = FILE !== '';

/** A wallet's seat from its harness name (`wallet-A`, `funder`). */
function seatOf(name) {
  const m = /^wallet-([A-Z])$/.exec(name || '');
  if (m) return m[1];
  return name || 'unknown';
}

const SEATS = ['A', 'B', 'C', 'funder'];

/** The seat's key from DIFF_ROOT_KEYS, or null when the variable is unset. */
function rootKeyFor(name) {
  const raw = process.env.DIFF_ROOT_KEYS;
  if (raw === undefined || raw === '') return null;
  if (process.env.E2E_HERMETIC !== '1') {
    throw new Error('DIFF_ROOT_KEYS is set outside E2E_HERMETIC=1: a known key never holds mainnet sats; unset it or run hermetic');
  }
  const keys = raw.split(',').map(k => k.trim());
  if (keys.length < 3 || keys.length > SEATS.length || !keys.every(k => /^[0-9a-f]{64}$/.test(k))) {
    throw new Error('DIFF_ROOT_KEYS must be three or four 64-digit lowercase hex keys, comma separated (seats A, B, C, funder)');
  }
  const i = SEATS.indexOf(seatOf(name));
  return i >= 0 && i < keys.length ? keys[i] : null;
}

// Refused at load, before any wallet starts, as well as at each seat's init.
if (process.env.DIFF_ROOT_KEYS) rootKeyFor('wallet-A');

// Each key of DIFF_ROOT_KEYS and its seat's name, `<root-key:A>`.
const KEY_NAMES = (process.env.DIFF_ROOT_KEYS || '').split(',').map(k => k.trim())
  .map((k, i) => [k, `<root-key:${SEATS[i]}>`]).filter(([k]) => k !== '');

/** The text with every root key replaced by its seat's name. */
function redact(text) {
  for (const [key, name] of KEY_NAMES) text = text.split(key).join(name);
  return text;
}

let phase = 'setup';
let seq = 0;
const dirs = new Map(); // wallet directory -> seat

function setPhase(name) { phase = name; }

function write(event) {
  if (!enabled) return;
  fs.appendFileSync(FILE, redact(JSON.stringify(event)) + '\n');
}

/** A wallet directory setup.js made, so its commands are named by seat. */
function walletDir(name, dir) {
  if (enabled) dirs.set(dir, seatOf(name));
}

/** A wallet started by setup.js: its directory, port and identity, by seat. */
function wallet(name, { dir, port, identityKey }) {
  if (!enabled) return;
  write({ kind: 'wallet', seq: seq++, phase, seat: seatOf(name), dir, port, identityKey });
}

/**
 * Under DIFF_RECORD the harness's calls run one at a time, in the order the
 * scenarios issue them: `serial(fn)` waits for the call before it. Calls a
 * scenario fires together (scenario 10's five sends) otherwise reach the
 * wallet's spending lock in the scheduler's order, and every coin graph after
 * them inherits that order, so two runs of one binary would differ for no
 * reason in the binary. The race itself is the plain run's to test.
 */
let queue = Promise.resolve();
function serial(fn) {
  if (!enabled) return fn();
  const run = queue.then(fn, fn);
  queue = run.catch(() => {});
  return run;
}

/**
 * Take a sequence number at the call's start; the returned function writes
 * the call when it ends. An answer without a response (refused connection,
 * timeout) is written with `status: null` and the error's message.
 */
function httpStart(name, method, path, body) {
  if (!enabled) return () => {};
  const event = { kind: 'http', seq: seq++, phase, seat: seatOf(name), method, path, request: body === undefined ? null : body };
  return (status, text, error) => {
    let response = text;
    try { response = text ? JSON.parse(text) : null; } catch { /* kept as text */ }
    write({ ...event, status, response, ...(error ? { error: error.message || String(error) } : {}) });
  };
}

/** The seat of a shell command `cd "<dir>" && ...`, by the directories seen. */
function seatOfCommand(cmd, cwd) {
  for (const [dir, seat] of dirs) {
    if ((cwd && cwd === dir) || cmd.includes(`"${dir}"`)) return seat;
  }
  return 'none';
}

/**
 * execSync with the command recorded. It runs the command as execSync does
 * (a shell, the caller's cwd, env, input and timeout) through spawnSync so
 * stderr is read as well as stdout; stderr still reaches the parent's stderr
 * when the caller did not pipe it, and a failure throws the error execSync
 * throws (status, signal, stdout, stderr), so a scenario sees no difference.
 * Commands that do not run the wallet binary pass straight through.
 */
function install(bin) {
  if (!enabled) return;
  const real = childProcess.execSync;
  childProcess.execSync = function recordedExecSync(cmd, opts = {}) {
    if (typeof cmd !== 'string' || !cmd.includes(bin)) return real.call(this, cmd, opts);
    const event = { kind: 'cli', seq: seq++, phase, seat: seatOfCommand(cmd, opts.cwd), argv: argvOf(redact(cmd), bin) };
    const stdio = normStdio(opts.stdio);
    const r = childProcess.spawnSync(cmd, {
      cwd: opts.cwd, env: opts.env, input: opts.input, timeout: opts.timeout,
      killSignal: opts.killSignal, maxBuffer: opts.maxBuffer ?? 1024 * 1024 * 1024,
      shell: opts.shell || true, windowsHide: opts.windowsHide,
      stdio: [stdio[0], 'pipe', 'pipe'],
    });
    const out = r.stdout || Buffer.alloc(0);
    const err = r.stderr || Buffer.alloc(0);
    write({ ...event, exit: r.status, signal: r.signal, stdout: out.toString('utf-8'), stderr: err.toString('utf-8'), ...(r.error ? { error: r.error.message } : {}) });
    if (stdio[2] === 'inherit') process.stderr.write(err);
    if (stdio[1] === 'inherit') process.stdout.write(out);
    const enc = opts.encoding && opts.encoding !== 'buffer' ? opts.encoding : null;
    const as = (b) => (enc ? b.toString(enc) : b);
    if (r.error) {
      r.error.message = redact(r.error.message);
      r.error.stdout = as(out); r.error.stderr = as(err);
      throw r.error;
    }
    if (r.status !== 0) {
      const e = new Error(redact(`Command failed: ${cmd}\n${err.toString('utf-8')}`));
      Object.assign(e, { status: r.status, signal: r.signal, pid: r.pid, stdout: as(out), stderr: as(err), output: [null, as(out), as(err)] });
      throw e;
    }
    return stdio[1] === 'inherit' ? null : as(out);
  };
}

/** execSync's stdio: unset pipes stdout and sends stderr to the parent's. */
function normStdio(stdio) {
  if (stdio === undefined) return ['pipe', 'pipe', 'inherit'];
  if (typeof stdio === 'string') return [stdio, stdio, stdio];
  return [stdio[0] ?? 'pipe', stdio[1] ?? 'pipe', stdio[2] ?? 'pipe'];
}

/**
 * The wallet binary's arguments from a `cd "<dir>" && "<bin>" args...`
 * command: the words after the binary, split as the shell splits them for
 * the double-quoted and plain words the scenarios write. The caller passes
 * the command redacted, so `init --key` is written `--key <root-key:SEAT>`.
 */
function argvOf(cmd, bin) {
  const at = cmd.indexOf(bin);
  let rest = cmd.slice(at + bin.length);
  if (rest.startsWith('"')) rest = rest.slice(1);
  const words = [];
  const re = /"((?:[^"\\]|\\.)*)"|'([^']*)'|(\S+)/g;
  let m;
  while ((m = re.exec(rest)) !== null) {
    words.push(m[1] !== undefined ? m[1].replace(/\\(.)/g, '$1') : m[2] !== undefined ? m[2] : m[3]);
  }
  return words;
}

module.exports = { enabled, install, setPhase, walletDir, wallet, serial, httpStart, rootKeyFor, seatOf, redact };
