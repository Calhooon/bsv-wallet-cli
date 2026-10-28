/**
 * E2E.21: MCP server parity.
 * Tests the MCP server via stdin/stdout JSON-RPC 2.0.
 * Verifies tool listing, compares tool results with HTTP API.
 * Cost: 0 sats (read-only queries + crypto round-trips).
 */

const { spawn } = require('child_process');
const path = require('path');
const { sleep } = require('../lib/woc');

// The MCP binary: the repository's target by default; BSV_WALLET_MCP_BIN names
// another (a shared CARGO_TARGET_DIR), as BSV_WALLET_BIN does for the wallet.
const MCP_BIN = process.env.BSV_WALLET_MCP_BIN || path.resolve(__dirname, '../../../target/release/bsv-wallet-mcp');

module.exports = {
  name: 'mcp-parity',
  description: 'MCP server tools match HTTP API (0 sats)',

  async run(walletA, walletB, assert) {
    // ─── Spawn MCP server connected to wallet A ───
    const mcp = new McpClient(walletA.port);
    await mcp.start();

    try {
      // ─── Initialize MCP session ───
      const initResult = await mcp.request('initialize', {
        protocolVersion: '2024-11-05',
        capabilities: {},
        clientInfo: { name: 'e2e-test', version: '1.0' },
      });
      assert(initResult.protocolVersion, 'MCP initialize must return protocolVersion');
      assert(initResult.capabilities, 'MCP initialize must return capabilities');
      assert(initResult.serverInfo, 'MCP initialize must return serverInfo');

      // Send initialized notification
      mcp.notify('notifications/initialized');
      await sleep(200);

      // ─── tools/list — verify all 34 tools (29 wallet, five host-route) ───
      const toolsResult = await mcp.request('tools/list', {});
      const tools = toolsResult.tools || [];
      assert(tools.length === 34,
        `MCP should expose 34 tools, got ${tools.length}`);

      const toolNames = tools.map(t => t.name).sort();
      const expectedTools = [
        'abort_action', 'acquire_certificate', 'create_action', 'create_hmac',
        'create_signature', 'decrypt', 'discover_by_attributes',
        'discover_by_identity_key', 'encrypt', 'get_header_for_height',
        'get_height', 'get_network', 'get_public_key', 'get_version',
        'internalize_action', 'is_authenticated', 'list_actions',
        'list_certificates', 'list_outputs', 'prove_certificate',
        'relinquish_certificate', 'relinquish_output',
        'reveal_counterparty_key_linkage', 'reveal_specific_key_linkage',
        'sign_action', 'verify_hmac', 'verify_signature',
        'wait_for_authentication', 'wallet_balance',
        'chain_feed_status', 'tx_word', 'health', 'pull', 'register',
      ].sort();

      for (const expected of expectedTools) {
        assert(toolNames.includes(expected),
          `MCP should have tool '${expected}', missing from: ${toolNames.join(', ')}`);
      }

      // ─── is_authenticated — compare with HTTP ───
      const mcpAuth = await mcp.callTool('is_authenticated', {});
      const httpAuth = await walletA.client.isAuthenticated();
      assert(mcpAuth.authenticated !== undefined,
        'MCP is_authenticated must return authenticated field');
      assert(mcpAuth.authenticated === httpAuth.authenticated,
        `MCP auth (${mcpAuth.authenticated}) must match HTTP (${httpAuth.authenticated})`);

      // ─── wallet_balance — compare with HTTP ───
      const mcpBalance = await mcp.callTool('wallet_balance', {});
      const httpBalance = await walletA.client.balance();
      assert(mcpBalance.satoshis !== undefined,
        'MCP wallet_balance must return satoshis');
      assert(mcpBalance.satoshis === httpBalance,
        `MCP balance (${mcpBalance.satoshis}) must match HTTP (${httpBalance})`);

      // ─── get_public_key (identity) — compare with HTTP ───
      const mcpKey = await mcp.callTool('get_public_key', { identityKey: true });
      const httpKey = await walletA.client.identityKey();
      assert(mcpKey.publicKey === httpKey,
        `MCP identity key must match HTTP key`);

      // ─── get_public_key (BRC-42 derived) — compare with HTTP ───
      const proto = [2, '3241645161d8'];
      const keyId = 'test-key-1';
      const mcpDerived = await mcp.callTool('get_public_key', {
        protocolID: proto, keyID: keyId, counterparty: 'self', forSelf: true,
      });
      const httpDerived = await walletA.client.getPublicKey(proto, keyId, 'self', true);
      assert(mcpDerived.publicKey === httpDerived.publicKey,
        'MCP derived key must match HTTP derived key');

      // ─── create_signature + verify_signature round-trip ───
      const testData = Array.from(Buffer.from('MCP e2e test message'));
      const sigProto = [2, 'e2e sig test'];
      const sigKeyId = 'sig key 1';

      const mcpSig = await mcp.callTool('create_signature', {
        data: testData, protocolID: sigProto, keyID: sigKeyId, counterparty: 'self',
      });
      assert(mcpSig.signature, 'MCP create_signature must return signature');

      const mcpVerify = await mcp.callTool('verify_signature', {
        data: testData,
        signature: mcpSig.signature,
        protocolID: sigProto,
        keyID: sigKeyId,
        counterparty: 'self',
        forSelf: true,
      });
      assert(mcpVerify.valid === true,
        `MCP verify_signature should be valid, got ${JSON.stringify(mcpVerify)}`);

      // ─── encrypt + decrypt round-trip ───
      const plaintext = Array.from(Buffer.from('Hello from MCP e2e'));
      const encProto = [2, 'e2e enc test'];
      const encKeyId = 'enc key 1';

      const mcpEnc = await mcp.callTool('encrypt', {
        plaintext, protocolID: encProto, keyID: encKeyId, counterparty: 'self',
      });
      assert(mcpEnc.ciphertext, 'MCP encrypt must return ciphertext');

      const mcpDec = await mcp.callTool('decrypt', {
        ciphertext: mcpEnc.ciphertext,
        protocolID: encProto,
        keyID: encKeyId,
        counterparty: 'self',
      });
      assert(mcpDec.plaintext, 'MCP decrypt must return plaintext');
      const decryptedText = Buffer.from(mcpDec.plaintext).toString();
      assert(decryptedText === 'Hello from MCP e2e',
        `MCP decrypt round-trip failed: got '${decryptedText}'`);

      // ─── list_outputs — compare with HTTP ───
      const mcpOutputs = await mcp.callTool('list_outputs', { basket: 'default' });
      const httpOutputs = await walletA.client.listOutputs('default', { limit: 10000 });
      const mcpTotal = mcpOutputs.totalOutputs || mcpOutputs.total_outputs || 0;
      const httpTotal = httpOutputs.totalOutputs || httpOutputs.total_outputs || 0;
      assert(mcpTotal === httpTotal,
        `MCP outputs total (${mcpTotal}) must match HTTP (${httpTotal})`);

      // ─── list_actions — compare with HTTP ───
      const mcpActions = await mcp.callTool('list_actions', { labels: [] });
      const httpActions = await walletA.client.listActions({ limit: 100 });
      const mcpActionTotal = mcpActions.totalActions || mcpActions.total_actions || 0;
      const httpActionTotal = httpActions.totalActions || httpActions.total_actions || 0;
      assert(mcpActionTotal === httpActionTotal,
        `MCP actions total (${mcpActionTotal}) must match HTTP (${httpActionTotal})`);

      // ─── get_height / get_network / get_version — compare with HTTP ───
      const mcpHeight = await mcp.callTool('get_height', {});
      const httpHeight = await walletA.client.getHeight();
      // Both should return a height object or number
      assert(mcpHeight !== undefined, 'MCP get_height must return a result');

      const mcpNetwork = await mcp.callTool('get_network', {});
      const httpNetwork = await walletA.client.getNetwork();
      assert(mcpNetwork.network === httpNetwork.network,
        `MCP network (${mcpNetwork.network}) must match HTTP (${httpNetwork.network})`);

      const mcpVersion = await mcp.callTool('get_version', {});
      const httpVersion = await walletA.client.getVersion();
      assert(mcpVersion.version === httpVersion.version,
        `MCP version must match HTTP version`);

      // The same empty discovery contract through MCP: toolbox #388 (CLI #331).
      for (const [tool, args] of [
        ['discover_by_identity_key', { identityKey: httpKey }],
        ['discover_by_attributes', { attributes: { role: 'e2e-tester' } }],
      ]) {
        const discovered = await mcp.callTool(tool, args);
        assert(discovered.totalCertificates === 0 && discovered.certificates.length === 0,
          `${tool}: empty discovery at the published beta.2 pin`);
      }

      return {
        txid: null,
        sats: 0,
        wocConfirmed: 'n/a',
        tools: tools.length,
        tests: 13,
        balance: mcpBalance.satoshis,
      };

    } finally {
      mcp.stop();
    }
  },
};


