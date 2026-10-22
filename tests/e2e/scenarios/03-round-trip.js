/**
 * E2E.3: Round-trip A → B → A.
 * Both wallets send AND receive. Proves freshly internalized funds are spendable.
 */
const { execSync } = require('child_process');
const { verifyOnChain } = require('../lib/woc');
const { BSV, extractJson } = require('../lib/setup');

module.exports = {
  name: 'round-trip',
  description: 'A sends to B via CLI, B sends back to A via CLI (round-trip)',

  async run(walletA, walletB, assert) {
    const aBalanceBefore = parseInt(
      execSync(`cd "${walletA.dir}" && "${BSV}" balance`, { encoding: 'utf-8' }).trim(),
    );

    // Step 1: A sends 15k to B via CLI
    const send1 = JSON.parse(extractJson(execSync(
      `cd "${walletA.dir}" && "${BSV}" --json send --allow-pending received "${walletB.address}" 15000`,
      { encoding: 'utf-8', stdio: ['pipe', 'pipe', 'pipe'] },
    ).trim()));

    // B internalizes
    execSync(`cd "${walletB.dir}" && "${BSV}" fund "${send1.beef}" --vout 0`,
      { stdio: ['pipe', 'pipe', 'pipe'] });

    const bBalanceAfterReceive = parseInt(
      execSync(`cd "${walletB.dir}" && "${BSV}" balance`, { encoding: 'utf-8' }).trim(),
    );
    assert(bBalanceAfterReceive >= 15000, `B should have >= 15000 after receive, got ${bBalanceAfterReceive}`);

    // Step 2: B sends 7k back to A via CLI (tests freshly internalized funds are spendable)
    const send2 = JSON.parse(extractJson(execSync(
      `cd "${walletB.dir}" && "${BSV}" --json send --allow-pending received "${walletA.address}" 7000`,
      { encoding: 'utf-8', stdio: ['pipe', 'pipe', 'pipe'] },
    ).trim()));

    // A internalizes
    execSync(`cd "${walletA.dir}" && "${BSV}" fund "${send2.beef}" --vout 0`,
      { stdio: ['pipe', 'pipe', 'pipe'] });

    // Verify final balances
    const aBalanceAfter = parseInt(
      execSync(`cd "${walletA.dir}" && "${BSV}" balance`, { encoding: 'utf-8' }).trim(),
    );
    const bBalanceAfter = parseInt(
      execSync(`cd "${walletB.dir}" && "${BSV}" balance`, { encoding: 'utf-8' }).trim(),
    );

    // A should be roughly: start - 15k + 7k - fees
    assert(aBalanceAfter < aBalanceBefore, 'A should have less than before (sent more than received)');
    assert(aBalanceAfter > aBalanceBefore - 15000, 'A should have more than start-15k (got 7k back)');

    // B should have some balance left (prior scenario balances may contribute)
    assert(bBalanceAfter > 0, 'B should have some balance left');
    assert(bBalanceAfter < bBalanceAfterReceive, 'B should have less than before sending back');

    // WoC audit on both txids
    const woc1 = await verifyOnChain(send1.txid, { maxRetries: 5, baseDelay: 1000 });
    const woc2 = await verifyOnChain(send2.txid, { maxRetries: 5, baseDelay: 1000 });
    assert(woc1.confirmed, 'A→B tx must appear on WoC');
    assert(woc2.confirmed, 'B→A tx must appear on WoC');

    return {
      tx1: send1.txid, tx2: send2.txid,
      aBalance: aBalanceAfter, bBalance: bBalanceAfter,
      wocConfirmed: true,
    };
  },
};
