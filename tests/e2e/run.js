#!/usr/bin/env node
/**
 * bsv-wallet-cli E2E Test Runner
 *
 * Spawns two fresh wallets on :3323 and :3324, funds them from the e2e funder on :3320,
 * runs scenarios, verifies on WoC, then sweeps everything back to the funder.
 *
 * Usage:
 *   node run.js                    # Run all scenarios
 *   node run.js --scenario 1       # Run single scenario
 *   node run.js --dry-run          # Setup only, no scenarios
 *   node run.js --skip-woc         # Skip WoC verification (faster)
 */

const { startWallet, fundWallet, sweepToFunder, teardownWallet, FUNDER_PORT } = require('./lib/setup');
const { WalletClient } = require('./lib/wallet-client');
const path = require('path');
const fs = require('fs');

// Parse CLI args
const args = process.argv.slice(2);
const scenarioFilter = args.includes('--scenario') ? parseInt(args[args.indexOf('--scenario') + 1]) : null;
const dryRun = args.includes('--dry-run');
const skipWoc = args.includes('--skip-woc');

// Load scenarios
const scenarioFiles = fs.readdirSync(path.join(__dirname, 'scenarios'))
  .filter(f => f.endsWith('.js'))
  .sort();

const scenarios = scenarioFiles.map(f => require(`./scenarios/${f}`));

