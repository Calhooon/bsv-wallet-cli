/**
 * E2E.24: an external sqlite3 writer on the served WAL store (#204).
 * The open reader pool prevents an EXCLUSIVE locking-mode connection from
 * taking the file. A normal WAL writer blocks writes, not reads. The read
 * doors' 503s remain covered by the unit/wire tests' unix-dotfile VFS.
 * https://www.sqlite.org/wal.html#use_of_wal_without_shared_memory
 */
const { spawn, execFileSync } = require('child_process');
const path = require('path');
const { FUNDER_PORT } = require('../lib/setup');
const { WalletClient, ANYONE_KEY, buildP2PKH } = require('../lib/wallet-client');
const hermetic = require('../lib/hermetic');

const BUSY_MS = 5_000; // toolbox 0.13.0: STORE_BUSY_CEILING_MS

async function within(promise, ms, message) {
  let timer;
  try {
    return await Promise.race([promise, new Promise((_, reject) => {
      timer = setTimeout(() => reject(new Error(message)), ms);
    })]);
  } finally { clearTimeout(timer); }
}

async function holdWriter(db, assert) {
  // Prove why the requested all-reader lock cannot be taken by a second
  // process on this served store. Do not quietly substitute another lock.
  let exclusive;
  try {
    execFileSync('sqlite3', ['-batch', '-bail', db,
      'PRAGMA busy_timeout=1000; PRAGMA journal_mode; PRAGMA locking_mode=EXCLUSIVE; BEGIN IMMEDIATE; CREATE TABLE IF NOT EXISTS e2e_busy(x); INSERT INTO e2e_busy VALUES(1); ROLLBACK;'],
    { encoding: 'utf8', timeout: BUSY_MS, stdio: 'pipe' });
  } catch (e) { exclusive = e; }
  assert(exclusive && exclusive.status > 0 && /\(5\)/.test(String(exclusive.stderr))
    && /\bwal\b/.test(String(exclusive.stdout)),
  `sqlite3 EXCLUSIVE attempt must be SQLITE_BUSY on WAL: exit ${exclusive?.status}, ${exclusive?.stdout}, ${exclusive?.stderr}`);
  console.log('       EXCLUSIVE locking_mode refused by the open WAL connections (SQLITE_BUSY); read doors must stay 200 under the normal writer');

  const proc = spawn('sqlite3', ['-batch', '-bail', db], { stdio: ['pipe', 'pipe', 'pipe'] });
  let output = '', errors = '', heldAt;
  let ready;
  const acquired = new Promise(resolve => { ready = resolve; });
  proc.stdout.on('data', chunk => {
    output += chunk;
    if (output.includes('E2E_BUSY_HELD')) { heldAt ??= Date.now(); ready(); }
  });
  proc.stderr.on('data', chunk => { errors += chunk; });
  const exited = new Promise(resolve => {
    proc.once('error', e => { errors += e.message; });
    proc.once('close', (code, signal) => resolve({ code, signal }));
  });
  const stop = () => proc.kill('SIGKILL');
  process.once('exit', stop);
  const release = async () => {
    try {
      proc.stdin.end('ROLLBACK;\n');
      const result = await within(exited, BUSY_MS, `sqlite3 pid ${proc.pid}: lock outlived ROLLBACK and stdin close`);
      assert(result.code === 0, `sqlite3 holder exits 0, got ${JSON.stringify(result)}: ${errors}`);
      if (heldAt !== undefined) console.log(`       sqlite3 pid ${proc.pid}: ROLLBACK, stdin closed, exit 0 after holding ${Date.now() - heldAt}ms`);
    } finally {
      if (proc.exitCode === null) { stop(); await exited; }
      process.removeListener('exit', stop);
    }
  };
  // No wallet row changes: the table and its one row are rolled back too.
  proc.stdin.on('error', () => {}); // an early exit is reported by exited
  proc.stdin.write('PRAGMA busy_timeout=1000;\nPRAGMA locking_mode=NORMAL;\nBEGIN IMMEDIATE;\nCREATE TABLE IF NOT EXISTS e2e_busy(x);\nINSERT INTO e2e_busy VALUES(1);\n.print E2E_BUSY_HELD\n');
  try {
    await within(Promise.race([acquired, exited.then(result => {
      throw new Error(`sqlite3 holder exited before its write: ${JSON.stringify(result)} ${errors}`);
    })]), BUSY_MS, 'sqlite3 holder did not acquire BEGIN IMMEDIATE and write e2e_busy');
  } catch (e) { await release(); throw e; }
  console.log(`       sqlite3 pid ${proc.pid} holds ${db}: WAL/NORMAL BEGIN IMMEDIATE + e2e_busy insert since ${new Date().toISOString()}`);
  return { release };
}

