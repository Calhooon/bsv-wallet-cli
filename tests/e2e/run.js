#!/usr/bin/env node
/**
 * bsv-wallet-cli E2E Test Runner
 *
 * Spawns two fresh wallets on :3370 and :3371 (WALLET_A_PORT, WALLET_B_PORT), funds them from the e2e funder on :3320,
 * runs scenarios, verifies on WoC, then sweeps everything back to the funder.
 *
 * Usage:
 *   node run.js                    # Run all scenarios
 *   node run.js --scenario 1       # Run single scenario
 *   node run.js --dry-run          # Setup only, no scenarios
 *   node run.js --skip-woc         # Skip WoC verification (faster)
 *   E2E_HERMETIC=1 node run.js     # Against the stub, nothing leaves the machine (docs/E2E-HERMETIC.md)
 */

// The hermetic module first: under E2E_HERMETIC=1 it sets the environment
// (the stub's bases, the wall, the funder's seat) that ./lib/setup and every
// child read.
const hermetic = require('./lib/hermetic');
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

// The scenarios this run was asked for: one under --scenario N, none under
// --dry-run, else all. A run that ends with fewer results than these failed.
const toRun = scenarioFilter !== null
  ? scenarios.filter((_, i) => i + 1 === scenarioFilter)
  : scenarios;
const expected = dryRun ? 0 : toRun.length;

