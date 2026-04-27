use anyhow::{anyhow, Result};
use bsv_sdk::transaction::Beef;
use std::collections::HashSet;

pub fn ensure_atomic(beef_bytes: &[u8]) -> Result<Vec<u8>> {
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
