//! The month's tally in `origin_spends` (#414, the design's L6, W7): a
//! spend per flow, its month in UTC, the deferred path's reference then its
//! txid, and the abort's delete.

use super::tests::{wallet_db, NOV_FIRST, NOW, OCT_LAST};
use super::*;

async fn rows(storage: &StorageSqlx) -> Vec<(String, String, Option<String>, Option<String>, i64)> {
    sqlx::query_as(
        "SELECT originator, month, txid, reference, satoshis FROM origin_spends ORDER BY spend_id",
    )
    .fetch_all(storage.pool())
    .await
    .unwrap()
}

#[tokio::test]
async fn a_spend_is_tallied_per_flow_under_its_hostname_and_month() {
    let (_dir, storage) = wallet_db().await;
    let txid = "ab".repeat(32);
    spent(&storage, "app.example:8443", Flow::Signed(&txid), 2500, NOW)
        .await
        .unwrap();
    spent(&storage, "App.Example", Flow::Deferred("cmVm"), 500, NOW)
        .await
        .unwrap();
    assert_eq!(
        rows(&storage).await,
        vec![
            (
                "app.example".into(),
                "2026-11".into(),
                Some(txid),
                None,
                2500
            ),
            (
                "app.example".into(),
                "2026-11".into(),
                None,
                Some("cmVm".into()),
                500
            ),
        ]
    );
    assert_eq!(
        month_tally(&storage, "app.example", NOW).await.unwrap(),
        3000
    );
    assert_eq!(
        month_tally(&storage, "other.example", NOW).await.unwrap(),
        0
    );
    let e = spent(
        &storage,
        "app.example",
        Flow::Signed(&"cd".repeat(32)),
        -1,
        NOW,
    )
    .await
    .unwrap_err();
    assert_eq!(e.downcast_ref::<Refused>(), Some(&Refused::Negative));
    assert_eq!(rows(&storage).await.len(), 2, "a refused spend writes none");
    // Zero is satoshis too: tallied, the month unmoved.
    let zero = "ef".repeat(32);
    spent(&storage, "app.example", Flow::Signed(&zero), 0, NOW)
        .await
        .unwrap();
    assert_eq!(rows(&storage).await.len(), 3);
    assert_eq!(
        month_tally(&storage, "app.example", NOW).await.unwrap(),
        3000
    );
}

#[tokio::test]
async fn the_tally_crosses_a_utc_month_boundary() {
    let (_dir, storage) = wallet_db().await;
    spent(
        &storage,
        "app.example",
        Flow::Signed(&"ab".repeat(32)),
        60_000,
        OCT_LAST,
    )
    .await
    .unwrap();
    assert_eq!(
        month_tally(&storage, "app.example", OCT_LAST)
            .await
            .unwrap(),
        60_000
    );
    assert_eq!(
        month_tally(&storage, "app.example", NOV_FIRST)
            .await
            .unwrap(),
        0
    );
    spent(
        &storage,
        "app.example",
        Flow::Signed(&"cd".repeat(32)),
        1,
        NOV_FIRST,
    )
    .await
    .unwrap();
    assert_eq!(
        month_tally(&storage, "app.example", NOV_FIRST)
            .await
            .unwrap(),
        1
    );
    assert_eq!(
        month_tally(&storage, "app.example", OCT_LAST)
            .await
            .unwrap(),
        60_000
    );
}

#[tokio::test]
async fn the_deferred_path_holds_its_reference_then_its_txid() {
    let (_dir, storage) = wallet_db().await;
    spent(&storage, "app.example", Flow::Deferred("cmVm"), 2500, NOW)
        .await
        .unwrap();
    let txid = "ab".repeat(32);
    assert_eq!(signed(&storage, "cmVm", &txid).await.unwrap(), 1);
    assert_eq!(
        rows(&storage).await,
        vec![(
            "app.example".into(),
            "2026-11".into(),
            Some(txid.clone()),
            Some("cmVm".into()),
            2500
        )]
    );
    // A second signing fills nothing; an unknown reference fills nothing.
    assert_eq!(signed(&storage, "cmVm", &"cd".repeat(32)).await.unwrap(), 0);
    assert_eq!(signed(&storage, "b3RoZXI=", &txid).await.unwrap(), 0);
    assert_eq!(rows(&storage).await[0].2, Some(txid));
}

#[tokio::test]
async fn an_abort_deletes_the_row_its_reference_or_txid_names_for_its_originator() {
    let (_dir, storage) = wallet_db().await;
    let (signed_txid, kept) = ("ab".repeat(32), "cd".repeat(32));
    spent(&storage, "app.example", Flow::Deferred("cmVm"), 2500, NOW)
        .await
        .unwrap();
    spent(
        &storage,
        "app.example",
        Flow::Signed(&signed_txid),
        700,
        NOW,
    )
    .await
    .unwrap();
    spent(&storage, "app.example", Flow::Signed(&kept), 300, NOW)
        .await
        .unwrap();
    spent(
        &storage,
        "other.example",
        Flow::Deferred("b3RoZXI="),
        900,
        NOW,
    )
    .await
    .unwrap();
    // Another originator's abort of the name deletes nothing of this one's.
    assert_eq!(unspent(&storage, "other.example", "cmVm").await.unwrap(), 0);
    assert_eq!(
        unspent(&storage, "app.example:8443", "cmVm").await.unwrap(),
        1
    );
    assert_eq!(
        unspent(&storage, "app.example", &signed_txid)
            .await
            .unwrap(),
        1
    );
    assert_eq!(unspent(&storage, "app.example", "cmVm").await.unwrap(), 0);
    assert_eq!(
        month_tally(&storage, "app.example", NOW).await.unwrap(),
        300
    );
    assert_eq!(
        month_tally(&storage, "other.example", NOW).await.unwrap(),
        900
    );
}

