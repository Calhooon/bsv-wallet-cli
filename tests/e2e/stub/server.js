#!/usr/bin/env node
'use strict';
/**
 * The hermetic stub (bsv-wallet-cli #74): one HTTP process that plays, over
 * one in-memory chain, every service the wallet binary and the e2e auditor
 * leave the machine for:
 *
 *   /chaintracks   the header service (CHAINTRACKS_URL)
 *   /woc           WhatsOnChain's shape (the CLI's WOC_BASE, the auditor's rung)
 *   /bitails       Bitails' shape (BITAILS_BASE, the auditor's rung)
 *   /bananablocks  BananaBlocks' shape (BANANABLOCKS_BASE)
 *   /arc           a classic ARC broadcaster (ARC_URL)
 *   /_stub         the harness's own: mine, fund, fault, stats, beef
 *
 * and the wall: any CONNECT or absolute-URI request (what a binary makes
 * when HTTPS_PROXY/HTTP_PROXY name this port) is refused 403 and counted
 * under `leaks`, so a request bound for a real host never leaves and is
 * named in the stats. The routes are listed in docs/E2E-HERMETIC.md.
 *
 *   node tests/e2e/stub/server.js --port 3380
 */

const http = require('http');
const { URL } = require('url');
const T = require('./tx');
const { Chain } = require('./chain');

const PROVIDERS = ['chaintracks', 'woc', 'bitails', 'bananablocks', 'arc'];

class Stub {
  constructor({ chain = Chain.fromFixture(), log = () => {} } = {}) {
    this.chain = chain;
    this.log = log;
    this.started = Date.now();
    /** provider -> [{ status, retryAfter, count, body }] */
    this.faults = new Map();
    this.stats = { calls: {}, total: 0, leaks: {}, unserved: {}, faulted: 0 };
  }

  // ─── stats and faults ─────────────────────────────────────────────────

  count(provider, route, status) {
    const p = (this.stats.calls[provider] ||= {});
    const key = `${route} ${status}`;
    p[key] = (p[key] || 0) + 1;
    this.stats.total += 1;
  }

  leak(target) {
    this.stats.leaks[target] = (this.stats.leaks[target] || 0) + 1;
  }

  /**
   * A route the binary asked that the stub does not play: 501, never a 404
   * the binary would read as a clean absent. Counted under `unserved` (the
   * harness fails on any), and under the provider's calls.
   */
  unserved(req, provider, route, done) {
    const key = `${req.method} /${provider}${generalize(route)}`;
    this.stats.unserved[key] = (this.stats.unserved[key] || 0) + 1;
    return done(501, `stub: unserved route ${req.method} /${provider}${route}`, 'text/plain');
  }

  /** The injected fault for a provider's next request, consumed; null when none. */
  takeFault(provider) {
    const queue = this.faults.get(provider);
    if (!queue || queue.length === 0) return null;
    const f = queue[0];
    f.count -= 1;
    if (f.count <= 0) queue.shift();
    this.stats.faulted += 1;
    return f;
  }

  addFault({ provider, status, retryAfter, count = 1, body }) {
    if (!PROVIDERS.includes(provider)) throw new HttpError(400, `provider must be one of ${PROVIDERS.join(', ')}`);
    status = Number(status);
    if (!Number.isInteger(status) || status < 100 || status > 599) throw new HttpError(400, 'status must be an HTTP status');
    count = Number(count);
    if (!Number.isInteger(count) || count < 1) throw new HttpError(400, 'count must be a positive integer');
    const queue = this.faults.get(provider) || [];
    queue.push({ status, retryAfter: retryAfter == null ? null : Number(retryAfter), count, body });
    this.faults.set(provider, queue);
    return this.faultsView();
  }

  faultsView() {
    const out = {};
    for (const [p, q] of this.faults) if (q.length) out[p] = q.map(f => ({ status: f.status, retryAfter: f.retryAfter, count: f.count }));
    return out;
  }

  // ─── the server ───────────────────────────────────────────────────────

