//! What the fuzz targets share: the transaction and BEEF builders of the
//! tree's tests (`src/test_support.rs`), and the steps of the served doors
//! that are written inline in an async handler, so a target cannot call
//! them without a wallet and a store.
//!
//! Each mirror names the lines it copies. A mirror drifts when its source
//! changes; `docs/NIGHTLY.md` lists them, and the cure is to lift the
//! source into a function the target calls (named in the lane's report).

use bsv_sdk::primitives::{PrivateKey, PublicKey};
use bsv_sdk::transaction::{Beef, MerklePath, Transaction};
use bsv_sdk::wallet::{
    Counterparty, CreateActionArgs, CreateActionInput, CreateActionOptions, CreateActionOutput,
    InternalizeActionArgs, KeyDeriver, Outpoint, Protocol, SecurityLevel, TrustSelf,
};
use bsv_wallet_cli::server::types::McCreateActionReq;
use bsv_wallet_cli::spend_gate::AllowPending;

/// The tests' synthetic root key (0x01 x32), never anyone's wallet.
pub fn synthetic_root() -> PrivateKey {
    PrivateKey::from_bytes(&[1u8; 32]).expect("a key")
}

/// A P2PKH locking script over `hash160` (`test_support::p2pkh`).
pub fn p2pkh(hash160: [u8; 20]) -> Vec<u8> {
    let mut s = vec![0x76, 0xa9, 0x14];
    s.extend_from_slice(&hash160);
    s.extend_from_slice(&[0x88, 0xac]);
    s
}

/// A version 1 transaction spending `inputs` (`(txid as displayed, vout)`,
/// empty unlocking scripts) into `outputs` (`test_support::raw_tx`; at most
/// 252 of each, and scripts under 253 bytes).
pub fn raw_tx(inputs: &[(&str, u32)], outputs: &[(u64, Vec<u8>)]) -> Vec<u8> {
    let mut tx = vec![1, 0, 0, 0];
    tx.push(inputs.len() as u8);
    for (txid, vout) in inputs {
        let mut prev = hex::decode(txid).expect("a txid");
        prev.reverse();
        tx.extend_from_slice(&prev);
        tx.extend_from_slice(&vout.to_le_bytes());
        tx.push(0);
        tx.extend_from_slice(&0xffff_ffffu32.to_le_bytes());
    }
    tx.push(outputs.len() as u8);
    for (satoshis, script) in outputs {
        tx.extend_from_slice(&satoshis.to_le_bytes());
        tx.push(script.len() as u8);
        tx.extend_from_slice(script);
    }
    tx.extend_from_slice(&[0, 0, 0, 0]);
    tx
}

/// The txid (as displayed) of raw transaction bytes.
pub fn txid_of(raw: &[u8]) -> String {
    Transaction::from_binary(raw).expect("a transaction").id()
}

/// `raw` under a one-transaction block's path at `height`, as BEEF bytes.
pub fn proven_beef(raw: Vec<u8>, height: u32) -> Vec<u8> {
    let txid = txid_of(&raw);
    let mut beef = Beef::new();
    let bump = beef.merge_bump(MerklePath::from_coinbase_txid(&txid, height));
    beef.merge_raw_tx(raw, Some(bump));
    beef.to_binary()
}

/// `child` spending a proven `parent`, as AtomicBEEF bytes on `child`.
pub fn atomic_payment(parent: Vec<u8>, child: Vec<u8>) -> Vec<u8> {
    let parent_id = txid_of(&parent);
    let child_id = txid_of(&child);
    let mut beef = Beef::new();
    let bump = beef.merge_bump(MerklePath::from_coinbase_txid(&parent_id, 900_000));
    beef.merge_raw_tx(parent, Some(bump));
    beef.merge_raw_tx(child, None);
    beef.to_binary_atomic(&child_id)
        .expect("the child is in the BEEF")
}

/// The BRC-29 public key the wallet at `root` derives for a payment
/// (`prefix`, `suffix`) from `sender` (none: self).
pub fn brc29_key(
    root: &PrivateKey,
    prefix: &str,
    suffix: &str,
    sender: Option<&PublicKey>,
) -> Option<PublicKey> {
    let counterparty = match sender {
        Some(key) => Counterparty::Other(key.clone()),
        None => Counterparty::Self_,
    };
    KeyDeriver::new(Some(root.clone()))
        .derive_public_key(
            &Protocol::new(SecurityLevel::Counterparty, "3241645161d8"),
            &format!("{prefix} {suffix}"),
            &counterparty,
            true,
        )
        .ok()
}