async function main() {
  let walletA, walletB;
  const results = [];
  let funderBalanceBefore;
  // A failure before or between the scenarios (the funder's refusal of the
  // funding, a wallet that would not start): the run is a failure, named
  // here, never a "0/0 passed" exit 0.
  let fatal = null;
  // The hermetic run's own: the stub process and the funder wallet it seeds.
  let stub = null;
  let funderWallet = null;

  try {
    if (hermetic.enabled) {
      console.log('');
      console.log(`  E2E_HERMETIC=1: starting the stub on :${hermetic.STUB_PORT} and a funder on :${FUNDER_PORT}`);
      stub = await hermetic.startStub();
      funderWallet = await startWallet(FUNDER_PORT, 'funder');
      const seed = parseInt(process.env.FUND_AMOUNT || '100000') * 2 + 10_000;
      const coin = await hermetic.seedFunder(stub, funderWallet, seed);
      console.log(`  Funder seeded: ${seed.toLocaleString()} sats from the stub's coinbase ${coin.txid.slice(0, 16)}... at height ${coin.height}`);
    }

    // Check funder is running
    const funder = new WalletClient(FUNDER_PORT, 'funder');
    try { await funder.isAuthenticated(); } catch {
      console.error(`ERROR: Funder wallet not running on :${FUNDER_PORT}`);
      if (stub) stub.stop();
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
    // The test wallets' ports (defaults 3370, 3371; scenario 23's third
    // wallet 3372): off every port ~/bsv/prod-test-wallets/REGISTRY.md
    // reserves (3323, 3330 to 3344: the harness's old 3323 was the
    // registry's recipient seat, and the 2026-10-23 run went through that
    // seat's daemon). WALLET_A_PORT, WALLET_B_PORT, WALLET_C_PORT move
    // them. A port already answering is refused by startWallet.
    const portA = parseInt(process.env.WALLET_A_PORT || '3370');
    const portB = parseInt(process.env.WALLET_B_PORT || '3371');
    walletA = await startWallet(portA, 'wallet-A');
    walletB = await startWallet(portB, 'wallet-B');
    console.log(`  Wallet A: :${portA} (${walletA.identityKey.slice(0, 12)}...)  ${walletA.address}`);
    console.log(`  Wallet B: :${portB} (${walletB.identityKey.slice(0, 12)}...)  ${walletB.address}`);

    // Fund wallet A. Scenario 22 needs 50,000 in A at its start and the
    // scenarios before it move about 40,000 out of A (to B, and fees), so a
    // full run funds 100,000; the sweeps bring all but fees back. The funder
    // must hold the funding and its fee, or the run stops here, before any
    // wallet is started or any sat moves.
    const fundAmount = parseInt(process.env.FUND_AMOUNT || '100000');
    if (funderBalanceBefore < fundAmount + 1_000) {
      console.error(`ERROR: funder holds ${funderBalanceBefore.toLocaleString()} sats; a run funds ${fundAmount.toLocaleString()} and needs ${(fundAmount + 1_000).toLocaleString()} (FUND_AMOUNT lowers it; scenario 22 needs 50,000 in A at its start)`);
      if (walletB) teardownWallet(walletB, { keep: false });
      if (walletA) teardownWallet(walletA, { keep: false });
      if (funderWallet) teardownWallet(funderWallet, { keep: false });
      if (stub) stub.stop();
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

      // A scenario that cannot be hermetic says so (`hermetic: false` with a
      // `hermeticReason`); under the flag it is skipped and named, never
      // silently, and counts as run.
      if (hermetic.enabled && scenario.hermetic === false) {
        console.log(`  [${num}] ${scenario.name}: SKIP under E2E_HERMETIC=1: ${scenario.hermeticReason || 'not hermetic'}`);
        results.push({ num, name: scenario.name, status: 'SKIP', elapsed: '0.0', reason: scenario.hermeticReason });
        continue;
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

  } catch (e) {
    fatal = e;
    console.log(`  FATAL: ${e.message}`);
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
    const keep = fatal !== null || results.some(r => r.status === 'FAIL') || results.length < expected;
    if (walletB) teardownWallet(walletB, { keep });
    if (walletA) teardownWallet(walletA, { keep });

    // Final accounting
    const funderClient = new WalletClient(FUNDER_PORT, 'funder');
    const funderBalanceAfter = await funderClient.balance().catch(() => funderBalanceBefore || 0);

    // The hermetic run's stats: every call each stub provider answered, and
    // every attempt to leave the machine the wall refused. Then the funder
    // and the stub go (the funder's directory kept, like the wallets', when
    // the run failed).
    let walled = 0;
    if (stub) {
      try {
        const stats = await stub.stats();
        walled = Object.values(stats.leaks || {}).reduce((s, n) => s + n, 0);
        console.log('');
        console.log(`  Stub (${stats.total} calls; chain tip ${stats.chain.height}, ${stats.chain.minedBlocks} block(s) mined, ${stats.chain.transactions} transactions held):`);
        console.log(hermetic.describeStats(stats));
        console.log(`  Stub log: ${stub.logPath}`);
      } catch (e) {
        console.log(`  WARNING: the stub's stats could not be read: ${e.message}`);
      }
    }
    if (funderWallet) {
      const keepFunder = fatal !== null || results.some(r => r.status === 'FAIL') || results.length < expected || walled > 0;
      funderWallet.left = 0; // the hermetic funder's sats are the stub's, not mainnet's
      teardownWallet(funderWallet, { keep: keepFunder });
    }
    if (stub) stub.stop();

    const passed = results.filter(r => r.status === 'PASS').length;
    const skipped = results.filter(r => r.status === 'SKIP').length;
    const total = results.length;
    const netCost = funderBalanceBefore - funderBalanceAfter;
    // Sats a kept wallet still holds are stranded, not spent: the key is in
    // its directory (teardown named it). The budget judges what was spent.
    const stranded = (walletA?.left || 0) + (walletB?.left || 0);
    const spent = netCost - stranded;

    console.log('');
    console.log('================================================================');
    console.log('  RESULTS');
    console.log('================================================================');

    for (const r of results) {
      const icon = r.status === 'PASS' ? 'OK' : r.status === 'SKIP' ? '--' : '!!';
      const leak = r.netLeak != null ? ` leak=${r.netLeak}` : '';
      const reason = r.status === 'SKIP' ? ` (${r.reason || 'not hermetic'})` : '';
      console.log(`  [${icon}] ${r.num}. ${r.name}: ${r.status} (${r.elapsed}s)${leak}${reason}`);
    }

    const totalLeak = results.reduce((s, r) => s + (r.netLeak || 0), 0);

    console.log('----------------------------------------------------------------');
    console.log(`  Passed:        ${passed}/${total}${skipped ? ` (${skipped} skipped under E2E_HERMETIC=1)` : ''}`);
    if (hermetic.enabled) console.log(`  Walled:        ${walled} attempt(s) to leave the machine`);
    console.log(`  Funder before: ${funderBalanceBefore.toLocaleString()} sats`);
    console.log(`  Funder after:  ${funderBalanceAfter.toLocaleString()} sats`);
    console.log(`  Net cost:      ${netCost.toLocaleString()} sats`);
    if (stranded) console.log(`  Stranded:      ${stranded.toLocaleString()} sats in kept wallets (recoverable); spent: ${spent.toLocaleString()} sats`);
    console.log(`  Scenario leak: ${totalLeak.toLocaleString()} sats (sum of per-scenario A+B deltas)`);
    console.log('================================================================');

    // Hard fail if net cost exceeds budget.
    // Expected: ~1,300 sats (fees) + ~2,000 (pending internalized outputs not yet spendable).
    // Anything over 5K indicates a real leak.
    const MAX_ACCEPTABLE_COST = 5_000;
    if (spent > MAX_ACCEPTABLE_COST) {
      console.log(`  BUDGET EXCEEDED: spent ${spent} > max ${MAX_ACCEPTABLE_COST} sats`);
      process.exit(1);
    }
    if (stranded) {
      console.log(`  STRANDED: ${stranded} sats remain in kept wallets; recover them before the next run`);
      process.exit(1);
    }

    if (fatal !== null) {
      console.log(`  FAILED before the scenarios ran: ${fatal.message}`);
      process.exit(1);
    }
    if (total < expected) {
      console.log(`  FAILED: ${total} of ${expected} scenarios ran`);
      process.exit(1);
    }
    // A hermetic run that tried to leave the machine is not hermetic: the
    // wall refused each attempt, and the stats above name the host and the
    // route that lacks a knob.
    if (walled > 0) {
      console.log(`  NOT HERMETIC: ${walled} attempt(s) to reach a real host were walled; see the stub's stats above`);
      process.exit(1);
    }
    process.exit(passed + skipped === total ? 0 : 1);
  }
}

main().catch(e => {
  console.error('FATAL:', e);
  process.exit(1);
});
