/**
 * E2E.24: an external sqlite3 writer on the served WAL store (#204).
 * The open reader pool prevents an EXCLUSIVE locking-mode connection from
 * taking the file. A normal WAL writer blocks writes, not reads. The read
 * doors' 503s remain covered by the unit/wire tests' unix-dotfile VFS.
 * abortAction's 503 comes from the toolbox's abort write; #168's pre-check
 * is a WAL read, covered end to end by the unit test and wire fixture 116.
 * https://www.sqlite.org/wal.html#use_of_wal_without_shared_memory
 */
const { spawn, execFileSync } = require('child_process');
const path = require('path');
const { FUNDER_PORT, FUNDER_EXTERNAL_BASKET, FUNDER_EXTERNAL_SATOSHIS } = require('../lib/setup');
const { WalletClient, ANYONE_KEY, buildP2PKH } = require('../lib/wallet-client');
const hermetic = require('../lib/hermetic');

const BUSY_MS = 5_000; // toolbox 0.13.0: STORE_BUSY_CEILING_MS
// Toolbox 0.14.0-beta.2 fees.rs: a 108-byte input script + two P2PKH outputs
// = 227 bytes; ceil(227 * 101 / 1,000) = 23 sats at the default 101 sat/kB.
// Keep the 500-sat margin for additional change outputs and fee rounding.
const EXTERNAL_INPUT_FEE_ALLOWANCE = 23 + 500;
const EXTERNAL_INPUT_MIN_MARGIN = 10_000;

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
  const lockFailure = `sqlite3 pid ${proc.pid}: e2e_busy write lock outlived the scenario`;
  const release = async () => {
    try {
      proc.stdin.end('ROLLBACK;\n');
      const result = await within(exited, BUSY_MS, `sqlite3 pid ${proc.pid}: lock outlived ROLLBACK and stdin close`);
      assert(result.code === 0, `${lockFailure}: holder must exit 0, got ${JSON.stringify(result)}: ${errors}`);
      if (heldAt !== undefined) console.log(`       sqlite3 pid ${proc.pid}: ROLLBACK, stdin closed, exit 0 after holding ${Date.now() - heldAt}ms`);
    } finally {
      if (proc.exitCode === null) { stop(); await exited; }
      process.removeListener('exit', stop);
    }
  };
  const confirmReleased = async () => {
    const start = Date.now();
    try {
      const result = await within(exited, BUSY_MS, `holder did not exit within ${BUSY_MS}ms`);
      assert(result.code === 0, `holder exit ${JSON.stringify(result)}: ${errors}`);
      // A WAL reader never detects this lock. Acquire the write side, allowing
      // the server's own brief writes up to two seconds to finish.
      execFileSync('sqlite3', ['-batch', '-bail', db,
        'PRAGMA busy_timeout=2000; BEGIN IMMEDIATE; ROLLBACK;'],
      { encoding: 'utf8', timeout: BUSY_MS, stdio: 'pipe' });
    } catch (e) {
      throw new Error(`${lockFailure}: ${e.message}`);
    } finally {
      // Even a missing release must fail before cleanup or the harness sweep,
      // without leaving this holder alive to block either one's writes.
      if (proc.exitCode === null && proc.signalCode === null) { stop(); await exited; }
      process.removeListener('exit', stop);
    }
    console.log(`       sqlite3 pid ${proc.pid}: released and exited 0; BEGIN IMMEDIATE / ROLLBACK write probe succeeded in ${Date.now() - start}ms (busy_timeout 2000ms)`);
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
  return { release, confirmReleased };
}