module.exports = {
  name: 'busy-store',
  description: 'External WAL writer: busy write doors retry, read doors stay available; release restores all five (0 sats)',
  hermetic: true,

  async run(walletA, walletB, assert) {
    try { execFileSync('sqlite3', ['--version'], { stdio: 'pipe', timeout: BUSY_MS }); }
    catch (e) { throw new Error(`busy-store requires the sqlite3 CLI on PATH: ${e.message}`); }
    const client = walletA.client;
    const before = await client.balance();
    const coins = await client.listOutputs('default', { limit: 1 });
    const txid = coins.outputs[0]?.outpoint.split('.')[0];
    assert(/^[0-9a-f]{64}$/.test(txid), 'A has a transaction for the host route');
    if (hermetic.enabled) {
      const mined = await fetch(`${hermetic.bases.stub}/mine`, {
        method: 'POST', headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ txids: [txid] }),
      });
      assert(mined.ok, 'the stub mines the existing transaction, without a new payment');
    }
    await client.get('chain-feed');

    const key = await client.getPublicKey([2, '3241645161d8'], 'SfKxPIJNgdI= NaGLC6fMH50=', ANYONE_KEY, true);
    const outputs = [{ lockingScript: buildP2PKH(key.publicKey), satoshis: 1_000, outputDescription: 'E2E.24 unsigned self output' }];
    // An ordinary template reserves A's sole funding coin in the standalone
    // run, making the next createAction 402 before it reaches SQLite. Supply
    // an external input for the kept template instead: the funder's existing
    // change, read with its BEEF, never signed or spent by this scenario.
    const funder = new WalletClient(FUNDER_PORT, 'funder');
    const source = await funder.listOutputs('default', { include: 'entire transactions', limit: 10_000 });
    const external = source.outputs.find(o => o.satoshis >= 2_000);
    assert(external && source.beef, 'the funder has an external input and its BEEF for the unsigned template');
    const pending = new Set();
    const db = path.join(walletA.dir, 'wallet.db');
    let inserted = false;
    let holder, failure;
    const create = () => client.createAction(outputs, 'E2E.24 busy write', { signAndProcess: false });
    const ask = async (name, call) => {
      const start = Date.now();
      let answer;
      try {
        const body = await within(call(), BUSY_MS * 2, `${name}: request exceeded two busy ceilings`);
        answer = { status: client.lastStatus, body, retryAfter: null };
        if (body.signableTransaction?.reference) pending.add(body.signableTransaction.reference);
      } catch (e) {
        answer = { status: e.status, body: e.body, retryAfter: e.retryAfter, error: e.message };
      }
      answer.ms = Date.now() - start;
      console.log(`       ${name}: ${answer.status ?? 'no response'} ${answer.body?.code || ''} Retry-After=${answer.retryAfter ?? 'absent'} ${answer.ms}ms (busy ceiling ${BUSY_MS}ms)`);
      return answer;
    };
    try {
      // The host route is a view of locally tracked transactions. Give it
      // an unknown row for this real txid if none exists, without running
      // a tracker pass (which could reannounce in a live run). Remove only
      // our row afterward. No proof or mined word is fabricated.
      inserted = execFileSync('sqlite3', ['-batch', '-bail', db,
        `INSERT OR IGNORE INTO tracker_states (txid, built, word) VALUES ('${txid}', 0, 'unknown'); SELECT changes();`],
      { encoding: 'utf8', timeout: BUSY_MS }).trim() === '1';
      await client.get(`tx/${txid}`);
      const kept = await client.post('createAction', {
        outputs, description: 'E2E.24 kept unsigned template',
        inputs: [{ outpoint: external.outpoint, unlockingScriptLength: 108, inputDescription: 'External input, never signed' }],
        inputBEEF: Array.from(Buffer.from(source.beef, 'hex')),
        options: { signAndProcess: false, randomizeOutputs: false },
      });
      const reference = kept.signableTransaction?.reference;
      assert(reference, 'createAction returns the kept unsigned reference');
      pending.add(reference);
      assert(await client.balance() === before, 'the kept external-input template leaves A funds available');
      const doors = [
        ['listOutputs', () => client.listOutputs(), 200],
        ['createAction', create, 503],
        ['abortAction', () => client.abortAction(reference), 503],
        ['GET /chain-feed', () => client.get('chain-feed'), 200],
        ['GET /tx', () => client.get(`tx/${txid}`), 200],
      ];
      holder = await holdWriter(db, assert);
      try {
        const answers = [];
        for (const [name, call] of doors) {
          const answer = await ask(`held ${name}`, call);
          if (name === 'abortAction' && answer.body?.aborted) pending.delete(reference);
          answers.push(answer);
        }
        // Observe every door before asserting, so the premature-release
        // negative prints all five successes and still fails this scenario.
        for (let i = 0; i < doors.length; i++) {
          const [name, , expected] = doors[i];
          const answer = answers[i];
          assert(answer.status === expected, `held ${name}: expected ${expected}, got ${answer.status}: ${answer.error || ''}`);
          if (expected === 503) {
            assert(answer.body?.code === 'STORE_BUSY', `held ${name}: STORE_BUSY, got ${answer.body?.code}`);
            assert(answer.retryAfter === '1', `held ${name}: Retry-After: 1, got ${answer.retryAfter}`);
            assert(answer.ms >= BUSY_MS - 1_000 && answer.ms < BUSY_MS * 2,
              `held ${name}: about one ${BUSY_MS}ms busy ceiling, got ${answer.ms}ms`);
          }
        }
      } finally {
        await holder.release();
        holder = null;
        const start = Date.now();
        await within(client.listOutputs(), BUSY_MS, 'sqlite3 e2e_busy lock outlived the scenario: release probe exceeded 5000ms');
        assert(client.lastStatus === 200, 'release probe is 200: no sqlite3 e2e_busy lock outlived the holder');
        console.log(`       sqlite3 released and exited 0; listOutputs probe 200 in ${Date.now() - start}ms`);
      }
      for (const [name, call] of doors) {
        const answer = await ask(`released ${name}`, call);
        assert(answer.status === 200, `released ${name}: expected 200, got ${answer.status}: ${answer.error || ''}`);
        if (name === 'createAction') {
          assert(answer.body.signableTransaction?.reference, 'released createAction builds a new unsigned template');
          const fresh = answer.body.signableTransaction.reference;
          assert((await client.abortAction(fresh)).aborted === true, 'the new unsigned template aborts');
          pending.delete(fresh);
        }
        if (name === 'abortAction') {
          assert(answer.body.aborted === true, 'the kept reference aborts after release');
          pending.delete(reference);
        }
      }
    } catch (e) { failure = e; throw e; }
    finally {
      try {
        if (holder) await holder.release();
        for (const reference of pending) await client.abortAction(reference);
        if (inserted) execFileSync('sqlite3', ['-batch', '-bail', db,
          `DELETE FROM tracker_states WHERE txid = '${txid}' AND built = 0 AND word = 'unknown';`],
        { timeout: BUSY_MS });
      } catch (e) {
        if (!failure) throw e;
        console.log(`       cleanup after ${failure.message}: ${e.message}`);
      }
    }
    const after = await client.balance();
    assert(after === before, `busy-store costs 0 sats: balance ${before} -> ${after}`);
    console.log(`       balance ${before} -> ${after}; cost 0 sats; read-door 503 coverage remains in the dotfile unit/wire world`);
    return { txid: null, sats: 0, wocConfirmed: 'n/a' };
  },
};
