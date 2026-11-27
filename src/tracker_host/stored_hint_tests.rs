//! The stored hint in the crate's own form, and the row it cannot read
//! (#612; bsv-tracker 0.9.0-beta.9, `### Hosts`; the stack's decisions
//! 28.8.1 (c), its fourth part). The row is inputs to replay: every text
//! 0.19.0-beta.8 stored reads back as its kind and writes back identical;
//! every word of `HintStatus::LABELS` is planted as a stored row with no
//! line of this host naming it and replayed as that kind; a status the
//! crate refuses holds its row as stored, surfaced, across passes; and
//! every re-ask this host can reach is shown opening with the crate's word.

use std::collections::BTreeSet;

use super::dead_proof_tests::stored_bytes;
use super::tests::{opts, wallet_storage, FixedClock};
use super::*;
use bsv_wallet_toolbox::services::mock::MockWalletServices;

const OBSERVED: Timestamp = 1_700_000_000;

/// 0.19.0-beta.8's `StoredHint` and its `of`, copied from origin/beta at
/// 2f2726c (`src/tracker_host.rs:414-458`), so the texts below are what
/// that build stored. Its match gains the arm a host's sentence asks for:
/// a kind the copy does not name fails aloud.
#[derive(Serialize)]
struct BetaEightStoredHint {
    source: String,
    status: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    height: Option<Height>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    reason: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    competitors: Vec<TxId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    courier: Option<String>,
    observed: Timestamp,
}

impl BetaEightStoredHint {
    fn of(hint: &Hint) -> Self {
        let mut courier = None;
        let (status, height, reason, competitors) = match &hint.status {
            HintStatus::Accepted => ("accepted", None, None, vec![]),
            HintStatus::Seen => ("seen", None, None, vec![]),
            HintStatus::Mined { height } => ("mined", Some(*height), None, vec![]),
            HintStatus::StaleBlock => ("stale_block", None, None, vec![]),
            HintStatus::Rejected { reason } => ("rejected", None, Some(reason.clone()), vec![]),
            HintStatus::DoubleSpend { competitors } => {
                ("double_spend", None, None, competitors.clone())
            }
            HintStatus::OrphanMempool => ("orphan_mempool", None, None, vec![]),
            HintStatus::Unknown => ("unknown", None, None, vec![]),
            HintStatus::MempoolPresent { source } => {
                courier = Some(source.clone());
                ("mempool_present", None, None, vec![])
            }
            other => panic!("a kind 0.19.0-beta.8's map did not name: {other:?}"),
        };
        Self {
            source: hint.source.clone(),
            status: status.to_string(),
            height,
            reason,
            competitors,
            courier,
            observed: hint.observed,
        }
    }
}

fn competitor() -> TxId {
    "ee".repeat(32)
}

/// Every text 0.19.0-beta.8's `of` wrote, by kind, key order included:
/// each of the nine kinds, a `rejected` with an empty reason, and a
/// `double_spend` with no competitor named.
fn beta_eight_rows() -> Vec<(String, HintStatus)> {
    let mut rows = beta_eight_payload_rows();
    rows.extend(beta_eight_bare_rows());
    rows
}

/// The texts of the five kinds with no payload.
fn beta_eight_bare_rows() -> Vec<(String, HintStatus)> {
    vec![
        (
            r#"{"source":"arc","status":"accepted","observed":1700000000}"#.to_string(),
            HintStatus::Accepted,
        ),
        (
            r#"{"source":"arc","status":"seen","observed":1700000000}"#.to_string(),
            HintStatus::Seen,
        ),
        (
            r#"{"source":"arc","status":"stale_block","observed":1700000000}"#.to_string(),
            HintStatus::StaleBlock,
        ),
        (
            r#"{"source":"arc","status":"orphan_mempool","observed":1700000000}"#.to_string(),
            HintStatus::OrphanMempool,
        ),
        (
            r#"{"source":"arc","status":"unknown","observed":1700000000}"#.to_string(),
            HintStatus::Unknown,
        ),
    ]
}

