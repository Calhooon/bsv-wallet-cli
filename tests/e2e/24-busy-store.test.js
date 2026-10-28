const { test } = require('node:test');
const assert = require('node:assert/strict');
const { execFileSync } = require('node:child_process');
const fs = require('node:fs');
const os = require('node:os');
const path = require('node:path');
const { WalletClient, ANYONE_KEY, buildP2PKH } = require('./lib/wallet-client');
const { fundWallet, FUNDER_EXTERNAL_BASKET, FUNDER_EXTERNAL_SATOSHIS } = require('./lib/setup');
const { keepExternalTemplate } = require('./scenarios/24-busy-store');

// Exercise the scenario's real SQLite queries and cleanup without a server,
// wallet binary, broadcast or funder. Only the HTTP clients are replaced.
function fixture(t, { available, desired = 6, minimum = 10_000, largest = 17_399, reserve = true, refuse = false, changed = false }) {
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
      (1, 1, 2, 1, 'P2PKH', 'storage', 'change', 'prefix', 'suffix', 1),
      (1, 2, 1, 1, 'P2PKH', 'storage', 'change', 'prefix', 'suffix', 1);`);
  for (let i = 0; i < available; i++) {
    sql("INSERT INTO outputs VALUES (1, 1, 1, 1, 'P2PKH', 'storage', 'change', 'prefix', 'suffix', 1);");
  }
  const logs = [];
  const warnings = [];
  t.mock.method(console, 'log', line => logs.push(line));
  t.mock.method(console, 'warn', line => warnings.push(line));
  const coins = [{ outpoint: 'smaller.0', satoshis: 4_000 }, { outpoint: 'largest.0', satoshis: largest }];
  const reserved = [{ outpoint: 'small.0', satoshis: 14_999 },
    { outpoint: 'unspendable.0', satoshis: 15_000, spendable: false },
    ...(reserve ? [{ outpoint: 'reserved.0', satoshis: 15_000 }] : [])];
  const list = t.mock.method(WalletClient.prototype, 'listOutputs', async (basket, opts) => {
    const outputs = basket === FUNDER_EXTERNAL_BASKET ? reserved : coins;
    if (opts.include === 'entire transactions') {
      assert.equal(opts.limit, 1);
      return { outputs: [{ ...outputs[opts.offset], ...(changed ? { outpoint: 'changed.0' } : {}) }],
        totalOutputs: outputs.length, BEEF: [1, 2, 3] };
    }
    return { outputs, totalOutputs: outputs.length };
  });
  const pending = new Set();
  const post = t.mock.fn(async (route, body) => {
    assert.equal(sql('SELECT number_of_desired_utxos FROM output_baskets WHERE basket_id = 1;'), '1');
    assert.equal(route, 'createAction');
    assert.equal(body.inputs[0].outpoint, reserve ? 'reserved.0' : 'largest.0');
    assert.deepEqual(body.inputBEEF, [1, 2, 3]);
    assert.equal(body.options.signAndProcess, false);
    assert.equal(body.allowPending, undefined);
    if (refuse) throw new Error('template refused');
    return { signableTransaction: { reference: 'kept' } };
  });
  return {
    logs, warnings, list, pending, post,
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
        && line.includes('funder basket=e2e-external; coin=15000 sats; margin=10977 sats (minimum=10000)')));
      assert.equal(f.warnings.length, 0);
      assert.deepEqual(f.list.mock.calls.map(call => call.arguments[0]), [FUNDER_EXTERNAL_BASKET, FUNDER_EXTERNAL_BASKET]);
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
      const f = fixture(t, { available: 0, largest, reserve: false });
      if (largest === 14_023) {
        await f.run();
        assert.equal(f.post.mock.callCount(), 1);
      } else {
        await assert.rejects(f.run(), /margin=9999 sats/);
        assert.equal(f.post.mock.callCount(), 0);
      }
      f.restored();
      assert.match(f.warnings[0], /WARNING #336:.*e2e-external.*default basket's largest coin/);
      assert(f.logs.some(line => line.includes(`funder basket=default; coin=${largest} sats`)));
    });
  }
});

test('template refusal restores the basket before propagating the error', async t => {
  const f = fixture(t, { available: 0, refuse: true });
  await assert.rejects(f.run(), /template refused/);
  f.restored();
  assert.equal(f.pending.size, 0);
});

test('fallback reads the selected default coin BEEF after the named basket', async t => {
  const f = fixture(t, { available: 0, reserve: false });
  await f.run();
  f.restored();
  assert.deepEqual(f.list.mock.calls.map(call => call.arguments[0]), [FUNDER_EXTERNAL_BASKET, 'default', 'default']);
  assert.deepEqual(f.list.mock.calls[2].arguments[1], { include: 'entire transactions', limit: 1, offset: 1 });
});

test('a changed external coin refuses the template and restores the basket', async t => {
  const f = fixture(t, { available: 0, changed: true });
  await assert.rejects(f.run(), /selected funder coin is unchanged/);
  f.restored();
  assert.equal(f.post.mock.callCount(), 0);
});

// Drive the real funding and transfer helpers with only HTTP replies replaced.
// A second funding call sees the same coin and must emit no extra output.
function fundingFixture(t, { existing = false, available = 300_000 } = {}) {
  const reserved = { outpoint: 'reserve.1', satoshis: FUNDER_EXTERNAL_SATOSHIS, spendable: true };
  const inventory = existing ? [reserved] : [];
  const actions = [];
  const logs = [];
  t.mock.method(console, 'log', line => logs.push(line));
  const recipientKey = '03' + ANYONE_KEY.slice(2);
  const recipient = {
    getPublicKey: t.mock.fn(async () => ({ publicKey: recipientKey })),
    internalizeAction: t.mock.fn(async () => ({ accepted: true })),
  };
  t.mock.method(WalletClient.prototype, 'post', async function (route, body) {
    assert.equal(this.name, 'funder');
    if (route === 'listOutputs') {
      const outputs = body.basket === FUNDER_EXTERNAL_BASKET ? inventory
        : [{ outpoint: 'funding.0', satoshis: available, spendable: true }];
      return { outputs: [...outputs], totalOutputs: outputs.length };
    }
    if (route === 'getPublicKey') {
      assert.deepEqual(body, { protocolId: [2, '3241645161d8'], keyId: 'SfKxPIJNgdI= NaGLC6fMH50=', counterparty: ANYONE_KEY, forSelf: true });
      return { publicKey: ANYONE_KEY };
    }
    assert.equal(route, 'createAction');
    assert.equal(body.allowPending, 'received');
    actions.push(body);
    available -= body.outputs.reduce((sum, output) => sum + output.satoshis, 0) + 50;
    if (body.outputs.some(output => output.basket === FUNDER_EXTERNAL_BASKET)) inventory.push(reserved);
    return { txid: 'funded', tx: [1, 2, 3] };
  });
  return { actions, logs, recipient, recipientKey,
    fund: () => fundWallet({ client: recipient }, 100_000),
    client: new WalletClient(1, 'funder') };
}

test('the funding transfer creates the named self output once and reuses it', async t => {
  const f = fundingFixture(t);
  const before = await f.client.balance() + await f.client.balance(FUNDER_EXTERNAL_BASKET);
  const created = await f.fund();
  assert.equal(f.actions.length, 1);
  assert.equal(f.actions[0].outputs.length, 2);
  assert.equal(f.actions[0].outputs[0].lockingScript, buildP2PKH(f.recipientKey));
  assert.equal(f.actions[0].outputs[0].satoshis, 100_000);
  assert.deepEqual(f.actions[0].outputs[1], {
    lockingScript: buildP2PKH(ANYONE_KEY), satoshis: 15_000,
    basket: FUNDER_EXTERNAL_BASKET, outputDescription: 'E2E.24 external input, never signed or swept',
  });
  assert.equal(f.recipient.internalizeAction.mock.calls[0].arguments[1][0].outputIndex, 0);
  const after = await f.client.balance() + await f.client.balance(FUNDER_EXTERNAL_BASKET);
  assert.equal(before - after, 100_050, 'the retained 15,000 sats are not counted as spent');
  const reused = await f.fund();
  assert.equal(reused.externalOutpoint, created.externalOutpoint);
  assert.equal(f.actions.length, 2);
  assert.equal(f.actions[1].outputs.length, 1);
  assert(f.logs.some(line => /basket=e2e-external; coin=15000 sats; created with funding transfer/.test(line)));
  assert(f.logs.some(line => /basket=e2e-external; coin=15000 sats; reused/.test(line)));
});

test('funding headroom includes the reserve only when it is missing', async t => {
  for (const existing of [false, true]) {
    const required = existing ? 101_000 : 116_000;
    for (const available of [required - 1, required]) {
      await t.test(`existing=${existing}, available=${available}`, async t => {
        const f = fundingFixture(t, { existing, available });
        if (available < required) {
          await assert.rejects(f.fund(), /funder default basket.*#336/);
          assert.equal(f.actions.length, 0);
          assert.equal(f.recipient.internalizeAction.mock.callCount(), 0);
        } else {
          await f.fund();
          assert.equal(f.actions.length, 1);
          assert.equal(f.actions[0].outputs.length, existing ? 1 : 2);
        }
      });
    }
  }
});
