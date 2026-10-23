/**
 * The auditor's helpers: the explorer ladder for "is it on the network"
 * (WhatsOnChain, Bitails, BananaBlocks), WoC for BEEF. Never a courier.
 */

const WOC_BASE = 'https://api.whatsonchain.com/v1/bsv/main';

const BITAILS_BASE = 'https://api.bitails.io';
const BANANABLOCKS_BASE = 'https://bananablocks.com/api/v1';

/**
 * The auditor's rungs, the explorer ladder rule (CLAUDE.md, #50): one
 * explorer's 404 is "not yet indexed there", never "not on the network".
 * Each returns the explorer's status for GET /tx/{txid}; a network error
 * is a fault, reported as its message.
 */
const RUNGS = [
  { name: 'whatsonchain', url: txid => `${WOC_BASE}/tx/${txid}` },
  { name: 'bitails', url: txid => `${BITAILS_BASE}/tx/${txid}` },
  { name: 'bananablocks', url: txid => `${BANANABLOCKS_BASE}/tx/${txid}` },
];

/**
 * Verify a transaction is on the network: the explorer ladder, WhatsOnChain
 * then Bitails then BananaBlocks on every poll, the first 200 the answer
 * (the explorer that listed it is named in `listedBy`). A 404 from one
 * explorer moves to the next rung; any other status or a network error is a
 * fault and moves on too. Polls with exponential backoff; after the last
 * poll with no 200 it throws, naming every rung's last answer.
 *
 * Seen 2026-10-23 (the 0.17.2 gate, scenario 14): WoC answered 404 for a
 * broadcast return payment for over 88 s while Bitails had it at 6 s (the
 * wallet's own ladder recorded it as seen through Bitails); a WoC-only
 * auditor failed the scenario.
 */
async function verifyOnChain(txid, opts = {}) {
  const maxRetries = opts.maxRetries || 15;
  const baseDelay = opts.baseDelay || 2000;
  const last = {};

  for (let i = 0; i < maxRetries; i++) {
    for (const rung of RUNGS) {
      try {
        const resp = await fetch(rung.url(txid));
        last[rung.name] = String(resp.status);
        if (resp.ok) {
          return { confirmed: true, txid, listedBy: rung.name };
        }
        // 404: not yet indexed there; anything else: a fault. Both move on.
      } catch (e) {
        last[rung.name] = `fault: ${e.message}`;
      }
    }
    if (i < maxRetries - 1) await sleep(baseDelay * Math.pow(1.5, i));
  }
  const answers = RUNGS.map(r => `${r.name}: ${last[r.name] || 'not asked'}`).join('; ');
  throw new Error(`Transaction ${txid} not listed by any explorer after ${maxRetries} polls (${answers})`);
}

/**
 * Fetch BEEF from WoC for a given txid.
 * Returns raw BEEF bytes as a Buffer.
 */
async function fetchBeef(txid) {
  const resp = await fetch(`${WOC_BASE}/tx/${txid}/beef`);
  if (!resp.ok) throw new Error(`WoC /tx/${txid}/beef returned ${resp.status}`);
  const raw = await resp.text();
  // WoC returns BEEF as hex string (possibly quoted)
  const clean = raw.trim().replace(/^"|"$/g, '');
  return Buffer.from(clean, 'hex');
}

/**
 * Build AtomicBEEF from raw BEEF bytes + txid.
 * Format: [0x01,0x01,0x01,0x01] + reversed_txid(32 bytes) + beef_bytes
 */
function buildAtomicBEEF(txid, beefBytes) {
  const prefix = Buffer.from([0x01, 0x01, 0x01, 0x01]);
  const txidBytes = Buffer.from(txid, 'hex');
  const reversed = Buffer.from(txidBytes.reverse());
  return Buffer.concat([prefix, reversed, beefBytes]);
}

function sleep(ms) {
  return new Promise(resolve => setTimeout(resolve, ms));
}

module.exports = { verifyOnChain, fetchBeef, buildAtomicBEEF, sleep, WOC_BASE, BITAILS_BASE, BANANABLOCKS_BASE };
