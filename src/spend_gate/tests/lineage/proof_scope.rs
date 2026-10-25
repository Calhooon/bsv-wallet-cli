//! Proof reads scoped to the creators judged by the gate (#217).

use super::*;

/// A proven, conflicted P whose heir exists in the store, but neither
/// P's nor its heir's outputs belong to the allocation's candidate set.
async fn unrelated_proven_lineage(store: &Store) {
    let (p, d) = dead_parent_with_proof(store, Death::Word("conflicted")).await;
    sqlx::query("UPDATE outputs SET change = 0 WHERE txid IN (?, ?)")
        .bind(p)
        .bind(d)
        .execute(store.0.writer())
        .await
        .unwrap();
}

/// The service history without its wall clock, as in review 213's probe.
fn call_history(services: &MockWalletServices) -> String {
    services
        .call_history()
        .iter()
        .map(|c| format!("{} {:?} {}", c.method, c.args, c.success))
        .collect::<Vec<_>>()
        .join("\n")
}

/// P's earliest heir is outside this call, but a later heir is judged.
/// P must retain its place before Q in the shared header reading.
#[tokio::test]
async fn selected_candidates_keep_the_original_proof_order() {
    let store = wallet_store().await;
    let (p, q) = ("d1".repeat(32), "d2".repeat(32));
    let p0 = coin(&store.0, store.1, &p, true, Some(800), 6_000, 3).await;
    let p1 = second_output(&store, &p, 1).await;
    let q0 = coin(&store.0, store.1, &q, true, Some(801), 6_000, 3).await;
    for (name, input) in [("d3", p0), ("d4", q0), ("d5", p1)] {
        let heir = name.repeat(32);
        change(&store, &heir).await;
        spends(&store.0, &heir, input).await;
    }
    for ancestor in [&p, &q] {
        node_verdict(&store.0, ancestor, "conflicted").await;
    }
    let lineage = lineage_candidates(&store.0).await.unwrap();
    let creators = [("d4".repeat(32), None), ("d5".repeat(32), None)];
    let selected: Vec<_> = lineage
        .proofs(&creators)
        .into_iter()
        .map(|(txid, _)| txid)
        .collect();
    assert_eq!(selected, vec![p, q]);
}

/// The live coin and optional unrelated lineage used by both door probes.
async fn probe_wallet(unrelated: bool) -> (Store, String, i64) {
    let store = wallet_store().await;
    let txid = "b2".repeat(32);
    let output = change(&store, &txid).await;
    if unrelated {
        unrelated_proven_lineage(&store).await;
    }
    (store, txid, output)
}

/// Read an allocation with the probe's own-change rule.
async fn allocation_read(
    store: &Store,
    services: &MockWalletServices,
    need: Option<u64>,
    retry: bool,
) -> Assessment {
    assess_retrying(
        UserCoins::new(&store.0, store.1 .0),
        services,
        &opts(1, true),
        need,
        retry,
    )
    .await
    .unwrap()
}

/// P has no allocation candidate of its own, so only D can bring its proof.
async fn ancestor_only() -> (Store, String, String) {
    let (store, (p, d), _) = conflicted_with_proof().await;
    sqlx::query("UPDATE outputs SET change = 0 WHERE txid = ?")
        .bind(&p)
        .execute(store.0.writer())
        .await
        .unwrap();
    (store, p, d)
}

/// Issue 217: P elsewhere in the store must cost neither another tip
/// read nor the retry pause when the named live coin has no proof.
#[tokio::test]
async fn an_unrelated_proven_candidate_does_not_pause_the_named_door() {
    for unrelated in [false, true] {
        let (store, live, _) = probe_wallet(unrelated).await;
        let services = silent_tip();
        let start = std::time::Instant::now();
        let held = assess_named(
            UserCoins::new(&store.0, store.1 .0),
            &services,
            &opts(1, true),
            &[(live.clone(), 0)],
            &[],
            None,
            true,
        )
        .await
        .unwrap();
        let elapsed = start.elapsed();
        println!(
            "named unrelated={unrelated}: {elapsed:?}, {}",
            call_history(&services)
        );
        assert!(held.is_empty(), "{held:?}");
        assert_eq!(call_history(&services), "get_chain_tip_header [] false");
        assert!(elapsed < HEADER_RETRY_PAUSE, "the named door paused");
    }
}

