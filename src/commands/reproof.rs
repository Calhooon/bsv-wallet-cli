//! `bsv-wallet reproof`: re-prove stored merkle proofs the chain no longer
//! confirms.
//!
//! After a reorg a wallet can hold `proven_txs` rows anchored to a block that
//! left the chain (2026-09-07: 28 fleet seats kept proofs against the orphan
//! at 965771; every spend touching one was refused with "Invalid merkle
//! root"). The daemon's header, reorg and review tasks repair this on their
//! own; this verb is the same repair on demand, with a table first.
//!
//! For every stored proof in the height window it compares the stored merkle
//! root with the canonical header's, lists the disagreements (a height whose
//! header cannot be read is never stale), and, with `--execute`, runs the
//! toolbox's `reprove_anchor` on each: a provider's validated proof for the
//! canonical block REPLACES the stored one; the providers still naming the
//! stored block, or faulting, RETAIN it (the monitor retries); only positive
//! evidence (the tracker refutes the stored root, two providers answer
//! cleanly "not mined", no provider serves a path) DEMOTES it to the
//! pre-proof state, bytes kept, re-proved by the monitor.
//!
//! The dry run (the default) performs NO storage write: `gather` reads the
//! anchors and the headers and nothing else. `--execute` first raises the
//! persisted proof LAG gate to `tip - 1` when it is 0 or below that (a stated
//! CLI-only divergence from the reference's "survived a cycle": a wallet
//! whose daemon is not running never sees a cycle, and a replacement proof
//! above the gate could not be stored), then re-proves. `--execute` is
//! refused when no chain tracker is configured (`CHAINTRACKS_URL=off`): a
//! heal without a header service cannot refute anything.
//!
//! The default window is the last 288 blocks and INCLUDES the un-aged tip:
//! a replacement proof at the tip is deferred by the gate, not stored, and
//! the monitor re-presents it. `--all` walks every stored proof, one header
//! read per distinct height with no bound; progress is printed every 50
//! heights.

use std::collections::{BTreeMap, HashMap};

use anyhow::{bail, Context, Result};
use bsv_wallet_toolbox::monitor::reorg_ops::{
    block_hash_of_header, merkle_root_of_header, reprove_anchor, stale_anchors_by_root,
    ReproveOutcome, ReproveTally,
};
use bsv_wallet_toolbox::services::WalletServices;
use bsv_wallet_toolbox::storage::{MonitorStorage, ProvenTxAnchor};
use serde::Serialize;

use crate::context::WalletContext;

/// The window below the tip when neither `--since-height` nor `--all` is given.
pub const DEFAULT_WINDOW_BLOCKS: u32 = 288;

/// How often `gather` reports progress over the distinct heights it reads.
pub const PROGRESS_EVERY_HEIGHTS: usize = 50;

/// One stale stored proof, as the table and the JSON report show it.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct StaleRow {
    pub txid: String,
    pub height: u32,
    pub stored_block_hash: String,
    pub canonical_block_hash: String,
    pub stored_merkle_root: String,
    pub canonical_merkle_root: String,
    /// `dry-run` before `--execute`; what `reprove_anchor` did after it.
    pub action: String,
}

/// What the dry run found: reads only.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct Plan {
    pub tip: u32,
    pub min_height: u32,
    pub max_height: u32,
    pub stored_proofs_in_window: usize,
    pub heights_checked: usize,
    pub heights_unreadable: usize,
    /// The persisted proof LAG gate as read (0 = closed).
    pub gate: u32,
    pub stale: Vec<StaleRow>,
}

/// The verb's report: the plan plus what `--execute` did.
#[derive(Debug, Clone, Serialize)]
pub struct Report {
    #[serde(flatten)]
    pub plan: Plan,
    pub executed: bool,
    /// The gate `--execute` raised to `tip - 1`, when it did.
    pub gate_set_to: Option<u32>,
    pub replaced: u32,
    pub demoted: u32,
    pub deferred: u32,
    pub unchanged: u32,
    pub errors: Vec<String>,
}

/// The height window: `--all` is everything, `--since-height` a floor, else
/// the last `DEFAULT_WINDOW_BLOCKS` below the tip. The tip itself is
/// included (the gate defers a replacement there; the monitor re-presents it).
pub fn window(tip: u32, since_height: Option<u32>, all: bool) -> (u32, u32) {
    if all {
        (0, tip)
    } else {
        (
            since_height.unwrap_or(tip.saturating_sub(DEFAULT_WINDOW_BLOCKS)),
            tip,
        )
    }
}