#[tokio::test]
async fn an_open_spend_counts_at_once_and_is_settled_or_dropped_by_its_id() {
    let (_dir, storage) = wallet_db().await;
    let first = open_spend(&storage, "app.example", 2500, NOW)
        .await
        .unwrap();
    let second = open_spend(&storage, "app.example", 500, NOW).await.unwrap();
    assert_ne!(first, second);
    assert_eq!(
        month_tally(&storage, "app.example", NOW).await.unwrap(),
        3000,
        "an open row is counted before the toolbox answers"
    );
    let txid = "ab".repeat(32);
    settle_spend(&storage, first, Flow::Signed(&txid))
        .await
        .unwrap();
    assert_eq!(drop_spend(&storage, second).await.unwrap(), 1);
    assert_eq!(drop_spend(&storage, second).await.unwrap(), 0);
    assert_eq!(
        rows(&storage).await,
        vec![(
            "app.example".into(),
            "2026-11".into(),
            Some(txid),
            None,
            2500
        )]
    );
    let deferred = open_spend(&storage, "app.example", 100, NOW).await.unwrap();
    assert_eq!(rows(&storage).await[1].3.as_deref(), Some(OPEN));
    settle_spend(&storage, deferred, Flow::Deferred("cmVm"))
        .await
        .unwrap();
    let settled = rows(&storage).await.remove(1);
    assert_eq!((settled.2, settled.3), (None, Some("cmVm".into())));
}

/// A grant row's reading in the listing's JSON: originator, kind, state,
/// the month's tally and whether it has reached the ceiling.
type Read = (String, String, String, Option<i64>, Option<bool>);

fn read_as(o: &str, k: &str, s: &str, spent: Option<i64>, reached: Option<bool>) -> Read {
    (o.to_string(), k.to_string(), s.to_string(), spent, reached)
}

/// The listing at `now`, and each grant row of its JSON as [`Read`].
async fn read_at(storage: &StorageSqlx, now: i64) -> (Shown, Vec<Read>) {
    let rows = listing(storage, true, now).await.unwrap();
    let shown = Shown::listed(storage, rows, now).await.unwrap();
    let json = serde_json::to_value(shown.json()).unwrap();
    let text = |g: &serde_json::Value, k: &str| g[k].as_str().unwrap().to_string();
    let read = json["grants"]
        .as_array()
        .unwrap()
        .iter()
        .map(|g| {
            let (spent, reached) = (g["spentThisMonth"].as_i64(), g["ceilingReached"].as_bool());
            (
                text(g, "originator"),
                text(g, "kind"),
                text(g, "state"),
                spent,
                reached,
            )
        })
        .collect();
    (shown, read)
}

/// A basket row expiring 2 s after `NOW`, a spending row of 1,500 with
/// 1,000 spent, and a revoked basket row of another originator.
async fn three_rows(storage: &StorageSqlx) {
    let basket = Power::basket("todo items").unwrap();
    let granting = |originator, power, amount, expiry| Granting {
        originator,
        power,
        amount,
        expiry,
    };
    let rows = [
        granting("app.example", &basket, None, NOW + 2),
        granting("app.example", &Power::Spending, Some(1_500), 0),
        granting("old.example", &basket, None, 0),
    ];
    for g in &rows {
        grant(storage, g, NOW).await.unwrap();
    }
    let every = Reach::named(None, None).unwrap();
    revoke(storage, "old.example", &every, NOW).await.unwrap();
    spent(
        storage,
        "app.example",
        Flow::Signed(&"ab".repeat(32)),
        1_000,
        NOW,
    )
    .await
    .unwrap();
}

#[tokio::test]
async fn every_grant_row_reads_its_state_ceiling_and_tally_once() {
    let (_dir, storage) = wallet_db().await;
    three_rows(&storage).await;
    let (shown, live) = read_at(&storage, NOW + 2).await;
    let revoked = read_as("old.example", "basket", "revoked", None, None);
    assert_eq!(
        live,
        vec![
            read_as("app.example", "basket", "granted", None, None),
            read_as(
                "app.example",
                "spending",
                "granted",
                Some(1_000),
                Some(false)
            ),
            revoked.clone(),
        ]
    );
    assert_eq!(shown.grants[1].month.as_deref(), Some("2026-11"));
    spent(
        &storage,
        "app.example",
        Flow::Signed(&"cd".repeat(32)),
        500,
        NOW,
    )
    .await
    .unwrap();
    let (_, later) = read_at(&storage, NOW + 3).await;
    assert_eq!(
        later,
        vec![
            read_as("app.example", "basket", "expired", None, None),
            read_as(
                "app.example",
                "spending",
                "granted",
                Some(1_500),
                Some(true)
            ),
            revoked,
        ]
    );
}
