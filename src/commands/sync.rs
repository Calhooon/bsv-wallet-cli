use anyhow::{anyhow, Result};
use bsv_sdk::wallet::{ListOutputsArgs, WalletInterface};
use bsv_wallet_toolbox::WalletServices;
use bsv_wallet_toolbox::{GetUtxoStatusOutputFormat, UtxoLeads};
use std::collections::HashSet;
use std::time::Instant;

use crate::brc29;
use crate::commands::cleanup_abandoned::{probe_input_spend, stored_locking_script, InputSpend};
use crate::commands::receive;
use crate::context::WalletContext;
use crate::explorers::{log_call, Explorers};

pub async fn run(ctx: &WalletContext, reconcile_spent: bool) -> Result<()> {
    let explorers = Explorers::from_env(ctx.chain);
    run_from(ctx, reconcile_spent, &explorers, ctx.wallet.services()).await
}

/// Break-glass (Rule 28, C4): a CHAIN SCAN for LEADS, through the
/// toolbox's `get_utxo_leads` (bsv-wallet-cli #50, toolbox 0.12.0) with the
/// deposit script handed as itself (`GetUtxoStatusOutputFormat::Script`),
/// so every lead source is asked: WhatsOnChain and Bitails by the script's
/// hash, BananaBlocks by the deposit address. This is discovery, not a
/// verdict: one explorer's list is enough to look, the union of every
/// answering explorer's list (keyed by outpoint) is the fullest look, and
/// each lead is received only after its bytes hash to its txid and its
/// proof meets the header service (`receive`). An empty set is every
/// answering explorer's empty list; no explorer answering at all is "could
/// not look", an error, never an empty list. An explorer whose list was cut
/// short (toolbox 0.12.0's `UtxoLeads::partial`: the page cap of
/// `LEAD_PAGES_MAX` pages of 1000, or a later page it could not read) still
/// answered: its leads are leads like any other, and the sync says it was
/// cut short (`cut_short_note`).
async fn unspent_at<V: WalletServices>(services: &V, script: &[u8]) -> Result<UtxoLeads> {
    let started = Instant::now();
    let found = services
        .get_utxo_leads(
            &hex::encode(script),
            Some(GetUtxoStatusOutputFormat::Script),
        )
        .await;
    match &found {
        Ok(found) => log_call("Services", "leads", &found.answered.join("+"), started),
        Err(_) => log_call("Services", "leads", "fault", started),
    }
    let found = found.map_err(|e| {
        anyhow!("could not look: no explorer gave the deposit address's unspent list: {e}")
    })?;
    if found.answered.is_empty() {
        return Err(anyhow!(
            "could not look: no explorer gave the deposit address's unspent list ({})",
            faults_of(&found)
        ));
    }
    Ok(found)
}

/// What the scan's summary says when a list was cut short: `None` when
/// every answering explorer listed everything it had. A sync whose every
/// answering list was cut short is not a full look: the outputs past the
/// cap are reached by a later sync only if the explorers' ordering brings
/// them into the pages read, which nothing promises.
fn cut_short_note(scan: &UtxoLeads) -> Option<String> {
    let cut = scan
        .answered
        .iter()
        .filter(|name| scan.partial.contains(name))
        .cloned()
        .collect::<Vec<_>>();
    if cut.is_empty() {
        return None;
    }
    let mut note = format!(
        "{} explorers answered ({} cut short: {})",
        scan.answered.len(),
        cut.len(),
        cut.join("+")
    );
    if cut.len() == scan.answered.len() {
        note.push_str(
            "; every answering list was cut short, so this was not a full look: \
             an output past the cap is not promised to a later sync",
        );
    }
    Some(note)
}

/// The explorers that could not look, `name: fault`, for a message.
fn faults_of(leads: &UtxoLeads) -> String {
    if leads.faults.is_empty() {
        return "no answer".to_string();
    }
    leads
        .faults
        .iter()
        .map(|(name, fault)| format!("{name}: {fault}"))
        .collect::<Vec<_>>()
        .join("; ")
}

