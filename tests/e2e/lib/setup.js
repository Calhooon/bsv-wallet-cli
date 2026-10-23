/**
 * Multi-wallet setup and teardown.
 * Spawns fresh wallets on test ports, funds them from the e2e funder (default :3320).
 */

const { execSync, spawn } = require('child_process');
const fs = require('fs');
const path = require('path');
const { WalletClient, ANYONE_KEY, buildP2PKH } = require('./wallet-client');
const { fetchBeef, buildAtomicBEEF, sleep } = require('./woc');

// The release binary: the repository's target by default; BSV_WALLET_BIN names
// another (a shared CARGO_TARGET_DIR).
const BSV = process.env.BSV_WALLET_BIN || path.resolve(__dirname, '../../../target/release/bsv-wallet');
const FUNDER_PORT = parseInt(process.env.FUNDER_PORT || '3320');
const FUNDER_DIR = process.env.FUNDER_DIR || path.resolve(__dirname, '..', '..', '..', 'e2e-funder');
// Where a test wallet's directory is made (`/tmp` as before; E2E_WALLET_TMP
// moves it, which the hermetic run does and CI reads for its artifacts).
const WALLET_TMP = process.env.E2E_WALLET_TMP || '/tmp';

/**
 * Initialize and start a wallet on a given port.
 * Returns { dir, proc, client, identityKey, address }.
 */
async function startWallet(port, name) {
  // The port must be free. A process already answering there (another
  // wallet's daemon, a leftover) would take this wallet's requests: the
  // 2026-10-23 run funded someone's daemon on :3323 and swept its coins
  // because its spawned `serve` died "Address already in use" while the
  // health check below answered from the stranger. Refuse before anything
  // is spawned or funded.
  const stranger = await new WalletClient(port, name).isAuthenticated().catch(() => null);
  if (stranger !== null) {
    throw new Error(`${name}: port ${port} is already answering (${JSON.stringify(stranger)}); not this harness's wallet. Stop it or move the harness (WALLET_A_PORT, WALLET_B_PORT, WALLET_C_PORT).`);
  }
  const dir = fs.mkdtempSync(path.join(WALLET_TMP, `bsv-wallet-${name}-`));

  // Init wallet (generates ROOT_KEY + wallet.db)
  execSync(`cd "${dir}" && "${BSV}" init`, { stdio: 'pipe' });

  // Start serve in background. The monitor daemon is NOT needed for test wallets
  // because internalized txs are spent via BEEF which includes the parent tx
  // (already broadcast by the sender). The monitor would actually interfere by
  // trying to re-broadcast internalized txs and saturating ARC connections.
  // The wallet's log goes to a file in its directory, never to a pipe this
  // harness does not read: a full pipe blocks every request that logs (the
  // 2026-10-22 run hung at 180 s from scenario 14 on, after the actions
  // themselves went through). The file is kept with the directory when a
  // run fails (see teardownWallet), so a failure has the wallet's own log.
  const logPath = path.join(dir, 'serve.log');
  const logFd = fs.openSync(logPath, 'a');
  const proc = spawn(BSV, ['--port', String(port), 'serve'], {
    cwd: dir,
    stdio: ['ignore', logFd, logFd],
    detached: false,
  });
  fs.closeSync(logFd);

  // Wait for health. A test wallet's only coins are received payments not
  // yet mined (the funder's, then the other wallet's), which the spend gate
  // holds from a plain spend (R1 (d), 0.15.0); the client names the gate's
  // word on every spend it makes, as the CLI scenarios do with
  // `--allow-pending received`.
  const client = new WalletClient(port, name, { allowPending: 'received' });
  for (let i = 0; i < 20; i++) {
    try {
      await client.isAuthenticated();
      break;
    } catch {
      if (i === 19) throw new Error(`${name} on :${port} failed to start (its log: ${logPath})`);
      await sleep(500);
    }
  }

  const identityKey = await client.identityKey();
  // The server answering is this directory's wallet: its identity is the
  // CLI's from the same `.env`. A stranger on the port is refused here even
  // if it appeared between the check above and the spawn.
  const cliIdentity = execSync(`cd "${dir}" && "${BSV}" identity`, { encoding: 'utf-8' })
    .split('\n').find(l => l.startsWith('Identity key:'))?.replace('Identity key:', '').trim();
  if (cliIdentity !== identityKey) {
    teardownWallet({ dir, proc }, { keep: false });
    throw new Error(`${name}: the server on :${port} is not this wallet (HTTP identity ${identityKey.slice(0, 12)}..., the directory's ${String(cliIdentity).slice(0, 12)}...); another process holds the port`);
  }
  // `address` prints the BRC-29 deposit address on its first line and the
  // gift pubkey on a second; the address is the first line alone.
  const addrResult = execSync(`cd "${dir}" && "${BSV}" address`, { encoding: 'utf-8' })
    .trim().split('\n')[0].trim();

  return { dir, proc, client, identityKey, address: addrResult, port, logPath };
}