  listen(port, host = '127.0.0.1') {
    const server = http.createServer((req, res) => {
      readBody(req).then(body => this.handle(req, res, body)).catch(e => {
        const status = e instanceof HttpError ? e.status : 500;
        send(res, status, { error: String(e.message || e) });
      });
    });
    // The wall: a CONNECT is a TLS tunnel to a real host. Refused, counted.
    server.on('connect', (req, socket) => {
      this.leak(`CONNECT ${req.url}`);
      this.log(`wall CONNECT ${req.url} 403`);
      socket.write(`HTTP/1.1 403 Forbidden\r\nConnection: close\r\nContent-Type: text/plain\r\n\r\nhermetic: ${req.url} is outside the stub\r\n`);
      socket.destroy();
    });
    return new Promise((resolve, reject) => {
      server.once('error', reject);
      server.listen(port, host, () => resolve(server));
    });
  }

  handle(req, res, body) {
    const started = Date.now();
    // The wall, plain HTTP: an absolute-URI request a proxy would forward.
    if (/^https?:\/\//i.test(req.url)) {
      this.leak(`${req.method} ${req.url}`);
      this.log(`wall ${req.method} ${req.url} 403`);
      return send(res, 403, { error: `hermetic: ${req.url} is outside the stub` }, 'application/json');
    }
    const url = new URL(req.url, 'http://stub');
    const [, provider, ...rest] = url.pathname.split('/');
    const route = '/' + rest.join('/');
    const done = (status, payload, type) => {
      this.count(provider, `${req.method} ${generalize(route)}`, status);
      this.log(`${provider} ${req.method} ${route} ${status} ${Date.now() - started}ms`);
      send(res, status, payload, type);
    };
    if (provider === '_stub') return this.stubRoute(req, url, route, body, done);
    if (!PROVIDERS.includes(provider)) return this.unserved(req, provider, route, done);
    const fault = this.takeFault(provider);
    if (fault) {
      if (fault.retryAfter != null) res.setHeader('Retry-After', String(fault.retryAfter));
      return done(fault.status, fault.body != null ? fault.body : { error: `injected fault ${fault.status}` }, typeof fault.body === 'string' ? 'text/plain' : 'application/json');
    }
    switch (provider) {
      case 'chaintracks': return this.chaintracks(req, url, route, done);
      case 'woc': return this.woc(req, url, route, body, done);
      case 'bitails': return this.bitails(req, url, route, body, done);
      case 'bananablocks': return this.bananablocks(req, url, route, done);
      case 'arc': return this.arc(req, route, body, done);
      default: return this.unserved(req, provider, route, done);
    }
  }

  // ─── /_stub ───────────────────────────────────────────────────────────

  stubRoute(req, url, route, body, done) {
    const m = (method, re) => req.method === method && re.exec(route);
    let mm;
    if (m('GET', /^\/stats$/)) {
      return done(200, { uptimeMs: Date.now() - this.started, chain: this.chain.summary(), calls: this.stats.calls, total: this.stats.total, faulted: this.stats.faulted, faults: this.faultsView(), leaks: this.stats.leaks, unserved: this.stats.unserved });
    }
    if (m('GET', /^\/chain$/)) return done(200, this.chain.summary());
    if (m('POST', /^\/mine$/)) {
      const args = body.length ? JSON.parse(body.toString()) : {};
      return done(200, this.chain.mine({ txids: args.txids ?? null }));
    }
    if (m('POST', /^\/fund$/)) {
      const args = JSON.parse(body.toString());
      if (!/^[0-9a-fA-F]+$/.test(args.script || '')) throw new HttpError(400, 'script: hex locking script required');
      const sats = Number(args.satoshis);
      if (!Number.isInteger(sats) || sats <= 0) throw new HttpError(400, 'satoshis: a positive integer');
      return done(200, this.chain.fund(Buffer.from(args.script, 'hex'), sats));
    }
    if (m('POST', /^\/forge$/)) {
      const args = JSON.parse(body.toString());
      if (!/^[0-9a-fA-F]+$/.test(args.script || '')) throw new HttpError(400, 'script: hex locking script required');
      const sats = Number(args.satoshis);
      if (!Number.isInteger(sats) || sats <= 0) throw new HttpError(400, 'satoshis: a positive integer');
      return done(200, this.chain.forgedFund(Buffer.from(args.script, 'hex'), sats));
    }
    if (m('POST', /^\/fault$/)) return done(200, { faults: this.addFault(JSON.parse(body.toString())) });
    if (m('DELETE', /^\/fault$/)) { this.faults.clear(); return done(200, { faults: {} }); }
    if ((mm = m('GET', /^\/beef\/([0-9a-fA-F]{64})$/))) {
      try { return done(200, this.chain.beefFor(mm[1], { atomic: true }).toString('hex'), 'text/plain'); } catch (e) { return done(404, { error: e.message }); }
    }
    if ((mm = m('GET', /^\/tx\/([0-9a-fA-F]{64})$/))) {
      const e = this.chain.get(mm[1]);
      if (!e) return done(404, { error: 'unknown transaction' });
      return done(200, { txid: e.tx.txid, blockHash: e.blockHash, height: e.height, index: e.index, inputs: e.tx.inputs.map(i => ({ txid: i.txid, vout: i.vout })), outputs: e.tx.outputs.map((o, vout) => ({ vout, satoshis: o.satoshis, script: o.script.toString('hex'), spentBy: this.chain.spenderOf(e.tx.txid, vout) })) });
    }
    return done(404, { error: `no such stub route: ${req.method} ${route}` });
  }