/// The same probe on the allocation, both full and lazy. P's heir is
/// outside the whole eligible set, so it must never consume the retry.
#[tokio::test]
async fn an_unrelated_proven_candidate_does_not_pause_allocation() {
    for need in [None, Some(1_000)] {
        for unrelated in [false, true] {
            let (store, _, live) = probe_wallet(unrelated).await;
            let services = silent_tip();
            let start = std::time::Instant::now();
            let a = allocation_read(&store, &services, need, true).await;
            let elapsed = start.elapsed();
            println!(
                "allocation {need:?} unrelated={unrelated}: {elapsed:?}, {}",
                call_history(&services)
            );
            assert_eq!(a.offer(), vec![live]);
            assert!(a.held.is_empty());
            assert!(!a.headers_retried);
            assert_eq!(call_history(&services), "get_chain_tip_header [] false");
            assert!(elapsed < HEADER_RETRY_PAUSE, "the allocation paused");
        }
    }
}

/// P is only an ancestor, never a judged creator. Both doors still read
/// its proof and retry the silent tip, leaving D held by the same lineage.
#[tokio::test]
async fn a_proven_ancestor_still_retries_on_both_doors() {
    let (store, p, d) = ancestor_only().await;
    let want = format!("dead lineage: ancestor {p} conflicted");
    for need in [None, Some(1_000)] {
        let services = silent_tip();
        let a = allocation_read(&store, &services, need, true).await;
        assert_eq!(reason_of(&a, &d), Some(want.clone()));
        assert!(a.headers_retried);
        assert_eq!(services.call_count("get_chain_tip_header"), 2);
    }
    let services = silent_tip();
    let held = assess_named(
        UserCoins::new(&store.0, store.1 .0),
        &services,
        &opts(1, true),
        &[(d, 0)],
        &[],
        None,
        true,
    )
    .await
    .unwrap();
    assert_eq!(held.len(), 1);
    assert_eq!(held[0].reason, want);
    assert_eq!(services.call_count("get_chain_tip_header"), 2);
}

/// A lazy offer's bound includes later eligible creators. Reading their
/// ancestors with the initial batch preserves the shared retry, even
/// when the offer covers its need before reaching those creators.
#[tokio::test]
async fn a_lazy_offer_keeps_later_ancestors_in_the_initial_read() {
    let (store, _, d) = ancestor_only().await;
    for i in 0..40u8 {
        let txid = format!("{:02x}", 0x20 + i).repeat(32);
        coin(&store.0, store.1, &txid, true, None, 10_000, 1).await;
    }
    let services = silent_tip();
    let gate = opts(1, true);
    let (a, ()) = tokio::join!(
        assess_retrying(
            UserCoins::new(&store.0, store.1 .0),
            &services,
            &gate,
            Some(1_000),
            true,
        ),
        recover_after_300_ms(&services)
    );
    let a = a.unwrap();
    assert!(a.headers_retried);
    assert_eq!(services.call_count("get_chain_tip_header"), 2);
    assert_eq!(services.call_count("get_header_for_height"), 2);
    assert_eq!(a.offered.len(), 1);
    assert_eq!(a.unread.len(), 40);
    assert!(a.held.is_empty(), "{d} must remain unread: {:?}", a.held);
}

/// Review 213's no-dead-candidate history: both needs, both retry modes,
/// both doors, and answering or silent headers. Each line is also replayed
/// against beta, where the proof filter is absent.
#[tokio::test]
async fn a_wallet_without_dead_candidates_keeps_its_call_history() {
    let store = wallet_store().await;
    let (p, d, _) = parent_and_child(&store, None).await;
    let m = "a1".repeat(32);
    coin(&store.0, store.1, &m, true, Some(800), 5_000, 3).await;
    coin(&store.0, store.1, &"b1".repeat(32), false, None, 7_000, 3).await;
    for silent in [false, true] {
        let services = headers_with(&m, 800, 900);
        if silent {
            services.set_tip_unavailable();
        }
        let once = format!(
            "get_chain_tip_header [] {}\nget_header_for_height [\"800\"] true",
            !silent
        );
        for retry in [false, true] {
            let want = if silent && retry {
                format!("{once}\n{once}")
            } else {
                once.clone()
            };
            for need in [None, Some(1_000)] {
                services.reset_history();
                assess_retrying(
                    UserCoins::new(&store.0, store.1 .0),
                    &services,
                    &opts(1, true),
                    need,
                    retry,
                )
                .await
                .unwrap();
                let history = call_history(&services);
                assert_eq!(history, want);
                println!(
                    "HISTORY silent={silent} retry={retry} need={need:?}: {}",
                    serde_json::to_string(&history).unwrap()
                );
            }
            services.reset_history();
            assess_named(
                UserCoins::new(&store.0, store.1 .0),
                &services,
                &opts(1, true),
                &[(m.clone(), 0), (d.clone(), 0), (p.clone(), 0)],
                &[],
                None,
                retry,
            )
            .await
            .unwrap();
            let history = call_history(&services);
            assert_eq!(history, want);
            println!(
                "HISTORY silent={silent} retry={retry} named: {}",
                serde_json::to_string(&history).unwrap()
            );
        }
    }
}