// Keep the allocator target at one only while assembling the unsigned template.
// Split's self-internalized outputs have purpose=receive (internalize_action.rs:
// 461-462), so SPEND_OWN_CHANGE alone does not make them count in this query.
async function keepExternalTemplate(walletA, outputs, pending, assert) {
  const db = path.join(walletA.dir, 'wallet.db');
  // Mirror toolbox 0.14.0-beta.2 mod.rs:333-340 and allocation.rs:21-31,59-96:
  // this request requires spendable change and has no received-pending rule.
  const readBasket = () => JSON.parse(execFileSync('sqlite3', ['-batch', '-bail', '-json',
      db,
      `SELECT b.basket_id, b.number_of_desired_utxos, b.minimum_desired_utxo_value,
        (SELECT COUNT(*) FROM outputs o
         JOIN transactions t ON o.transaction_id = t.transaction_id
         WHERE o.user_id = b.user_id AND o.basket_id = b.basket_id
           AND o.change = 1 AND t.status IN ('completed', 'unproven')
           AND o.type = 'P2PKH' AND o.provided_by = 'storage'
           AND o.purpose IN ('change', 'receive')
           AND COALESCE(o.derivation_prefix, '') <> ''
           AND COALESCE(o.derivation_suffix, '') <> ''
           AND (t.status = 'completed' OR COALESCE(o.purpose, '') <> 'receive' OR 0)
           AND o.spendable = 1) AS available
       FROM output_baskets b JOIN users u ON b.user_id = u.user_id
       WHERE b.name = 'default' AND u.identity_key = '${walletA.identityKey}';`],
    { encoding: 'utf8', timeout: BUSY_MS }))[0];
  const original = readBasket();
  assert(original, 'A has a default basket for the allocator demand');
  const setDesired = desired => execFileSync('sqlite3', ['-batch', '-bail', db,
    `UPDATE output_baskets SET number_of_desired_utxos = ${desired} WHERE basket_id = ${original.basket_id};`],
  { encoding: 'utf8', timeout: BUSY_MS });
  try {
    setDesired(1);
    const basket = readBasket();
    assert(basket.number_of_desired_utxos === 1, 'the kept template has a one-change target');
    const desired = basket.number_of_desired_utxos;
    const available = basket.available;
    const initial = basket.minimum_desired_utxo_value;
    const first = Math.max(1, Math.floor(initial / 4));
    // change.rs:78-87,318-345 opens the target's change outputs regardless of
    // surplus, and any positive surplus opens one even when the target is met.
    const target = Math.max(desired - available, 1);
    assert(target === 1, 'the kept template needs one change output for every available count');
    const outputSatoshis = outputs[0].satoshis;
    const demand = outputSatoshis + EXTERNAL_INPUT_FEE_ALLOWANCE
      + first + Math.max(target - 1, 0) * initial;
    const funder = new WalletClient(FUNDER_PORT, 'funder');
    let sourceBasket = FUNDER_EXTERNAL_BASKET;
    let source = await funder.listOutputs(sourceBasket, { limit: 10_000 });
    assert(source.outputs.length === source.totalOutputs, `the ${sourceBasket} inventory includes every coin`);
    let external = source.outputs.find(coin =>
      coin.spendable !== false && coin.satoshis === FUNDER_EXTERNAL_SATOSHIS);
    if (!external) {
      console.warn(`       WARNING #336: no ${FUNDER_EXTERNAL_SATOSHIS}-sat coin in funder basket ${sourceBasket}; falling back to the default basket's largest coin`);
      sourceBasket = 'default';
      source = await funder.listOutputs(sourceBasket, { limit: 10_000 });
      assert(source.outputs.length === source.totalOutputs, 'the funder list includes every coin for largest-input selection');
      external = source.outputs.filter(coin => coin.spendable !== false).reduce((largest, coin) =>
        !largest || coin.satoshis > largest.satoshis ? coin : largest, null);
    }
    const fundingDemand = `external input demand=${demand} sats (output=${outputSatoshis}, desired=${desired}, available=${available}, target=${target}, first=${first}, initial=${initial}, fee=${EXTERNAL_INPUT_FEE_ALLOWANCE}=23+500); funder basket=${sourceBasket}; coin=${external?.satoshis ?? 'none'} sats`;
    const margin = external ? external.satoshis - demand : null;
    const fundingMargin = `${fundingDemand}; margin=${margin ?? 'none'} sats (minimum=${EXTERNAL_INPUT_MIN_MARGIN})`;
    console.log(`       ${fundingMargin}`);
    assert(external && margin >= EXTERNAL_INPUT_MIN_MARGIN, fundingMargin);
    // Read the selected coin's BEEF from the same basket on both paths.
    source = await funder.listOutputs(sourceBasket, {
      include: 'entire transactions', limit: 1, offset: source.outputs.indexOf(external),
    });
    assert(source.outputs[0]?.outpoint === external.outpoint, 'the selected funder coin is unchanged while reading its BEEF');
    const beef = source.BEEF || source.beef; // beta.2 bytes / beta.1 hex (#105)
    assert(beef, 'the funder has BEEF for the unsigned template');
    const kept = await walletA.client.post('createAction', {
      outputs, description: 'E2E.24 kept unsigned template',
      inputs: [{ outpoint: external.outpoint, unlockingScriptLength: 108, inputDescription: 'External input, never signed' }],
      inputBEEF: Array.isArray(beef) ? beef : Array.from(Buffer.from(beef, 'hex')),
      options: { signAndProcess: false, randomizeOutputs: false },
    });
    if (kept.signableTransaction?.reference) pending.add(kept.signableTransaction.reference);
    return kept;
  } finally {
    setDesired(original.number_of_desired_utxos);
    const restored = readBasket();
    assert(restored.number_of_desired_utxos === original.number_of_desired_utxos,
      "A's desired change count is restored before testing the busy store");
    console.log(`       desired change count ${original.number_of_desired_utxos} -> 1 -> ${restored.number_of_desired_utxos}; restored before the WAL writer`);
  }
}