  // ─── /chaintracks ─────────────────────────────────────────────────────

  chaintracks(req, url, route, done) {
    const frame = (value) => ({ status: 'success', value });
    const header = (h) => ({ version: h.version, previousHash: h.previousHash, merkleRoot: h.merkleRoot, time: h.time, bits: h.bits, nonce: h.nonce, height: h.height, hash: h.hash });
    if (req.method !== 'GET') return this.unserved(req, 'chaintracks', route, done);
    if (route === '/getPresentHeight') return done(200, frame(this.chain.height()));
    if (route === '/findChainTipHeaderHex') return done(200, frame(header(this.chain.tip())));
    if (route === '/findHeaderHexForHeight') {
      const height = Number(url.searchParams.get('height'));
      const h = this.chain.headerAt(height);
      return h ? done(200, frame(header(h))) : done(404, { status: 'error', value: null, error: `no header at height ${height}` });
    }
    if (route === '/findHeaderHexForBlockHash') {
      const h = this.chain.headerByHash(url.searchParams.get('hash') || '');
      return h ? done(200, frame(header(h))) : done(404, { status: 'error', value: null, error: 'no header with that hash' });
    }
    return this.unserved(req, 'chaintracks', route, done);
  }

  // ─── /woc (WhatsOnChain's shape) ──────────────────────────────────────