/**
 * Minimal MCP JSON-RPC 2.0 client over stdio.
 * Spawns bsv-wallet-mcp as a subprocess, sends JSON-RPC to stdin, reads from stdout.
 */
class McpClient {
  constructor(walletPort) {
    this.walletPort = walletPort;
    this.proc = null;
    this.nextId = 1;
    this.pending = new Map(); // id -> { resolve, reject }
    this.buffer = '';
  }

  async start() {
    this.proc = spawn(MCP_BIN, [], {
      env: {
        ...process.env,
        WALLET_URL: `http://localhost:${this.walletPort}`,
        WALLET_ORIGIN: 'http://e2e-test',
        RUST_LOG: 'warn',
      },
      stdio: ['pipe', 'pipe', 'pipe'],
    });

    // Parse newline-delimited JSON-RPC from stdout
    this.proc.stdout.on('data', (chunk) => {
      this.buffer += chunk.toString();
      let lines = this.buffer.split('\n');
      this.buffer = lines.pop(); // keep incomplete line in buffer

      for (const line of lines) {
        const trimmed = line.trim();
        if (!trimmed) continue;
        try {
          const msg = JSON.parse(trimmed);
          if (msg.id !== undefined && this.pending.has(msg.id)) {
            const { resolve, reject } = this.pending.get(msg.id);
            this.pending.delete(msg.id);
            if (msg.error) {
              reject(new Error(`MCP error: ${JSON.stringify(msg.error)}`));
            } else {
              resolve(msg.result);
            }
          }
          // Ignore notifications (no id)
        } catch {
          // Ignore non-JSON lines (logging, etc.)
        }
      }
    });

    // Wait for process to be ready (poll up to 5s)
    for (let i = 0; i < 10; i++) {
      if (this.proc.exitCode !== null) {
        throw new Error(`MCP server exited with code ${this.proc.exitCode}`);
      }
      await sleep(500);
      // MCP server writes to stderr on startup — if we see output, it's ready
      if (i >= 1) break; // Give at least 1s before proceeding
    }
  }

