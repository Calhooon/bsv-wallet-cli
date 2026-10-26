'use strict';
/**
 * The bytes the stub reads and writes: transactions (raw and Extended
 * Format), BEEF and AtomicBEEF (BRC-62, BRC-95), BUMPs (BRC-74), TSC proof
 * nodes, block headers and merkle trees. Node's crypto alone; no package.
 *
 * Byte orders, the one place they are decided:
 *   - a "display" hash is the hex a person reads (a txid, a block hash, a
 *     merkle root), which is the reverse of the bytes on the wire;
 *   - a BUMP's leaf hash, a header's previousHash and merkleRoot, and a
 *     transaction input's txid are written in wire order (display reversed).
 * `rev(display)` gives the wire bytes; `display(wire)` the hex.
 */

const { createHash } = require('crypto');

function sha256(b) { return createHash('sha256').update(b).digest(); }
function sha256d(b) { return sha256(sha256(b)); }
function hash160(b) { return createHash('ripemd160').update(sha256(b)).digest(); }

/** Display hex to wire bytes. */
function rev(hex) { return Buffer.from(hex, 'hex').reverse(); }
/** Wire bytes to display hex. */
function display(buf) { return Buffer.from(buf).reverse().toString('hex'); }

/** The script hash WhatsOnChain, Bitails and BananaBlocks key on: sha256 of the script, displayed. */
function scriptHash(script) { return display(sha256(script)); }

function isHex64(s) { return typeof s === 'string' && /^[0-9a-fA-F]{64}$/.test(s); }

// ─── varints and a reader ───────────────────────────────────────────────

function varint(n) {
  if (n < 0xfd) return Buffer.from([n]);
  if (n <= 0xffff) { const b = Buffer.alloc(3); b[0] = 0xfd; b.writeUInt16LE(n, 1); return b; }
  if (n <= 0xffffffff) { const b = Buffer.alloc(5); b[0] = 0xfe; b.writeUInt32LE(n, 1); return b; }
  const b = Buffer.alloc(9); b[0] = 0xff; b.writeBigUInt64LE(BigInt(n), 1); return b;
}

function u32le(n) { const b = Buffer.alloc(4); b.writeUInt32LE(n >>> 0); return b; }
function u64le(n) { const b = Buffer.alloc(8); b.writeBigUInt64LE(BigInt(n)); return b; }

class Reader {
  constructor(buf) { this.buf = buf; this.pos = 0; }
  need(n) { if (this.pos + n > this.buf.length) throw new Error(`truncated at ${this.pos}: need ${n}, have ${this.buf.length - this.pos}`); }
  u8() { this.need(1); return this.buf[this.pos++]; }
  u32() { this.need(4); const v = this.buf.readUInt32LE(this.pos); this.pos += 4; return v; }
  u64() { this.need(8); const v = this.buf.readBigUInt64LE(this.pos); this.pos += 8; return Number(v); }
  varint() {
    const first = this.u8();
    if (first < 0xfd) return first;
    if (first === 0xfd) { this.need(2); const v = this.buf.readUInt16LE(this.pos); this.pos += 2; return v; }
    if (first === 0xfe) return this.u32();
    return this.u64();
  }
  bytes(n) { this.need(n); const b = this.buf.subarray(this.pos, this.pos + n); this.pos += n; return Buffer.from(b); }
  remaining() { return this.buf.length - this.pos; }
}

// ─── transactions ───────────────────────────────────────────────────────

const EF_MARKER = Buffer.from('0000000000ef', 'hex');
const COINBASE_TXID = '0'.repeat(64);

/**
 * Read one transaction from a reader (raw or Extended Format), consuming
 * exactly its bytes. Returns the parsed transaction with its standard
 * serialization (`raw`) and txid, EF extras stripped.
 */
function readTx(r) {
  const version = r.u32();
  let ef = false;
  if (r.remaining() >= 6 && r.buf.subarray(r.pos, r.pos + 6).equals(EF_MARKER)) { ef = true; r.pos += 6; }
  const nIn = r.varint();
  const inputs = [];
  for (let i = 0; i < nIn; i++) {
    const txid = display(r.bytes(32));
    const vout = r.u32();
    const script = r.bytes(r.varint());
    const sequence = r.u32();
    const input = { txid, vout, script, sequence };
    if (ef) { input.satoshis = r.u64(); input.lockingScript = r.bytes(r.varint()); }
    inputs.push(input);
  }
  const nOut = r.varint();
  const outputs = [];
  for (let i = 0; i < nOut; i++) {
    const satoshis = r.u64();
    const script = r.bytes(r.varint());
    outputs.push({ satoshis, script });
  }
  const locktime = r.u32();
  const tx = { version, inputs, outputs, locktime, ef };
  tx.raw = serializeTx(tx);
  tx.txid = display(sha256d(tx.raw));
  return tx;
}

function parseTx(buf) {
  const r = new Reader(buf);
  const tx = readTx(r);
  if (r.remaining() !== 0) throw new Error(`${r.remaining()} trailing bytes after the transaction`);
  return tx;
}