  woc(req, url, route, body, done) {
    const m = (method, re) => req.method === method && re.exec(route);
    let mm;
    if ((mm = m('GET', /^\/tx\/(?:hash\/)?([0-9a-fA-F]{64})$/))) {
      const e = this.chain.get(mm[1]);
      return e ? done(200, this.wocTx(e)) : done(404, 'Not Found', 'text/plain');
    }
    if ((mm = m('GET', /^\/tx\/([0-9a-fA-F]{64})\/hex$/))) {
      const e = this.chain.get(mm[1]);
      return e ? done(200, e.tx.raw.toString('hex'), 'text/plain') : done(404, 'Not Found', 'text/plain');
    }
    if ((mm = m('GET', /^\/tx\/([0-9a-fA-F]{64})\/beef$/))) {
      const e = this.chain.get(mm[1]);
      if (!e) return done(404, 'Not Found', 'text/plain');
      try { return done(200, this.chain.beefFor(mm[1]).toString('hex'), 'text/plain'); } catch (err) { return done(500, err.message, 'text/plain'); }
    }
    if ((mm = m('GET', /^\/tx\/([0-9a-fA-F]{64})\/proof\/tsc$/))) {
      const e = this.chain.get(mm[1]);
      if (!e) return done(404, 'Not Found', 'text/plain');
      const tsc = this.chain.tscFor(mm[1]);
      if (!tsc) return done(200, '', 'text/plain'); // unmined: no proof yet
      return done(200, [{ index: tsc.index, txOrId: tsc.txid, target: tsc.blockHash, targetType: 'hash', nodes: tsc.nodes }]);
    }
    if ((mm = m('GET', /^\/tx\/([0-9a-fA-F]{64})\/(\d+)\/spent$/))) {
      const spender = this.chain.spenderOf(mm[1], Number(mm[2]));
      if (!spender) return done(404, 'Not Found', 'text/plain');
      const s = this.chain.get(spender);
      const vin = s.tx.inputs.findIndex(i => i.txid === mm[1].toLowerCase() && i.vout === Number(mm[2]));
      return done(200, { txid: spender, vin });
    }
    if (m('POST', /^\/tx\/raw$/)) {
      let txhex;
      try { txhex = JSON.parse(body.toString()).txhex; } catch { return done(400, 'bad json', 'text/plain'); }
      const answer = this.broadcast(Buffer.from(String(txhex || ''), 'hex'));
      if (answer.status === 'malformed') return done(400, `400: ${answer.reason}`, 'text/plain');
      if (answer.status === 'double_spend') return done(400, '258: txn-mempool-conflict', 'text/plain');
      return done(200, JSON.stringify(answer.txid), 'application/json');
    }
    if (m('POST', /^\/txs\/status$/)) {
      let txids;
      try { txids = JSON.parse(body.toString()).txids || []; } catch { return done(400, 'bad json', 'text/plain'); }
      return done(200, txids.map(t => {
        const e = this.chain.get(t);
        if (!e) return { txid: t, error: 'unknown' };
        if (!e.blockHash) return { txid: t, blockhash: null, blockheight: null, blocktime: null, confirmations: null };
        return { txid: t, blockhash: e.blockHash, blockheight: e.height, blocktime: this.chain.headerByHash(e.blockHash).time, confirmations: this.chain.confirmations(e.height) };
      }));
    }
    if ((mm = m('GET', /^\/script\/([0-9a-fA-F]{64})\/unspent\/all$/))) {
      const unspent = this.chain.outputsOfScriptHash(mm[1]).filter(o => !o.spentBy);
      return done(200, { script: mm[1], result: unspent.map(o => ({ height: o.height || 0, tx_pos: o.vout, tx_hash: o.txid, value: o.satoshis, isSpentInMempoolTx: false, status: o.height ? 'confirmed' : 'unconfirmed' })) });
    }
    if ((mm = m('GET', /^\/script\/([0-9a-fA-F]{64})\/(confirmed|unconfirmed)\/history$/))) {
      const wantMined = mm[2] === 'confirmed';
      const items = this.chain.historyOfScriptHash(mm[1]).filter(h => wantMined ? h.height != null : h.height == null);
      return done(200, { script: mm[1], result: items.map(h => ({ tx_hash: h.txid, height: h.height })) });
    }
    if ((mm = m('GET', /^\/block\/([0-9a-fA-F]{64})\/header$/))) {
      const h = this.chain.headerByHash(mm[1]);
      if (!h) return done(404, 'Not Found', 'text/plain');
      const block = this.chain.blocks.get(h.hash);
      const next = this.chain.headerAt(h.height + 1);
      return done(200, { hash: h.hash, size: null, height: h.height, version: h.version, versionHex: h.version.toString(16).padStart(8, '0'), merkleroot: h.merkleRoot, time: h.time, mediantime: h.time, nonce: h.nonce, bits: h.bits.toString(16), difficulty: null, chainwork: null, previousblockhash: h.previousHash, nextblockhash: next ? next.hash : null, confirmations: this.chain.confirmations(h.height), txcount: block ? block.txids.length : null });
    }
    if (m('GET', /^\/exchangerate$/)) return done(200, { currency: 'USD', rate: 50.0, time: Math.floor(Date.now() / 1000) });
    return this.unserved(req, 'woc', route, done);
  }

  wocTx(e) {
    const mined = !!e.blockHash;
    return {
      txid: e.tx.txid, hash: e.tx.txid, version: e.tx.version, size: e.tx.raw.length, locktime: e.tx.locktime,
      vin: e.tx.inputs.map((i, n) => ({ txid: i.txid, vout: i.vout, scriptSig: { hex: i.script.toString('hex') }, sequence: i.sequence, n })),
      vout: e.tx.outputs.map((o, n) => ({ value: o.satoshis / 1e8, n, scriptPubKey: { hex: o.script.toString('hex') } })),
      blockhash: mined ? e.blockHash : '', blockheight: mined ? e.height : 0,
      confirmations: mined ? this.chain.confirmations(e.height) : 0,
      time: Math.floor(e.seen / 1000), blocktime: mined ? this.chain.headerByHash(e.blockHash).time : 0,
    };
  }

  // ─── /bitails ─────────────────────────────────────────────────────────

