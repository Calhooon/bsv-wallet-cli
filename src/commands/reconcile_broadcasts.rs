//! `bsv-wallet reconcile-broadcasts`: one pass of the broadcast reconciler
//! by hand (see `broadcast_reconcile`). Dry run by default; `--execute`
//! applies the poison retirements.

use anyhow::Result;
use bsv_wallet_toolbox::PoisonOutcome;

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
    let report = broadcast_reconcile::run_pass(&ctx.wallet, &verifier, &opts).await?;

    if ctx.json_output {
        let retired: Vec<serde_json::Value> = report
            .retired
            .iter()
            .map(|r| {
                serde_json::json!({
                    "root": r.root,
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
        println!(
            "{}",
            serde_json::json!({
                "executed": execute,
                "sseEvents": report.sse_events,
                "candidates": report.candidates,
                "fresh": report.fresh,
                "probed": report.probed,
                "seen": report.seen,
                "mined": report.mined,
                "held": report.held,
                "inconclusive": report.inconclusive,
                "absent": report.absent.iter().map(|(t, m)| serde_json::json!({"txid": t, "minutes": m})).collect::<Vec<_>>(),
                "fatal": report.fatal,
                "retired": retired,
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
        "Candidates: {} unproven/stale-sending; {} with fresh network evidence, {} probed.",
        report.candidates, report.fresh, report.probed
    );
    for txid in &report.seen {
        println!("    seen on the network: {}", txid);
    }
    for txid in &report.mined {
        println!("    mined: {}", txid);
    }
    for txid in &report.held {
        println!("    held by a store, no network evidence: {}", txid);
    }
    for txid in &report.inconclusive {
        println!("    inconclusive (kept): {}", txid);
    }
    for (txid, minutes) in &report.absent {
        println!(
            "    ABSENT from every network source for {} min{}: {}",
            minutes,
            if *minutes >= opts.absence_minutes {
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
            match &r.outcome {
                PoisonOutcome::Retired => {
                    println!(
                        "  root {} : {} transaction(s) {}",
                        r.root,
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
                            "      failed {}, inputs restored {} ({} sats), kept locked {}, outputs invalidated {} ({} sats)",
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
                    println!("  root {} : alive per the status service, kept", r.root);
                }
                PoisonOutcome::Refused { proven_txid } => {
                    println!(
                        "  root {} : REFUSED, descendant {} is proven",
                        r.root, proven_txid
                    );
                }
                PoisonOutcome::NotFound => {
                    println!("  root {} : not in this wallet", r.root);
                }
            }
        }
    }

    if !execute
        && report
            .retired
            .iter()
            .any(|r| r.outcome == PoisonOutcome::Retired)
    {
        println!();
        println!("Dry run. Re-run with --execute to apply.");
    }
    Ok(())
}
