use anyhow::{Context, Result};
use bsv_sdk::wallet::{ListOutputsArgs, WalletInterface};
use serde::Deserialize;
use std::collections::HashSet;

use crate::brc29;
use crate::commands::receive;
use crate::context::WalletContext;

#[derive(Deserialize)]
struct WocUnspent {
    tx_hash: String,
    tx_pos: u32,
    value: u64,
}

pub async fn run(ctx: &WalletContext, reconcile_spent: bool) -> Result<()> {
    let address = brc29::deposit_address(&ctx.root_key, ctx.chain)?;
    let base = receive::woc_base(ctx.chain);
    let client = reqwest::Client::new();

    let unspent: Vec<WocUnspent> = client
        .get(format!("{}/address/{}/unspent", base, address))
        .send()
        .await
        .with_context(|| format!("WoC unspent fetch failed for {}", address))?
        .error_for_status()?
        .json()
        .await?;

    let known = known_outpoints(ctx).await?;

    let mut received = 0u32;
    let mut skipped = 0u32;
    let mut sats_in = 0u64;

    for u in &unspent {
        let outpoint = format!("{}.{}", u.tx_hash, u.tx_pos);
        if known.contains(&outpoint) {
            skipped += 1;
            continue;
        }
        match receive::receive_txid(ctx, &u.tx_hash, Some(u.tx_pos)).await {
            Ok((_, true)) => {
                received += 1;
                sats_in += u.value;
            }
            Ok((_, false)) => {
                eprintln!("not accepted: {}", outpoint);
            }
            Err(e) => {
                eprintln!("failed {}: {}", outpoint, e);
            }
        }
    }

    // --reconcile-spent (2026-08-27): a restored-from-backup wallet holds rows
    // for outputs the chain has since seen SPENT; selecting them builds
    // double-spend inputs the network refuses (the fleet-restore incident).
    // For every DB outpoint MISSING from the chain's unspent set at the
    // deposit address, ask WoC's per-outpoint spent endpoint; a definitive
    // spender ⇒ relinquish. Anything else (404 / transport) is LEFT ALONE:
    // only a positive spent answer may remove spendability (fail-safe —
    // unknown never relinquishes).
    let mut reconciled = 0u32;
    let mut reconcile_checked = 0u32;
    if reconcile_spent {
        let chain_unspent: HashSet<String> = unspent
            .iter()
            .map(|u| format!("{}.{}", u.tx_hash, u.tx_pos))
            .collect();
        let db_outpoints = known_outpoints(ctx).await?;
        for op in db_outpoints {
            if chain_unspent.contains(&op) {
                continue;
            }
            let Some((txid, vout)) = op.split_once('.') else { continue };
            reconcile_checked += 1;
            tokio::time::sleep(std::time::Duration::from_millis(350)).await;
            let spent = match client
                .get(format!("{}/tx/{}/{}/spent", base, txid, vout))
                .send()
                .await
            {
                Ok(r) if r.status().is_success() => true,
                _ => false,
            };
            if spent {
                use bsv_sdk::wallet::RelinquishOutputArgs;
                match ctx
                    .wallet
                    .relinquish_output(
                        RelinquishOutputArgs {
                            basket: "default".to_string(),
                            output: match bsv_sdk::wallet::Outpoint::from_string(&op) {
                                Ok(o) => o,
                                Err(e) => {
                                    eprintln!("bad outpoint {}: {}", op, e);
                                    continue;
                                }
                            },
                        },
                        "bsv-wallet-cli",
                    )
                    .await
                {
                    Ok(_) => {
                        reconciled += 1;
                        eprintln!("reconciled spent: {}", op);
                    }
                    Err(e) => eprintln!("relinquish failed {}: {}", op, e),
                }
            }
        }
    }

    if ctx.json_output {
        println!(
            "{}",
            serde_json::json!({
                "address": address,
                "unspent_on_chain": unspent.len(),
                "received": received,
                "skipped": skipped,
                "sats_received": sats_in,
                "reconcile_checked": reconcile_checked,
                "reconciled_spent": reconciled,
            })
        );
    } else {
        println!(
            "Sync complete: {} on chain, {} new received ({} sats), {} already known",
            unspent.len(),
            received,
            sats_in,
            skipped
        );
    }

    Ok(())
}

async fn known_outpoints(ctx: &WalletContext) -> Result<HashSet<String>> {
    let mut known = HashSet::new();
    let mut offset: i32 = 0;
    let limit: u32 = 1000;
    loop {
        let res = ctx
            .wallet
            .list_outputs(
                ListOutputsArgs {
                    basket: "default".to_string(),
                    tags: None,
                    tag_query_mode: None,
                    include: None,
                    include_custom_instructions: None,
                    include_tags: None,
                    include_labels: None,
                    limit: Some(limit),
                    offset: Some(offset),
                    seek_permission: None,
                },
                "bsv-wallet-cli",
            )
            .await?;
        let n = res.outputs.len() as u32;
        for o in &res.outputs {
            known.insert(o.outpoint.to_string());
        }
        if n < limit {
            break;
        }
        offset += n as i32;
    }
    Ok(known)
}