/// The texts of the four kinds with a payload, a `rejected` with an empty
/// reason and a `double_spend` with no competitor among them.
fn beta_eight_payload_rows() -> Vec<(String, HintStatus)> {
    vec![
        (
            r#"{"source":"arc","status":"mined","height":900,"observed":1700000000}"#.to_string(),
            HintStatus::Mined { height: 900 },
        ),
        (
            r#"{"source":"arc","status":"rejected","reason":"r","observed":1700000000}"#
                .to_string(),
            HintStatus::Rejected {
                reason: "r".to_string(),
            },
        ),
        (
            r#"{"source":"arc","status":"rejected","reason":"","observed":1700000000}"#.to_string(),
            HintStatus::Rejected {
                reason: String::new(),
            },
        ),
        (
            format!(
                r#"{{"source":"arc","status":"double_spend","competitors":["{}"],"observed":1700000000}}"#,
                competitor()
            ),
            HintStatus::DoubleSpend {
                competitors: vec![competitor()],
            },
        ),
        (
            r#"{"source":"arc","status":"double_spend","observed":1700000000}"#.to_string(),
            HintStatus::DoubleSpend {
                competitors: vec![],
            },
        ),
        (
            r#"{"source":"arc","status":"mempool_present","courier":"c","observed":1700000000}"#
                .to_string(),
            HintStatus::MempoolPresent {
                source: "c".to_string(),
            },
        ),
    ]
}

/// The rows already stored read as they are (#612 (i)): each text the
/// hand map of 0.19.0-beta.8 wrote is the text its copy writes for the
/// kind, reads back as that kind through the crate's form, and writes
/// back identical, through `StoredHint` and through `StoredHint::of`.
#[test]
fn every_text_the_hand_map_stored_reads_as_its_kind_and_writes_back_identical() {
    let rows = beta_eight_rows();
    for (text, status) in &rows {
        let hint = Hint::new("arc", status.clone(), OBSERVED);
        assert_eq!(
            &serde_json::to_string(&BetaEightStoredHint::of(&hint)).unwrap(),
            text,
            "the copy of 0.19.0-beta.8's map writes this text for {status:?}"
        );
        let read: StoredHint = serde_json::from_str(text).unwrap_or_else(|e| panic!("{text}: {e}"));
        assert_eq!(read.hint(), hint, "{text}");
        assert_eq!(&serde_json::to_string(&read).unwrap(), text);
        assert_eq!(
            &serde_json::to_string(&StoredHint::of(&hint)).unwrap(),
            text
        );
    }
    // Every kind 0.19.0-beta.8 knew; a later minor only appends to LABELS.
    let kinds: BTreeSet<&str> = rows.iter().map(|(_, s)| s.label()).collect();
    let nine: BTreeSet<&str> = HintStatus::LABELS[..9].iter().copied().collect();
    assert_eq!(kinds, nine);
}

/// The answers that move on purpose (#612 (iv)): a `mined` hint with no
/// height, a `mempool_present` hint with no courier and a `rejected` hint
/// with no reason are refused, where the hand map read the `mined` and
/// `mempool_present` ones as `unknown` and the `rejected` one as an empty
/// reason. 0.19.0-beta.8's `of` wrote none of the three.
#[test]
fn a_kind_stored_without_its_payload_is_refused_never_read_as_unknown() {
    for text in [
        r#"{"source":"arc","status":"mined","observed":1700000000}"#,
        r#"{"source":"arc","status":"mempool_present","observed":1700000000}"#,
        r#"{"source":"arc","status":"rejected","observed":1700000000}"#,
        r#"{"source":"arc","status":"a_word_no_tracker_knows","observed":1700000000}"#,
    ] {
        assert!(
            serde_json::from_str::<StoredHint>(text).is_err(),
            "{text} must be refused"
        );
    }
}

/// The stored text of a hint of the kind `word`, planted with every
/// payload key a kind of 0.9.0-beta.9 reads, so a kind a later minor adds
/// to `HintStatus::LABELS` is planted with no line of this host naming it.
/// The crate's reader keeps the keys its kind has and the text is its own
/// writer's.
fn planted(word: &str) -> String {
    let full = serde_json::json!({
        "source": "arc",
        "status": word,
        "height": 900,
        "reason": "r",
        "competitors": [competitor()],
        "courier": "c",
        "observed": OBSERVED,
    });
    let read: StoredHint = serde_json::from_value(full)
        .unwrap_or_else(|e| panic!("`{word}`, a word of HintStatus::LABELS, does not plant: {e}"));
    serde_json::to_string(&read).unwrap()
}