/**
 * Fund a wallet from the funder via HTTP API (wallet-to-wallet transfer).
 * The funder daemon on FUNDER_PORT handles UTXO selection and signing.
 * The recipient internalizes the AtomicBEEF directly — no WoC needed.
 */
async function fundWallet(wallet, satoshis) {
  // The funder's coins are its own change and the sweeps the last run sent
  // back, received and not yet mined: the funding names the gate's word for
  // those, the one spend the funder makes under it (its own just-broadcast
  // top-ups, whose ancestry it checked at internalize).
  const funderClient = new WalletClient(FUNDER_PORT, 'funder');
  const result = await funderClient.transferTo(wallet.client, satoshis, 'e2e funding', { allowPending: 'received' });
  return { txid: result.txid, sats: satoshis };
}

/**
 * Sweep all funds from a wallet back to the funder.
 * Returns sats swept, or 0 if balance too low.
 *
 * If the full sweep fails (e.g., some UTXOs are unspendable due to wrong derivation
 * or basket insertion), retries with progressively smaller amounts to recover as much
 * as possible. This prevents losing sats when a scenario creates unspendable outputs.
 */
async function sweepToFunder(wallet) {
  // Use HTTP API for full balance (covers all baskets)
  const balance = await wallet.client.balance();

  if (balance < 600) {
    console.log(`  ${wallet.client.name}: ${balance} sats (dust, skipping sweep)`);
    return 0;
  }

  // Get funder's receiving key via HTTP API
  const funderClient = new WalletClient(FUNDER_PORT, 'funder');
  const funderKey = await funderClient.getPublicKey(
    [2, '3241645161d8'], 'SfKxPIJNgdI= NaGLC6fMH50=', ANYONE_KEY, true,
  );
  const funderScript = buildP2PKH(funderKey.publicKey);

  let sweepAmount = balance - 300; // leave room for fee
  let swept = 0;

  for (let attempt = 0; attempt < 4 && sweepAmount >= 600; attempt++) {
    try {
      const result = await wallet.client.createAction(
        [{ lockingScript: funderScript, satoshis: sweepAmount, outputDescription: 'sweep to funder' }],
        `sweep ${sweepAmount} sats back to funder`,
      );

      // Funder internalizes the BEEF
      await funderClient.internalizeAction(result.tx, [{
        outputIndex: 0,
        protocol: 'wallet payment',
        paymentRemittance: {
          derivationPrefix: 'SfKxPIJNgdI=',
          derivationSuffix: 'NaGLC6fMH50=',
          senderIdentityKey: ANYONE_KEY,
        },
      }], `sweep from ${wallet.client.name}`);

      console.log(`  ${wallet.client.name}: swept ${sweepAmount} sats back to funder`);
      swept += sweepAmount;
      break;
    } catch (e) {
      // Check if the sweep actually went through despite the timeout.
      // The wallet may have broadcast successfully but the HTTP response
      // took too long (e.g., ARC providers slow to respond).
      const balNow = await wallet.client.balance().catch(() => balance);
      if (balNow < balance - 500) {
        console.log(`  ${wallet.client.name}: sweep timed out but balance dropped (${balance} → ${balNow}) — tx went through`);
        swept += balance - balNow;
        break;
      }
      if (attempt < 3) {
        const prev = sweepAmount;
        // The wallet's refusal names the shortfall ("need N, have M"): the
        // fee outran the margin. Retry short by exactly that plus a little,
        // so the sweep takes the whole balance less the fee; halve only
        // when the refusal says something else.
        const short = /need (\d+), have (\d+)/.exec(e.message || '');
        const shortfall = short ? parseInt(short[1]) - parseInt(short[2]) : 0;
        sweepAmount = shortfall > 0 ? sweepAmount - shortfall - 50 : Math.floor(sweepAmount / 2);
        console.log(`  ${wallet.client.name}: sweep ${prev} failed (${e.message.slice(0, 60)}), retrying with ${sweepAmount}`);
      } else {
        console.log(`  WARNING: ${wallet.client.name}: could not sweep (${sweepAmount} sats stuck: ${e.message.slice(0, 80)})`);
      }
    }
  }

  // What the sweep left behind stays the wallet's: teardown keeps a
  // directory holding more than dust, with the key in its .env, instead of
  // deleting the only way to spend it.
  const left = await wallet.client.balance().catch(() => balance - swept);
  wallet.left = left >= 600 ? left : 0;

  return swept;
}