/// Pure: the stale rows, sorted by height then txid. An anchor at a height
/// with no canonical root is never stale.
pub fn plan_rows(
    anchors: &[ProvenTxAnchor],
    canonical_roots: &HashMap<u32, String>,
    canonical_hashes: &HashMap<u32, String>,
) -> Vec<StaleRow> {
    let mut rows: BTreeMap<(u32, String), StaleRow> = BTreeMap::new();
    for a in stale_anchors_by_root(anchors, canonical_roots) {
        rows.insert(
            (a.height, a.txid.clone()),
            StaleRow {
                txid: a.txid.clone(),
                height: a.height,
                stored_block_hash: a.block_hash.clone(),
                canonical_block_hash: canonical_hashes.get(&a.height).cloned().unwrap_or_default(),
                stored_merkle_root: a.merkle_root.clone(),
                canonical_merkle_root: canonical_roots.get(&a.height).cloned().unwrap_or_default(),
                action: "dry-run".to_string(),
            },
        );
    }
    rows.into_values().collect()
}

/// The chain tip: the header service's tip header when there is one, else
/// the height services.
async fn read_tip<V: WalletServices + ?Sized>(services: &V) -> Result<u32> {
    match services.get_chain_tip_header().await {
        Ok(header) => Ok(header.height),
        Err(_) => services.get_height().await.context("chain tip unavailable"),
    }
}

/// READ-ONLY: the stored anchors in the window, the canonical header per
/// distinct height, the stale rows. Performs no storage write. `progress`
/// is called every [`PROGRESS_EVERY_HEIGHTS`] heights with (done, total).
pub async fn gather<S, V>(
    storage: &S,
    services: &V,
    since_height: Option<u32>,
    all: bool,
    progress: &mut dyn FnMut(usize, usize),
) -> Result<Plan>
where
    S: MonitorStorage + ?Sized,
    V: WalletServices + ?Sized,
{
    let tip = read_tip(services).await?;
    let (min_height, max_height) = window(tip, since_height, all);
    let gate = storage
        .max_acceptable_proof_height()
        .await
        .context("reading the proof gate")?;
    let anchors: Vec<ProvenTxAnchor> = storage
        .find_proven_txs_in_heights(min_height, max_height)
        .await
        .context("reading stored proofs")?;

    // One header read per distinct height; an unreadable header leaves that
    // height out of the verdict (never stale by default).
    let mut heights: Vec<u32> = anchors.iter().map(|a| a.height).collect();
    heights.sort_unstable();
    heights.dedup();
    let total = heights.len();
    let mut canonical_roots: HashMap<u32, String> = HashMap::new();
    let mut canonical_hashes: HashMap<u32, String> = HashMap::new();
    let mut unreadable = 0usize;
    for (done, h) in heights.iter().enumerate() {
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
        if (done + 1) % PROGRESS_EVERY_HEIGHTS == 0 {
            progress(done + 1, total);
        }
    }

    Ok(Plan {
        tip,
        min_height,
        max_height,
        stored_proofs_in_window: anchors.len(),
        heights_checked: canonical_roots.len(),
        heights_unreadable: unreadable,
        gate,
        stale: plan_rows(&anchors, &canonical_roots, &canonical_hashes),
    })
}

/// Refuse the heal when no chain tracker is configured: without a header
/// service nothing can be refuted, and a re-prove that cannot refute cannot
/// demote or replace safely.
pub async fn refuse_execute_without_tracker<V: WalletServices + ?Sized>(
    services: &V,
) -> Result<()> {
    if services.get_chain_tracker().await.is_err() {
        bail!(
            "reproof --execute refused: no chain tracker is configured (CHAINTRACKS_URL=off); \
             a heal without a header service cannot refute anything. Run the dry run, or set CHAINTRACKS_URL."
        );
    }
    Ok(())
}

/// The CLI-only gate rule: raise the persisted proof LAG gate to `tip - 1`
/// when it is 0 or below that, so a replacement proof at a buried height
/// can be stored now instead of after a monitor cycle this wallet may never
/// run. Returns the height it was set to, or `None` when it already stood
/// at or above `tip - 1`.
pub async fn raise_gate_for_heal<S: MonitorStorage + ?Sized>(
    storage: &S,
    tip: u32,
) -> Result<Option<u32>> {
    let target = tip.saturating_sub(1);
    let gate = storage
        .max_acceptable_proof_height()
        .await
        .context("reading the proof gate")?;
    if gate == 0 || gate < target {
        storage
            .set_max_acceptable_proof_height(target)
            .await
            .context("raising the proof gate")?;
        Ok(Some(target))
    } else {
        Ok(None)
    }
}

