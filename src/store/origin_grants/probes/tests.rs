//! The probe record (#539): one row per (originator, door, power key) with
//! its count and times, the cap's fold into the remainder, the table made
//! by `ensure`, and the grants' functions never touching it.

use super::super::tests::{wallet_db, NOW};
use super::super::{ensure, grant, listing, revoke, Granting, Power, Reach, OBJECTS};
use super::*;

fn probe(originator: &str, door: &str, power: Option<&str>, at: i64) -> Probe {
    Probe {
        originator: originator.to_string(),
        door: door.to_string(),
        power: power.map(str::to_string),
        at,
    }
}

/// `(originator, door, power, count, first_seen, last_seen)` of each row.
async fn rows(storage: &StorageSqlx) -> Vec<(String, String, Option<String>, i64, i64, i64)> {
    probes(storage)
        .await
        .unwrap()
        .into_iter()
        .map(|r| {
            let ProbeRow {
                originator,
                door,
                power,
                count,
                first_seen,
                last_seen,
                ..
            } = r;
            (originator, door, power, count, first_seen, last_seen)
        })
        .collect()
}

#[tokio::test]
async fn each_door_and_power_is_one_row_with_its_count_and_times() {
    let (_dir, storage) = wallet_db().await;
    let zero = Power::level_zero_key("wire probe").unwrap();
    assert_eq!(zero, r#"[false,0,"wire probe",""]"#);
    record(
        &storage,
        &[
            probe("New.Example:8080", "getVersion", None, NOW),
            probe("new.example", "createSignature", Some(&zero), NOW + 1),
        ],
    )
    .await
    .unwrap();
    record(
        &storage,
        &[probe("new.example", "getVersion", None, NOW + 5)],
    )
    .await
    .unwrap();
    let key = |s: &str| s.to_string();
    assert_eq!(
        rows(&storage).await,
        vec![
            (
                key("new.example"),
                key("createSignature"),
                Some(zero),
                1,
                NOW + 1,
                NOW + 1
            ),
            (key("new.example"), key("getVersion"), None, 2, NOW, NOW + 5),
        ]
    );
    let listed = probes(&storage).await.unwrap();
    assert_eq!(
        listed[1].raw_originator, "new.example",
        "the name last given"
    );
    // A polling page: a thousand answers in one batch are one row.
    let polls: Vec<Probe> = (0..1000)
        .map(|i| probe("poll.example", "getHeight", None, NOW + i))
        .collect();
    record(&storage, &polls).await.unwrap();
    let poll = &rows(&storage).await[2];
    assert_eq!((poll.3, poll.4, poll.5), (1000, NOW, NOW + 999));
    assert!(
        record(&storage, &[probe("http://x", "getVersion", None, NOW)])
            .await
            .is_err()
    );
}

#[tokio::test]
async fn past_the_cap_the_rows_seen_longest_ago_fold_into_the_remainder() {
    let (_dir, storage) = wallet_db().await;
    let named = |i: i64| Power::level_zero_key(&format!("probe number {i}")).unwrap();
    let past: i64 = 9;
    let seen: Vec<Probe> = (0..i64::try_from(PER_ORIGINATOR).unwrap() - 1 + past)
        .map(|i| probe("busy.example", "encrypt", Some(&named(i)), NOW + i))
        .collect();
    record(&storage, &seen).await.unwrap();
    record(&storage, &[probe("calm.example", "getVersion", None, NOW)])
        .await
        .unwrap();
    let all = rows(&storage).await;
    let busy: Vec<_> = all.iter().filter(|r| r.0 == "busy.example").collect();
    assert_eq!(busy.len(), PER_ORIGINATOR, "the cap holds the remainder");
    let remainder = busy.last().unwrap();
    assert_eq!(
        (remainder.1.as_str(), remainder.2.as_deref(), remainder.3),
        (REMAINDER, None, past),
        "the remainder is listed last"
    );
    assert_eq!((remainder.4, remainder.5), (NOW, NOW + past - 1));
    assert!(
        busy.iter().all(|r| r.2 != Some(named(past - 1))),
        "the rows seen longest ago folded"
    );
    assert!(
        busy.iter().any(|r| r.2 == Some(named(past))),
        "the rest kept"
    );
    // A row seen again is kept; one more new row folds the next oldest.
    record(
        &storage,
        &[
            probe("busy.example", "encrypt", Some(&named(past)), NOW + 100),
            probe("busy.example", "decrypt", None, NOW + 101),
        ],
    )
    .await
    .unwrap();
    let busy: Vec<_> = rows(&storage)
        .await
        .into_iter()
        .filter(|r| r.0 == "busy.example")
        .collect();
    assert_eq!(busy.len(), PER_ORIGINATOR);
    assert_eq!(busy.last().unwrap().3, past + 1);
    assert!(busy.iter().any(|r| r.2 == Some(named(past)) && r.3 == 2));
    assert!(busy.iter().all(|r| r.2 != Some(named(past + 1))));
    let calm: Vec<_> = rows(&storage)
        .await
        .into_iter()
        .filter(|r| r.0 == "calm.example")
        .collect();
    assert_eq!(calm.len(), 1, "another originator's rows are its own");
}

#[tokio::test]
async fn ensure_makes_the_table_and_the_grants_never_touch_it() {
    let (_dir, storage) = wallet_db().await;
    ensure(&storage).await.unwrap();
    // The table dropped takes its index with it.
    let dropped = sqlx::query("DROP TABLE origin_probes");
    dropped.execute(storage.writer()).await.unwrap();
    assert!(
        ensure(&storage).await.unwrap(),
        "the missing table was not seen"
    );
    assert!(OBJECTS.contains(&("table", "origin_probes")));
    record(&storage, &[probe("page.example", "getNetwork", None, NOW)])
        .await
        .unwrap();
    let power = Power::basket("todo items").unwrap();
    let granting = Granting {
        originator: "page.example",
        power: &power,
        amount: None,
        expiry: 0,
    };
    grant(&storage, &granting, NOW).await.unwrap();
    revoke(&storage, "page.example", &Reach::Every, NOW + 1)
        .await
        .unwrap();
    let listed = listing(&storage, true, NOW).await.unwrap();
    assert_eq!(listed.grants.len(), 1);
    assert_eq!(
        listed.probes,
        probes(&storage).await.unwrap(),
        "the listing carries the record"
    );
    assert_eq!(rows(&storage).await.len(), 1, "a revoke leaves the record");
}
