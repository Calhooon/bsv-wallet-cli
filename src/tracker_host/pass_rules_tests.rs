//! The host's rules that the mutation oracle found unjudged (the owner's
//! word of 2026-11-01, 23.5 (c); the first batch of `src/tracker_host.rs`'s
//! survivors of night 41340299977): a hint never rewrites a proven word and
//! is heard once per broadcaster word (the charter's evidence rule), the
//! pass takes its parameters from the configuration (R3 (b)), a header
//! moved under a mined word is found without reading every header and the
//! ring of tips stays bounded (never a schedule over every transaction),
//! one transaction's re-asks stop at their cap (the re-ask policy), and the
//! pass's line says what it did.

use super::tests::{
    header, header_with_hash, insert_tx, mined_at_900, opts, proof_answer, wallet_storage,
    FixedClock,
};
use super::*;
use bsv_wallet_toolbox::services::mock::MockWalletServices;
use bsv_wallet_toolbox::BROADCAST_STATUS_MINED;

/// The evidence rule (the tracker charter, section 2): a broadcaster's word
/// heard about a transaction whose path the host already stored is read,
/// never fed. The word stays `mined` at its height with no re-ask, and the
/// call returns the stored word, not a replay against no headers.
#[tokio::test]
async fn a_hint_heard_after_a_stored_path_never_rewrites_the_mined_word() {
    let txid = "e1".repeat(32);
    let (storage, _services, clock) = mined_at_900(&txid).await;
    let before = stored_word(&storage, &txid).await.unwrap().unwrap();
    assert_eq!(before, ("mined".to_string(), Some(900), None));

    let word = heard(
        &storage,
        &clock,
        &opts(),
        &txid,
        Hint::new(
            "arcade|rejected",
            HintStatus::Rejected {
                reason: "a late refusal".to_string(),
            },
            clock.0,
        ),
        Some(clock.0 + 3600),
        None,
    )
    .await
    .unwrap();
    assert_eq!(word, ("mined".to_string(), None));
    assert_eq!(stored_word(&storage, &txid).await.unwrap().unwrap(), before);
    let row = load_row(&storage, &txid).await.unwrap().unwrap();
    assert!(
        !row.hints.iter().any(|h| h.source == "arcade|rejected"),
        "the hint is not fed to a proven row"
    );
}

/// The evidence rule's hint side (section 2: a hint is recorded and may
/// move the hint tier): a tracked row without a path hears each new
/// broadcaster word, so a second broadcaster's `seen` moves `announced` to
/// `seen`; a word a broadcaster repeats is one hint, never a second entry.
#[tokio::test]
async fn heard_feeds_each_new_broadcaster_word_once_to_a_row_without_a_path() {
    let (storage, _user) = wallet_storage().await;
    let txid = "e2".repeat(32);
    let clock = FixedClock(1_800_000_000);
    let hear = |source: &'static str, status: HintStatus, at: Timestamp| {
        let (storage, txid, clock) = (&storage, &txid, &clock);
        async move {
            heard(
                storage,
                clock,
                &opts(),
                txid,
                Hint::new(source, status, at),
                None,
                None,
            )
            .await
            .unwrap()
        }
    };

    let word = hear("arcade|accepted", HintStatus::Accepted, clock.0).await;
    assert_eq!(word.0, "announced");
    let word = hear("arcade|accepted", HintStatus::Accepted, clock.0 + 60).await;
    assert_eq!(word.0, "announced");
    let word = hear("taal|seen", HintStatus::Seen, clock.0 + 120).await;
    assert_eq!(word.0, "seen");
    assert_eq!(
        stored_word(&storage, &txid).await.unwrap().unwrap().0,
        "seen"
    );

    let row = load_row(&storage, &txid).await.unwrap().unwrap();
    let sources: Vec<&str> = row.hints.iter().map(|h| h.source.as_str()).collect();
    assert_eq!(sources, vec!["arcade|accepted", "taal|seen"]);
}

/// R3 (b) and the re-ask policy's host parameters (the charter's section
/// 4: `ageThreshold` and the asks a pass runs are the host's): the pass
/// takes `TRACKER_AGE_SECS`, `TRACKER_MAX_ASKS` and `SPEND_DEPTH` from the
/// configuration, the depth floored at 1, and holds no feed and runs no
/// monitor of its own.
#[test]
fn the_pass_takes_its_age_its_asks_and_its_spend_depth_from_the_configuration() {
    let mut config = crate::config::Config::default();
    config.host.tracker_age_secs = 1234;
    config.host.tracker_max_asks = 7;
    config.host.spend_depth = 3;
    let options = TickOptions::from_config(&config);
    assert_eq!(
        (
            options.age_threshold,
            options.max_asks,
            options.spend_depth,
            options.feed_held,
            options.monitor_runs,
        ),
        (1234, 7, 3, false, false)
    );

    config.host.spend_depth = 0;
    assert_eq!(TickOptions::from_config(&config).spend_depth, 1);
}

