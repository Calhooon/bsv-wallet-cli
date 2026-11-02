/**
 * BRC-100 Wallet HTTP Client
 * Wraps all 28 endpoints with proper Origin header and JSON handling.
 * Under DIFF_RECORD every call is recorded here (./record.js).
 */
const record = require('./record');

/** The harness's origin: every wallet it starts admits it (#411). */
const E2E_ORIGIN = 'http://e2e-test';

class WalletClient {
  /**
   * `allowPending: 'received'` names the spend gate's word R1 (d) on every
   * createAction this client makes: a received coin whose creator is not yet
   * mined (its ancestry proven at internalize) is offered for that action and
   * the action is recorded under the rule. A test wallet is funded by the
   * funder's just-broadcast payment and spends in the same block, so its
   * spends carry the word; the funder's own client never does (its coins are
   * mined or its own change). The scoped form, `allowPending: { received:
   * ['txid:vout', ...] }` (0.18.0, #63), offers only the named received coins
   * under the word; a call passes it in `opts.allowPending` (scenario 23).
   * Every call sends `Origin` (the served doors refuse a call without one,
   * #411): `E2E_ORIGIN` by default, which every wallet the harness starts
   * lists in its `ALLOWED_ORIGINS`; the browser gate's scenes name others.
   */
  constructor(port, name = `wallet:${port}`, { allowPending, origin = E2E_ORIGIN } = {}) {
    this.base = `http://localhost:${port}`;
    this.name = name;
    this.origin = origin;
    this.allowPending = allowPending;
  }

  async post(endpoint, body = {}) {
    return record.serial(() => this.postNow(endpoint, body));
  }

  async postNow(endpoint, body) {
    const done = record.httpStart(this.name, 'POST', `/${endpoint}`, body);
    let resp, text;
    try {
      resp = await fetch(`${this.base}/${endpoint}`, {
        method: 'POST',
        headers: { 'Origin': this.origin, 'Content-Type': 'application/json' },
        body: JSON.stringify(body),
        signal: AbortSignal.timeout(180_000), // 3 min — broadcast can retry across multiple providers
      });
      text = await resp.text();
    } catch (e) { done(resp ? resp.status : null, null, e); throw e; }
    this.lastStatus = resp.status;
    done(resp.status, text);
    let data = {};
    try { data = text ? JSON.parse(text) : {}; } catch { data = { _raw: text }; }
    if (!resp.ok) {
      throw this.responseError(endpoint, resp, data, text);
    }
    return data;
  }

  async get(endpoint) {
    return record.serial(() => this.getNow(endpoint));
  }

  async getNow(endpoint) {
    const done = record.httpStart(this.name, 'GET', `/${endpoint}`);
    let resp, text;
    try {
      resp = await fetch(`${this.base}/${endpoint}`, {
        headers: { 'Origin': this.origin },
      });
      text = await resp.text();
    } catch (e) { done(resp ? resp.status : null, null, e); throw e; }
    done(resp.status, text);
    try { return text ? JSON.parse(text) : {}; } catch { return { _raw: text }; }
  }

  // Scenario 24 reads the HTTP response without changing the shared GET contract.
  async getResponse(endpoint) {
    return record.serial(async () => {
      const done = record.httpStart(this.name, 'GET', `/${endpoint}`);
      let resp, text;
      try {
        resp = await fetch(`${this.base}/${endpoint}`, {
          headers: { 'Origin': this.origin },
        });
        text = await resp.text();
      } catch (e) { done(resp ? resp.status : null, null, e); throw e; }
      done(resp.status, text);
      let body;
      try { body = text ? JSON.parse(text) : {}; } catch { body = { _raw: text }; }
      return { status: resp.status, retryAfter: resp.headers.get('retry-after'), body };
    });
  }

  responseError(endpoint, resp, data, text) {
    const msg = data.message || data.error || data._raw || text || `HTTP ${resp.status}`;
    // Keep the POST refusal and its retry header for scenario 24.
    const err = new Error(`${this.name} /${endpoint} ${resp.status}: ${msg}`);
    err.status = resp.status;
    err.body = data;
    err.retryAfter = resp.headers.get('retry-after');
    return err;
  }