function serializeTx({ version, inputs, outputs, locktime }) {
  const parts = [u32le(version), varint(inputs.length)];
  for (const i of inputs) parts.push(rev(i.txid), u32le(i.vout), varint(i.script.length), i.script, u32le(i.sequence));
  parts.push(varint(outputs.length));
  for (const o of outputs) parts.push(u64le(o.satoshis), varint(o.script.length), o.script);
  parts.push(u32le(locktime));
  return Buffer.concat(parts);
}

function isCoinbase(tx) {
  return tx.inputs.length === 1 && tx.inputs[0].txid === COINBASE_TXID && tx.inputs[0].vout === 0xffffffff;
}

/** A coinbase paying `satoshis` to `script`, unique per height (the height is pushed first, as BIP34 does). */
function makeCoinbase(height, script, satoshis) {
  const h = Buffer.alloc(4); h.writeUInt32LE(height);
  const tag = Buffer.from('e2e-hermetic');
  const unlocking = Buffer.concat([Buffer.from([4]), h, Buffer.from([tag.length]), tag]);
  const tx = {
    version: 1,
    inputs: [{ txid: COINBASE_TXID, vout: 0xffffffff, script: unlocking, sequence: 0xffffffff }],
    outputs: [{ satoshis, script }],
    locktime: 0,
    ef: false,
  };
  tx.raw = serializeTx(tx);
  tx.txid = display(sha256d(tx.raw));
  return tx;
}

/** OP_DUP OP_HASH160 <20> OP_EQUALVERIFY OP_CHECKSIG */
function p2pkh(hash160Bytes) {
  return Buffer.concat([Buffer.from('76a914', 'hex'), hash160Bytes, Buffer.from('88ac', 'hex')]);
}

function p2pkhHash160(script) {
  if (script.length === 25 && script[0] === 0x76 && script[1] === 0xa9 && script[2] === 0x14 && script[23] === 0x88 && script[24] === 0xac) {
    return script.subarray(3, 23);
  }
  return null;
}

// ─── base58check (addresses, for BananaBlocks' address route) ───────────

const B58 = '123456789ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnopqrstuvwxyz';

function base58Decode(s) {
  let n = 0n;
  for (const c of s) {
    const i = B58.indexOf(c);
    if (i < 0) throw new Error(`not base58: ${c}`);
    n = n * 58n + BigInt(i);
  }
  let hex = n.toString(16); if (hex.length % 2) hex = '0' + hex;
  let bytes = Buffer.from(hex, 'hex');
  let zeros = 0; for (const c of s) { if (c !== '1') break; zeros++; }
  return Buffer.concat([Buffer.alloc(zeros), bytes]);
}

/** The hash160 of a base58check P2PKH address, or null when it is not one. */
function addressHash160(address) {
  try {
    const b = base58Decode(address);
    if (b.length !== 25) return null;
    const check = sha256d(b.subarray(0, 21)).subarray(0, 4);
    if (!check.equals(b.subarray(21))) return null;
    return Buffer.from(b.subarray(1, 21));
  } catch { return null; }
}

// ─── merkle trees, BUMPs and TSC nodes ──────────────────────────────────

/** The tree's levels, wire-order node hashes; level 0 the txids in block order. */
function merkleLevels(txids) {
  let level = txids.map(rev);
  const levels = [level];
  while (level.length > 1) {
    const next = [];
    for (let i = 0; i < level.length; i += 2) {
      const left = level[i];
      const right = i + 1 < level.length ? level[i + 1] : left;
      next.push(sha256d(Buffer.concat([left, right])));
    }
    level = next;
    levels.push(level);
  }
  return levels;
}

function merkleRoot(txids) {
  const levels = merkleLevels(txids);
  return display(levels[levels.length - 1][0]);
}

/**
 * The BRC-74 BUMP proving `txids[index]` in a block at `height`: level 0
 * holds the txid (flag 2) and its sibling, each level above the sibling of
 * the walk, a duplicate flag (1) where the sibling does not exist. A block
 * of one transaction is one level with its lone leaf at offset 0.
 */
function bump(height, txids, index) {
  const levels = merkleLevels(txids);
  const path = [];
  if (txids.length === 1) {
    path.push([{ offset: 0, flags: 2, hash: txids[0] }]);
  } else {
    for (let h = 0; h < levels.length - 1; h++) {
      const idx = index >> h;
      const sib = idx ^ 1;
      const leaves = [];
      if (h === 0) leaves.push({ offset: idx, flags: 2, hash: txids[index] });
      if (sib < levels[h].length) leaves.push({ offset: sib, flags: 0, hash: display(levels[h][sib]) });
      else leaves.push({ offset: sib, flags: 1 });
      leaves.sort((a, b) => a.offset - b.offset);
      path.push(leaves);
    }
  }
  const parts = [varint(height), Buffer.from([path.length])];
  for (const level of path) {
    parts.push(varint(level.length));
    for (const leaf of level) {
      parts.push(varint(leaf.offset), Buffer.from([leaf.flags]));
      if (!(leaf.flags & 1)) parts.push(rev(leaf.hash));
    }
  }
  return Buffer.concat(parts);
}