async fn plant_row(storage: &StorageSqlx, txid: &str, hints: &str, word: &str, reask: &str) {
    sqlx::query(
        "INSERT INTO tracker_states (txid, word, hints, reask, next_ask_at, updated_at) \
         VALUES (?, ?, ?, ?, ?, 7)",
    )
    .bind(txid)
    .bind(word)
    .bind(hints)
    .bind(reask)
    // No re-ask falls due in the passes below: nothing is fetched.
    .bind(i64::try_from(OBSERVED + 10_000_000).unwrap())
    .execute(storage.writer())
    .await
    .unwrap();
}

/// Every word of `HintStatus::LABELS` (#612 (vi)), planted as a stored row
/// with its payload keys, is read by the pass as that kind and replayed
/// (no fault names it), and the pass writes the row back with its hints
/// column the identical text.
#[tokio::test]
async fn every_status_word_of_the_tracker_replays_from_a_stored_row_and_is_rewritten_identical() {
    let (storage, _) = wallet_storage().await;
    ensure_tables(&storage).await.unwrap();
    let clock = FixedClock(OBSERVED + 1_000);
    let mut rows = Vec::new();
    for (i, word) in HintStatus::LABELS.iter().enumerate() {
        let txid = format!("{:02x}", 0x40 + i).repeat(32);
        let column = format!("[{}]", planted(word));
        plant_row(&storage, &txid, &column, "built", "age").await;
        rows.push((txid, *word, column));
    }
    let services = MockWalletServices::builder().build();
    let report = tick(&storage, &services, &clock, &opts()).await.unwrap();
    assert_eq!(report.tracked, HintStatus::LABELS.len(), "{report:?}");
    assert!(
        !report.faults.iter().any(|f| f.contains(HINTS_UNREADABLE)),
        "{report:?}"
    );
    for (txid, word, column) in &rows {
        let row = load_row(&storage, txid).await.unwrap().unwrap();
        let labels: Vec<&str> = row.hints.iter().map(|h| h.status.label()).collect();
        assert_eq!(labels, vec![*word], "{txid}");
        let (hints, updated): (String, i64) =
            sqlx::query_as("SELECT hints, updated_at FROM tracker_states WHERE txid = ?")
                .bind(txid)
                .fetch_one(storage.pool())
                .await
                .unwrap();
        assert_eq!(
            &hints, column,
            "`{word}` is rewritten to the identical text"
        );
        assert_eq!(
            updated,
            i64::try_from(clock.0).unwrap(),
            "`{word}`'s row was written by the pass"
        );
    }
}

/// A status outside `HintStatus::LABELS`, a kind without its payload and a
/// column that is not JSON (#612 (iii), (vi)): each row is held as stored
/// across two passes (every column unchanged), named among each pass's
/// faults with its txid and the parse error, while a readable row beside
/// them is replayed and written.
#[tokio::test]
async fn an_unreadable_stored_hint_holds_its_row_and_is_surfaced_on_every_pass() {
    let (storage, _) = wallet_storage().await;
    ensure_tables(&storage).await.unwrap();
    let unreadable = [
        r#"[{"source":"arc","status":"a_word_no_tracker_knows","observed":1700000000}]"#,
        r#"[{"source":"arc","status":"seen","observed":1700000000},{"source":"arc","status":"mined","observed":1700000001}]"#,
        r#"[{"source":"arc","status":"mempool_present","observed":1700000000}]"#,
        "not json",
    ];
    let mut held = Vec::new();
    for (i, column) in unreadable.iter().enumerate() {
        let txid = format!("{:02x}", 0x60 + i).repeat(32);
        plant_row(&storage, &txid, column, "announced", "age").await;
        held.push((txid.clone(), stored_bytes(&storage, &txid).await));
    }
    let readable = "6f".repeat(32);
    plant_row(
        &storage,
        &readable,
        r#"[{"source":"arc","status":"seen","observed":1700000000}]"#,
        "built",
        "age",
    )
    .await;

    let services = MockWalletServices::builder().build();
    for pass in 0..2u64 {
        let clock = FixedClock(OBSERVED + 1_000 + pass * 60);
        let report = tick(&storage, &services, &clock, &opts()).await.unwrap();
        assert_eq!(report.tracked, unreadable.len() + 1, "{report:?}");
        for (txid, before) in &held {
            let named: Vec<&String> = report
                .faults
                .iter()
                .filter(|f| f.starts_with(&format!("{txid}: {HINTS_UNREADABLE}: ")))
                .collect();
            assert_eq!(named.len(), 1, "pass {pass}: {txid} named once: {report:?}");
            assert!(named[0].contains("does not parse"), "{}", named[0]);
            assert_eq!(
                &stored_bytes(&storage, txid).await,
                before,
                "pass {pass}: {txid}"
            );
        }
        let (word, updated): (String, i64) =
            sqlx::query_as("SELECT word, updated_at FROM tracker_states WHERE txid = ?")
                .bind(&readable)
                .fetch_one(storage.pool())
                .await
                .unwrap();
        assert_eq!(
            (word.as_str(), updated),
            ("seen", i64::try_from(clock.0).unwrap())
        );
    }
}

