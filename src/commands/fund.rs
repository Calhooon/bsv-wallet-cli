use anyhow::{anyhow, Result};
use bsv_sdk::transaction::Beef;
use bsv_sdk::wallet::{InternalizeActionArgs, InternalizeOutput, WalletInterface, WalletPayment};
use std::collections::HashSet;

use crate::brc29;
use crate::context::WalletContext;

pub async fn run(ctx: &WalletContext, beef_hex: &str, vout: u32) -> Result<()> {
    let beef_bytes = hex::decode(beef_hex)?;
    let atomic_bytes = ensure_atomic(&beef_bytes)?;

    let (_, anyone_pubkey) = bsv_sdk::wallet::KeyDeriver::anyone_key();
    let sender_identity_key = anyone_pubkey.to_hex();

    let args = InternalizeActionArgs {
        tx: atomic_bytes,
        outputs: vec![InternalizeOutput {
            output_index: vout,
            protocol: "wallet payment".to_string(),
            payment_remittance: Some(WalletPayment {
                derivation_prefix: brc29::DEFAULT_DERIVATION_PREFIX.to_string(),
                derivation_suffix: brc29::DEFAULT_DERIVATION_SUFFIX.to_string(),
                sender_identity_key,
            }),
            insertion_remittance: None,
        }],
        description: "Internalize external funding".to_string(),
        labels: Some(vec!["funding".to_string()]),
        seek_permission: None,
    };

    let result = ctx
        .wallet
        .internalize_action(args, "bsv-wallet-cli")
        .await?;

    if ctx.json_output {
        println!("{}", serde_json::json!({ "accepted": result.accepted }));
    } else if result.accepted {
        println!("Transaction internalized successfully");
    } else {
        println!("Transaction was not accepted");
    }

    Ok(())
}

fn ensure_atomic(beef_bytes: &[u8]) -> Result<Vec<u8>> {
    let mut beef = Beef::from_binary(beef_bytes)?;
    let target_txid = match &beef.atomic_txid {
        Some(t) => t.clone(),
        None => find_leaf_txid(&beef)?,
    };
    Ok(beef.to_binary_atomic(&target_txid)?)
}

fn find_leaf_txid(beef: &Beef) -> Result<String> {
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