async fn run_from<V: WalletServices>(
    ctx: &WalletContext,
    reconcile_spent: bool,
    explorers: &Explorers,
    services: &V,
) -> Result<()> {
    let address = brc29::deposit_address(&ctx.root_key, ctx.chain)?;
    let deposit_script = brc29::deposit_script(&ctx.root_key)?;

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
    // reaches it), asked of every lead source the toolbox has
    // (`unspent_at`, `get_utxo_leads`), and its negative is weak on
    // purpose: an empty list is "the answering explorers list nothing",
    // never "nothing was paid", and nothing is removed on it. An explorer
    // that faults is passed over and named; when none could look it is an
    // error ("could not look"), never an empty list.
    let scan = unspent_at(services, &deposit_script).await?;
    sync_leads(ctx, reconcile_spent, explorers, services, &address, scan).await
}

/// The scan's leads, each received after `receive`'s checks, then
/// `--reconcile-spent` over what the leads do not name.
async fn sync_leads<V: WalletServices>(
    ctx: &WalletContext,
    reconcile_spent: bool,
    explorers: &Explorers,
    services: &V,
    address: &str,
    scan: UtxoLeads,
) -> Result<()> {
    let unspent = &scan.leads;
    let listed_by = scan.answered.join("+");
    tracing::info!(
        address = %address,
        listed = unspent.len(),
        listed_by = %listed_by,
        cut_short = %scan.partial.join("+"),
        could_not_look = %faults_of(&scan),
        "the deposit address's unspent leads"
    );

    let known = known_outpoints(ctx).await?;

    let mut tally = Tally::default();

    for u in unspent {
        let outpoint = format!("{}.{}", u.txid, u.vout);
        if known.contains(&outpoint) {
            tally.skipped += 1;
            continue;
        }
        // A lead, not a verdict: `receive` binds the bytes to the txid and
        // checks the proof against the header service before it stores
        // anything, whichever explorer listed it and however many did; a
        // lead from a list cut short is no different.
        match receive::receive_txid_from(ctx, explorers, services, &u.txid, Some(u.vout)).await {
            Ok((_, true)) => {
                tally.received += 1;
                tally.sats_in += u.satoshis.unwrap_or(0);
            }
            Ok((_, false)) => {
                tally.refused += 1;
                eprintln!("not accepted: {}", outpoint);
            }
            Err(e) => {
                tally.refused += 1;
                eprintln!("failed {}: {:#}", outpoint, e);
            }
        }
    }

    // --reconcile-spent (2026-08-27): a restored-from-backup wallet holds rows
    // for outputs the chain has since seen SPENT; selecting them builds
    // double-spend inputs the network refuses (the fleet-restore incident).
    // Every DB outpoint MISSING from the leads the explorers gave for
    // the deposit address is put to the ONE spend probe (Rule 28, C5 and C7: the same
    // function `cleanup-abandoned` and `reconcile-outputs` ask, with the
    // same three answers; this command no longer holds a copy with its own
    // error rule). Only a spend we hold a proof of relinquishes; a named
    // but unproven spender, and anything the probe could not decide, is
    // LEFT ALONE and counted (fail-safe: unknown never relinquishes).
    // Missing from the leads is never itself "spent" (#50): the leads may
    // be one explorer's word, and an explorer that could not look listed
    // nothing, so a missing outpoint is only a question for the probe, and
    // what it cannot decide is "could not decide". A list cut short
    // (`scan.partial`) leaves out what lies past its pages, and that too is
    // only a question for the probe.
    if reconcile_spent {
        let chain_unspent: HashSet<String> = unspent
            .iter()
            .map(|u| format!("{}.{}", u.txid, u.vout))
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
            tally.reconcile_checked += 1;
            let script = stored_locking_script(pool, txid, vout).await;
            let answer =
                probe_input_spend(explorers, services, txid, vout, script.as_deref()).await;
            match reconcile_step(&answer) {
                ReconcileStep::Keep => {}
                ReconcileStep::LeaveUnproven => {
                    tally.spent_unproven += 1;
                    eprintln!(
                        "spent by a transaction we hold no proof of yet (left alone): {}",
                        op
                    );
                }
                ReconcileStep::LeaveUnknown => {
                    tally.unknown += 1;
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
                            tally.reconciled += 1;
                            eprintln!("reconciled spent: {}", op);
                        }
                        Err(e) => eprintln!("relinquish failed {}: {}", op, e),
                    }
                }
            }
        }
    }

    if ctx.json_output {
        println!("{}", json_report(address, &scan, &tally));
    } else {
        println!("{}", summary_line(&scan, &tally));
        // A money verb must never finish SILENT about what it did (or did not)
        // touch: "checked 0" (nothing qualified) and "checked 12, relinquished
        // 0" (all verified live) are different facts a drain decision rests on.
        if reconcile_spent {
            println!(
                "Reconcile: {} outpoint(s) chain-checked, {} relinquished as spent (proven), {} spent but not proven yet (left alone), {} undecided (left alone; run cleanup-abandoned)",
                tally.reconcile_checked, tally.reconciled, tally.spent_unproven, tally.unknown
            );
        }
    }

    if tally.refused > 0 {
        return Err(anyhow!(
            "sync refused {} outpoint(s); other outpoints were processed",
            tally.refused
        ));
    }
    Ok(())
}