  /** Send a JSON-RPC request and wait for the response. */
  async request(method, params, timeoutMs = 15000) {
    const id = this.nextId++;
    const msg = { jsonrpc: '2.0', method, params, id };

    return new Promise((resolve, reject) => {
      this.pending.set(id, { resolve, reject });
      const timer = setTimeout(() => {
        this.pending.delete(id);
        reject(new Error(`MCP request '${method}' timed out after ${timeoutMs}ms`));
      }, timeoutMs);

      const origResolve = resolve;
      this.pending.set(id, {
        resolve: (v) => { clearTimeout(timer); origResolve(v); },
        reject: (e) => { clearTimeout(timer); reject(e); },
      });

      this.proc.stdin.write(JSON.stringify(msg) + '\n');
    });
  }

  /** Send a JSON-RPC notification (no response expected). */
  notify(method, params = {}) {
    const msg = { jsonrpc: '2.0', method, params };
    this.proc.stdin.write(JSON.stringify(msg) + '\n');
  }

  /** Call an MCP tool and parse the text content as JSON. */
  async callTool(name, args) {
    const result = await this.request('tools/call', { name, arguments: args });
    const content = result.content || [];
    const textContent = content.find(c => c.type === 'text');
    if (!textContent) {
      throw new Error(`MCP tool '${name}' returned no text content: ${JSON.stringify(result)}`);
    }
    try {
      return JSON.parse(textContent.text);
    } catch {
      return { _raw: textContent.text };
    }
  }

  stop() {
    if (this.proc && !this.proc.killed) {
      this.proc.stdin.end();
      this.proc.kill('SIGTERM');
    }
    // Reject all pending
    for (const [id, { reject }] of this.pending) {
      reject(new Error('MCP client stopped'));
    }
    this.pending.clear();
  }
}
