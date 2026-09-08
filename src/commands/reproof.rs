//! `bsv-wallet reproof` (M19 R1, 2026-09-08): re-prove stored merkle proofs the
//! chain no longer confirms.
//!
//! After a reorg a wallet can hold `proven_txs` rows anchored to a block that
//! left the chain (2026-09-07: 28 fleet seats kept proofs against the orphan
//! at 965771; every spend touching one was refused with "Invalid merkle
//! root"). The daemon's header, reorg and review tasks now repair this on
//! their own; this verb is the same repair on demand, with a table first.
//!
//! For every stored proof in the height window it compares the stored block
//! hash (and merkle root) with the canonical header, lists the disagreements,
//! and, with `--execute`, runs the toolbox's `reprove_anchor` on each: a
//! provider's validated proof for the canonical block REPLACES the stored
//! one; with no replacement the proof is DEMOTED (the transaction is unmined
//! again and the monitor re-proves it on its next pass; `bsv-wallet tick`
//! runs that pass now). Dry-run by default; an unknown header never counts
//! as stale.

use std::collections::{BTreeMap, HashMap};

use anyhow::{Context, Result};
use bsv_wallet_toolbox::monitor::reorg_ops::{
    block_hash_of_header, merkle_root_of_header, reprove_anchor, stale_anchors_by_root, ReproveOutcome,
    ReproveTally,
};
use bsv_wallet_toolbox::services::WalletServices;
use bsv_wallet_toolbox::storage::{MonitorStorage, ProvenTxAnchor};
use serde::Serialize;

use crate::context::WalletContext;

/// The window below the tip when neither `--since-height` nor `--all` is given.
const DEFAULT_WINDOW_BLOCKS: u32 = 288;

#[derive(Serialize)]
struct StaleRow {
    txid: String,
    height: u32,
    stored_block_hash: String,
    canonical_block_hash: String,
    stored_merkle_root: String,
    canonical_merkle_root: String,
    action: String,
}

#[derive(Serialize)]
struct Report {
    tip: u32,
    min_height: u32,
    max_height: u32,
    stored_proofs_in_window: usize,
    heights_checked: usize,
    heights_unreadable: usize,
    stale: Vec<StaleRow>,
    executed: bool,
    replaced: u32,
    demoted: u32,
    deferred: u32,
    unchanged: u32,
    errors: Vec<String>,
}

pub async fn run(ctx: &WalletContext, since_height: Option<u32>, all: bool, execute: bool) -> Result<()> {
    let storage = ctx.wallet.storage();
    let services = ctx.wallet.services();

    let tip = services.get_height().await.context("chain tip unavailable")?;
    let (min_height, max_height) = if all {
        (0, tip)
    } else {
        (since_height.unwrap_or(tip.saturating_sub(DEFAULT_WINDOW_BLOCKS)), tip)
    };
    let anchors: Vec<ProvenTxAnchor> = storage
        .find_proven_txs_in_heights(min_height, max_height)
        .await
        .context("reading stored proofs")?;

    // One header read per distinct height; an unreadable header leaves that
    // height out of the verdict (never stale by default).
    let mut heights: Vec<u32> = anchors.iter().map(|a| a.height).collect();
    heights.sort_unstable();
    heights.dedup();
    let mut canonical_roots: HashMap<u32, String> = HashMap::new();
    let mut canonical_hashes: HashMap<u32, String> = HashMap::new();
    let mut unreadable = 0usize;
    for h in &heights {
        match services.get_header_for_height(*h).await {
            Ok(bytes) => {
                if let Some(root) = merkle_root_of_header(&bytes) {
                    canonical_roots.insert(*h, root);
                }
                if let Some(hash) = block_hash_of_header(&bytes) {
                    canonical_hashes.insert(*h, hash);
                }
            }
            Err(e) => {
                unreadable += 1;
                eprintln!("height {}: header unreadable ({}); skipped", h, e);
            }
        }
    }

    let stale: Vec<ProvenTxAnchor> = stale_anchors_by_root(&anchors, &canonical_roots)
        .into_iter()
        .cloned()
        .collect();

    let mut rows: BTreeMap<(u32, String), StaleRow> = BTreeMap::new();
    for a in &stale {
        rows.insert(
            (a.height, a.txid.clone()),
            StaleRow {
                txid: a.txid.clone(),
                height: a.height,
                stored_block_hash: a.block_hash.clone(),
                canonical_block_hash: canonical_hashes.get(&a.height).cloned().unwrap_or_default(),
                stored_merkle_root: a.merkle_root.clone(),
                canonical_merkle_root: canonical_roots.get(&a.height).cloned().unwrap_or_default(),
                action: if execute { "pending".into() } else { "dry-run".into() },
            },
        );
    }

    let mut tally = ReproveTally::default();
    if execute {
        for a in &stale {
            let outcome = reprove_anchor(storage, services, a).await;
            tally.record(&a.txid, &outcome);
            let action = match &outcome {
                ReproveOutcome::Replaced { height, block_hash } => {
                    format!("replaced → {} {}", height, short(block_hash))
                }
                ReproveOutcome::Unchanged => "unchanged (the providers name the stored block)".into(),
                ReproveOutcome::Deferred { height } => format!("deferred (height {} has not aged)", height),
                ReproveOutcome::Demoted => "demoted → unmined (run `bsv-wallet tick` to re-prove)".into(),
                ReproveOutcome::TransientError(e) => format!("error: {}", e),
            };
            if let Some(r) = rows.get_mut(&(a.height, a.txid.clone())) {
                r.action = action;
            }
        }
    }

    let report = Report {
        tip,
        min_height,
        max_height,
        stored_proofs_in_window: anchors.len(),
        heights_checked: canonical_roots.len(),
        heights_unreadable: unreadable,
        stale: rows.into_values().collect(),
        executed: execute,
        replaced: tally.replaced,
        demoted: tally.demoted,
        deferred: tally.deferred,
        unchanged: tally.unchanged,
        errors: tally.errors.clone(),
    };

    if ctx.json_output {
        println!("{}", serde_json::to_string_pretty(&report)?);
        return Ok(());
    }

    println!(
        "reproof: tip {} · window {}..{} · {} stored proof(s) · {} height(s) checked · {} unreadable",
        report.tip, report.min_height, report.max_height, report.stored_proofs_in_window, report.heights_checked, report.heights_unreadable
    );
    if report.stale.is_empty() {
        println!("every stored proof in the window matches the canonical header");
    } else {
        println!("txid             height  stored block       canonical block    action");
        for r in &report.stale {
            println!(
                "{:<14} {:>8}  {:<18} {:<18} {}",
                &r.txid[..12],
                r.height,
                short(&r.stored_block_hash),
                short(&r.canonical_block_hash),
                r.action
            );
        }
    }
    if execute {
        println!(
            "executed: {} replaced · {} demoted · {} deferred · {} unchanged · {} error(s)",
            report.replaced, report.demoted, report.deferred, report.unchanged, report.errors.len()
        );
        for e in &report.errors {
            println!("  error: {}", e);
        }
        if report.demoted > 0 {
            println!("demoted proofs are re-proved by the monitor; `bsv-wallet tick` runs that pass now");
        }
    } else if !report.stale.is_empty() {
        println!("dry run; re-run with --execute to replace or demote the {} stale proof(s)", report.stale.len());
    }
    Ok(())
}

/// Block hashes begin with sixteen or more zeros; the TAIL tells them apart.
fn short(h: &str) -> String {
    if h.is_empty() {
        "(none)".into()
    } else if h.len() > 16 {
        format!("…{}", &h[h.len() - 16..])
    } else {
        h.to_string()
    }
}
