/**
 * E2E.5 (partial): CLI send + fund with direct BEEF.
 * Tests the CLI path: `bsv-wallet --json send` → capture BEEF → `bsv-wallet fund`.
 */
const { execSync } = require('child_process');
const { verifyOnChain } = require('../lib/woc');
const { BSV, extractJson } = require('../lib/setup');

module.exports = {
  name: 'cli-send-fund',
  description: 'A sends to B via CLI, B funds from BEEF directly (no WoC)',

  async run(walletA, walletB, assert) {
    const sendAmount = 8_000;

    // A sends to B via CLI with --json
    const rawResult = execSync(
      `cd "${walletA.dir}" && "${BSV}" --json send --allow-pending received "${walletB.address}" ${sendAmount}`,
      { encoding: 'utf-8', stdio: ['pipe', 'pipe', 'pipe'] },
    ).trim();

    const parsed = JSON.parse(extractJson(rawResult));
    assert(parsed.txid, 'CLI send must return txid');
    assert(parsed.beef, 'CLI --json send must return beef hex');
    assert(parsed.txid.length === 64, 'txid must be 64 hex chars');
    assert(parsed.beef.startsWith('01010101'), 'BEEF must start with AtomicBEEF magic');

    // B internalizes via CLI fund (no WoC!)
    const fundOutput = execSync(
      `cd "${walletB.dir}" && "${BSV}" fund "${parsed.beef}" --vout 0`,
      { encoding: 'utf-8', stdio: ['pipe', 'pipe', 'pipe'] },
    ).trim();
    // fund should not error (exit code 0 enforced by execSync)

    // Verify B's balance via CLI
    const bBalance = parseInt(
      execSync(`cd "${walletB.dir}" && "${BSV}" balance`, { encoding: 'utf-8' }).trim(),
    );
    assert(bBalance >= sendAmount, `B CLI balance should be >= ${sendAmount}, got ${bBalance}`);

    // WoC audit
    const woc = await verifyOnChain(parsed.txid, { maxRetries: 5, baseDelay: 1000 });
    assert(woc.confirmed, 'Transaction must appear on WoC');

    return { txid: parsed.txid, sats: sendAmount, wocConfirmed: woc.confirmed, path: 'CLI' };
  },
};