  // --- Status ---
  async isAuthenticated() { return this.get('isAuthenticated'); }
  async getHeight() { return this.get('getHeight'); }
  async getNetwork() { return this.get('getNetwork'); }
  async getVersion() { return this.get('getVersion'); }

  // --- Keys & Crypto ---
  async getPublicKey(protocolId, keyId, counterparty = 'self', forSelf = false) {
    return this.post('getPublicKey', { protocolId, keyId, counterparty, forSelf });
  }

  async identityKey() {
    const r = await this.post('getPublicKey', { identityKey: true });
    return r.publicKey;
  }

  async createSignature(data, protocolId, keyId, counterparty = 'self') {
    return this.post('createSignature', { data: Array.from(data), protocolId, keyId, counterparty });
  }

  async verifySignature(data, signature, protocolId, keyId, counterparty = 'self', forSelf = false) {
    return this.post('verifySignature', {
      data: Array.from(data), signature: Array.from(signature),
      protocolId, keyId, counterparty, forSelf,
    });
  }

  async encrypt(plaintext, protocolId, keyId, counterparty = 'self') {
    const data = typeof plaintext === 'string' ? Array.from(Buffer.from(plaintext)) : Array.from(plaintext);
    return this.post('encrypt', { plaintext: data, protocolId, keyId, counterparty });
  }

  async decrypt(ciphertext, protocolId, keyId, counterparty = 'self') {
    return this.post('decrypt', { ciphertext: Array.from(ciphertext), protocolId, keyId, counterparty });
  }

  // --- Transactions ---
  async createAction(outputs, description, opts = {}) {
    const allowPending = opts.allowPending ?? this.allowPending;
    return this.post('createAction', {
      outputs,
      description,
      ...(allowPending ? { allowPending } : {}),
      options: {
        signAndProcess: opts.signAndProcess ?? true,
        acceptDelayedBroadcast: opts.acceptDelayedBroadcast ?? false,
        randomizeOutputs: opts.randomizeOutputs ?? false,
        noSend: opts.noSend ?? false,
      },
      labels: opts.labels,
    });
  }

  async signAction(reference, spends = {}) {
    return this.post('signAction', { reference, spends });
  }

  async abortAction(reference) {
    return this.post('abortAction', { reference });
  }

  async internalizeAction(tx, outputs, description) {
    return this.post('internalizeAction', { tx, outputs, description });
  }

  async listOutputs(basket = 'default', opts = {}) {
    return this.post('listOutputs', { basket, ...opts });
  }

  async listActions(opts = {}) {
    return this.post('listActions', { labels: [], ...opts });
  }

  // BRC-100 relinquishOutput takes { basket, output } with the output an
  // outpoint string "txid.vout" (bsv-rs RelinquishOutputArgs, #119).
  async relinquishOutput(txid, vout, basket) {
    return this.post('relinquishOutput', { basket, output: `${txid}.${vout}` });
  }

  // --- Certificates ---
  async acquireCertificate(certificateType, certifier, fields, opts = {}) {
    return this.post('acquireCertificate', {
      type: certificateType,
      certifier,
      acquisitionProtocol: 'direct',
      fields,
      serialNumber: opts.serialNumber || 'AQID',
      revocationOutpoint: opts.revocationOutpoint || 'deadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeef.0',
      signature: opts.signature || 'deadbeef',
      keyringRevealer: opts.keyringRevealer || 'certifier',
      keyringForSubject: opts.keyringForSubject || opts.masterKeyring || {},
    });
  }

  async listCertificates(certifiers = [], types = []) {
    return this.post('listCertificates', { certifiers, types });
  }

  async proveCertificate(certificate, verifier, fieldsToReveal) {
    return this.post('proveCertificate', { certificate, fieldsToReveal, verifier });
  }

  async relinquishCertificate(certificateType, serialNumber, certifier) {
    return this.post('relinquishCertificate', { type: certificateType, serialNumber, certifier });
  }