/// The heal: raise the gate, then re-prove every stale row. Returns the
/// report; each row's `action` names what `reprove_anchor` did.
pub async fn apply<S, V>(storage: &S, services: &V, plan: Plan) -> Result<Report>
where
    S: MonitorStorage + ?Sized,
    V: WalletServices + ?Sized,
{
    let gate_set_to = raise_gate_for_heal(storage, plan.tip).await?;
    let mut plan = plan;
    let mut tally = ReproveTally::default();
    for row in plan.stale.iter_mut() {
        let anchor = ProvenTxAnchor {
            txid: row.txid.clone(),
            height: row.height,
            block_hash: row.stored_block_hash.clone(),
            merkle_root: row.stored_merkle_root.clone(),
        };
        let outcome = reprove_anchor(storage, services, &anchor).await;
        tally.record(&anchor.txid, &outcome);
        row.action = match &outcome {
            ReproveOutcome::Replaced { height, block_hash } => {
                format!("replaced by {} {}", height, short(block_hash))
            }
            ReproveOutcome::Unchanged => {
                "unchanged (the providers still name the stored block, or the tracker refutes their path); retained, the monitor retries".into()
            }
            ReproveOutcome::Deferred { height } => {
                format!("deferred (height {} is above the proof gate); the monitor re-presents it", height)
            }
            ReproveOutcome::Demoted => {
                "demoted to unproven (positive evidence); run `bsv-wallet tick` twice to re-prove".into()
            }
            ReproveOutcome::TransientError(e) => format!("retained: {}", e),
        };
    }
    Ok(Report {
        plan,
        executed: true,
        gate_set_to,
        replaced: tally.replaced,
        demoted: tally.demoted,
        deferred: tally.deferred,
        unchanged: tally.unchanged,
        errors: tally.errors,
    })
}

