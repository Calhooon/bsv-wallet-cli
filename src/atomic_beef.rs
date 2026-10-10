use anyhow::{anyhow, Result};
use bsv_sdk::transaction::Beef;
use std::collections::HashSet;

/// Reads a stranger's BEEF through bsv-rs's streaming reader (the toolbox's
/// `refuse_invalid_beef_bytes`) and refuses it only for invalid bytes: the
/// offset of the byte in the caller's own bytes and the reader's kind (a cut
/// field, a bad varint, a transaction with no input, trailing bytes, ...).
/// Nothing is refused for its size or its counts. The bytes are already held
/// whole by the caller (a hex argument, a courier's response body).
pub fn refuse_invalid_bytes(beef_bytes: &[u8]) -> Result<()> {
    bsv_wallet_toolbox::storage::sqlx::refuse_invalid_beef_bytes(beef_bytes)?;
    Ok(())
}

pub fn ensure_atomic(beef_bytes: &[u8]) -> Result<Vec<u8>> {
    refuse_invalid_bytes(beef_bytes)?;
    let mut beef = Beef::from_binary(beef_bytes)?;
    let target_txid = match &beef.atomic_txid {
        Some(t) => t.clone(),
        None => find_leaf_txid(&beef)?,
    };
    Ok(beef.to_binary_atomic(&target_txid)?)
}

pub fn find_leaf_txid(beef: &Beef) -> Result<String> {
    let beef_txids: HashSet<String> = beef.txs.iter().map(|t| t.txid()).collect();
    let referenced: HashSet<String> = beef
        .txs
        .iter()
        .flat_map(|t| t.input_txids.iter().cloned())
        .collect();
    let leaves: Vec<String> = beef_txids.difference(&referenced).cloned().collect();
    match leaves.len() {
        1 => Ok(leaves.into_iter().next().unwrap()),
        0 => Err(anyhow!(
            "BEEF has no leaf transaction (every tx is referenced as an input — looks like a cycle)"
        )),
        n => Err(anyhow!(
            "BEEF has {} leaf transactions; ambiguous which one to internalize. \
             Convert to AtomicBEEF first or pass a single-target BEEF.",
            n
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{p2pkh, raw_tx, txid_of};
    use bsv_sdk::primitives::{sha256d, to_hex};
    use bsv_sdk::transaction::MerklePath;

    /// A version 1 transaction with no input and one output (invalid bytes
    /// at bsv-rs 0.4.1: a transaction with no input is `NoInputs`).
    fn no_input_tx() -> Vec<u8> {
        let lock = p2pkh([0xdb; 20]);
        let mut tx = vec![1, 0, 0, 0, 0, 1];
        tx.extend_from_slice(&1000u64.to_le_bytes());
        tx.push(lock.len() as u8);
        tx.extend_from_slice(&lock);
        tx.extend_from_slice(&[0, 0, 0, 0]);
        tx
    }

    fn display_txid(raw: &[u8]) -> String {
        let mut h = sha256d(raw).to_vec();
        h.reverse();
        to_hex(&h)
    }

    /// `raw` under a one-transaction block's path, as BEEF bytes.
    fn proven_beef(raw: Vec<u8>, txid: &str) -> Vec<u8> {
        let mut beef = Beef::new();
        let bump = beef.merge_bump(MerklePath::from_coinbase_txid(txid, 900_000));
        beef.merge_raw_tx(raw, Some(bump));
        beef.to_binary()
    }

    fn offset_of(haystack: &[u8], needle: &[u8]) -> usize {
        haystack
            .windows(needle.len())
            .position(|w| w == needle)
            .expect("the transaction is in the BEEF")
    }

    /// A stranger's BEEF carrying a transaction with no input is refused at
    /// the transaction's leading byte, even under a BUMP whose root the
    /// headers would carry; the same frame around one input is taken.
    #[test]
    fn a_transaction_with_no_input_is_refused_at_its_offset() {
        let raw = no_input_tx();
        let bytes = proven_beef(raw.clone(), &display_txid(&raw));
        let at = offset_of(&bytes, &raw);

        let err = ensure_atomic(&bytes).expect_err("a transaction with no input is invalid bytes");
        let text = err.to_string();
        assert!(
            text.contains(&format!("Invalid BEEF at byte {at}")) && text.contains("NoInputs"),
            "{text}"
        );

        let control = raw_tx(&[(&"11".repeat(32), 0)], &[(1000, p2pkh([0xdb; 20]))]);
        let control_id = txid_of(&control);
        let atomic = ensure_atomic(&proven_beef(control, &control_id)).expect("one input");
        assert_eq!(
            Beef::from_binary(&atomic).unwrap().atomic_txid,
            Some(control_id)
        );
    }

    /// A stranger's BEEF carrying a transaction with one input and no output
    /// is refused at the transaction's leading byte under a BUMP too
    /// (bsv-rs 0.4.3, bsv-stack-lean #59); the same frame around one output
    /// is taken.
    #[test]
    fn a_transaction_with_no_output_is_refused_at_its_offset() {
        let raw = raw_tx(&[(&"11".repeat(32), 0)], &[]);
        let bytes = proven_beef(raw.clone(), &display_txid(&raw));
        let at = offset_of(&bytes, &raw);

        let err = ensure_atomic(&bytes).expect_err("a transaction with no output is invalid bytes");
        let text = err.to_string();
        assert!(
            text.contains(&format!("Invalid BEEF at byte {at}")) && text.contains("NoOutputs"),
            "{text}"
        );

        let control = raw_tx(&[(&"11".repeat(32), 0)], &[(1000, p2pkh([0xdb; 20]))]);
        let control_id = txid_of(&control);
        assert!(ensure_atomic(&proven_beef(control, &control_id)).is_ok());
    }

    /// Bytes cut short are refused at the field that ran out, with the
    /// reader's kind, not the parser's text.
    #[test]
    fn a_cut_beef_is_refused_at_the_field_that_ran_out() {
        let raw = raw_tx(&[(&"11".repeat(32), 0)], &[(1000, p2pkh([0xdb; 20]))]);
        let txid = txid_of(&raw);
        let mut bytes = proven_beef(raw, &txid);
        bytes.truncate(bytes.len() - 6);

        let text = ensure_atomic(&bytes).expect_err("cut bytes").to_string();
        assert!(
            text.contains("Invalid BEEF at byte") && text.contains("Truncated"),
            "{text}"
        );
    }
}