/// The served `/createAction` body to the toolbox's arguments, with the
/// door's `allowPending` word and the audited total. Mirror of
/// `src/server/handlers.rs` `create_action`, from the `allowPending` read
/// to `total_sats` (no store, no lock, no broadcast). Production builds
/// without overflow checks, and so does `cargo fuzz` by default: the total
/// wraps here as it does in the shipped binary.
pub fn create_action_mapping(
    req: McCreateActionReq,
) -> anyhow::Result<(CreateActionArgs, AllowPending, u64)> {
    use anyhow::Context;
    let allow_pending = AllowPending::from_json(req.allow_pending.as_ref())
        .map_err(|message| anyhow::anyhow!("Invalid argument: {message}"))?;
    let outputs = req
        .outputs
        .map(|outs| {
            outs.into_iter()
                .map(|o| {
                    Ok(CreateActionOutput {
                        locking_script: hex::decode(&o.locking_script)?,
                        satoshis: o.satoshis,
                        output_description: o.output_description,
                        basket: o.basket,
                        custom_instructions: o.custom_instructions,
                        tags: o.tags,
                    })
                })
                .collect::<anyhow::Result<Vec<_>>>()
        })
        .transpose()?;
    let inputs = match req.inputs {
        Some(reqs) if !reqs.is_empty() => {
            let mut mapped = Vec::with_capacity(reqs.len());
            for i in reqs {
                let outpoint = Outpoint::from_string(&i.outpoint)
                    .map_err(|e| anyhow::anyhow!("invalid input outpoint '{}': {e}", i.outpoint))?;
                let unlocking_script = match i.unlocking_script {
                    Some(h) => Some(hex::decode(&h).context("invalid unlockingScript hex")?),
                    None => None,
                };
                mapped.push(CreateActionInput {
                    outpoint,
                    input_description: i.input_description.unwrap_or_else(|| "input".to_string()),
                    unlocking_script,
                    unlocking_script_length: i.unlocking_script_length,
                    sequence_number: i.sequence_number,
                });
            }
            Some(mapped)
        }
        _ => Some(vec![]),
    };
    let args = CreateActionArgs {
        description: req.description,
        input_beef: req.input_beef,
        inputs,
        outputs,
        lock_time: req.lock_time,
        version: None,
        labels: req.labels,
        options: req.options.map(|o| CreateActionOptions {
            accept_delayed_broadcast: o.accept_delayed_broadcast,
            randomize_outputs: o.randomize_outputs,
            sign_and_process: o.sign_and_process,
            no_send: o.no_send,
            trust_self: match o.trust_self.as_deref() {
                Some("known") => Some(TrustSelf::Known),
                _ => None,
            },
            ..Default::default()
        }),
    };
    let total_sats: u64 = args
        .outputs
        .as_ref()
        .map(|o| o.iter().map(|x| x.satoshis).sum())
        .unwrap_or(0);
    Ok((args, allow_pending, total_sats))
}

/// The toolbox wallet's checks of an internalize before its store is
/// touched. Mirror of bsv-wallet-toolbox-rs 0.12.1
/// `src/wallet/wallet.rs` `internalize_action` (the derivation checks, the
/// "death spiral prevention"), with the deriver at `root`.
pub fn internalize_precheck(args: &InternalizeActionArgs, root: &PrivateKey) -> Result<(), String> {
    for output in &args.outputs {
        if let Some(ref payment) = output.payment_remittance {
            if payment.derivation_prefix.is_empty() || payment.derivation_suffix.is_empty() {
                return Err("empty derivation".to_string());
            }
        }
    }
    let deriver = KeyDeriver::new(Some(root.clone()));
    if let Ok(tx) = Transaction::from_beef(&args.tx, None) {
        for output in &args.outputs {
            if let Some(ref payment) = output.payment_remittance {
                let vout = output.output_index as usize;
                if vout >= tx.outputs.len() {
                    return Err("output_index past the outputs".to_string());
                }
                let script = tx.outputs[vout].locking_script.as_script().to_binary();
                if script.len() == 25
                    && script[0] == 0x76
                    && script[1] == 0xa9
                    && script[2] == 0x14
                    && script[23] == 0x88
                    && script[24] == 0xac
                {
                    let counterparty = if !payment.sender_identity_key.is_empty() {
                        Counterparty::Other(
                            PublicKey::from_hex(&payment.sender_identity_key)
                                .map_err(|e| format!("Invalid sender_identity_key: {e}"))?,
                        )
                    } else {
                        Counterparty::Self_
                    };
                    let protocol = Protocol::new(SecurityLevel::Counterparty, "3241645161d8");
                    let key_id = format!(
                        "{} {}",
                        payment.derivation_prefix, payment.derivation_suffix
                    );
                    let derived = deriver
                        .derive_public_key(&protocol, &key_id, &counterparty, true)
                        .map_err(|e| format!("BRC-29 key derivation failed: {e}"))?;
                    if derived.hash160()[..] != script[3..23] {
                        return Err(format!("Derivation mismatch for output {vout}"));
                    }
                }
            }
        }
    }
    Ok(())
}