/// What one sync did, counted.
#[derive(Debug, Default)]
struct Tally {
    received: u32,
    refused: u32,
    skipped: u32,
    sats_in: u64,
    reconcile_checked: u32,
    reconciled: u32,
    spent_unproven: u32,
    unknown: u32,
}

/// `sync --json`: the scan (who listed, who could not look, whose list was
/// cut short, each lead) and what was done with it.
fn json_report(address: &str, scan: &UtxoLeads, tally: &Tally) -> serde_json::Value {
    serde_json::json!({
        "address": address,
        "unspent_on_chain": scan.leads.len(),
        "unspent_listed_by": scan.answered,
        "unspent_could_not_look": scan.faults.iter().map(|(name, _)| name).collect::<Vec<_>>(),
        "unspent_partial": scan.partial,
        "unspent_leads": scan.leads
            .iter()
            .map(|u| serde_json::json!({
                "outpoint": format!("{}.{}", u.txid, u.vout),
                "listed_by": u.listed_by,
            }))
            .collect::<Vec<_>>(),
        "received": tally.received,
        "refused": tally.refused,
        "skipped": tally.skipped,
        "sats_received": tally.sats_in,
        "reconcile_checked": tally.reconcile_checked,
        "reconciled_spent": tally.reconciled,
        "spent_unproven": tally.spent_unproven,
        "unknown": tally.unknown,
    })
}

