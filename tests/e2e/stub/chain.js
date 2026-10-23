'use strict';
/**
 * The stub's chain: a recorded header chain (fixtures/headers.json, twelve
 * linked mainnet headers read from the header service) that the stub mines
 * on top of, an in-memory mempool that takes every well-formed transaction
 * whose inputs are not already spent by another, blocks mined on demand,
 * and the proofs (BUMP, TSC nodes) and BEEFs of what it holds.
 *
 * Nothing here checks a signature or a script: the wallet under test is the
 * signer, and the chain is the oracle of inclusion, not of validity. A
 * transaction's inputs are checked for a competing spend only (as ARC's
 * DOUBLE_SPEND_ATTEMPTED does), so a wallet bug that spends one coin twice
 * is seen, never hidden.
 */

const fs = require('fs');
const path = require('path');
const T = require('./tx');

const MINER_SCRIPT = T.p2pkh(Buffer.alloc(20, 0x5e)); // the stub miner's own coinbase script

class Chain {
  constructor(recorded) {
    if (!Array.isArray(recorded) || recorded.length === 0) throw new Error('a recorded header chain is required');
    this.headers = recorded.map(h => ({ ...h }));
    this.headersByHash = new Map(this.headers.map(h => [h.hash, h]));
    for (const h of this.headers) {
      if (T.headerHash(h) !== h.hash) throw new Error(`recorded header ${h.height} does not hash to ${h.hash}`);
    }
    for (let i = 1; i < this.headers.length; i++) {
      if (this.headers[i].previousHash !== this.headers[i - 1].hash) throw new Error(`recorded chain breaks at ${this.headers[i].height}`);
    }
    this.recordedTip = this.headers.length - 1;
    /** block hash -> { height, txids } for the blocks the stub mined */
    this.blocks = new Map();
    /** txid -> { tx, blockHash, height, index, seen } */
    this.txs = new Map();
    /** "txid:vout" -> spender txid */
    this.spends = new Map();
    /** unmined txids in arrival order */
    this.mempool = [];
    this.mined = 0;
  }

  static fromFixture(file = path.join(__dirname, 'fixtures', 'headers.json')) {
    const fixture = JSON.parse(fs.readFileSync(file, 'utf-8'));
    return new Chain(fixture.headers);
  }

  tip() { return this.headers[this.headers.length - 1]; }
  height() { return this.tip().height; }
  headerAt(height) { return this.headers.find(h => h.height === height) || null; }
  headerByHash(hash) { return this.headersByHash.get(String(hash).toLowerCase()) || null; }
  confirmations(height) { return this.height() - height + 1; }

  get(txid) { return this.txs.get(String(txid).toLowerCase()) || null; }

  /**
   * Take a transaction into the mempool. Returns `{ status, txid, competing }`
   * with status `accepted` (new), `known` (already held, mined or not) or
   * `double_spend` (an input is spent by another transaction, not taken).
   */
  addTx(tx) {
    const known = this.txs.get(tx.txid);
    if (known) return { status: 'known', txid: tx.txid, entry: known };
    if (!T.isCoinbase(tx)) {
      const competing = new Set();
      for (const i of tx.inputs) {
        const spender = this.spends.get(`${i.txid}:${i.vout}`);
        if (spender && spender !== tx.txid) competing.add(spender);
      }
      if (competing.size) return { status: 'double_spend', txid: tx.txid, competing: [...competing] };
    }
    const entry = { tx, blockHash: null, height: null, index: null, seen: Date.now() };
    this.txs.set(tx.txid, entry);
    if (!T.isCoinbase(tx)) for (const i of tx.inputs) this.spends.set(`${i.txid}:${i.vout}`, tx.txid);
    this.mempool.push(tx.txid);
    return { status: 'accepted', txid: tx.txid, entry };
  }

  /**
   * Take every transaction of a BEEF the stub does not hold yet (parents
   * first, as the BEEF orders them). Returns the subject's answer: the
   * subject's status is the broadcast's status; an ancestor that is a
   * double spend makes the subject one too.
   */
  addBeef(beef) {
    let subjectAnswer = null;
    let ancestorConflict = null;
    for (const tx of beef.txs) {
      if (tx.txidOnly) continue;
      const answer = this.addTx(tx);
      if (answer.status === 'double_spend' && tx.txid !== beef.subject) ancestorConflict = answer;
      if (tx.txid === beef.subject) subjectAnswer = answer;
    }
    if (!subjectAnswer) return { status: 'malformed', txid: beef.subject, reason: 'the subject is not a transaction of the BEEF' };
    if (ancestorConflict && subjectAnswer.status === 'accepted') {
      return { status: 'double_spend', txid: beef.subject, competing: ancestorConflict.competing };
    }
    return subjectAnswer;
  }