async function main() {
  let walletA, walletB;
  const results = [];
  let funderBalanceBefore;

  try {
    // Check funder is running
    const funder = new WalletClient(FUNDER_PORT, 'funder');
    try { await funder.isAuthenticated(); } catch {
      console.error(`ERROR: Funder wallet not running on :${FUNDER_PORT}`);
      process.exit(1);
    }

    funderBalanceBefore = await funder.balance();

    console.log('');
    console.log('================================================================');
    console.log('  bsv-wallet-cli E2E Test Suite');
    console.log('================================================================');
    console.log(`  Funder: :${FUNDER_PORT}  Balance: ${funderBalanceBefore.toLocaleString()} sats`);

    // Start test wallets
    console.log('  Starting test wallets...');
    walletA = await startWallet(3323, 'wallet-A');
    walletB = await startWallet(3324, 'wallet-B');
    console.log(`  Wallet A: :3323 (${walletA.identityKey.slice(0, 12)}...)  ${walletA.address}`);
    console.log(`  Wallet B: :3324 (${walletB.identityKey.slice(0, 12)}...)  ${walletB.address}`);

    // Fund wallet A. Scenario 22 needs 50,000 in A at its start and the
    // scenarios before it move about 40,000 out of A (to B, and fees), so a
    // full run funds 100,000; the sweeps bring all but fees back. The funder
    // must hold the funding and its fee, or the run stops here, before any
    // wallet is started or any sat moves.
    const fundAmount = parseInt(process.env.FUND_AMOUNT || '100000');
    if (funderBalanceBefore < fundAmount + 1_000) {
      console.error(`ERROR: funder holds ${funderBalanceBefore.toLocaleString()} sats; a run funds ${fundAmount.toLocaleString()} and needs ${(fundAmount + 1_000).toLocaleString()} (FUND_AMOUNT lowers it; scenario 22 needs 50,000 in A at its start)`);
      process.exit(1);
    }
    console.log(`  Funding A with ${fundAmount.toLocaleString()} sats from funder...`);
    const funded = await fundWallet(walletA, fundAmount);
    console.log(`  Funded: txid=${funded.txid.slice(0, 16)}...`);

    const aBalance = await walletA.client.balance();
    console.log(`  A balance: ${aBalance.toLocaleString()} sats`);
    console.log('================================================================');

    if (dryRun) {
      console.log('  --dry-run: skipping scenarios');
      console.log('================================================================');
      return;
    }

    // Run scenarios
    const toRun = scenarioFilter !== null
      ? scenarios.filter((_, i) => i + 1 === scenarioFilter)
      : scenarios;

    for (let i = 0; i < toRun.length; i++) {
      const scenario = toRun[i];
      const num = scenarioFilter || (i + 1);
      const assertions = [];
      let failed = false;

      function assert(condition, message) {
        if (!condition) {
          failed = true;
          assertions.push(`FAIL: ${message}`);
          throw new Error(message);
        }
        assertions.push(`OK: ${message}`);
      }

      // Balance checkpoint before scenario
      const aBalBefore = await walletA.client.balance();
      const bBalBefore = await walletB.client.balance();

      console.log(`  [${num}] ${scenario.name}: ${scenario.description}`);
      const start = Date.now();

      try {
        const timeout = new Promise((_, reject) =>
          setTimeout(() => reject(new Error('TIMEOUT: scenario exceeded 300s')), 300_000));
        const result = await Promise.race([scenario.run(walletA, walletB, assert), timeout]);
        const elapsed = ((Date.now() - start) / 1000).toFixed(1);

        // Balance checkpoint after scenario
        const aBalAfter = await walletA.client.balance();
        const bBalAfter = await walletB.client.balance();
        const aDelta = aBalAfter - aBalBefore;
        const bDelta = bBalAfter - bBalBefore;
        const netLeak = -(aDelta + bDelta); // positive = sats left the system

        console.log(`       PASS (${elapsed}s) txid=${result.txid?.slice(0, 12) || 'n/a'}... woc=${result.wocConfirmed || 'n/a'}`);
        console.log(`       A: ${aBalAfter} (${aDelta >= 0 ? '+' : ''}${aDelta})  B: ${bBalAfter} (${bDelta >= 0 ? '+' : ''}${bDelta})  net: ${netLeak > 0 ? '-' : '+'}${Math.abs(netLeak)} sats`);
        results.push({ num, name: scenario.name, status: 'PASS', elapsed, netLeak, ...result });
      } catch (e) {
        const elapsed = ((Date.now() - start) / 1000).toFixed(1);
        console.log(`       FAIL (${elapsed}s): ${e.message}`);
        results.push({ num, name: scenario.name, status: 'FAIL', elapsed, error: e.message });
      }
    }

  } finally {
    // Sweep and teardown
    console.log('================================================================');
    console.log('  Sweeping remaining funds back to funder...');

    let sweptTotal = 0;
    if (walletB) {
      try { sweptTotal += await sweepToFunder(walletB); } catch (e) {
        console.log(`  WARNING: B sweep failed: ${e.message}`);
      }
    }
    if (walletA) {
      try { sweptTotal += await sweepToFunder(walletA); } catch (e) {
        console.log(`  WARNING: A sweep failed: ${e.message}`);
      }
    }

    // Teardown: a failed run keeps both wallet directories (keys and logs)
    const keep = results.some(r => r.status === 'FAIL') || results.length < scenarios.length;
    if (walletB) teardownWallet(walletB, { keep });
    if (walletA) teardownWallet(walletA, { keep });

    // Final accounting
    const funderClient = new WalletClient(FUNDER_PORT, 'funder');
    const funderBalanceAfter = await funderClient.balance();

    const passed = results.filter(r => r.status === 'PASS').length;
    const total = results.length;
    const netCost = funderBalanceBefore - funderBalanceAfter;

    console.log('');
    console.log('================================================================');
    console.log('  RESULTS');
    console.log('================================================================');

    for (const r of results) {
      const icon = r.status === 'PASS' ? 'OK' : '!!';
      const leak = r.netLeak != null ? ` leak=${r.netLeak}` : '';
      console.log(`  [${icon}] ${r.num}. ${r.name}: ${r.status} (${r.elapsed}s)${leak}`);
    }

    const totalLeak = results.reduce((s, r) => s + (r.netLeak || 0), 0);

    console.log('----------------------------------------------------------------');
    console.log(`  Passed:        ${passed}/${total}`);
    console.log(`  Funder before: ${funderBalanceBefore.toLocaleString()} sats`);
    console.log(`  Funder after:  ${funderBalanceAfter.toLocaleString()} sats`);
    console.log(`  Net cost:      ${netCost.toLocaleString()} sats`);
    console.log(`  Scenario leak: ${totalLeak.toLocaleString()} sats (sum of per-scenario A+B deltas)`);
    console.log('================================================================');

    // Hard fail if net cost exceeds budget.
    // Expected: ~1,300 sats (fees) + ~2,000 (pending internalized outputs not yet spendable).
    // Anything over 5K indicates a real leak.
    const MAX_ACCEPTABLE_COST = 5_000;
    if (netCost > MAX_ACCEPTABLE_COST) {
      console.log(`  BUDGET EXCEEDED: net cost ${netCost} > max ${MAX_ACCEPTABLE_COST} sats`);
      process.exit(1);
    }

    process.exit(passed === total ? 0 : 1);
  }
}

main().catch(e => {
  console.error('FATAL:', e);
  process.exit(1);
});
