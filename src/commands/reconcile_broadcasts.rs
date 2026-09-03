//! `bsv-wallet reconcile-broadcasts`: one pass of the broadcast reconciler
//! by hand (see `broadcast_reconcile`). Dry run by default; `--execute`
//! applies the poison retirements and the locked-input restores.

use anyhow::Result;
use bsv_wallet_toolbox::{LockedInputVerdict, PoisonOutcome};

use crate::broadcast_reconcile::{self, ReconcileOptions};
use crate::broadcast_verify::BroadcastVerifier;
use crate::context::WalletContext;

pub async fn run(ctx: &WalletContext, execute: bool, max_probes: usize) -> Result<()> {
    let verifier = BroadcastVerifier::single_pass(ctx.chain);
    let opts = ReconcileOptions::for_command(
        execute,
        max_probes,
        broadcast_reconcile::arcade_sse_for(&ctx.db_path),
    );
    println!(
        "Reconciling broadcasts{} (absence threshold {} min, up to {} probe(s){})...",
        if execute { "" } else { " [dry run]" },
        opts.absence_minutes,
        opts.max_probes,
        if opts.sse.is_some() {
            ", Arcade SSE drained first"
        } else {
            ""
        }
    );
    let report = broadcast_reconcile::run_pass(
        ctx.wallet.storage(),
        ctx.wallet.services(),
        &verifier,
        &opts,
    )
    .await?;

    if ctx.json_output {
        let retired: Vec<serde_json::Value> = report
            .retired
            .iter()
            .map(|r| {
                serde_json::json!({
                    "root": r.root,
                    "origin": r.origin,
                    "climbed": r.climbed,
                    "outcome": format!("{:?}", r.outcome),
                    "executed": r.executed,
                    "chain": r.chain.iter().map(|t| serde_json::json!({
                        "txid": t.txid, "status": t.status, "depth": t.depth, "isOutgoing": t.is_outgoing,
                    })).collect::<Vec<_>>(),
                    "failed": r.failed,
                    "restored": r.restored,
                    "restoredSats": r.restored_sats,
                    "keptLocked": r.kept,
                    "invalidated": r.invalidated,
                    "invalidatedSats": r.invalidated_sats,
                    "internalized": r.internalized.iter().map(|p| serde_json::json!({
                        "txid": p.txid, "vout": p.vout, "satoshis": p.satoshis,
                    })).collect::<Vec<_>>(),
                })
            })
            .collect();
        let locked: Vec<serde_json::Value> = report
            .locked
            .checks
            .iter()
            .map(|c| {
                serde_json::json!({
                    "outputId": c.output_id,
                    "outpoint": format!("{}:{}", c.source_txid, c.vout),
                    "satoshis": c.satoshis,
                    "lockedBy": c.locked_by,
                    "verdict": format!("{:?}", c.verdict),
                    "attempts": c.attempts,
                    "nextCheckMinutes": c.next_check_minutes,
                })
            })
            .collect();
        println!(
            "{}",
            serde_json::json!({
                "executed": execute,
                "sseEvents": report.sse_events,
                "candidates": report.candidates,
                "fresh": report.fresh,
                "probed": report.probed,
                "chainSeen": report.seen,
                "chainMined": report.mined,
                "held": report.held,
                "inconclusive": report.inconclusive,
                "absent": report.absent.iter().map(|(t, m)| serde_json::json!({"txid": t, "ageMinutes": m})).collect::<Vec<_>>(),
                "fatal": report.fatal,
                "retired": retired,
                "lockedInputs": {
                    "adopted": report.locked.adopted,
                    "due": report.locked.due,
                    "restored": report.locked.restored,
                    "restoredSats": report.locked.restored_sats,
                    "spent": report.locked.spent,
                    "unknown": report.locked.unknown,
                    "checks": locked,
                },
            })
        );
        return Ok(());
    }

    if report.sse_events > 0 {
        println!(
            "Arcade SSE: {} status event(s) applied{}",
            report.sse_events,
            if report.sse_fatal.is_empty() {
                String::new()
            } else {
                format!(", {} fatal", report.sse_fatal.len())
            }
        );
    }
    println!(
        "Candidates: {} unproven/stale-sending; {} with fresh chain evidence, {} probed.",
        report.candidates, report.fresh, report.probed
    );
    for txid in &report.seen {
        println!("    chain index has it: {}", txid);
    }
    for txid in &report.mined {
        println!("    mined: {}", txid);
    }
    for txid in &report.held {
        println!("    held by a store, chain index gave no answer: {}", txid);
    }
    for txid in &report.inconclusive {
        println!("    inconclusive (kept): {}", txid);
    }
    for (txid, age) in &report.absent {
        println!(
            "    ABSENT from the chain index (the broadcaster holds it), {} min old{}: {}",
            age,
            if *age >= opts.absence_minutes {
                " (past the threshold)"
            } else {
                ""
            },
            txid
        );
    }
    for txid in &report.fatal {
        println!("    REJECTED by the broadcaster: {}", txid);
    }

    if report.retired.is_empty() {
        println!("No poisoned chains.");
    } else {
        println!();
        println!("Poisoned chains:");
        for r in &report.retired {
            let climb = if r.climbed.is_empty() {
                String::new()
            } else {
                format!(
                    " (climbed from {} through {} step(s))",
                    r.origin,
                    r.climbed.len()
                )
            };
            match &r.outcome {
                PoisonOutcome::Retired => {
                    println!(
                        "  root {}{} : {} transaction(s) {}",
                        r.root,
                        climb,
                        r.retirable_txids().len(),
                        if r.executed {
                            "retired"
                        } else {
                            "would be retired"
                        }
                    );
                    for tx in &r.chain {
                        println!(
                            "      depth {} {} ({}{})",
                            tx.depth,
                            tx.txid,
                            tx.status,
                            if tx.is_outgoing { "" } else { ", received" }
                        );
                    }
                    if r.executed {
                        println!(
                            "      failed {}, inputs restored {} ({} sats), kept locked {} (re-checked with backoff), outputs invalidated {} ({} sats)",
                            r.failed, r.restored, r.restored_sats, r.kept, r.invalidated, r.invalidated_sats
                        );
                        for p in &r.internalized {
                            println!(
                                "      internalized payment {}:{} ({} sats) traces to a phantom source: unspendable",
                                p.txid, p.vout, p.satoshis
                            );
                        }
                    }
                }
                PoisonOutcome::Alive => {
                    println!(
                        "  root {}{} : alive per the status service, kept",
                        r.root, climb
                    );
                }
                PoisonOutcome::Refused { proven_txid } => {
                    println!(
                        "  root {}{} : REFUSED, descendant {} is proven",
                        r.root, climb, proven_txid
                    );
                }
                PoisonOutcome::NotFound => {
                    println!("  root {} : not in this wallet", r.root);
                }
            }
        }
    }

    let locked = &report.locked;
    if locked.due > 0 || locked.adopted > 0 {
        println!();
        println!(
            "Locked inputs of failed transactions: {} adopted, {} due, {} restored ({} sats), {} spent (left), {} undecided (backoff), {} dropped.",
            locked.adopted, locked.due, locked.restored, locked.restored_sats, locked.spent, locked.unknown, locked.dropped
        );
        for c in &locked.checks {
            let what = match c.verdict {
                LockedInputVerdict::Restored => {
                    if execute {
                        "UNSPENT on chain: restored to coin selection".to_string()
                    } else {
                        "UNSPENT on chain: would be restored".to_string()
                    }
                }
                LockedInputVerdict::Spent => {
                    "SPENT on chain by another transaction: left locked".to_string()
                }
                LockedInputVerdict::Unknown => format!(
                    "undecided (attempt {}), next re-check in {} min",
                    c.attempts,
                    c.next_check_minutes.unwrap_or(0)
                ),
                LockedInputVerdict::Phantom => "source is a retired phantom: dropped".to_string(),
                LockedInputVerdict::Released => "no longer locked: dropped".to_string(),
            };
            println!(
                "    {}:{} ({} sats, locked by {}): {}",
                c.source_txid, c.vout, c.satoshis, c.locked_by, what
            );
        }
    }

    if !execute
        && (report
            .retired
            .iter()
            .any(|r| r.outcome == PoisonOutcome::Retired)
            || locked.restored > 0)
    {
        println!();
        println!("Dry run. Re-run with --execute to apply.");
    }
    Ok(())
}