/**
 * Kill the wallet process and remove its temp dir, unless the sweep left
 * funds in it (`wallet.left`, set by sweepToFunder) or the run asks to keep
 * it (`keep`, a failed run): then the directory is kept and named, since its
 * .env is the only key to those sats and serve.log the only record of what
 * the wallet did.
 */
function teardownWallet(wallet, { keep = false } = {}) {
  if (wallet.proc && !wallet.proc.killed) {
    wallet.proc.kill('SIGTERM');
  }
  if (wallet.dir && fs.existsSync(wallet.dir)) {
    if (wallet.left || keep) {
      const why = wallet.left ? `${wallet.left} sats remain; the key is in its .env (recover with \`bsv-wallet drain <funder address>\` once offerable)` : 'the run failed';
      console.log(`  KEPT ${wallet.dir}: ${why}; its log is serve.log there`);
      return;
    }
    fs.rmSync(wallet.dir, { recursive: true, force: true });
  }
}

/**
 * Extract JSON from CLI output that may contain tracing log lines.
 * Finds the line starting with '{' and returns it (for single-line JSON).
 */
function extractJson(output) {
  const lines = output.split('\n');
  for (let i = lines.length - 1; i >= 0; i--) {
    const line = lines[i].trim();
    if (line.startsWith('{')) return line;
  }
  throw new Error(`No JSON found in output: ${output.slice(0, 200)}`);
}

/**
 * Parse JSON from CLI output that may be multi-line (pretty-printed).
 * Tries JSON.parse on the full output first, then extracts from first '{' to last '}'.
 */
function parseCliJson(output) {
  const trimmed = output.trim();
  // Try direct parse first (handles both single-line and multi-line)
  try { return JSON.parse(trimmed); } catch { /* fall through */ }
  // Find first '{' and last '}' to extract the JSON block
  const start = trimmed.indexOf('{');
  const end = trimmed.lastIndexOf('}');
  if (start === -1 || end === -1 || end <= start) {
    throw new Error(`No JSON found in output: ${trimmed.slice(0, 200)}`);
  }
  return JSON.parse(trimmed.slice(start, end + 1));
}

module.exports = { startWallet, fundWallet, sweepToFunder, teardownWallet, extractJson, parseCliJson, BSV, FUNDER_PORT };