module.exports = {
  name: 'busy-store',
  description: 'External WAL writer: busy write doors retry, read doors stay available; release restores all five (0 sats)',
  hermetic: true,
  keepExternalTemplate,

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
    // the funder's e2e-external coin, read with its BEEF, never signed or
    // spent by this scenario. Its basket keeps it outside ordinary funding.
    const pending = new Set();
    const db = path.join(walletA.dir, 'wallet.db');
    let inserted = false;
    let holder, failure;
    const create = () => client.createAction(outputs, 'E2E.24 busy write', { signAndProcess: false });
    const ask = async (name, call, raw = false) => {
      const start = Date.now();
      let answer;
      try {
        const result = await within(call(), BUSY_MS * 2, `${name}: request exceeded two busy ceilings`);
        answer = raw ? result : { status: client.lastStatus, body: result, retryAfter: null };
        if (answer.body.signableTransaction?.reference) pending.add(answer.body.signableTransaction.reference);
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
      const kept = await keepExternalTemplate(walletA, outputs, pending, assert);
      const reference = kept.signableTransaction?.reference;
      assert(reference, 'createAction returns the kept unsigned reference');
      pending.add(reference);
      assert(await client.balance() === before, 'the kept external-input template leaves A funds available');
      const doors = [
        ['listOutputs', () => client.listOutputs(), 200],
        ['createAction', create, 503],
        ['abortAction', () => client.abortAction(reference), 503],
        ['GET /chain-feed', () => client.getResponse('chain-feed'), 200, true],
        ['GET /tx', () => client.getResponse(`tx/${txid}`), 200, true],
      ];
      holder = await holdWriter(db, assert);
      try {
        const answers = [];
        for (const [name, call, , raw] of doors) {
          const answer = await ask(`held ${name}`, call, raw);
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
        const releasedHolder = holder;
        await holder.release();
        holder = null;
        await releasedHolder.confirmReleased();
      }
      for (const [name, call, , raw] of doors) {
        const answer = await ask(`released ${name}`, call, raw);
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