/// [`ancestor_only`] beside 40 larger unproven coins, so D's creator is
/// the 41st eligible creator, in the allocation's second lazy batch.
/// A need of 400,001 is past the 40 coins, so D is judged. (P, D, the 41
/// coins named.)
async fn later_batch_heir() -> (Store, String, String, Vec<(String, u32)>) {
    let (store, p, d) = ancestor_only().await;
    let mut named: Vec<(String, u32)> = (0x20..0x48u8)
        .map(|b| (format!("{b:02x}").repeat(32), 0))
        .collect();
    for (txid, _) in &named {
        coin(&store.0, store.1, txid, true, None, 10_000, 1).await;
    }
    named.push((d.clone(), 0));
    (store, p, d, named)
}

const LATER_HEIR_NEED: u64 = 400_001;

/// A tip silent at the start that answers after 300 ms when `recover`.
async fn tip_after(services: &MockWalletServices, recover: bool) {
    if recover {
        recover_after_300_ms(services).await;
    }
}

/// Both doors' answers under a tip silent at the start that recovers after
/// 300 ms (`recover`) or stays silent: (the allocation, the named door,
/// the named door's tip reads).
async fn later_heir_doors(
    store: &Store,
    named: &[(String, u32)],
    recover: bool,
) -> (Assessment, Vec<HeldCoin>, usize) {
    let services = silent_tip();
    let (a, ()) = tokio::join!(
        allocation_read(store, &services, Some(LATER_HEIR_NEED), true),
        tip_after(&services, recover)
    );
    let services = silent_tip();
    let coins = UserCoins::new(&store.0, store.1 .0);
    let gate = opts(1, true);
    let door = assess_named(coins, &services, &gate, named, &[], None, true);
    let (held, ()) = tokio::join!(door, tip_after(&services, recover));
    let tips = services.call_count("get_chain_tip_header");
    (a, held.unwrap(), tips)
}

/// The heir in the second lazy batch is judged: P's proof, read with the
/// first batch, checks on the retry once the tip recovers, so D is offered
/// with the other 40 on both doors, as on beta. A selection filtered per
/// batch leaves P unread and holds D.
#[tokio::test]
async fn a_later_batch_heir_is_offered_when_the_tip_recovers() {
    let (store, _, d, named) = later_batch_heir().await;
    let d0: i64 = sqlx::query_scalar("SELECT output_id FROM outputs WHERE txid = ?")
        .bind(&d)
        .fetch_one(store.0.pool())
        .await
        .unwrap();
    let (a, held, tips) = later_heir_doors(&store, &named, true).await;
    assert_eq!(reason_of(&a, &d), None, "D must be offered: {:?}", a.held);
    assert!(a.offer().contains(&d0), "D is not offered");
    assert_eq!(a.offered.len(), 41);
    assert!(a.held.is_empty(), "{:?}", a.held);
    assert!(held.is_empty(), "the named door held {held:?}");
    assert!(a.headers_retried);
    assert_eq!(tips, 2);
}

/// The tip silent throughout: P keeps its stored word, so D in the second
/// lazy batch is held by P's lineage on both doors, as on beta.
#[tokio::test]
async fn a_later_batch_heir_is_held_when_the_tip_stays_silent() {
    let (store, p, d, named) = later_batch_heir().await;
    let want = format!("dead lineage: ancestor {p} conflicted");
    let (a, held, tips) = later_heir_doors(&store, &named, false).await;
    assert_eq!(reason_of(&a, &d), Some(want.clone()));
    assert_eq!(a.offered.len(), 40);
    assert_eq!(a.held.len(), 1, "{:?}", a.held);
    assert_eq!(held.len(), 1, "{held:?}");
    assert!(held[0].outpoint.starts_with(&d));
    assert_eq!(held[0].reason, want);
    assert!(a.headers_retried);
    assert_eq!(tips, 2);
}
