/**
 * E2E.17: CLI split + spend.
 * Tests the split command end-to-end: splits UTXOs, verifies count increased,
 * spends from a split output to prove derivation is correct and outputs are truly spendable.
 * Cost: ~fees only (split tx + spend tx).
 */

const { execSync } = require('child_process');
const { BSV, parseCliJson } = require('../lib/setup');

module.exports = {
  name: 'cli-split-spend',
  description: 'Split UTXOs via CLI, then spend to prove they are spendable (fees only)',

  async run(walletA, walletB, assert) {
    const SPLIT_COUNT = 3;
    // The differential also drives the previous binary, before #313 exists.
    const supportsFrom = execSync(`"${BSV}" send --help`, { encoding: 'utf-8', stdio: 'pipe' }).includes('--from');

    // ─── Record A's pre-split state ───
    const balanceBefore = await walletA.client.balance();
    const outputsBefore = await walletA.client.listOutputs('default', {
      limit: 10000, includeEnvelope: false,
    });
    const utxoCountBefore = (outputsBefore.outputs || [])
      .filter(o => o.spendable !== false).length;

    assert(balanceBefore > 1000, `A needs balance > 1000 to split, has ${balanceBefore}`);

    const splitCoin = outputsBefore.outputs.find(o => o.spendable !== false).outpoint.replace('.', ':');
    const splitFrom = supportsFrom ? `--from ${splitCoin}` : '';
    // The named coin is mandatory; the remaining wallet coins stay available.
    // ─── Split into 3 UTXOs via CLI ───
    const splitRaw = execSync(
      `cd "${walletA.dir}" && "${BSV}" --json split --allow-pending received ${splitFrom} --count ${SPLIT_COUNT}`,
      { encoding: 'utf-8', stdio: ['pipe', 'pipe', 'pipe'] },
    ).trim();
    const splitResult = parseCliJson(splitRaw);

    assert(splitResult.txid && splitResult.txid.length === 64,
      `Split must return valid txid, got ${splitResult.txid}`);
    assert(splitResult.outputs === SPLIT_COUNT,
      `Split must create ${SPLIT_COUNT} outputs, got ${splitResult.outputs}`);
    assert(splitResult.satoshisPerOutput > 0,
      `Split per-output amount must be > 0, got ${splitResult.satoshisPerOutput}`);

    // ─── Verify UTXO count increased ───
    // Wait briefly for the wallet to process the self-internalization
    await new Promise(r => setTimeout(r, 1000));

    const outputsAfterSplit = await walletA.client.listOutputs('default', {
      limit: 10000, includeEnvelope: false,
    });
    const spendableAfterSplit = (outputsAfterSplit.outputs || [])
      .filter(o => o.spendable !== false);
    const utxoCountAfterSplit = spendableAfterSplit.length;

    assert(utxoCountAfterSplit >= SPLIT_COUNT,
      `After split, should have >= ${SPLIT_COUNT} UTXOs, got ${utxoCountAfterSplit}`);

    // ─── Verify balance only decreased by fee ───
    const balanceAfterSplit = await walletA.client.balance();
    const splitCost = balanceBefore - balanceAfterSplit;
    // Split should cost only the tx fee (typically < 500 sats for a simple split)
    assert(splitCost < 1000,
      `Split cost should be < 1000 sats (fee only), was ${splitCost}`);
    assert(splitCost >= 0,
      `Split cost should not be negative, was ${splitCost}`);

    // ─── Spend from a split output to prove it's truly spendable ───
    // Send a small amount from A to B — this uses split outputs as inputs
    const sendAmount = 1000;
    const sendCoin = spendableAfterSplit.find(o => o.outpoint.startsWith(splitResult.txid)).outpoint;

    const sendFrom = supportsFrom ? `--from ${sendCoin.replace('.', ':')}` : '';
    const sendRaw = execSync(
      `cd "${walletA.dir}" && "${BSV}" --json send --allow-pending received ${sendFrom} "${walletB.address}" ${sendAmount}`,
      { encoding: 'utf-8', stdio: ['pipe', 'pipe', 'pipe'] },
    ).trim();
    const sendResult = parseCliJson(sendRaw);

    assert(sendResult.txid && sendResult.txid.length === 64,
      'Spend from split output must produce valid txid');
    assert(sendResult.beef && sendResult.beef.startsWith('01010101'),
      'Spend must return AtomicBEEF');

    const remaining = await walletA.client.listOutputs('default', { limit: 10000, includeEnvelope: false });
    if (supportsFrom) assert(!remaining.outputs.some(o => o.outpoint === sendCoin), '--from spends the named split output');

    // B internalizes via CLI fund
    execSync(
      `cd "${walletB.dir}" && "${BSV}" fund "${sendResult.beef}" --vout 0`,
      { encoding: 'utf-8', stdio: ['pipe', 'pipe', 'pipe'] },
    );

    // Verify B received the sats
    const bBalance = await walletB.client.balance();
    assert(bBalance >= sendAmount,
      `B should have >= ${sendAmount} sats after receiving, has ${bBalance}`);

    // ─── Final balance check ───
    const balanceFinal = await walletA.client.balance();
    const totalCost = balanceBefore - balanceFinal - sendAmount;
    // Total cost should be just fees (split fee + send fee)
    assert(totalCost < 2000,
      `Total fee cost should be < 2000 sats, was ${totalCost}`);

    return {
      txid: splitResult.txid,
      sats: sendAmount,
      wocConfirmed: 'n/a',
      splitCount: SPLIT_COUNT,
      perOutput: splitResult.satoshisPerOutput,
      utxosBefore: utxoCountBefore,
      utxosAfterSplit: utxoCountAfterSplit,
      fees: totalCost,
    };
  },
};
