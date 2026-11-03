/**
 * A minimal MCP JSON-RPC 2.0 client over stdio (scenario 21's, shared with
 * the browser gate's scene 27 since #414 L5b): it spawns bsv-wallet-mcp
 * against a wallet's port, writes JSON-RPC to its stdin and reads its
 * stdout.
 */
const { spawn } = require('child_process');
const path = require('path');
const { sleep } = require('./woc');

// The MCP binary: the repository's target by default; BSV_WALLET_MCP_BIN names
// another (a shared CARGO_TARGET_DIR), as BSV_WALLET_BIN does for the wallet.
const MCP_BIN = process.env.BSV_WALLET_MCP_BIN || path.resolve(__dirname, '../../../target/release/bsv-wallet-mcp');

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

module.exports = { McpClient, MCP_BIN };