  /**
   * Mine a block: a coinbase first, then `txids` (default: the whole
   * mempool, in arrival order). `coinbase` replaces the miner's own.
   */
  mine({ txids = null, coinbase = null } = {}) {
    const prev = this.tip();
    const height = prev.height + 1;
    const cb = coinbase || T.makeCoinbase(height, MINER_SCRIPT, 625_000_000);
    const entry = this.addTx(cb);
    if (entry.status === 'double_spend') throw new Error('a coinbase cannot double spend');
    const chosen = txids === null ? [...this.mempool] : txids.map(t => String(t).toLowerCase());
    const blockTxids = [cb.txid, ...chosen.filter(t => t !== cb.txid)];
    for (const t of blockTxids) {
      const e = this.txs.get(t);
      if (!e) throw new Error(`cannot mine unknown transaction ${t}`);
      if (e.blockHash) throw new Error(`transaction ${t} is already mined`);
    }
    const header = {
      version: prev.version,
      previousHash: prev.hash,
      merkleRoot: T.merkleRoot(blockTxids),
      time: Math.max(prev.time + 600, Math.floor(Date.now() / 1000)),
      bits: prev.bits,
      nonce: this.mined + 1,
      height,
    };
    header.hash = T.headerHash(header);
    this.headers.push(header);
    this.headersByHash.set(header.hash, header);
    this.blocks.set(header.hash, { height, txids: blockTxids });
    blockTxids.forEach((t, index) => {
      const e = this.txs.get(t);
      e.blockHash = header.hash; e.height = height; e.index = index;
    });
    const minedSet = new Set(blockTxids);
    this.mempool = this.mempool.filter(t => !minedSet.has(t));
    this.mined += 1;
    return { height, hash: header.hash, txids: blockTxids, merkleRoot: header.merkleRoot };
  }

  /**
   * A coinbase-shaped funding: a block whose coinbase pays `satoshis` to
   * `script`, mined alone. Returns the coin and its AtomicBEEF.
   */
  fund(script, satoshis) {
    const height = this.height() + 1;
    const cb = T.makeCoinbase(height, script, satoshis);
    const block = this.mine({ txids: [], coinbase: cb });
    return { txid: cb.txid, vout: 0, satoshis, height: block.height, hash: block.hash, beef: this.beefFor(cb.txid, { atomic: true }).toString('hex') };
  }

  /** The BUMP of a mined transaction, or null. */
  bumpFor(txid) {
    const e = this.get(txid);
    if (!e || !e.blockHash) return null;
    const block = this.blocks.get(e.blockHash);
    return T.bump(e.height, block.txids, e.index);
  }

  /** TSC proof parts of a mined transaction, or null. */
  tscFor(txid) {
    const e = this.get(txid);
    if (!e || !e.blockHash) return null;
    const block = this.blocks.get(e.blockHash);
    const header = this.headerByHash(e.blockHash);
    return { index: e.index, txid: e.tx.txid, blockHash: e.blockHash, merkleRoot: header.merkleRoot, nodes: T.tscNodes(block.txids, e.index) };
  }

  /**
   * The BEEF of a transaction the stub holds: the transaction, every
   * unmined ancestor (recursively) and a BUMP for each mined one; the
   * AtomicBEEF of the subject with `atomic`. Throws when an ancestor is not
   * held (a coin of the recorded chain the stub never saw).
   */
  beefFor(txid, { atomic = false } = {}) {
    const subject = this.get(txid);
    if (!subject) throw new Error(`unknown transaction ${txid}`);
    const entries = [];
    const bumps = [];
    const bumpIndexByBlock = new Map();
    const seen = new Set();
    const visit = (e) => {
      if (seen.has(e.tx.txid)) return;
      seen.add(e.tx.txid);
      if (e.blockHash) {
        let idx = bumpIndexByBlock.get(e.blockHash);
        if (idx === undefined) {
          // One BUMP per block; each proven txid of that block is a flagged leaf.
          idx = bumps.length;
          bumps.push(null);
          bumpIndexByBlock.set(e.blockHash, idx);
        }
        entries.push({ raw: e.tx.raw, bumpIndex: idx, txid: e.tx.txid, blockHash: e.blockHash });
        return;
      }
      if (!T.isCoinbase(e.tx)) {
        for (const i of e.tx.inputs) {
          const parent = this.get(i.txid);
          if (!parent) throw new Error(`ancestor ${i.txid} of ${txid} is not held by the stub`);
          visit(parent);
        }
      }
      entries.push({ raw: e.tx.raw, bumpIndex: null, txid: e.tx.txid });
    };
    visit(subject);
    // Fill each block's BUMP with every leaf it must prove (one BUMP, several flagged txids).
    for (const [blockHash, idx] of bumpIndexByBlock) {
      const block = this.blocks.get(blockHash);
      const proven = entries.filter(e => e.blockHash === blockHash).map(e => e.txid);
      bumps[idx] = combinedBump(block.height, block.txids, proven);
    }
    return T.buildBeef(entries, bumps, atomic ? subject.tx.txid : null);
  }

