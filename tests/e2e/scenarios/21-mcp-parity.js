/**
 * E2E.21: MCP server parity.
 * Tests the MCP server via stdin/stdout JSON-RPC 2.0.
 * Verifies tool listing, compares tool results with HTTP API.
 * Cost: 0 sats (read-only queries + crypto round-trips).
 */

const { McpClient } = require('../lib/mcp');
const { sleep } = require('../lib/woc');

/** JSON with every object's keys sorted, so two answers compare by content. */
function canonical(value) {
  return JSON.stringify(value, (_, v) => (v && typeof v === 'object' && !Array.isArray(v)
    ? Object.fromEntries(Object.keys(v).sort().map(k => [k, v[k]]))
    : v));
}

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

      // ─── tools/list — verify all 37 tools (29 wallet, five host-route, the owner's three) ───
      const toolsResult = await mcp.request('tools/list', {});
      const tools = toolsResult.tools || [];
      assert(tools.length === 37,
        `MCP should expose 37 tools, got ${tools.length}`);

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
        'list_origins', 'answer_origin', 'revoke_origin',
      ].sort();

      for (const expected of expectedTools) {
        assert(toolNames.includes(expected),
          `MCP should have tool '${expected}', missing from: ${toolNames.join(', ')}`);
      }

      // ─── list_origins — the owner's GET /origins, sent with no Origin (#414) ───
      const ownersUrl = `http://localhost:${walletA.port}/origins`;
      const mcpOrigins = await mcp.callTool('list_origins', {});
      const httpOrigins = await (await fetch(ownersUrl)).json();
      assert(Array.isArray(mcpOrigins.grants) && Array.isArray(mcpOrigins.pending) && Array.isArray(mcpOrigins.probes),
        `MCP list_origins must return {grants, pending, probes}: ${JSON.stringify(mcpOrigins)}`);
      // The same rows, compared with their keys sorted: the MCP re-serializes
      // the door's JSON with its keys in order, the door keeps section (4)'s
      // order, and scenario 20 leaves wallet A a granted row to compare.
      assert(canonical(mcpOrigins) === canonical(httpOrigins),
        `MCP list_origins must match GET /origins: ${canonical(mcpOrigins)} vs ${canonical(httpOrigins)}`);
      // The harness's page origin is refused there: the owner's door is no page's.
      const asPage = await fetch(ownersUrl, { headers: { Origin: walletA.client.origin } });
      const pageBody = await asPage.json();
      assert(asPage.status === 403 && pageBody.code === 'ACCESS_DENIED',
        `GET /origins with an Origin must be 403 ACCESS_DENIED, got ${asPage.status} ${JSON.stringify(pageBody)}`);

      // ─── revoke_origin — one power by its kind and key (#414, the four kinds) ───
      // An originator holding no row: the tool names the power the route
      // refuses, as POST /origins/revoke answers it, and writes nothing.
      const power = { originator: 'mcp-parity.example', kind: 'basket', key: '["browser gate"]' };
      const httpRevoke = await fetch(`${ownersUrl}/revoke`, {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify(power),
      });
      const httpRevokeBody = await httpRevoke.json();
      const nothing = 'mcp-parity.example holds no live grant row for basket ["browser gate"]: nothing to revoke';
      assert(httpRevoke.status === 409 && httpRevokeBody.message === nothing,
        `POST /origins/revoke of a power held by none must be 409 naming it, got ${httpRevoke.status} ${JSON.stringify(httpRevokeBody)}`);
      let mcpRevokeError = null;
      try {
        await mcp.callTool('revoke_origin', power);
      } catch (e) {
        mcpRevokeError = e.message;
      }
      assert(mcpRevokeError && mcpRevokeError.includes('HTTP 409') && mcpRevokeError.includes('holds no live grant row for basket'),
        `MCP revoke_origin must carry the route's refusal naming the power, got ${mcpRevokeError}`);

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

      // The MCP's two discover tools answer the HTTP door's discovery
      // (#331): wallet A reads no TRUST_SETTINGS and holds no certificate,
      // so both are empty through either door.
      for (const [tool, args, http] of [
        ['discover_by_identity_key', { identityKey: httpKey },
          () => walletA.client.discoverByIdentityKey(httpKey)],
        ['discover_by_attributes', { attributes: { role: 'e2e-tester' } },
          () => walletA.client.discoverByAttributes({ role: 'e2e-tester' })],
      ]) {
        const discovered = await mcp.callTool(tool, args);
        const served = await http();
        assert(discovered.totalCertificates === served.totalCertificates
          && JSON.stringify(discovered.certificates) === JSON.stringify(served.certificates),
          `${tool}: the MCP answers the HTTP door's discovery: ${JSON.stringify(discovered)} vs ${JSON.stringify(served)}`);
      }

      return {
        txid: null,
        sats: 0,
        wocConfirmed: 'n/a',
        tools: tools.length,
        tests: 14,
        balance: mcpBalance.satoshis,
      };

    } finally {
      mcp.stop();
    }
  },
};