pub async fn run(
    ctx: &WalletContext,
    since_height: Option<u32>,
    all: bool,
    execute: bool,
) -> Result<()> {
    let storage = ctx.wallet.storage();
    let services = ctx.wallet.services();
    if execute {
        refuse_execute_without_tracker(services).await?;
    }

    let mut progress = |done: usize, total: usize| {
        eprintln!("reproof: {} of {} heights read", done, total);
    };
    let plan = gather(storage, services, since_height, all, &mut progress).await?;
    let report = if execute {
        apply(storage, services, plan).await?
    } else {
        Report {
            plan,
            executed: false,
            gate_set_to: None,
            replaced: 0,
            demoted: 0,
            deferred: 0,
            unchanged: 0,
            errors: vec![],
        }
    };

    if ctx.json_output {
        println!("{}", serde_json::to_string_pretty(&report)?);
        return Ok(());
    }

    let p = &report.plan;
    println!(
        "reproof: tip {} | window {}..{} | {} stored proof(s) | {} height(s) checked | {} unreadable | proof gate {}",
        p.tip,
        p.min_height,
        p.max_height,
        p.stored_proofs_in_window,
        p.heights_checked,
        p.heights_unreadable,
        if p.gate == 0 { "closed".to_string() } else { p.gate.to_string() }
    );
    if p.stale.is_empty() {
        println!("every stored proof in the window matches the canonical header");
    } else {
        println!("txid             height  stored block       canonical block    action");
        for r in &p.stale {
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
    if report.executed {
        if let Some(h) = report.gate_set_to {
            println!("proof gate raised to {} (tip - 1) for the heal", h);
        }
        println!(
            "executed: {} replaced | {} demoted | {} deferred | {} unchanged | {} retained on error",
            report.replaced,
            report.demoted,
            report.deferred,
            report.unchanged,
            report.errors.len()
        );
        for e in &report.errors {
            println!("  retained: {}", e);
        }
        if report.demoted > 0 {
            println!("demoted proofs are re-proved by the monitor: `bsv-wallet tick` twice (the first run queues the header, the second processes it)");
        }
    } else if !p.stale.is_empty() {
        println!(
            "dry run (no storage write); re-run with --execute to replace, retain or demote the {} stale proof(s)",
            p.stale.len()
        );
    }
    Ok(())
}

/// Block hashes begin with sixteen or more zeros; the TAIL tells them apart.
fn short(h: &str) -> String {
    if h.is_empty() {
        "(none)".into()
    } else if h.len() > 16 {
        format!("..{}", &h[h.len() - 16..])
    } else {
        h.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bsv_wallet_toolbox::services::mock::MockWalletServices;
    use bsv_wallet_toolbox::services::{BlockHeader, Services, ServicesOptions};
    use bsv_wallet_toolbox::{Chain, StorageSqlx, WalletStorageWriter};

    fn anchor(txid: &str, height: u32, hash: &str, root: &str) -> ProvenTxAnchor {
        ProvenTxAnchor {
            txid: txid.into(),
            height,
            block_hash: hash.into(),
            merkle_root: root.into(),
        }
    }

    fn header(height: u32, root: &str) -> BlockHeader {
        BlockHeader {
            version: 1,
            previous_hash: "00".repeat(32),
            merkle_root: root.to_string(),
            time: 0,
            bits: 0,
            nonce: height,
            hash: String::new(),
            height,
        }
    }

    async fn storage() -> StorageSqlx {
        let s = StorageSqlx::in_memory().await.unwrap();
        s.migrate("reproof-test", &("02".to_string() + &"ab".repeat(32)))
            .await
            .unwrap();
        s.make_available().await.unwrap();
        s
    }

    async fn seed_proven(s: &StorageSqlx, txid: &str, height: u32, hash: &str, root: &str) {
        sqlx::query(
            "INSERT INTO proven_txs (txid, height, idx, block_hash, merkle_root, merkle_path, raw_tx) VALUES (?, ?, 0, ?, ?, X'00', X'00')",
        )
        .bind(txid)
        .bind(height as i64)
        .bind(hash)
        .bind(root)
        .execute(s.pool())
        .await
        .unwrap();
    }

    /// Every row count and the newest `updated_at` of every table a heal
    /// could touch: equal before and after means no write.
    async fn snapshot(s: &StorageSqlx) -> Vec<(String, i64, Option<String>)> {
        let mut out = Vec::new();
        for (table, stamp) in [
            ("proven_txs", "updated_at"),
            ("proven_tx_reqs", "updated_at"),
            ("transactions", "updated_at"),
            ("broadcast_seen", "seen_at"),
            ("monitor_state", "updated_at"),
        ] {
            let (count, newest): (i64, Option<String>) =
                sqlx::query_as(&format!("SELECT COUNT(*), MAX({stamp}) FROM {table}"))
                    .fetch_one(s.pool())
                    .await
                    .unwrap();
            out.push((table.to_string(), count, newest));
        }
        out
    }

    #[test]
    fn the_window_defaults_to_the_last_288_blocks_and_all_covers_everything() {
        assert_eq!(window(1000, None, false), (712, 1000));
        assert_eq!(window(100, None, false), (0, 100));
        assert_eq!(window(1000, Some(950), false), (950, 1000));
        assert_eq!(window(1000, Some(950), true), (0, 1000));
    }

    #[test]
    fn plan_rows_names_only_the_anchors_whose_root_disagrees_and_skips_unknown_heights() {
        let anchors = vec![
            anchor("t1", 100, "h1", "r-old"),
            anchor("t2", 100, "h2", "R-NEW"),
            anchor("t3", 101, "", "r-any"),
        ];
        let roots: HashMap<u32, String> = [(100u32, "r-new".to_string())].into();
        let hashes: HashMap<u32, String> = [(100u32, "h-new".to_string())].into();
        let rows = plan_rows(&anchors, &roots, &hashes);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].txid, "t1");
        assert_eq!(rows[0].canonical_block_hash, "h-new");
        assert_eq!(rows[0].canonical_merkle_root, "r-new");
        assert_eq!(rows[0].action, "dry-run");
    }

    /// F14: the dry run is read-only. Every table a heal could touch is
    /// byte-for-byte the same before and after, and the gate stays closed.
    #[tokio::test]
    async fn the_dry_run_performs_no_storage_writes() {
        let s = storage().await;
        seed_proven(&s, &"a".repeat(64), 100, &"11".repeat(32), &"aa".repeat(32)).await;
        seed_proven(&s, &"b".repeat(64), 100, &"11".repeat(32), &"22".repeat(32)).await;
        let services = MockWalletServices::builder().height(101).build();
        services.set_header_for_height(header(100, &"22".repeat(32)));
        let before = snapshot(&s).await;

        let mut ticks = 0usize;
        let plan = gather(&s, &services, None, false, &mut |_, _| ticks += 1)
            .await
            .unwrap();

        assert_eq!(plan.tip, 101);
        assert_eq!((plan.min_height, plan.max_height), (0, 101));
        assert_eq!(plan.stored_proofs_in_window, 2);
        assert_eq!(plan.heights_checked, 1);
        assert_eq!(plan.gate, 0, "closed, and left closed");
        assert_eq!(plan.stale.len(), 1);
        assert_eq!(plan.stale[0].txid, "a".repeat(64));
        assert_eq!(plan.stale[0].canonical_merkle_root, "22".repeat(32));
        assert_eq!(ticks, 0, "no progress line under 50 heights");
        assert_eq!(snapshot(&s).await, before, "the dry run wrote nothing");
        assert_eq!(
            MonitorStorage::max_acceptable_proof_height(&s)
                .await
                .unwrap(),
            0
        );
    }

    /// The CLI-only gate rule: `--execute` raises a closed or low gate to
    /// `tip - 1`, and leaves a gate at or above that alone.
    #[tokio::test]
    async fn execute_raises_the_gate_to_tip_minus_one_only_when_it_is_below() {
        let s = storage().await;
        assert_eq!(raise_gate_for_heal(&s, 101).await.unwrap(), Some(100));
        assert_eq!(
            MonitorStorage::max_acceptable_proof_height(&s)
                .await
                .unwrap(),
            100
        );
        assert_eq!(raise_gate_for_heal(&s, 101).await.unwrap(), None);
        MonitorStorage::set_max_acceptable_proof_height(&s, 500)
            .await
            .unwrap();
        assert_eq!(raise_gate_for_heal(&s, 101).await.unwrap(), None);
        assert_eq!(
            MonitorStorage::max_acceptable_proof_height(&s)
                .await
                .unwrap(),
            500
        );
    }

    /// `--execute` is refused without a chain tracker; the mock (which has
    /// one) passes.
    #[tokio::test]
    async fn execute_is_refused_without_a_chain_tracker() {
        let off = Services::with_options(Chain::Main, ServicesOptions::mainnet()).unwrap();
        let err = refuse_execute_without_tracker(&off).await.unwrap_err();
        assert!(err.to_string().contains("CHAINTRACKS_URL=off"), "{err}");
        let mock = MockWalletServices::new();
        assert!(refuse_execute_without_tracker(&mock).await.is_ok());
    }

    /// Progress is reported every 50 distinct heights.
    #[tokio::test]
    async fn progress_is_reported_every_fifty_heights() {
        let s = storage().await;
        for h in 1..=120u32 {
            seed_proven(&s, &format!("{:064x}", h), h, "", &"22".repeat(32)).await;
        }
        let services = MockWalletServices::builder().height(200).build();
        let mut seen = Vec::new();
        let plan = gather(&s, &services, None, true, &mut |done, total| {
            seen.push((done, total))
        })
        .await
        .unwrap();
        assert_eq!(plan.stored_proofs_in_window, 120);
        assert_eq!(seen, vec![(50, 120), (100, 120)]);
    }

    /// The JSON report's documented shape.
    #[test]
    fn the_json_report_has_the_documented_shape() {
        let report = Report {
            plan: Plan {
                tip: 101,
                min_height: 0,
                max_height: 101,
                stored_proofs_in_window: 1,
                heights_checked: 1,
                heights_unreadable: 0,
                gate: 0,
                stale: vec![StaleRow {
                    txid: "t".into(),
                    height: 100,
                    stored_block_hash: "s".into(),
                    canonical_block_hash: "c".into(),
                    stored_merkle_root: "r1".into(),
                    canonical_merkle_root: "r2".into(),
                    action: "dry-run".into(),
                }],
            },
            executed: false,
            gate_set_to: None,
            replaced: 0,
            demoted: 0,
            deferred: 0,
            unchanged: 0,
            errors: vec![],
        };
        let value = serde_json::to_value(&report).unwrap();
        let object = value.as_object().unwrap();
        let mut keys: Vec<&str> = object.keys().map(|k| k.as_str()).collect();
        keys.sort_unstable();
        assert_eq!(
            keys,
            vec![
                "deferred",
                "demoted",
                "errors",
                "executed",
                "gate",
                "gate_set_to",
                "heights_checked",
                "heights_unreadable",
                "max_height",
                "min_height",
                "replaced",
                "stale",
                "stored_proofs_in_window",
                "tip",
                "unchanged",
            ]
        );
        let row = &value["stale"][0];
        for key in [
            "txid",
            "height",
            "stored_block_hash",
            "canonical_block_hash",
            "stored_merkle_root",
            "canonical_merkle_root",
            "action",
        ] {
            assert!(row.get(key).is_some(), "missing {key}");
        }
    }
}
