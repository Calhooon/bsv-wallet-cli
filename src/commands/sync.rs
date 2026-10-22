use anyhow::{anyhow, Result};
use bsv_sdk::wallet::{ListOutputsArgs, WalletInterface};
use bsv_wallet_toolbox::WalletServices;
use bsv_wallet_toolbox::{GetUtxoStatusOutputFormat, UtxoDetail};
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

/// The deposit script's unspent set as the explorers gave it.
struct UnspentScan {
    /// Which explorer gave it: one name for a positive (the first explorer
    /// that listed the script's outputs), `A+B` for the empty set two gave.
    by: String,
    /// The outputs listed (`txid`, `vout`, satoshis).
    outputs: Vec<UtxoDetail>,
}

/// Break-glass (Rule 28, C4): a CHAIN SCAN, through the toolbox's ladder
/// (`get_utxo_status` by the script's hash, bsv-wallet-cli #50): WhatsOnChain
/// and Bitails with a rotating start (BananaBlocks answers an outpoint
/// only: it holds no script's unspent set, so it is not applicable here and
/// is neither asked nor counted). A fault on one explorer moves to the
/// next; a list is a positive from the first explorer that gives one; the
/// empty set needs both explorers' clean answers. Anything else is "could
/// not look", an error, never an empty list.
async fn unspent_at<V: WalletServices>(services: &V, script: &[u8]) -> Result<UnspentScan> {
    let started = Instant::now();
    let found = services
        .get_utxo_status(
            &hex::encode(script),
            Some(GetUtxoStatusOutputFormat::Script),
            None,
            false,
        )
        .await;
    match &found {
        Ok(found) => log_call(&found.name, "unspent", &found.status, started),
        Err(_) => log_call("Services", "unspent", "fault", started),
    }
    let found = found.map_err(|e| {
        anyhow!("could not look: no explorer gave the deposit address's unspent set: {e}")
    })?;
    match (found.status.as_str(), found.is_utxo) {
        ("success", Some(true)) => Ok(UnspentScan {
            by: found.name,
            outputs: found.details,
        }),
        ("success", Some(false)) => Ok(UnspentScan {
            by: found.name,
            outputs: Vec::new(),
        }),
        _ => Err(anyhow!(
            "could not look: no explorer gave the deposit address's unspent set ({})",
            found.error.as_deref().unwrap_or("no answer")
        )),
    }
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
    // reaches it), asked of the toolbox's explorer ladder (`unspent_at`),
    // and its negative is weak on purpose: an empty list is "these
    // explorers list nothing", never "nothing was paid", and nothing is
    // removed on it. A fault on one explorer moves to the next; when none
    // could look it is an error ("could not look"), never an empty list.
    let scan = unspent_at(services, &deposit_script).await?;
    let unspent = &scan.outputs;
    tracing::info!(
        address = %address,
        listed = unspent.len(),
        listed_by = %scan.by,
        "the deposit address's unspent set"
    );

    let known = known_outpoints(ctx).await?;

    let mut refused = 0u32;
    let mut received = 0u32;
    let mut skipped = 0u32;
    let mut sats_in = 0u64;

    for u in unspent {
        let outpoint = format!("{}.{}", u.txid, u.index);
        if known.contains(&outpoint) {
            skipped += 1;
            continue;
        }
        match receive::receive_txid_from(ctx, explorers, services, &u.txid, Some(u.index)).await {
            Ok((_, true)) => {
                received += 1;
                sats_in += u.satoshis;
            }
            Ok((_, false)) => {
                refused += 1;
                eprintln!("not accepted: {}", outpoint);
            }
            Err(e) => {
                refused += 1;
                eprintln!("failed {}: {:#}", outpoint, e);
            }
        }
    }

    // --reconcile-spent (2026-08-27): a restored-from-backup wallet holds rows
    // for outputs the chain has since seen SPENT; selecting them builds
    // double-spend inputs the network refuses (the fleet-restore incident).
    // Every DB outpoint MISSING from the unspent set the explorers gave for
    // the deposit address is put to the ONE spend probe (Rule 28, C5 and C7: the same
    // function `cleanup-abandoned` and `reconcile-outputs` ask, with the
    // same three answers; this command no longer holds a copy with its own
    // error rule). Only a spend we hold a proof of relinquishes; a named
    // but unproven spender, and anything the probe could not decide, is
    // LEFT ALONE and counted (fail-safe: unknown never relinquishes).
    // Missing from the set is never itself "spent" (#50): the set may be
    // one explorer's word, so a missing outpoint is only a question for
    // the probe, and what it cannot decide is "could not decide".
    let mut reconciled = 0u32;
    let mut reconcile_checked = 0u32;
    let mut spent_unproven = 0u32;
    let mut unknown = 0u32;
    if reconcile_spent {
        let chain_unspent: HashSet<String> = unspent
            .iter()
            .map(|u| format!("{}.{}", u.txid, u.index))
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
            let answer =
                probe_input_spend(explorers, services, txid, vout, script.as_deref()).await;
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
                "unspent_listed_by": scan.by,
                "received": received,
                "refused": refused,
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
            "Sync complete: {} on chain (listed by {}), {} new received ({} sats), {} already known",
            unspent.len(),
            scan.by,
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

    if refused > 0 {
        return Err(anyhow!(
            "sync refused {refused} outpoint(s); other outpoints were processed"
        ));
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

    use crate::commands::receive::tests::{stub, stub_context};
    use crate::test_support::{raw_tx, txid_of, Fixture};
    use bsv_wallet_toolbox::services::mock::{MockErrorKind, MockResponse, MockWalletServices};
    use bsv_wallet_toolbox::{GetMerklePathResult, GetUtxoStatusResult};

    /// The toolbox's unspent ladder, answered: `by` listed `outputs`.
    fn listed(by: &str, outputs: &[(&str, u32, u64)]) -> MockResponse<GetUtxoStatusResult> {
        MockResponse::Success(GetUtxoStatusResult {
            name: by.to_string(),
            status: "success".to_string(),
            is_utxo: Some(!outputs.is_empty()),
            details: outputs
                .iter()
                .map(|(txid, index, satoshis)| UtxoDetail {
                    txid: txid.to_string(),
                    index: *index,
                    satoshis: *satoshis,
                    height: Some(900_000),
                })
                .collect(),
            error: None,
        })
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
        let services = MockWalletServices::builder()
            .get_utxo_status_response(listed("WoC", &[(&one, 0, 1000), (&two, 0, 1000)]))
            .build();
        let explorers = Explorers::at(Some(&courier.base), None, None);
        let error = run_from(&ctx, false, &explorers, &services)
            .await
            .unwrap_err();
        assert!(error.to_string().contains("sync refused 2"), "{error}");
        assert_eq!(courier.hits(), vec![route_one, route_two]);
        assert_eq!(services.call_count("get_utxo_status"), 1);
        let count: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM transactions WHERE txid IN (?, ?)")
                .bind(&one)
                .bind(&two)
                .fetch_one(ctx.wallet.storage().pool())
                .await
                .unwrap();
        assert_eq!(count, 0);
    }

    /// #50: the scan asks the toolbox's ladder (a WhatsOnChain fault moves
    /// to Bitails there), and Bitails' list is the scan; the payment's
    /// courier is BananaBlocks while WhatsOnChain is rate limited. The sync
    /// completes and receives. Red at the base: "WoC unspent fetch failed",
    /// the end of the sync.
    #[tokio::test]
    async fn sync_with_woc_faulting_and_bitails_answering_completes() {
        use bsv_sdk::transaction::{MerklePath, MockChainTracker};
        let ctx = stub_context().await;
        ctx.wallet
            .storage()
            .set_chain_tracker(std::sync::Arc::new(MockChainTracker::always_valid(900_001)))
            .await;
        let script = brc29::deposit_script(&ctx.root_key).unwrap();
        let raw = raw_tx(&[(&"31".repeat(32), 0)], &[(1500, script)]);
        let txid = txid_of(&raw);
        let path = MerklePath::from_coinbase_txid(&txid, 900_000).to_hex();
        let services = MockWalletServices::builder()
            .get_utxo_status_response(listed("Bitails", &[(&txid, 0, 1500)]))
            .get_merkle_path_response(MockResponse::Success(GetMerklePathResult {
                name: Some("Services".to_string()),
                merkle_path: Some(path),
                header: None,
                error: None,
                notes: vec![],
            }))
            .build();
        let beef_route = format!("/tx/{txid}/beef");
        let woc =
            Fixture::start_with_headers(&[(&beef_route, 429, "", &[("retry-after", "30")])], 500)
                .await;
        let hex_route = format!("/tx/{txid}/hex");
        let bb = Fixture::start(&[(&hex_route, 200, &hex::encode(&raw))], 404).await;
        let explorers = Explorers::at(Some(&woc.base), Some(&bb.base), None);

        run_from(&ctx, false, &explorers, &services)
            .await
            .expect("the sync completes");
        assert_eq!(woc.hits(), vec![beef_route]);
        assert_eq!(bb.hits(), vec![hex_route]);
        let known = known_outpoints(&ctx).await.unwrap();
        assert!(known.contains(&format!("{txid}.0")), "{known:?}");
    }

    /// The ladder could not look (no explorer gave the set): the sync is
    /// an error that says so, never "nothing on chain".
    #[tokio::test]
    async fn no_explorer_giving_the_set_is_could_not_look_never_empty() {
        let ctx = stub_context().await;
        let fixture = Fixture::start(&[], 500).await;
        let explorers = Explorers::at(Some(&fixture.base), None, None);
        let one_negative = MockWalletServices::builder()
            .get_utxo_status_response(MockResponse::Success(GetUtxoStatusResult {
                name: "Services".to_string(),
                status: "error".to_string(),
                is_utxo: None,
                details: vec![],
                error: Some(
                    "could not look: WoC does not list the output as unspent and no second \
                     explorer confirmed it"
                        .to_string(),
                ),
            }))
            .build();
        let err = run_from(&ctx, true, &explorers, &one_negative)
            .await
            .unwrap_err()
            .to_string();
        assert!(err.contains("could not look"), "{err}");
        assert_eq!(fixture.total(), 0, "nothing was probed on no answer");

        let down = MockWalletServices::builder()
            .get_utxo_status_response(MockResponse::Error(
                MockErrorKind::NetworkError,
                "down".to_string(),
            ))
            .build();
        let err = run_from(&ctx, false, &explorers, &down)
            .await
            .unwrap_err()
            .to_string();
        assert!(err.contains("could not look"), "{err}");
    }

    /// #50: an outpoint of ours missing from a set one explorer gave is a
    /// question for the probe, never "spent"; the probe's rungs (WhatsOnChain
    /// rate limited, BananaBlocks "no spend") and an unspent set that could
    /// not be read leave it "could not decide", spendable and untouched.
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
            .get_utxo_status_response(listed("WoC", &[(&txid, 0, 1500)]))
            .is_utxo_response(MockResponse::Error(
                MockErrorKind::ServiceError,
                "one explorer's negative, the others could not look".to_string(),
            ))
            .build();

        run_from(&ctx, true, &explorers, &services).await.unwrap();
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
    /// toolbox's explorer ladder, and no hand-rolled WhatsOnChain request
    /// is left. Red at the base: neither named; the scan was one explorer.
    #[test]
    fn the_address_scan_asks_the_ladder_and_is_named_at_the_site() {
        let source = include_str!("sync.rs");
        let code = source.split("#[cfg(test)]").next().unwrap();
        assert!(!code.contains("/address/"), "a hand-rolled address scan");
        assert!(!code.contains("reqwest"), "a request of sync's own");
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
