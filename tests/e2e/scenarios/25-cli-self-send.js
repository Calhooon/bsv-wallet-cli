/**
 * E2E.25: CLI send to the wallet's own address keeps its sats (#38).
 * A sends 1,000 sats to its own BRC-29 address: its balance after is lower
 * than before by the fee alone, the fee read from the transaction (its
 * inputs' values in the BEEF less its outputs). A then sends 1,000 sats to
 * B: lower by the amount plus that transaction's fee, as before; B takes the
 * payment in with `fund` so the sweep returns it. Runs under the stub and on
 * mainnet alike. Cost: two fees.
 */
const { execSync } = require('child_process');
const { BSV, parseCliJson } = require('../lib/setup');
const { parseBeef } = require('../stub/tx');

const AMOUNT = 1_000;

function cli(wallet, args) {
  return execSync(`cd "${wallet.dir}" && "${BSV}" ${args}`,
    { encoding: 'utf-8', stdio: ['pipe', 'pipe', 'pipe'] }).trim();
}

function balance(wallet) {
  return parseCliJson(cli(wallet, '--json balance')).satoshis;
}

/** The subject's fee: its inputs' values (their parents in the BEEF) less its outputs. */
function feeOf(beefHex, txid) {
  const beef = parseBeef(Buffer.from(beefHex, 'hex'));
  const byTxid = new Map(beef.txs.filter(t => !t.txidOnly).map(t => [t.txid, t]));
  const tx = byTxid.get(txid);
  if (!tx) throw new Error(`the BEEF does not hold ${txid}`);
  let fee = 0n;
  for (const input of tx.inputs) {
    const parent = byTxid.get(input.txid);
    if (!parent) throw new Error(`the BEEF does not hold ${txid}'s parent ${input.txid}`);
    fee += BigInt(parent.outputs[input.vout].satoshis);
  }
  for (const output of tx.outputs) fee -= BigInt(output.satoshis);
  return { fee: Number(fee), tx };
}

module.exports = {
  name: 'cli-self-send',
  description: 'A sends to its own address (fee only), then to B (amount plus fee)',

  async run(walletA, walletB, assert) {
    // The differential also drives the previous binary, which relinquishes a
    // self-payment: it skips that half rather than give 1,000 sats away.
    const keeps = execSync(`"${BSV}" send --help`, { encoding: 'utf-8', stdio: 'pipe' }).includes('keeps');
    const start = balance(walletA);
    assert(start >= 2 * AMOUNT + 1_000, `A needs ${2 * AMOUNT + 1_000} sats for two sends, has ${start}`);

    let self = null;
    if (keeps) {
      const ownAddress = parseCliJson(cli(walletA, '--json address')).address;
      const sent = parseCliJson(cli(walletA, `--json send --allow-pending received "${ownAddress}" ${AMOUNT}`));
      assert(sent.txid && sent.beef, 'self-send must return txid and beef');
      assert(sent.kept && Number.isInteger(sent.kept.vout), `self-send must name the kept output, got ${JSON.stringify(sent.kept)}`);
      const { fee, tx } = feeOf(sent.beef, sent.txid);
      assert(Number(tx.outputs[sent.kept.vout].satoshis) === AMOUNT, `kept output ${sent.kept.vout} must carry ${AMOUNT} sats`);
      const after = balance(walletA);
      console.log(`       self-send ${sent.txid.slice(0, 16)}...: balance ${start} -> ${after}, fee ${fee} (from the transaction), kept vout ${sent.kept.vout}`);
      assert(fee > 0, `self-send fee must be positive, got ${fee}`);
      assert(start - after === fee, `self-send must cost the fee alone: ${start} - ${after} = ${start - after}, fee ${fee}`);
      const outputs = await walletA.client.listOutputs('default', { limit: 10000, includeEnvelope: false });
      assert((outputs.outputs || []).some(o => o.outpoint === `${sent.txid}.${sent.kept.vout}` && o.spendable !== false),
        `the kept output ${sent.txid}.${sent.kept.vout} must be a spendable coin of A's default basket`);
      self = { txid: sent.txid, before: start, after, fee };
    } else {
      console.log('       self-send skipped: this binary predates #38 (send --help does not say it keeps)');
    }

    const before = balance(walletA);
    const sent = parseCliJson(cli(walletA, `--json send --allow-pending received "${walletB.address}" ${AMOUNT}`));
    assert(sent.txid && sent.beef, 'send to B must return txid and beef');
    assert(sent.kept === undefined, 'a send to B keeps nothing');
    const { fee } = feeOf(sent.beef, sent.txid);
    const after = balance(walletA);
    console.log(`       send to B ${sent.txid.slice(0, 16)}...: balance ${before} -> ${after}, amount ${AMOUNT} + fee ${fee} (from the transaction)`);
    assert(before - after === AMOUNT + fee, `send to B must cost amount plus fee: ${before} - ${after} = ${before - after}, expected ${AMOUNT + fee}`);
    // B takes it in so the sweep returns it to the funder.
    cli(walletB, `fund "${sent.beef}" --vout 0`);

    return { self, foreign: { txid: sent.txid, before, after, fee }, path: 'CLI' };
  },
};
