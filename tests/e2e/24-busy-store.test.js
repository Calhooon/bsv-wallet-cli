const { test } = require('node:test');
const assert = require('node:assert/strict');
const { execFileSync } = require('node:child_process');
const fs = require('node:fs');
const os = require('node:os');
const path = require('node:path');
const { WalletClient } = require('./lib/wallet-client');
const { keepExternalTemplate } = require('./scenarios/24-busy-store');

// Exercise the scenario's real SQLite queries and cleanup without a server,
// wallet binary, broadcast or funder. Only the HTTP clients are replaced.
function fixture(t, { available, desired = 6, minimum = 10_000, largest = 17_399, refuse = false }) {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), 'e2e-24-unit-'));
  t.after(() => fs.rmSync(dir, { recursive: true, force: true }));
  const db = path.join(dir, 'wallet.db');
  const sql = query => execFileSync('sqlite3', ['-batch', '-bail', db, query],
    { encoding: 'utf8', timeout: 5_000 }).trim();
  sql(`CREATE TABLE users (user_id INTEGER, identity_key TEXT);
    CREATE TABLE output_baskets (basket_id INTEGER, user_id INTEGER, name TEXT,
      number_of_desired_utxos INTEGER, minimum_desired_utxo_value INTEGER);
    CREATE TABLE transactions (transaction_id INTEGER, status TEXT);
    CREATE TABLE outputs (user_id INTEGER, basket_id INTEGER, transaction_id INTEGER,
      change INTEGER, type TEXT, provided_by TEXT, purpose TEXT,
      derivation_prefix TEXT, derivation_suffix TEXT, spendable INTEGER);
    INSERT INTO users VALUES (1, 'fixture');
    INSERT INTO output_baskets VALUES (1, 1, 'default', ${desired}, ${minimum});
    INSERT INTO output_baskets VALUES (2, 1, 'other', 9, 20000);
    INSERT INTO transactions VALUES (1, 'unproven'), (2, 'failed');
    INSERT INTO outputs VALUES
      (1, 1, 1, 1, 'P2PKH', 'storage', 'receive', 'prefix', 'suffix', 1),
      (1, 1, 1, 1, 'P2PKH', 'storage', 'change', 'prefix', 'suffix', 0),
      (1, 1, 2, 1, 'P2PKH', 'storage', 'change', 'prefix', 'suffix', 1);`);
  for (let i = 0; i < available; i++) {
    sql("INSERT INTO outputs VALUES (1, 1, 1, 1, 'P2PKH', 'storage', 'change', 'prefix', 'suffix', 1);");
  }
  const logs = [];
  t.mock.method(console, 'log', line => logs.push(line));
  const coins = [{ outpoint: 'smaller.0', satoshis: 4_000 }, { outpoint: 'largest.0', satoshis: largest }];
  t.mock.method(WalletClient.prototype, 'listOutputs', async (_basket, opts) => ({
    outputs: opts.limit === 1 ? [coins[opts.offset]] : coins,
    totalOutputs: coins.length,
    BEEF: [1, 2, 3],
  }));
  const pending = new Set();
  const post = t.mock.fn(async (route, body) => {
    assert.equal(sql('SELECT number_of_desired_utxos FROM output_baskets WHERE basket_id = 1;'), '1');
    assert.equal(route, 'createAction');
    assert.equal(body.inputs[0].outpoint, 'largest.0');
    if (refuse) throw new Error('template refused');
    return { signableTransaction: { reference: 'kept' } };
  });
  return {
    logs, pending, post,
    run: () => keepExternalTemplate({ dir, identityKey: 'fixture', client: { post } },
      [{ satoshis: 1_000 }], pending, assert.ok),
    restored() {
      assert.equal(sql('SELECT number_of_desired_utxos FROM output_baskets WHERE basket_id = 1;'), String(desired));
      assert.equal(sql('SELECT number_of_desired_utxos FROM output_baskets WHERE basket_id = 2;'), '9');
    },
  };
}

test('kept demand has margin at every available count, including zero', async t => {
  for (const available of [0, 1, 3, 6, 8]) {
    await t.test(`available=${available}`, async t => {
      const f = fixture(t, { available });
      await f.run();
      f.restored();
      assert(f.pending.has('kept'));
      assert(f.logs.some(line => line.includes(`demand=4023 sats (output=1000, desired=1, available=${available}, target=1`)
        && line.includes('funder largest=17399 sats; margin=13376 sats (minimum=10000)')));
    });
  }
});

test('kept demand reads the basket minimum and restores its original desired count', async t => {
  const f = fixture(t, { available: 0, desired: 12, minimum: 8_000 });
  await f.run();
  f.restored();
  assert(f.logs.some(line => line.includes('demand=3523 sats') && line.includes('initial=8000')));
});

test('kept margin accepts exactly 10000 sats and refuses one sat below it', async t => {
  for (const largest of [14_023, 14_022]) {
    await t.test(`largest=${largest}`, async t => {
      const f = fixture(t, { available: 0, largest });
      if (largest === 14_023) {
        await f.run();
        assert.equal(f.post.mock.callCount(), 1);
      } else {
        await assert.rejects(f.run(), /margin=9999 sats/);
        assert.equal(f.post.mock.callCount(), 0);
      }
      f.restored();
    });
  }
});

test('template refusal restores the basket before propagating the error', async t => {
  const f = fixture(t, { available: 0, refuse: true });
  await assert.rejects(f.run(), /template refused/);
  f.restored();
  assert.equal(f.pending.size, 0);
});
