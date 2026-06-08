//! Build a fully-signed claim transaction for a TimeLockedGift covenant.
//!
//! The claim spends the covenant (deposit vout 0) + the recipient-owned fee UTXO
//! (deposit vout 1), pays the pinned `amount` to the recipient's P2PKH, sends the
//! leftover fee-utxo as change, and sets nLockTime = lockUntil + a non-final
//! sequence (so it satisfies `timeLock` and can only confirm post-unlock).
//!
//! Covenant input unlock = `<sig> <preimage>` (ANYONECANPAY|SINGLE|FORKID), where
//! `preimage` is the BIP-143 sighash preimage the covenant re-derives and checks.
//! Fee input unlock = standard P2PKH `<sig> <pubkey>` (ALL|FORKID).
//!
//! Pure / no I/O — callers fetch the deposit and broadcast. Shared by gift-claim
//! (broadcast) and gift-inspect (build to prove signability, then discard).

use bsv_sdk::primitives::bsv::sighash::{
    build_sighash_preimage, compute_sighash_for_signing, parse_transaction, SighashParams, TxInput,
    TxOutput, SIGHASH_ALL, SIGHASH_ANYONECANPAY, SIGHASH_FORKID, SIGHASH_SINGLE,
};
use bsv_sdk::primitives::bsv::TransactionSignature;
use bsv_sdk::primitives::{hash160, sha256d, to_hex, PrivateKey, Writer};

use super::covenant::{parse_locking_script, CovenantParams};

const TX_VERSION: u32 = 1;
const COVENANT_VOUT: usize = 0;
const FEE_VOUT: usize = 1;
const NON_FINAL_SEQUENCE: u32 = 0xffff_fffe;

pub struct ClaimPlan {
    pub deposit_txid: String,
    pub covenant: CovenantParams,
    pub fee_utxo_sats: u64,
    pub fee: u64,
    pub change: u64,
    pub claim_raw_hex: String,
    pub claim_txid: String,
}

fn p2pkh_script(pkh: &[u8]) -> Vec<u8> {
    let mut s = Vec::with_capacity(25);
    s.extend_from_slice(&[0x76, 0xa9, 0x14]);
    s.extend_from_slice(pkh);
    s.extend_from_slice(&[0x88, 0xac]);
    s
}

/// Minimal data push (direct / PUSHDATA1 / PUSHDATA2 / PUSHDATA4) for unlocking scripts.
fn push_bytes(data: &[u8]) -> Vec<u8> {
    let n = data.len();
    let mut v = Vec::with_capacity(n + 4);
    if n < 0x4c {
        v.push(n as u8);
    } else if n <= 0xff {
        v.push(0x4c);
        v.push(n as u8);
    } else if n <= 0xffff {
        v.push(0x4d);
        v.extend_from_slice(&(n as u16).to_le_bytes());
    } else {
        v.push(0x4e);
        v.extend_from_slice(&(n as u32).to_le_bytes());
    }
    v.extend_from_slice(data);
    v
}

fn serialize_tx(version: u32, inputs: &[TxInput], outputs: &[TxOutput], locktime: u32) -> Vec<u8> {
    let mut w = Writer::new();
    w.write_u32_le(version);
    w.write_var_int(inputs.len() as u64);
    for inp in inputs {
        w.write_bytes(&inp.txid);
        w.write_u32_le(inp.output_index);
        w.write_var_int(inp.script.len() as u64);
        w.write_bytes(&inp.script);
        w.write_u32_le(inp.sequence);
    }
    w.write_var_int(outputs.len() as u64);
    for out in outputs {
        w.write_u64_le(out.satoshis);
        w.write_var_int(out.script.len() as u64);
        w.write_bytes(&out.script);
    }
    w.write_u32_le(locktime);
    w.into_bytes()
}

fn txid_display(raw: &[u8]) -> String {
    let mut h = sha256d(raw);
    h.reverse();
    to_hex(&h)
}

