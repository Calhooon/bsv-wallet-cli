//! The host's store, judged by the rules that the mutation oracle found
//! unjudged (the owner's word of 2026-11-01, 23.5 (c); the third batch of
//! `src/tracker_host.rs`'s survivors of night 41340299977): a store an older
//! release made gains every table and column the tracker reads before it is
//! read (`ensure_tables`), and a mined word's stored header is the live one
//! only once its recheck is complete, never while a re-ask is open
//! (`settled_header_hash`, #258 over #233's replay invariant).

use super::tests::{header, header_with_hash, mined_at_900, opts, reorg_at_900, wallet_storage};
use super::*;

/// R6 (the state is the replay of what the host stored) across an upgrade:
/// a store from before each of the tracker's later columns and tables (the
/// chain feed's `chain`, 0.9.0; the stale follow-up's `verdict` and
/// `follow_up`, 0.14.0; `tracker_meta`; the dead row's reports'
/// `tracker_adopt_checks`, 0.19.0, #328) gains the missing one, whichever
/// the others are present. The read-first check sends nothing to the
/// writer only when every one is there.
#[tokio::test]
async fn a_store_an_older_release_made_gains_each_missing_table_and_column() {
    let present = [
        ("tracker_states", "follow_up"),
        ("tracker_states", "verdict"),
        ("tracker_states", "chain"),
        ("tracker_meta", "value"),
        ("tracker_adopt_checks", "reported"),
    ];
    let older = [
        "ALTER TABLE tracker_states DROP COLUMN follow_up",
        "ALTER TABLE tracker_states DROP COLUMN verdict",
        "ALTER TABLE tracker_states DROP COLUMN chain",
        "DROP TABLE tracker_meta",
        "DROP TABLE tracker_adopt_checks",
    ];
    for (sql, missing) in older.into_iter().zip(present) {
        let (storage, _user) = wallet_storage().await;
        ensure_tables(&storage).await.unwrap();
        sqlx::query(sql).execute(storage.writer()).await.unwrap();
        let (table, column) = missing;
        assert!(!crate::store::has_column(storage.pool(), table, column)
            .await
            .unwrap());

        ensure_tables(&storage).await.unwrap();
        for (table, column) in present {
            assert!(
                crate::store::has_column(storage.pool(), table, column)
                    .await
                    .unwrap(),
                "{table}.{column} after `{sql}`"
            );
        }
    }
}

/// #258 over #233: a proof checked on a header a kept reorg deactivated
/// stores the live header's hash only when the recheck on the new branch
/// completed (the word plain `mined`, not suspect, no re-ask). A stored
/// verdict reopens the mined word's recheck (a re-ask, not suspect), and
/// while it is open the row keeps the header its proof was checked on.
#[tokio::test]
async fn an_open_reask_keeps_the_checked_header_where_a_complete_recheck_takes_the_live_one() {
    let txid = "c9".repeat(32);
    let (storage, services, clock) = mined_at_900(&txid).await;
    let gone = header_with_hash(900, &txid, 0).hash;
    let event = decode_envelope(reorg_at_900(&gone, &txid).to_string().as_bytes()).unwrap();
    deliver_chain_event(&storage, &services, &clock, &opts(), 1, event)
        .await
        .unwrap();
    let live = header_with_hash(900, &txid, 1).hash;
    services.set_header_for_height(header(900, &txid, 1));
    let snapshot = HeaderSnapshot::take(&services, [900]).await;
    let settled = load_row(&storage, &txid).await.unwrap().unwrap();
    let kept: Vec<_> = settled.deactivated()[&900]
        .iter()
        .map(|h| h.hash.clone())
        .collect();
    assert_eq!(kept, vec![gone.clone()]);

    // A stored verdict on a mined word is its recheck reopened (bsv-tracker
    // `on_verdict`): the re-ask without the suspect mark.
    let open = TrackedRow {
        verdict: Some(StoredVerdict::Conflicted {
            competitors: vec!["ee".repeat(32)],
        }),
        ..settled.clone()
    };
    for (row, reask, stored) in [
        (&settled, None, Some(live)),
        (&open, Some(&Reask::Recheck), None),
    ] {
        let (state, fault) = row.replay(&opts().params(), &snapshot);
        assert_eq!(fault, None);
        let Word::Mined(mined) = state.word() else {
            panic!("a replay off mined: {:?}", state.word());
        };
        assert_eq!(mined.checked().header().hash, gone);
        assert!(!state.suspect());
        assert_eq!(state.reask(), reask);
        assert_eq!(
            settled_header_hash(&state, &snapshot, &row.deactivated()),
            stored,
            "the verdict {:?}",
            row.verdict
        );
    }
}