/// A held row read by every other reader (#612 (iii)): `/tx/:txid`'s read
/// gives the word and re-ask the row last stored and no tier; `heard` adds
/// nothing and writes nothing; the spend door offers nothing on it and
/// names the hold; every other caller of `load_row`, the spend guard among
/// them, gets an error naming it.
#[tokio::test]
async fn a_held_row_is_read_as_stored_and_offered_nothing_by_every_other_reader() {
    let (storage, _) = wallet_storage().await;
    ensure_tables(&storage).await.unwrap();
    let txid = &"7a".repeat(32);
    plant_row(
        &storage,
        txid,
        r#"[{"source":"arc","status":"a_word_no_tracker_knows","observed":1700000000}]"#,
        "announced",
        "age",
    )
    .await;
    let before = &stored_bytes(&storage, txid).await;
    let services = MockWalletServices::builder().build();
    // `/tx/:txid` reads the columns the last readable build wrote.
    assert_eq!(
        stored_word(&storage, txid).await.unwrap().unwrap(),
        ("announced".to_string(), None, Some("age".to_string()))
    );
    assert_eq!(stored_tier(&storage, txid).await.unwrap(), None);
    // A broadcaster's word heard for it is not added and nothing is written.
    let clock = FixedClock(OBSERVED + 5_000);
    let word = heard(
        &storage,
        &clock,
        &opts(),
        txid,
        Hint::new("taal|seen", HintStatus::Seen, clock.0),
        None,
        Keep::Nothing,
    )
    .await
    .unwrap();
    assert_eq!(word, ("announced".to_string(), Some("age".to_string())));
    assert_eq!(&stored_bytes(&storage, txid).await, before);
    // The spend door offers nothing on it and names the hold.
    let readings = read_creators(&storage, &services, &opts(), &[(txid.clone(), None)])
        .await
        .unwrap();
    let reading = &readings[txid];
    assert_eq!((reading.word, reading.door), ("announced", false));
    assert!(reading
        .fault
        .as_deref()
        .is_some_and(|f| f.starts_with(HINTS_UNREADABLE)));
    // The guard and every other caller of `load_row` get an error naming it.
    let fault = load_row(&storage, txid).await.unwrap_err().to_string();
    assert!(
        fault.starts_with(&format!("{txid}: {HINTS_UNREADABLE}")),
        "{fault}"
    );
}

/// Every re-ask this host can reach is shown by a string that opens with
/// its `Reask::label` (#612 (ii), (vi)), each string the one served before
/// the pin (`tests/tx_word_route.rs`, `tests/wire/061-tx.pending.json`),
/// and the kinds covered are counted against `Reask::LABELS`. A kind a
/// later tracker minor adds fails the count here, at that pin: it is shown
/// by its word alone (`reask_label`'s wildcard) until this host names its
/// payload.
#[test]
fn every_reask_is_shown_opening_with_the_crates_word_and_every_kind_is_covered() {
    let cases = [
        (Reask::Reorg { fork_height: 900 }, "reorg@900"),
        (Reask::Fork { height: 901 }, "fork@901"),
        (
            Reask::Hint(Hint::new("arcade|seen", HintStatus::Seen, OBSERVED)),
            "hint:arcade|seen",
        ),
        (Reask::ProofFailed { height: 902 }, "proof_failed@902"),
        (Reask::Recheck, "recheck"),
        (Reask::Age, "age"),
        (Reask::Spend, "spend"),
    ];
    for (reask, shown) in &cases {
        assert_eq!(reask_label(reask), *shown);
        assert!(shown.starts_with(reask.label()), "{shown}");
    }
    let covered: BTreeSet<&str> = cases.iter().map(|(r, _)| r.label()).collect();
    let all: BTreeSet<&str> = Reask::LABELS.iter().copied().collect();
    assert_eq!(
        (covered.len(), &covered),
        (Reask::LABELS.len(), &all),
        "a re-ask kind of Reask::LABELS this test does not cover: name its payload \
         in reask_label or keep its bare word, and add it here"
    );
}