  bitails(req, url, route, body, done) {
    const m = (method, re) => req.method === method && re.exec(route);
    let mm;
    if ((mm = m('GET', /^\/tx\/([0-9a-fA-F]{64})$/))) {
      const e = this.chain.get(mm[1]);
      if (!e) return done(404, { error: 'not found' });
      return done(200, { txid: e.tx.txid, blockHash: e.blockHash, blockHeight: e.height, size: e.tx.raw.length, inputsCount: e.tx.inputs.length, outputsCount: e.tx.outputs.length });
    }
    if ((mm = m('GET', /^\/tx\/([0-9a-fA-F]{64})\/hex$/))) {
      const e = this.chain.get(mm[1]);
      return e ? done(200, e.tx.raw.toString('hex'), 'text/plain') : done(404, { error: 'not found' });
    }
    if ((mm = m('GET', /^\/tx\/([0-9a-fA-F]{64})\/proof\/tsc$/))) {
      const tsc = this.chain.tscFor(mm[1]);
      if (!tsc) return done(404, { error: 'no proof' });
      return done(200, { index: tsc.index, txOrId: tsc.txid, target: tsc.blockHash, nodes: tsc.nodes });
    }
    if (m('POST', /^\/tx\/broadcast\/multi$/)) {
      let raws;
      try { raws = JSON.parse(body.toString()).raws || []; } catch { return done(400, { error: 'bad json' }); }
      return done(200, raws.map(hex => {
        const answer = this.broadcast(Buffer.from(String(hex), 'hex'));
        if (answer.status === 'malformed') return { error: { code: 'MALFORMED', message: answer.reason } };
        if (answer.status === 'double_spend') return { txid: answer.txid, error: { code: 'DOUBLE_SPEND', message: `competes with ${answer.competing.join(',')}` } };
        return { txid: answer.txid };
      }));
    }
    if ((mm = m('GET', /^\/block\/([0-9a-fA-F]{64})$/))) {
      const h = this.chain.headerByHash(mm[1]);
      return h ? done(200, { hash: h.hash, height: h.height, header: T.headerBytes(h).toString('hex') }) : done(404, { error: 'not found' });
    }
    if ((mm = m('GET', /^\/scripthash\/([0-9a-fA-F]{64})\/unspent$/))) {
      const unspent = this.chain.outputsOfScriptHash(mm[1]).filter(o => !o.spentBy);
      const limit = Number(url.searchParams.get('limit') || unspent.length) || unspent.length;
      const from = Number(url.searchParams.get('from') || 0) || 0;
      return done(200, { unspent: unspent.slice(from, from + limit).map(o => ({ txid: o.txid, vout: o.vout, satoshis: o.satoshis, blockheight: o.height })) });
    }
    if ((mm = m('GET', /^\/address\/scripthash\/([0-9a-fA-F]{64})\/history$/))) {
      return done(200, this.chain.historyOfScriptHash(mm[1]).map(h => ({ txid: h.txid, height: h.height })));
    }
    return this.unserved(req, 'bitails', route, done);
  }

  // ─── /bananablocks ────────────────────────────────────────────────────