/// The pass's line goes to debug when it is quiet and to info otherwise
/// (`broadcast_reconcile`): a pass that only read rows is quiet, and a pass
/// that did any one thing (adopted, fed, asked, announced, moved, followed
/// up, judged or faulted) is not.
#[test]
fn a_pass_is_quiet_only_when_it_did_nothing_but_read() {
    /// One count of the pass, and how a report comes to hold it.
    type Loud = (&'static str, fn(&mut TickReport));
    fn one() -> Vec<String> {
        vec!["ab".repeat(32)]
    }

    assert!(TickReport::default().is_quiet());
    assert!(TickReport {
        tracked: 3,
        ..TickReport::default()
    }
    .is_quiet());

    let loud: [Loud; 13] = [
        ("adopted", |r| r.adopted = 1),
        ("adopted_mined", |r| r.adopted_mined = one()),
        ("adopted_proof_failed", |r| r.adopted_proof_failed = one()),
        ("hints", |r| r.hints = 1),
        ("asked", |r| r.asked = 1),
        ("reannounced", |r| r.reannounced = one()),
        ("not_final", |r| r.not_final = one()),
        ("moved", |r| r.moved = one()),
        ("stale_known", |r| r.stale_known = one()),
        ("stale_announced", |r| r.stale_announced = one()),
        ("stale_to_monitor", |r| r.stale_to_monitor = one()),
        ("verdicts", |r| r.verdicts = one()),
        ("faults", |r| r.faults = one()),
    ];
    for (what, set) in loud {
        let mut report = TickReport::default();
        set(&mut report);
        assert!(!report.is_quiet(), "a pass with {what} is not quiet");
    }
}

/// The line `tracker-tick` prints and the daemon logs: every count of the
/// pass, in its place.
#[test]
fn the_pass_line_names_every_count() {
    let n = |count: usize| vec!["ab".repeat(32); count];
    let report = TickReport {
        tracked: 5,
        adopted: 1,
        adopted_mined: n(1),
        adopted_proof_failed: n(2),
        hints: 2,
        asked: 4,
        mined: n(1),
        no_proof: n(2),
        moved: n(3),
        reannounced: n(1),
        not_final: n(1),
        stale_known: n(1),
        stale_announced: n(2),
        verdicts: n(1),
        stale_to_monitor: n(1),
        faults: n(2),
        ..TickReport::default()
    };
    assert_eq!(
        report.summary(),
        "tracker tick: 5 tracked (1 new), 3 proven creator(s) adopted (1 mined, 2 proof failed), \
         2 hint(s), 1 announced again (1 still not final), 4 re-ask(s): 1 mined, 2 without a \
         proof, 3 moved, 5 stale followed up (1 known, 2 announced, 1 verdict(s), 1 the \
         monitor's), 2 fault(s)"
    );
}

/// The hash a header at `height` with merkle root `root` has.
fn hash_at(height: Height, root: &str) -> String {
    header_with_hash(height, root, 0).hash
}

fn root_at(height: Height) -> String {
    format!("{:064x}", height + 1)
}

/// A header service on one chain, every height up to `tip` answered, the
/// tip at `tip`; and the snapshot a pass takes of it.
async fn chain_to(tip: Height) -> (MockWalletServices, HeaderSnapshot) {
    let services = MockWalletServices::builder().build();
    for height in 0..=tip {
        services.set_header_for_height(header(height, &root_at(height), 0));
    }
    services.set_tip_header(header_with_hash(tip, &root_at(tip), 0));
    let snapshot = HeaderSnapshot::take(&services, []).await;
    (services, snapshot)
}

fn ring_of(heights: impl IntoIterator<Item = Height>) -> Vec<(Height, String)> {
    heights
        .into_iter()
        .map(|h| (h, hash_at(h, &root_at(h))))
        .collect()
}

/// Never a schedule over every transaction (section 4): a tip that
/// advanced on the chain the last pass saw moves no header, so no mined
/// row is read again; the ring gains the new tip.
#[tokio::test]
async fn a_tip_that_advanced_on_the_same_chain_moves_no_header() {
    let (services, mut snapshot) = chain_to(201).await;
    let (moved, ring) = moved_since(&services, &mut snapshot, ring_of([199, 200])).await;
    assert_eq!(moved, None);
    assert_eq!(ring, ring_of([199, 200, 201]));
    assert_eq!(services.call_count("get_header_for_height"), 1);
}

/// A tip the last pass saw above the active tip is gone from the active
/// chain: the header at that height moved, without a header read for it,
/// and the mined rows at or above it are read again; the ring keeps the
/// tip that stands.
#[tokio::test]
async fn a_ring_tip_above_the_active_tip_is_a_moved_header_without_a_read() {
    let (services, mut snapshot) = chain_to(200).await;
    services.set_header_for_height(header(201, &root_at(201), 0));
    let (moved, ring) = moved_since(&services, &mut snapshot, ring_of([199, 200, 201])).await;
    assert_eq!(moved, Some(201));
    assert_eq!(ring, ring_of([199, 200]));
    assert_eq!(services.call_count("get_header_for_height"), 0);
}

/// The ring of tips is bounded (`TIP_RING`, the last twelve): pass after
/// pass it keeps the newest twelve, never more.
#[tokio::test]
async fn the_ring_keeps_the_last_twelve_tips_pass_after_pass() {
    let mut ring = Vec::new();
    for tip in 100..=114 {
        let (services, mut snapshot) = chain_to(tip).await;
        let (moved, next) = moved_since(&services, &mut snapshot, ring).await;
        assert_eq!(moved, None, "the tip at {tip}");
        assert!(next.len() <= TIP_RING, "{} tips at {tip}", next.len());
        ring = next;
    }
    assert_eq!(ring, ring_of(103..=114));
}

/// The bound holds whatever ring the pass read: a stored ring longer than
/// twelve is cut to its newest twelve.
#[tokio::test]
async fn a_ring_read_longer_than_twelve_is_cut_to_its_newest_twelve() {
    let (services, mut snapshot) = chain_to(30).await;
    let (moved, ring) = moved_since(&services, &mut snapshot, ring_of(1..=30)).await;
    assert_eq!(moved, None);
    assert_eq!(ring, ring_of(19..=30));
}

/// The re-ask policy's cap (`MAX_ASKS_PER_TX`): a transaction asked sixteen
/// times in a row with no proof is not asked again, however long the pause
/// since, until a new hint names it.
#[tokio::test]
async fn a_transaction_asked_sixteen_times_without_a_proof_is_not_asked_again() {
    let services = MockWalletServices::builder()
        .get_merkle_path_response(proof_answer(None))
        .build();
    let (storage, user) = wallet_storage().await;
    let txid = "e9".repeat(32);
    insert_tx(&storage, user, &txid, "unproven").await;
    let mut now = 1_800_000_000;
    storage
        .record_broadcast_status(&txid, "arcade", BROADCAST_STATUS_MINED)
        .await
        .unwrap();
    let mut asked = 0;
    for _ in 0..MAX_ASKS_PER_TX + 4 {
        let report = tick(&storage, &services, &FixedClock(now), &opts())
            .await
            .unwrap();
        asked += report.asked;
        now += 100_000;
    }
    assert_eq!(asked, MAX_ASKS_PER_TX as usize);
    assert_eq!(
        services.call_count("get_merkle_path"),
        MAX_ASKS_PER_TX as usize
    );
}

/// The host's row is its replay's input (R6: the tracker's state is the
/// replay of what the host stored): each broadcaster word a hint carries
/// reads back from the stored row as the word heard, a `mined` claim with
/// its height among them, never as `unknown`.
#[test]
fn every_broadcaster_word_reads_back_from_the_stored_row_as_heard() {
    let words = [
        HintStatus::Accepted,
        HintStatus::Seen,
        HintStatus::Mined { height: 900 },
        HintStatus::StaleBlock,
        HintStatus::Rejected {
            reason: "a refusal".to_string(),
        },
        HintStatus::DoubleSpend {
            competitors: vec!["ee".repeat(32)],
        },
        HintStatus::OrphanMempool,
        HintStatus::MempoolPresent {
            source: "courier-a".to_string(),
        },
    ];
    for status in words {
        let hint = Hint::new("arcade", status, 1_700_000_000);
        let json = serde_json::to_string(&StoredHint::of(&hint)).unwrap();
        let back: StoredHint = serde_json::from_str(&json).unwrap();
        assert_eq!(back.hint(), hint);
    }
}

/// A failing check is reported once and again only on a change of its
/// state (#328 (2)), across an upgrade too: a record v0.19.0-beta.6 stored
/// (`tracker_adopt_checks`, the fault's class in its words: `parse` for a
/// toolbox path that does not parse, `word` for one that checks but
/// replays to another word) is the same state when the same path is
/// checked again, so a marked creator is not reported again; a record of
/// another class is.
#[test]
fn a_check_record_stored_by_the_release_before_is_not_reported_again() {
    let txid = "e3".repeat(32);
    let marked = |bump: Option<String>| TrackedRow {
        bump,
        stored_word: Some("rejected".to_string()),
        next_ask_at: Some(700),
        ..TrackedRow::new(&txid)
    };
    let unparsed = marked(None);
    let parsed = marked(Some(super::tests::path_of(&txid, 900)));
    let path = "0123456789abcdef";
    let cases = [
        (&unparsed, "parse", false),
        (&parsed, "word", false),
        (&unparsed, "root", true),
        (&parsed, "invalid", true),
    ];
    for (row, class, again) in cases {
        let record = CheckRecord {
            reported: Some(format!("{class} {path}")),
            path: path.to_string(),
            asks: 0,
        };
        let (mut report, mut writes) = (TickReport::default(), Vec::new());
        let check = record.of_check(row, None);
        check.report(&mut report, "the line".to_string(), &mut writes);
        assert_eq!(!report.faults.is_empty(), again, "a stored {class} record");
        assert_eq!(!writes.is_empty(), again, "a stored {class} record");
    }
}