/// Build a fully-signed claim. `key` MUST be the covenant recipient's key.
/// `lock_time` is the nLockTime to set (use `covenant.lock_until` for a real claim).
pub fn build_claim_tx(deposit_raw: &[u8], key: &PrivateKey, lock_time: u32) -> Result<ClaimPlan, String> {
    let dep = parse_transaction(deposit_raw).map_err(|e| format!("parse deposit tx: {e}"))?;
    let cov_out = dep.outputs.get(COVENANT_VOUT).ok_or("deposit has no vout 0")?;
    let fee_out = dep
        .outputs
        .get(FEE_VOUT)
        .ok_or("deposit has no vout 1 (recipient fee utxo)")?;

    let params = parse_locking_script(&cov_out.script)?;
    if cov_out.satoshis != params.amount {
        return Err(format!(
            "covenant output value {} != pinned amount {}",
            cov_out.satoshis, params.amount
        ));
    }
    let our_pub = key.public_key().to_compressed();
    if our_pub.as_slice() != params.recipient.as_slice() {
        return Err("this gift is not locked to your key".into());
    }

    let pay = p2pkh_script(&hash160(&params.recipient));
    let dep_txid_internal = sha256d(deposit_raw);

    let mut inputs = vec![
        TxInput {
            txid: dep_txid_internal,
            output_index: COVENANT_VOUT as u32,
            script: vec![],
            sequence: NON_FINAL_SEQUENCE,
        },
        TxInput {
            txid: dep_txid_internal,
            output_index: FEE_VOUT as u32,
            script: vec![],
            sequence: NON_FINAL_SEQUENCE,
        },
    ];
    let mut outputs = vec![
        // output 0: PINNED — exactly `amount` to recipient (covenant enforces this)
        TxOutput {
            satoshis: params.amount,
            script: pay.clone(),
        },
        // output 1: change from the fee utxo (value finalized after sizing)
        TxOutput {
            satoshis: fee_out.satoshis,
            script: pay.clone(),
        },
    ];

    // ---- unlock covenant input 0 (ANYONECANPAY | SINGLE | FORKID) ----
    let cov_scope = SIGHASH_ANYONECANPAY | SIGHASH_SINGLE | SIGHASH_FORKID; // 0xc3
    let (preimage, cov_sighash) = {
        let p = SighashParams {
            version: TX_VERSION as i32,
            inputs: &inputs,
            outputs: &outputs,
            locktime: lock_time,
            input_index: COVENANT_VOUT,
            subscript: &cov_out.script,
            satoshis: params.amount,
            scope: cov_scope,
        };
        (build_sighash_preimage(&p), compute_sighash_for_signing(&p))
    };
    let cov_sig = key.sign(&cov_sighash).map_err(|e| format!("covenant sign: {e}"))?;
    let cov_txsig = TransactionSignature::new(cov_sig, cov_scope).to_low_s();
    let mut cov_unlock = push_bytes(&cov_txsig.to_checksig_format());
    cov_unlock.extend_from_slice(&push_bytes(&preimage));
    inputs[0].script = cov_unlock;

    // ---- size + fee (covenant unlock attached; reserve ~108B for the fee unlock) ----
    inputs[1].script = vec![0u8; 108];
    let est = serialize_tx(TX_VERSION, &inputs, &outputs, lock_time).len() as u64;
    inputs[1].script = vec![];
    let fee = (est * 11) / 10 + 50; // ~1.1 sat/byte + buffer
    if fee_out.satoshis <= fee {
        return Err(format!(
            "fee utxo {} is too small for the ~{fee}-sat claim fee",
            fee_out.satoshis
        ));
    }
    let change = fee_out.satoshis - fee;
    if change < 1 {
        outputs.pop(); // no room for change; whole fee utxo is the fee
    } else {
        outputs[1].satoshis = change;
    }

    // ---- sign fee input 1 (ALL | FORKID) as standard P2PKH ----
    let fee_scope = SIGHASH_ALL | SIGHASH_FORKID; // 0x41
    let fee_sighash = {
        let p = SighashParams {
            version: TX_VERSION as i32,
            inputs: &inputs,
            outputs: &outputs,
            locktime: lock_time,
            input_index: FEE_VOUT,
            subscript: &fee_out.script,
            satoshis: fee_out.satoshis,
            scope: fee_scope,
        };
        compute_sighash_for_signing(&p)
    };
    let fee_sig = key.sign(&fee_sighash).map_err(|e| format!("fee sign: {e}"))?;
    let fee_txsig = TransactionSignature::new(fee_sig, fee_scope).to_low_s();
    let mut fee_unlock = push_bytes(&fee_txsig.to_checksig_format());
    fee_unlock.extend_from_slice(&push_bytes(&our_pub));
    inputs[1].script = fee_unlock;

    let raw = serialize_tx(TX_VERSION, &inputs, &outputs, lock_time);
    let claim_txid = txid_display(&raw);

    Ok(ClaimPlan {
        deposit_txid: txid_display(deposit_raw),
        fee_utxo_sats: fee_out.satoshis,
        fee,
        change: outputs.get(1).map(|o| o.satoshis).unwrap_or(0),
        claim_raw_hex: to_hex(&raw),
        claim_txid,
        covenant: params,
    })
}