  bananablocks(req, url, route, done) {
    const m = (method, re) => req.method === method && re.exec(route);
    let mm;
    if ((mm = m('GET', /^\/tx\/([0-9a-fA-F]{64})$/))) {
      const e = this.chain.get(mm[1]);
      if (!e) return done(404, { error: 'not found' });
      return done(200, { txid: e.tx.txid, block_hash: e.blockHash, block_height: e.height, size: e.tx.raw.length });
    }
    if ((mm = m('GET', /^\/tx\/([0-9a-fA-F]{64})\/hex$/))) {
      const e = this.chain.get(mm[1]);
      return e ? done(200, e.tx.raw.toString('hex'), 'text/plain') : done(404, { error: 'not found' });
    }
    if ((mm = m('GET', /^\/tx\/([0-9a-fA-F]{64})\/proof$/))) {
      const tsc = this.chain.tscFor(mm[1]);
      if (!tsc) return done(404, { error: 'no proof' });
      return done(200, { index: tsc.index, txOrId: tsc.txid, targetType: 'merkleRoot', target: tsc.merkleRoot, nodes: tsc.nodes });
    }
    if ((mm = m('GET', /^\/txo\/([0-9a-fA-F]{64})\/(\d+)$/))) {
      const e = this.chain.get(mm[1]);
      const vout = Number(mm[2]);
      if (!e || vout >= e.tx.outputs.length) return done(404, { error: 'not found' });
      const o = e.tx.outputs[vout];
      const spender = this.chain.spenderOf(mm[1], vout);
      const txo = { txid: e.tx.txid, index: vout, value: o.satoshis, script_pubkey: o.script.toString('hex'), spent: !!spender };
      if (spender) { txo.spent_txid = spender; txo.spend_scope = this.chain.get(spender).blockHash ? 'block' : 'mempool'; }
      return done(200, txo);
    }
    if ((mm = m('GET', /^\/address\/([1-9A-HJ-NP-Za-km-z]+)\/utxos$/))) {
      const h160 = T.addressHash160(mm[1]);
      if (!h160) return done(400, { error: 'not an address' });
      const unspent = this.chain.outputsOfHash160(h160).filter(o => !o.spentBy);
      return done(200, { items: unspent.map(o => ({ txid: o.txid, vout: o.vout, value: o.satoshis, block_height: o.height, script_type: 'p2pkh' })), hasMore: false });
    }
    return this.unserved(req, 'bananablocks', route, done);
  }

  // ─── /arc (classic ARC) ───────────────────────────────────────────────

  arc(req, route, body, done) {
    const m = (method, re) => req.method === method && re.exec(route);
    let mm;
    if (m('POST', /^\/v1\/tx$/)) {
      const bytes = arcBodyBytes(req, body);
      if (!bytes) return done(400, arcError(400, 'Bad Request', 'the body is neither bytes nor {"rawTx": hex}'));
      const answer = this.broadcast(bytes);
      // Bytes ARC cannot read are its HTTP 400 (a fault of the poster, transient for the transaction).
      if (answer.status === 'malformed') return done(400, arcError(400, 'Bad Request', answer.reason));
      return done(200, this.arcAnswer(answer));
    }
    if (m('POST', /^\/v1\/txs$/)) {
      let items;
      try { items = JSON.parse(body.toString()); } catch { return done(400, arcError(400, 'Bad Request', 'bad json')); }
      if (!Array.isArray(items)) return done(400, arcError(400, 'Bad Request', 'a JSON array of {rawTx} is expected'));
      return done(200, items.map(item => {
        const hex = item && item.rawTx;
        if (!/^[0-9a-fA-F]+$/.test(String(hex || ''))) return arcError(400, 'Bad Request', 'rawTx is not hex');
        return this.arcAnswer(this.broadcast(Buffer.from(hex, 'hex')));
      }));
    }
    if ((mm = m('GET', /^\/v1\/tx\/([0-9a-fA-F]{64})$/))) {
      const e = this.chain.get(mm[1]);
      if (!e) return done(404, arcError(404, 'Not Found', 'transaction not found'));
      return done(200, this.arcStatus(e));
    }
    if (m('GET', /^\/v1\/policy$/)) return done(200, { policy: { maxscriptsizepolicy: 500000, maxtxsigopscountspolicy: 4294967295, maxtxsizepolicy: 10000000, miningFee: { satoshis: 1, bytes: 1000 } }, timestamp: new Date().toISOString() });
    return this.unserved(req, 'arc', route, done);
  }

  arcAnswer(answer) {
    const base = { txid: answer.txid, timestamp: new Date().toISOString(), extraInfo: '', merklePath: '', blockHash: '', blockHeight: 0, status: 200, title: 'OK' };
    switch (answer.status) {
      case 'malformed': return arcError(400, 'Bad Request', answer.reason);
      case 'double_spend': return { ...base, txStatus: 'DOUBLE_SPEND_ATTEMPTED', competingTxs: answer.competing };
      case 'known': return this.arcStatus(answer.entry);
      default: return { ...base, txStatus: 'SEEN_ON_NETWORK' };
    }
  }

  arcStatus(e) {
    const base = { txid: e.tx.txid, timestamp: new Date(e.seen).toISOString(), extraInfo: '', status: 200, title: 'OK' };
    if (!e.blockHash) return { ...base, txStatus: 'SEEN_ON_NETWORK', blockHash: '', blockHeight: 0, merklePath: '' };
    return { ...base, txStatus: 'MINED', blockHash: e.blockHash, blockHeight: e.height, merklePath: this.chain.bumpFor(e.tx.txid).toString('hex') };
  }

