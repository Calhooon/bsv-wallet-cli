use anyhow::{Context, Result};
use bsv_sdk::wallet::{ListOutputsArgs, WalletInterface};
use serde::Deserialize;
use std::collections::HashSet;

use crate::brc29;
use crate::commands::cleanup_abandoned::{probe_input_spend, stored_locking_script, InputSpend};
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

    tracing::warn!(
        marker = "break_glass_chain_scan",
        "sync is a chain scan at an explorer (break-glass): the routine way to receive is \
         `fund` with the BEEF the payer hands over"
    );
    // Break-glass (Rule 28, C4): a CHAIN SCAN. "Which outputs pay our
    // deposit address" has no header, proof or own-index answer by
    // construction: it is the question of a payment nobody handed us. The
    // routine path is `fund` with a BEEF from the payer, which asks no
    // explorer. This read is an operator's command only (no daemon path
    // reaches it), one explorer, and its negative is weak on purpose: an
    // empty list is "this explorer lists nothing", never "nothing was
    // paid", and nothing is removed on it. A fault is an error ("could not
    // look"), never an empty list.
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
    // Every DB outpoint MISSING from the chain's unspent set at the deposit
    // address is put to the ONE spend probe (Rule 28, C5 and C7: the same
    // function `cleanup-abandoned` and `reconcile-outputs` ask, with the
    // same three answers; this command no longer holds a copy with its own
    // error rule). Only a spend we hold a proof of relinquishes; a named
    // but unproven spender, and anything the probe could not decide, is
    // LEFT ALONE and counted (fail-safe: unknown never relinquishes).
    let mut reconciled = 0u32;
    let mut reconcile_checked = 0u32;
    let mut spent_unproven = 0u32;
    let mut unknown = 0u32;
    if reconcile_spent {
        let chain_unspent: HashSet<String> = unspent
            .iter()
            .map(|u| format!("{}.{}", u.tx_hash, u.tx_pos))
            .collect();
        let db_outpoints = known_outpoints(ctx).await?;
        let pool = ctx.wallet.storage().pool();
        for op in db_outpoints {
            if chain_unspent.contains(&op) {
                continue;
            }
            let Some((txid, vout)) = op.split_once('.') else {
                continue;
            };
            let Ok(vout) = vout.parse::<u32>() else {
                continue;
            };
            reconcile_checked += 1;
            let script = stored_locking_script(pool, txid, vout).await;
            let answer = probe_input_spend(
                &client,
                base,
                ctx.wallet.services(),
                txid,
                vout,
                script.as_deref(),
            )
            .await;
            match reconcile_step(&answer) {
                ReconcileStep::Keep => {}
                ReconcileStep::LeaveUnproven => {
                    spent_unproven += 1;
                    eprintln!(
                        "spent by a transaction we hold no proof of yet (left alone): {}",
                        op
                    );
                }
                ReconcileStep::LeaveUnknown => {
                    unknown += 1;
                    eprintln!(
                        "could not decide (no spender named and not in an unspent set, or could not look): {}",
                        op
                    );
                }
                ReconcileStep::Relinquish => {
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
                "spent_unproven": spent_unproven,
                "unknown": unknown,
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
        // A money verb must never finish SILENT about what it did (or did not)
        // touch: "checked 0" (nothing qualified) and "checked 12, relinquished
        // 0" (all verified live) are different facts a drain decision rests on.
        if reconcile_spent {
            println!(
                "Reconcile: {} outpoint(s) chain-checked, {} relinquished as spent (proven), {} spent but not proven yet (left alone), {} undecided (left alone; run cleanup-abandoned)",
                reconcile_checked, reconciled, spent_unproven, unknown
            );
        }
    }

    Ok(())
}

/// What `--reconcile-spent` does with one probe answer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ReconcileStep {
    /// In an unspent set: the row is right.
    Keep,
    /// Spent by a transaction we hold a proof of: the coin is gone.
    Relinquish,
    /// A spender is named but not proven: left alone, counted.
    LeaveUnproven,
    /// The probe could not decide: left alone, counted.
    LeaveUnknown,
}

/// Only a proven spend may remove spendability.
fn reconcile_step(answer: &InputSpend) -> ReconcileStep {
    match answer {
        InputSpend::Unspent => ReconcileStep::Keep,
        InputSpend::SpentBy {
            confirmed: true, ..
        } => ReconcileStep::Relinquish,
        InputSpend::SpentBy {
            confirmed: false, ..
        } => ReconcileStep::LeaveUnproven,
        InputSpend::Unknown => ReconcileStep::LeaveUnknown,
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    /// C5 (Rule 28): `sync --reconcile-spent` holds no spend probe of its
    /// own. Red at the base: the copy asked `/tx/{txid}/{vout}/spent` and
    /// `/tx/hash/{txid}` here, and read any failure as "not spent".
    #[test]
    fn sync_holds_no_copy_of_the_spend_probe() {
        let source = include_str!("sync.rs");
        let code = source.split("#[cfg(test)]").next().unwrap();
        assert!(!code.contains("/spent\""), "a spent request in sync.rs");
        assert!(!code.contains("/tx/hash/"), "a parent request in sync.rs");
        assert!(
            code.contains("probe_input_spend("),
            "the one probe is asked"
        );
    }

    /// C4 (Rule 28): the scan is named a chain scan and a break-glass read
    /// at the site, and says so when it runs. Red at the base: neither.
    #[test]
    fn the_address_scan_is_named_at_the_site() {
        let source = include_str!("sync.rs");
        let code = source.split("#[cfg(test)]").next().unwrap();
        let site = code.find("/address/{}/unspent").expect("the scan");
        let before = &code[..site];
        assert!(
            before.contains("Break-glass (Rule 28, C4): a CHAIN SCAN"),
            "the scan is not named at its site"
        );
        assert!(
            before.contains("break_glass_chain_scan"),
            "and says so when run"
        );
    }

    /// Only a proven spend relinquishes. Red at the base: any 200 from the
    /// explorer's spent route relinquished, mined or not.
    #[test]
    fn only_a_proven_spend_relinquishes() {
        let spender = "bb".repeat(32);
        assert_eq!(
            reconcile_step(&InputSpend::SpentBy {
                txid: spender.clone(),
                confirmed: true
            }),
            ReconcileStep::Relinquish
        );
        assert_eq!(
            reconcile_step(&InputSpend::SpentBy {
                txid: spender,
                confirmed: false
            }),
            ReconcileStep::LeaveUnproven
        );
        assert_eq!(
            reconcile_step(&InputSpend::Unknown),
            ReconcileStep::LeaveUnknown
        );
        assert_eq!(reconcile_step(&InputSpend::Unspent), ReconcileStep::Keep);
    }
}