/** Read one BUMP from a reader (to split a BEEF's bumps and know their heights). */
function readBump(r) {
  const start = r.pos;
  const height = r.varint();
  const treeHeight = r.u8();
  const levels = [];
  for (let h = 0; h < treeHeight; h++) {
    const n = r.varint();
    const leaves = [];
    for (let i = 0; i < n; i++) {
      const offset = r.varint();
      const flags = r.u8();
      const leaf = { offset, flags };
      if (!(flags & 1)) leaf.hash = display(r.bytes(32));
      leaves.push(leaf);
    }
    levels.push(leaves);
  }
  return { height, levels, raw: Buffer.from(r.buf.subarray(start, r.pos)) };
}

/** TSC proof nodes for `txids[index]`: the sibling at each level, "*" for a duplicate. */
function tscNodes(txids, index) {
  const levels = merkleLevels(txids);
  const nodes = [];
  for (let h = 0; h < levels.length - 1; h++) {
    const sib = (index >> h) ^ 1;
    nodes.push(sib < levels[h].length ? display(levels[h][sib]) : '*');
  }
  return nodes;
}

// ─── BEEF ───────────────────────────────────────────────────────────────

const BEEF_V1 = 0xEFBE0001;
const BEEF_V2 = 0xEFBE0002;
const ATOMIC_BEEF = 0x01010101;

/**
 * Read a BEEF or AtomicBEEF: its bumps (raw and parsed) and its
 * transactions in order (raw entries parsed; txid-only entries as
 * `{ txidOnly: txid }`). The subject is the atomic txid when there is one,
 * else the last raw transaction.
 */
function parseBeef(buf) {
  const r = new Reader(buf);
  let version = r.u32();
  let atomicTxid = null;
  if (version === ATOMIC_BEEF) { atomicTxid = display(r.bytes(32)); version = r.u32(); }
  if (version !== BEEF_V1 && version !== BEEF_V2) throw new Error(`not a BEEF: version 0x${version.toString(16)}`);
  const nBumps = r.varint();
  const bumps = [];
  for (let i = 0; i < nBumps; i++) bumps.push(readBump(r));
  const nTxs = r.varint();
  const txs = [];
  for (let i = 0; i < nTxs; i++) {
    if (version === BEEF_V2) {
      const format = r.u8();
      if (format === 2) { txs.push({ txidOnly: display(r.bytes(32)) }); continue; }
      if (format !== 0 && format !== 1) throw new Error(`invalid BEEF V2 transaction format ${format}`);
      const bumpIndex = format === 1 ? r.varint() : null;
      const tx = readTx(r);
      tx.bumpIndex = bumpIndex;
      txs.push(tx);
    } else {
      const tx = readTx(r);
      const hasBump = r.u8() !== 0;
      tx.bumpIndex = hasBump ? r.varint() : null;
      txs.push(tx);
    }
  }
  if (r.remaining() !== 0) throw new Error(`${r.remaining()} trailing bytes after the BEEF`);
  const raws = txs.filter(t => !t.txidOnly);
  const subject = atomicTxid || (raws.length ? raws[raws.length - 1].txid : null);
  return { version, atomicTxid, bumps, txs, subject };
}

/**
 * A BEEF V1 of `entries` (`{ raw, bumpIndex }`, parents before children) over
 * `bumps` (raw BUMP buffers); with `atomicTxid`, the AtomicBEEF of that subject.
 */
function buildBeef(entries, bumps, atomicTxid = null) {
  const parts = [];
  if (atomicTxid) parts.push(u32le(ATOMIC_BEEF), rev(atomicTxid));
  parts.push(u32le(BEEF_V1), varint(bumps.length), ...bumps, varint(entries.length));
  for (const e of entries) {
    parts.push(e.raw, Buffer.from([e.bumpIndex == null ? 0 : 1]));
    if (e.bumpIndex != null) parts.push(varint(e.bumpIndex));
  }
  return Buffer.concat(parts);
}

/** What a body posted to a broadcaster is: a BEEF, an EF or raw transaction. */
function classifyBytes(buf) {
  if (buf.length >= 4) {
    const v = buf.readUInt32LE(0);
    if (v === ATOMIC_BEEF || v === BEEF_V1 || v === BEEF_V2) return 'beef';
  }
  return 'tx';
}

// ─── block headers ──────────────────────────────────────────────────────

function headerBytes(h) {
  return Buffer.concat([u32le(h.version), rev(h.previousHash), rev(h.merkleRoot), u32le(h.time), u32le(h.bits), u32le(h.nonce)]);
}

function headerHash(h) { return display(sha256d(headerBytes(h))); }

module.exports = {
  sha256, sha256d, hash160, rev, display, scriptHash, isHex64, varint, Reader,
  readTx, parseTx, serializeTx, isCoinbase, makeCoinbase, p2pkh, p2pkhHash160, addressHash160, COINBASE_TXID,
  merkleLevels, merkleRoot, bump, readBump, tscNodes,
  parseBeef, buildBeef, classifyBytes, BEEF_V1, BEEF_V2, ATOMIC_BEEF,
  headerBytes, headerHash,
};