  /** One broadcast of bytes (BEEF, EF or raw): the chain's answer, or `malformed`. */
  broadcast(bytes) {
    try {
      if (T.classifyBytes(bytes) === 'beef') return this.chain.addBeef(T.parseBeef(bytes));
      return this.chain.addTx(T.parseTx(bytes));
    } catch (e) {
      return { status: 'malformed', txid: null, reason: e.message };
    }
  }
}

// ─── helpers ──────────────────────────────────────────────────────────────

class HttpError extends Error { constructor(status, message) { super(message); this.status = status; } }

function arcError(status, title, detail) {
  return { type: `https://arc.bitcoinsv.com/errors/${status}`, title, status, detail, instance: null, txid: null, extraInfo: detail };
}

function arcBodyBytes(req, body) {
  const type = String(req.headers['content-type'] || '').toLowerCase();
  if (type.includes('json')) {
    try {
      const hex = JSON.parse(body.toString()).rawTx;
      return /^[0-9a-fA-F]+$/.test(String(hex || '')) ? Buffer.from(hex, 'hex') : null;
    } catch { return null; }
  }
  if (type.includes('text/plain')) {
    const hex = body.toString().trim();
    return /^[0-9a-fA-F]+$/.test(hex) ? Buffer.from(hex, 'hex') : null;
  }
  return body.length ? body : null;
}

function readBody(req) {
  return new Promise((resolve, reject) => {
    const chunks = [];
    req.on('data', c => chunks.push(c));
    req.on('end', () => resolve(Buffer.concat(chunks)));
    req.on('error', reject);
  });
}

function send(res, status, payload, type) {
  let bodyOut;
  if (Buffer.isBuffer(payload)) { bodyOut = payload; type = type || 'application/octet-stream'; }
  else if (typeof payload === 'string') { bodyOut = Buffer.from(payload); type = type || 'text/plain'; }
  else { bodyOut = Buffer.from(JSON.stringify(payload)); type = type || 'application/json'; }
  res.writeHead(status, { 'Content-Type': type, 'Content-Length': bodyOut.length });
  res.end(bodyOut);
}

/** A route with its ids replaced, for the stats (one line per route, not per txid). */
function generalize(route) {
  return route.replace(/[0-9a-fA-F]{64}/g, '{id}').replace(/\/\d+(?=\/|$)/g, '/{n}').replace(/\/[1-9A-HJ-NP-Za-km-z]{25,35}(?=\/|$)/g, '/{addr}');
}

/** The base URLs the binary and the auditor are pointed at, for a stub on `port`. */
function bases(port, host = '127.0.0.1') {
  const root = `http://${host}:${port}`;
  return { root, chaintracks: `${root}/chaintracks`, woc: `${root}/woc`, bitails: `${root}/bitails`, bananablocks: `${root}/bananablocks`, arc: `${root}/arc`, stub: `${root}/_stub` };
}

async function main() {
  const args = process.argv.slice(2);
  const port = Number(args.includes('--port') ? args[args.indexOf('--port') + 1] : process.env.E2E_STUB_PORT || 3380);
  const quiet = args.includes('--quiet');
  const stub = new Stub({ log: quiet ? () => {} : (line) => console.log(`${new Date().toISOString()} ${line}`) });
  const server = await stub.listen(port);
  const b = bases(port);
  console.log(`hermetic stub on ${b.root} (chain tip ${stub.chain.height()}, ${stub.chain.headers.length} recorded headers)`);
  console.log(`  CHAINTRACKS_URL=${b.chaintracks} WOC_BASE=${b.woc} BITAILS_BASE=${b.bitails} BANANABLOCKS_BASE=${b.bananablocks} ARC_URL=${b.arc}`);
  console.log(`  HTTPS_PROXY=${b.root} HTTP_PROXY=${b.root} NO_PROXY=127.0.0.1,localhost`);
  const stop = () => { server.close(); process.exit(0); };
  process.on('SIGTERM', stop);
  process.on('SIGINT', stop);
}

module.exports = { Stub, bases, PROVIDERS, generalize };

if (require.main === module) {
  main().catch(e => { console.error(e); process.exit(1); });
}