/// The human summary of the scan and what was received.
fn summary_line(scan: &UtxoLeads, tally: &Tally) -> String {
    let mut line = format!(
        "Sync complete: {} on chain (listed by {}), {} new received ({} sats), {} already known",
        scan.leads.len(),
        scan.answered.join("+"),
        tally.received,
        tally.sats_in,
        tally.skipped
    );
    if let Some(note) = cut_short_note(scan) {
        line.push_str("; ");
        line.push_str(&note);
    }
    line
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

    use crate::commands::receive::tests::{stub, stub_context};
    use crate::test_support::{raw_tx, txid_of, Fixture};
    use bsv_wallet_toolbox::services::mock::MockWalletServicesBuilder;
    use bsv_wallet_toolbox::services::mock::{MockErrorKind, MockResponse, MockWalletServices};
    use bsv_wallet_toolbox::{GetMerklePathResult, GetUtxoStatusResult, UtxoLead};

    /// The toolbox's `get_utxo_leads`, answered: `answered` listed
    /// `outputs` (`txid`, `vout`, satoshis, who listed it) and `faults`
    /// could not look.
    fn leads(
        answered: &[&str],
        faults: &[&str],
        outputs: &[(&str, u32, u64, &[&str])],
    ) -> UtxoLeads {
        UtxoLeads {
            leads: outputs
                .iter()
                .map(|(txid, vout, satoshis, by)| UtxoLead {
                    txid: txid.to_string(),
                    vout: *vout,
                    satoshis: Some(*satoshis),
                    listed_by: by.iter().map(|b| b.to_string()).collect(),
                })
                .collect(),
            answered: answered.iter().map(|a| a.to_string()).collect(),
            faults: faults
                .iter()
                .map(|f| (f.to_string(), "Rate limited".to_string()))
                .collect(),
            ..Default::default()
        }
    }

    /// A merkle path for `txid` as the services return it after its root
    /// met the header service.
    fn proved(txid: &str) -> MockWalletServices {
        proved_by(MockWalletServices::builder(), txid).build()
    }

    /// `builder` with a merkle path for `txid` (as [`proved`]).
    fn proved_by(builder: MockWalletServicesBuilder, txid: &str) -> MockWalletServicesBuilder {
        let path = bsv_sdk::transaction::MerklePath::from_coinbase_txid(txid, 900_000).to_hex();
        builder.get_merkle_path_response(MockResponse::Success(GetMerklePathResult {
            name: Some("Services".to_string()),
            merkle_path: Some(path),
            header: None,
            error: None,
            notes: vec![],
        }))
    }

    /// A stub context whose header service takes the root of a proof at
    /// height 900,000.
    async fn headed_context() -> WalletContext {
        use bsv_sdk::transaction::MockChainTracker;
        let ctx = stub_context().await;
        ctx.wallet
            .storage()
            .set_chain_tracker(std::sync::Arc::new(MockChainTracker::always_valid(900_001)))
            .await;
        ctx
    }

    /// A payment to the deposit script nobody handed us: its bytes, txid.
    fn payment(ctx: &WalletContext, salt: u8, satoshis: u64) -> (Vec<u8>, String) {
        let script = brc29::deposit_script(&ctx.root_key).unwrap();
        let raw = raw_tx(
            &[(&format!("{salt:02x}").repeat(32), 0)],
            &[(satoshis, script)],
        );
        let txid = txid_of(&raw);
        (raw, txid)
    }

    #[tokio::test]
    async fn l2_sync_continues_after_stub_refusal_and_returns_error() {
        let ctx = stub_context().await;
        let (one, body_one) = stub(&ctx, 17);
        let (two, body_two) = stub(&ctx, 18);
        let route_one = format!("/tx/{one}/beef");
        let route_two = format!("/tx/{two}/beef");
        let courier = Fixture::start(
            &[(&route_one, 200, &body_one), (&route_two, 200, &body_two)],
            500,
        )
        .await;
        let services = MockWalletServices::new();
        let explorers = Explorers::at(Some(&courier.base), None, None);
        let scan = leads(
            &["WhatsOnChain"],
            &[],
            &[
                (&one, 0, 1000, &["WhatsOnChain"]),
                (&two, 0, 1000, &["WhatsOnChain"]),
            ],
        );
        let error = sync_leads(&ctx, false, &explorers, &services, "addr", scan)
            .await
            .unwrap_err();
        assert!(error.to_string().contains("sync refused 2"), "{error}");
        assert_eq!(courier.hits(), vec![route_one, route_two]);
        let count: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM transactions WHERE txid IN (?, ?)")
                .bind(&one)
                .bind(&two)
                .fetch_one(ctx.wallet.storage().pool())
                .await
                .unwrap();
        assert_eq!(count, 0);
    }

    /// #50, M1 of the review: WhatsOnChain could not look at the scan and
    /// Bitails listed the payment; the lead's courier is BananaBlocks while
    /// WhatsOnChain is rate limited. The sync completes and receives. Red
    /// at 0.16: "WoC unspent fetch failed", the end of the sync. (Was
    /// `sync_with_woc_faulting_and_bitails_answering_completes`.)
    #[tokio::test]
    async fn a_whatsonchain_fault_with_bitails_listing_is_received_through_bananablocks() {
        let ctx = headed_context().await;
        let (raw, txid) = payment(&ctx, 0x31, 1500);
        let services = proved(&txid);
        let beef_route = format!("/tx/{txid}/beef");
        let woc =
            Fixture::start_with_headers(&[(&beef_route, 429, "", &[("retry-after", "30")])], 500)
                .await;
        let hex_route = format!("/tx/{txid}/hex");
        let bb = Fixture::start(&[(&hex_route, 200, &hex::encode(&raw))], 404).await;
        let explorers = Explorers::at(Some(&woc.base), Some(&bb.base), None);
        let scan = leads(
            &["Bitails", "BananaBlocks"],
            &["WhatsOnChain"],
            &[(&txid, 0, 1500, &["Bitails"])],
        );

        sync_leads(&ctx, false, &explorers, &services, "addr", scan)
            .await
            .expect("the sync completes");
        assert_eq!(woc.hits(), vec![beef_route]);
        assert_eq!(bb.hits(), vec![hex_route]);
        let known = known_outpoints(&ctx).await.unwrap();
        assert!(known.contains(&format!("{txid}.0")), "{known:?}");
    }

    /// M1: WhatsOnChain and Bitails both could not look and BananaBlocks
    /// listed the payment by the deposit address: one explorer's list is
    /// enough to look, and the sync completes and receives it.
    #[tokio::test]
    async fn whatsonchain_and_bitails_faulting_with_bananablocks_listing_completes() {
        let ctx = headed_context().await;
        let (raw, txid) = payment(&ctx, 0x33, 1700);
        let services = proved(&txid);
        let woc = Fixture::start(&[], 503).await;
        let hex_route = format!("/tx/{txid}/hex");
        let bb = Fixture::start(&[(&hex_route, 200, &hex::encode(&raw))], 404).await;
        let explorers = Explorers::at(Some(&woc.base), Some(&bb.base), None);
        let scan = leads(
            &["BananaBlocks"],
            &["WhatsOnChain", "Bitails"],
            &[(&txid, 0, 1700, &["BananaBlocks"])],
        );

        sync_leads(&ctx, false, &explorers, &services, "addr", scan)
            .await
            .expect("the sync completes on BananaBlocks' list alone");
        assert_eq!(bb.hits(), vec![hex_route]);
        let known = known_outpoints(&ctx).await.unwrap();
        assert!(known.contains(&format!("{txid}.0")), "{known:?}");
    }

    /// M1: no explorer answered the scan (WhatsOnChain, Bitails and
    /// BananaBlocks all could not look): the sync is an error that says
    /// "could not look", never an empty sync, and nothing is received or
    /// probed on it. Both shapes come through the `WalletServices` trait
    /// (`unspent_at`): the toolbox's `Error::NoExplorerAnswered` (the mock
    /// derives it from an unspent list that could not be read), and an
    /// answer naming no explorer that answered. (Was
    /// `no_explorer_giving_the_set_is_could_not_look_never_empty`, on the
    /// verdict ladder.)
    #[tokio::test]
    async fn every_explorer_faulting_is_could_not_look_never_an_empty_sync() {
        let ctx = stub_context().await;
        let fixture = Fixture::start(&[], 500).await;
        let explorers = Explorers::at(Some(&fixture.base), Some(&fixture.base), None);
        let erred = MockWalletServices::builder()
            .get_utxo_status_response(MockResponse::Error(
                MockErrorKind::ServiceError,
                "Rate limited".to_string(),
            ))
            .build();
        let unanswered = MockWalletServices::builder()
            .get_utxo_leads_response(MockResponse::Success(leads(
                &[],
                &["WhatsOnChain", "Bitails", "BananaBlocks"],
                &[],
            )))
            .build();
        for none in [&erred, &unanswered] {
            for reconcile in [false, true] {
                let err = run_from(&ctx, reconcile, &explorers, none)
                    .await
                    .unwrap_err()
                    .to_string();
                assert!(err.contains("could not look"), "{err}");
            }
            assert_eq!(none.call_count("get_utxo_leads"), 2);
        }
        assert!(
            erred
                .get_utxo_leads(&"00".repeat(25), None)
                .await
                .is_err_and(|e| matches!(e, bsv_wallet_toolbox::Error::NoExplorerAnswered { .. })),
            "the trait's own could-not-look"
        );
        assert_eq!(fixture.total(), 0, "nothing was received or probed");
        assert!(known_outpoints(&ctx).await.unwrap().is_empty());
    }

    /// 6f958f3 (toolbox `UtxoLeads::partial`): a list cut short is still a
    /// list. Through the trait end to end (`run_from`, `unspent_at`, the
    /// mock's `get_utxo_leads_response`): BananaBlocks' list was cut short
    /// at the page cap and is the only one naming the payment;
    /// WhatsOnChain listed everything it had (nothing). The lead is
    /// received after its bytes and proof check, as any other.
    #[tokio::test]
    async fn a_cut_short_explorers_leads_are_received_through_the_trait() {
        let ctx = headed_context().await;
        let (raw, txid) = payment(&ctx, 0x35, 1900);
        let mut scan = leads(
            &["WhatsOnChain", "BananaBlocks"],
            &["Bitails"],
            &[(&txid, 0, 1900, &["BananaBlocks"])],
        );
        scan.partial = vec!["BananaBlocks".to_string()];
        let services = proved_by(MockWalletServices::builder(), &txid)
            .get_utxo_leads_response(MockResponse::Success(scan))
            .build();
        let woc = Fixture::start(&[], 404).await;
        let hex_route = format!("/tx/{txid}/hex");
        let bb = Fixture::start(&[(&hex_route, 200, &hex::encode(&raw))], 404).await;
        let explorers = Explorers::at(Some(&woc.base), Some(&bb.base), None);

        run_from(&ctx, false, &explorers, &services)
            .await
            .expect("the sync completes on a list cut short");
        assert_eq!(services.call_count("get_utxo_leads"), 1);
        assert_eq!(bb.hits(), vec![hex_route]);
        let known = known_outpoints(&ctx).await.unwrap();
        assert_eq!(known, HashSet::from([format!("{txid}.0")]));
    }

    /// An explorer's unspent list through the trait as the mock derives it
    /// when no leads are set (from `get_utxo_status_response`'s details):
    /// the one lead is received, listed by the provider that answered.
    #[tokio::test]
    async fn a_list_through_the_trait_is_received() {
        let ctx = headed_context().await;
        let (raw, txid) = payment(&ctx, 0x36, 2100);
        let services = proved_by(MockWalletServices::builder(), &txid)
            .get_utxo_status_response(MockResponse::Success(GetUtxoStatusResult {
                name: "WhatsOnChain".to_string(),
                status: "success".to_string(),
                is_utxo: Some(true),
                details: vec![bsv_wallet_toolbox::UtxoDetail {
                    txid: txid.clone(),
                    index: 0,
                    satoshis: 2100,
                    height: Some(900_000),
                }],
                error: None,
            }))
            .build();
        let beef_route = format!("/tx/{txid}/beef");
        let woc = Fixture::start(&[], 404).await;
        let hex_route = format!("/tx/{txid}/hex");
        let bb = Fixture::start(&[(&hex_route, 200, &hex::encode(&raw))], 404).await;
        let explorers = Explorers::at(Some(&woc.base), Some(&bb.base), None);

        run_from(&ctx, false, &explorers, &services)
            .await
            .expect("the sync completes");
        assert_eq!(woc.hits(), vec![beef_route]);
        let known = known_outpoints(&ctx).await.unwrap();
        assert_eq!(known, HashSet::from([format!("{txid}.0")]));
    }

    /// `--json`'s `unspent_partial` names each explorer whose list was cut
    /// short (empty when none was), beside who answered and who could not.
    #[test]
    fn unspent_partial_names_the_cut_short_explorer() {
        let txid = "ab".repeat(32);
        let mut scan = leads(
            &["WhatsOnChain", "BananaBlocks"],
            &["Bitails"],
            &[(&txid, 0, 1000, &["WhatsOnChain", "BananaBlocks"])],
        );
        let full = json_report("addr", &scan, &Tally::default());
        assert_eq!(full["unspent_partial"], serde_json::json!([]));
        scan.partial = vec!["BananaBlocks".to_string()];
        let report = json_report("addr", &scan, &Tally::default());
        assert_eq!(
            report["unspent_partial"],
            serde_json::json!(["BananaBlocks"])
        );
        assert_eq!(
            report["unspent_listed_by"],
            serde_json::json!(["WhatsOnChain", "BananaBlocks"])
        );
        assert_eq!(
            report["unspent_could_not_look"],
            serde_json::json!(["Bitails"])
        );
        assert_eq!(report["unspent_on_chain"], 1);
    }

    /// The summary line says how many answering explorers were cut short,
    /// and when every one was, that this was not a full look.
    #[test]
    fn the_summary_line_says_a_list_was_cut_short() {
        let tally = Tally {
            received: 1,
            sats_in: 1000,
            ..Default::default()
        };
        let mut scan = leads(&["WhatsOnChain", "BananaBlocks"], &["Bitails"], &[]);
        let line = summary_line(&scan, &tally);
        assert_eq!(
            line,
            "Sync complete: 0 on chain (listed by WhatsOnChain+BananaBlocks), 1 new received (1000 sats), 0 already known"
        );

        scan.partial = vec!["BananaBlocks".to_string()];
        let line = summary_line(&scan, &tally);
        assert!(
            line.ends_with("; 2 explorers answered (1 cut short: BananaBlocks)"),
            "{line}"
        );
        assert!(!line.contains("not a full look"), "{line}");

        scan.partial = vec!["WhatsOnChain".to_string(), "BananaBlocks".to_string()];
        let line = summary_line(&scan, &tally);
        assert!(
            line.contains("2 explorers answered (2 cut short: WhatsOnChain+BananaBlocks)"),
            "{line}"
        );
        assert!(line.contains("not a full look"), "{line}");
    }

    /// M1: a lead is not a verdict. A lead one explorer listed is received
    /// only after its bytes hash to its txid and its proof meets the header
    /// service, like any other: bytes that do not hash to the lead's txid,
    /// and bytes with no checked proof, are refused and nothing is stored;
    /// the same lead with its proof is received.
    #[tokio::test]
    async fn a_lead_from_one_explorer_is_received_only_after_its_bytes_and_proof_check() {
        let ctx = headed_context().await;
        let (raw, txid) = payment(&ctx, 0x34, 1800);
        let forged = "44".repeat(32);
        let hex_route = format!("/tx/{txid}/hex");
        let forged_route = format!("/tx/{forged}/hex");
        let bb = Fixture::start(
            &[
                (&hex_route, 200, &hex::encode(&raw)),
                (&forged_route, 200, &hex::encode(&raw)),
            ],
            404,
        )
        .await;
        let woc = Fixture::start(&[], 404).await;
        let explorers = Explorers::at(Some(&woc.base), Some(&bb.base), None);
        let only_bb = |txid: &str| {
            leads(
                &["BananaBlocks"],
                &["WhatsOnChain", "Bitails"],
                &[(txid, 0, 1800, &["BananaBlocks"])],
            )
        };

        let err = sync_leads(
            &ctx,
            false,
            &explorers,
            &proved(&forged),
            "addr",
            only_bb(&forged),
        )
        .await
        .unwrap_err();
        assert!(err.to_string().contains("sync refused 1"), "{err}");
        assert_eq!(
            bb.count(&forged_route),
            1,
            "the forged lead's bytes were read"
        );
        let unproven = MockWalletServices::builder()
            .get_merkle_path_response(MockResponse::Error(
                MockErrorKind::ServiceError,
                "no proof".to_string(),
            ))
            .build();
        let err = sync_leads(&ctx, false, &explorers, &unproven, "addr", only_bb(&txid))
            .await
            .unwrap_err();
        assert!(err.to_string().contains("sync refused 1"), "{err}");
        assert!(known_outpoints(&ctx).await.unwrap().is_empty());

        sync_leads(
            &ctx,
            false,
            &explorers,
            &proved(&txid),
            "addr",
            only_bb(&txid),
        )
        .await
        .expect("the checked lead is received");
        let known = known_outpoints(&ctx).await.unwrap();
        assert_eq!(known, HashSet::from([format!("{txid}.0")]));
    }

    /// #50: an outpoint of ours missing from the leads one explorer gave
    /// (the others could not look) is a question for the probe, never
    /// "spent"; the probe's rungs (WhatsOnChain rate limited, BananaBlocks
    /// "no spend") and an unspent set that could not be read leave it
    /// "could not decide", spendable and untouched.
    #[tokio::test]
    async fn missing_from_one_explorers_set_is_could_not_decide_never_spent() {
        use bsv_sdk::transaction::{MerklePath, MockChainTracker};
        let ctx = stub_context().await;
        ctx.wallet
            .storage()
            .set_chain_tracker(std::sync::Arc::new(MockChainTracker::always_valid(900_001)))
            .await;
        let script = brc29::deposit_script(&ctx.root_key).unwrap();
        let raw = raw_tx(
            &[(&"32".repeat(32), 0)],
            &[(1500, script.clone()), (1600, script)],
        );
        let txid = txid_of(&raw);
        let mut beef = bsv_sdk::transaction::Beef::new();
        let bump = beef.merge_bump(MerklePath::from_coinbase_txid(&txid, 900_000));
        beef.merge_raw_tx(raw, Some(bump));
        crate::commands::fund::internalize_beef(&ctx, &beef.to_binary(), 0)
            .await
            .unwrap();
        crate::commands::fund::internalize_beef(&ctx, &beef.to_binary(), 1)
            .await
            .unwrap();
        let known = known_outpoints(&ctx).await.unwrap();
        assert_eq!(known.len(), 2, "{known:?}");

        let missing_route = format!("/tx/{txid}/1/spent");
        let woc = Fixture::start_with_headers(
            &[(&missing_route, 429, "", &[("retry-after", "30")])],
            500,
        )
        .await;
        let spend_route = format!("/txo/{txid}/1");
        let held = hex::encode(brc29::deposit_script(&ctx.root_key).unwrap());
        let bb = Fixture::start(
            &[(
                &spend_route,
                200,
                &format!(r#"{{"script_pubkey":"{held}","spent":false}}"#),
            )],
            404,
        )
        .await;
        let explorers = Explorers::at(Some(&woc.base), Some(&bb.base), None);
        let services = MockWalletServices::builder()
            .is_utxo_response(MockResponse::Error(
                MockErrorKind::ServiceError,
                "one explorer's negative, the others could not look".to_string(),
            ))
            .build();
        let scan = leads(
            &["WhatsOnChain"],
            &["Bitails", "BananaBlocks"],
            &[(&txid, 0, 1500, &["WhatsOnChain"])],
        );

        sync_leads(&ctx, true, &explorers, &services, "addr", scan)
            .await
            .unwrap();
        assert_eq!(
            woc.hits(),
            vec![missing_route],
            "only the missing outpoint is probed"
        );
        assert_eq!(bb.hits(), vec![spend_route]);
        assert_eq!(services.call_count("is_utxo"), 1);
        let known = known_outpoints(&ctx).await.unwrap();
        assert!(
            known.contains(&format!("{txid}.1")),
            "relinquished on one explorer's set"
        );
    }

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
    /// at the site, and says so when it runs; since #50 the site asks the
    /// toolbox's lead sources (`get_utxo_leads`, the script itself, so
    /// BananaBlocks is asked by address), never the verdict ladder, and no
    /// hand-rolled WhatsOnChain request is left. Red at the base: neither
    /// named; the scan was one explorer.
    #[test]
    fn the_address_scan_asks_the_ladder_and_is_named_at_the_site() {
        let source = include_str!("sync.rs");
        let code = source.split("#[cfg(test)]").next().unwrap();
        assert!(!code.contains("/address/"), "a hand-rolled address scan");
        assert!(!code.contains("reqwest"), "a request of sync's own");
        assert!(code.contains(".get_utxo_leads("), "the lead sources");
        assert!(
            code.contains("Some(GetUtxoStatusOutputFormat::Script)"),
            "the script itself, so BananaBlocks is asked"
        );
        assert!(!code.contains(".get_utxo_status("), "the verdict ladder");
        let site = code.find("let scan = unspent_at(").expect("the scan");
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