  spenderOf(txid, vout) { return this.spends.get(`${String(txid).toLowerCase()}:${vout}`) || null; }

  /** The outputs under a script hash (display sha256 of the script), spent or not. */
  outputsOfScriptHash(hash) {
    const want = String(hash).toLowerCase();
    const out = [];
    for (const [txid, e] of this.txs) {
      e.tx.outputs.forEach((o, vout) => {
        if (T.scriptHash(o.script) === want) {
          out.push({ txid, vout, satoshis: o.satoshis, height: e.height, spentBy: this.spends.get(`${txid}:${vout}`) || null, script: o.script });
        }
      });
    }
    return out;
  }

  /** Unspent outputs paying a P2PKH hash160. */
  outputsOfHash160(h160) {
    const out = [];
    for (const [txid, e] of this.txs) {
      e.tx.outputs.forEach((o, vout) => {
        const h = T.p2pkhHash160(o.script);
        if (h && h.equals(h160)) out.push({ txid, vout, satoshis: o.satoshis, height: e.height, spentBy: this.spends.get(`${txid}:${vout}`) || null });
      });
    }
    return out;
  }

  /** Every transaction touching a script hash: paying it, or spending an output that pays it. */
  historyOfScriptHash(hash) {
    const touched = new Map();
    for (const o of this.outputsOfScriptHash(hash)) {
      touched.set(o.txid, this.get(o.txid).height);
      if (o.spentBy) touched.set(o.spentBy, this.get(o.spentBy).height);
    }
    return [...touched].map(([txid, height]) => ({ txid, height }));
  }

  summary() {
    return { height: this.height(), tip: this.tip().hash, recordedTip: this.headers[this.recordedTip].height, minedBlocks: this.mined, mempool: this.mempool.length, transactions: this.txs.size };
  }
}

/**
 * One BUMP for a block proving several of its txids: level 0 holds each
 * proven txid (flag 2) and the sibling of each; every level above the
 * sibling of each walk, duplicates flagged. Offsets are unique per level.
 */
function combinedBump(height, txids, proven) {
  const levels = T.merkleLevels(txids);
  const path = [];
  if (txids.length === 1) {
    path.push([{ offset: 0, flags: 2, hash: txids[0] }]);
  } else {
    const indexOf = new Map(txids.map((t, i) => [t, i]));
    for (let h = 0; h < levels.length - 1; h++) {
      const leaves = new Map();
      for (const t of proven) {
        const index = indexOf.get(t);
        const idx = index >> h;
        const sib = idx ^ 1;
        if (h === 0) leaves.set(idx, { offset: idx, flags: 2, hash: t });
        if (!leaves.has(sib) || (leaves.get(sib).flags & 2) === 0) {
          if (sib < levels[h].length) {
            const existing = leaves.get(sib);
            // A sibling that is itself a proven txid keeps its txid flag.
            if (!existing) leaves.set(sib, { offset: sib, flags: 0, hash: T.display(levels[h][sib]) });
          } else {
            leaves.set(sib, { offset: sib, flags: 1 });
          }
        }
      }
      path.push([...leaves.values()].sort((a, b) => a.offset - b.offset));
    }
  }
  const parts = [T.varint(height), Buffer.from([path.length])];
  for (const level of path) {
    parts.push(T.varint(level.length));
    for (const leaf of level) {
      parts.push(T.varint(leaf.offset), Buffer.from([leaf.flags]));
      if (!(leaf.flags & 1)) parts.push(T.rev(leaf.hash));
    }
  }
  return Buffer.concat(parts);
}

module.exports = { Chain, combinedBump, MINER_SCRIPT };
