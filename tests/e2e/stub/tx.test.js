'use strict';
const assert = require('node:assert/strict');
const test = require('node:test');
const fs = require('node:fs');
const path = require('node:path');
const T = require('./tx');

test('BRC-96 reads the proof index before a V2 raw transaction', () => {
  const fixture = JSON.parse(fs.readFileSync(path.join(__dirname, '../../wire/103-internalizeAction.badUnlock.json')));
  const source = T.parseBeef(Buffer.from(fixture.request.json.tx));
  const version = Buffer.alloc(4);
  version.writeUInt32LE(T.BEEF_V2);
  const bytes = Buffer.concat([version, T.varint(source.bumps.length), ...source.bumps.map(b => b.raw),
    T.varint(source.txs.length), ...source.txs.map(tx => Buffer.concat([
      Buffer.from([tx.bumpIndex === null ? 0 : 1]),
      ...(tx.bumpIndex === null ? [] : [T.varint(tx.bumpIndex)]), tx.raw,
    ]))]);
  const parsed = T.parseBeef(bytes);
  assert.deepEqual(parsed.txs.map(tx => [tx.txid, tx.bumpIndex]), source.txs.map(tx => [tx.txid, tx.bumpIndex]));
  assert.deepEqual(parsed.txs.map(tx => tx.raw), source.txs.map(tx => tx.raw));
});