  // --- Discovery ---
  async discoverByIdentityKey(identityKey, limit = 10, offset = 0) {
    return this.post('discoverByIdentityKey', { identityKey, limit, offset });
  }

  async discoverByAttributes(attributes, limit = 10, offset = 0) {
    return this.post('discoverByAttributes', { attributes, limit, offset });
  }

  // --- High-level helpers ---

  /** Get a basket's spendable balance in satoshis. */
  async balance(basket = 'default') {
    const r = await this.listOutputs(basket, { limit: 10000, includeEnvelope: false });
    const outputs = r.outputs || [];
    if (Array.isArray(outputs)) {
      return outputs
        .filter(o => o.spendable !== false)
        .reduce((sum, o) => sum + (o.satoshis || 0), 0);
    }
    return 0;
  }

  /**
   * Send sats to another wallet's BRC-29 address via HTTP API.
   * Returns { txid, beef } where beef is the AtomicBEEF byte array
   * that the recipient can pass directly to internalizeAction.
   */
  async sendTo(lockingScript, satoshis, description = 'e2e transfer') {
    return this.createAction([{
      lockingScript,
      satoshis,
      outputDescription: description,
      tags: ['e2e-test'],
    }], description);
  }

  /**
   * Direct wallet-to-wallet transfer via HTTP API.
   * A sends sats to B. B internalizes the AtomicBEEF directly. No WoC needed.
   * Returns { txid, satsSent }.
   */
  async transferTo(recipientWallet, satoshis, description = 'e2e transfer', opts = {}) {
    // Get recipient's BRC-29 derived address
    const bKey = await recipientWallet.getPublicKey(
      [2, '3241645161d8'],
      'SfKxPIJNgdI= NaGLC6fMH50=',
      ANYONE_KEY, true,
    );
    const lockingScript = buildP2PKH(bKey.publicKey);

    // Create action (sends + broadcasts)
    const result = await this.createAction([{
      lockingScript,
      satoshis,
      outputDescription: description,
      tags: ['e2e-test'],
    }, ...(opts.additionalOutputs || [])], description, opts);

    // Find which vout matches the recipient's script
    const vout = findVoutByScript(result.tx, lockingScript, satoshis);

    // Recipient internalizes directly from AtomicBEEF
    await recipientWallet.internalizeAction(
      result.tx,  // AtomicBEEF bytes from createAction response
      [{
        outputIndex: vout,
        protocol: 'wallet payment',
        paymentRemittance: {
          derivationPrefix: 'SfKxPIJNgdI=',
          derivationSuffix: 'NaGLC6fMH50=',
          senderIdentityKey: ANYONE_KEY,
        },
      }],
      `Received ${satoshis} sats`,
    );

    return { txid: result.txid, satsSent: satoshis };
  }
}

// secp256k1 generator point G (anyone can derive, only wallet owner can spend)
const ANYONE_KEY = '0279be667ef9dcbbac55a06295ce870b07029bfcdb2dce28d959f2815b16f81798';

/** Build P2PKH locking script from compressed public key hex. */
function buildP2PKH(pubkeyHex) {
  const pubkey = Buffer.from(pubkeyHex, 'hex');
  const { createHash } = require('crypto');
  const sha = createHash('sha256').update(pubkey).digest();
  const hash160 = createHash('ripemd160').update(sha).digest();
  // OP_DUP OP_HASH160 <20 bytes> OP_EQUALVERIFY OP_CHECKSIG
  return '76a914' + hash160.toString('hex') + '88ac';
}

/**
 * Find the vout index matching a given locking script in AtomicBEEF.
 * Falls back to 0 if we can't parse the BEEF (the common case for simple sends).
 */
function findVoutByScript(txBytes, expectedScript, expectedSats) {
  // For now, use vout 0 as default — the wallet creates our output first
  // when randomizeOutputs is false.
  // TODO: parse AtomicBEEF to find exact vout by script match.
  return 0;
}

module.exports = { WalletClient, ANYONE_KEY, buildP2PKH, findVoutByScript, E2E_ORIGIN };
