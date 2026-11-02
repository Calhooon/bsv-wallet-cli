//! The fold of `ALLOWED_ORIGINS`, the month's tally and the stored words
//! (#414, the design's L1 exit test), on the fixture of `super::tests`.

use super::tests::{count, owner, refusal, wallet_db, NOV_FIRST, NOW, OCT_LAST};
use super::*;

#[tokio::test]
async fn the_fold_writes_env_rows_once_and_says_so() {
    let (_dir, storage) = wallet_db().await;
    let entries = vec!["app.example:8443".to_string(), "app.example".to_string()];

    let first = fold(&storage, &entries, NOW).await.unwrap();

    assert_eq!(
        first.lines[0].to_string(),
        "origin grants: ALLOWED_ORIGINS entry app.example:8443 is now the grant row app.example \
         (wallet, spending without a ceiling; source env)"
    );
    assert_eq!(first.lines[1].folded, Folded::AlreadyGranted);
    assert_eq!(
        first.lines[1].to_string(),
        "origin grants: ALLOWED_ORIGINS entry app.example names the grant row app.example: \
         already granted"
    );
    assert!(first.seeded && first.unlisted.is_empty());
    let Standing::Live(spending) = lookup(&storage, "app.example", Scope::Spending, NOW)
        .await
        .unwrap()
    else {
        panic!("no spending row");
    };
    assert_eq!(
        (spending.amount, spending.source, spending.expiry),
        (None, Source::Env, 0)
    );

    let rows = grant_rows_text(&storage).await;
    let second = fold(&storage, &entries, NOW + 60).await.unwrap();
    assert!(second
        .lines
        .iter()
        .all(|l| l.folded == Folded::AlreadyGranted));
    assert_eq!(grant_rows_text(&storage).await, rows, "a second fold wrote");
}

/// The grant rows, as text.
async fn grant_rows_text(storage: &StorageSqlx) -> Vec<String> {
    sqlx::query_scalar(
        "SELECT originator || '|' || scope || '|' || granted_at || '|' || source \
         FROM origin_grants ORDER BY grant_id",
    )
    .fetch_all(storage.pool())
    .await
    .unwrap()
}

#[tokio::test]
async fn a_revoked_row_is_never_folded_again() {
    let (_dir, storage) = wallet_db().await;
    let entries = vec!["app.example".to_string()];
    fold(&storage, &entries, NOW).await.unwrap();
    revoke(&storage, "app.example", None, NOW + 1)
        .await
        .unwrap();

    let refold = fold(&storage, &entries, NOW + 2).await.unwrap();

    assert_eq!(refold.lines[0].folded, Folded::Revoked(NOW + 1));
    assert_eq!(
        refold.lines[0].to_string(),
        "origin grants: ALLOWED_ORIGINS entry app.example names the grant row app.example: \
         revoked by the owner at 2026-11-02T00:00Z; the env entry does not grant it again"
    );
    for scope in [Scope::Wallet, Scope::Spending] {
        assert!(matches!(
            lookup(&storage, "app.example", scope, NOW + 2)
                .await
                .unwrap(),
            Standing::Revoked(_)
        ));
    }
}

#[tokio::test]
async fn the_fold_keeps_the_owners_rows_and_names_unlisted_env_rows() {
    let (_dir, storage) = wallet_db().await;
    grant(
        &storage,
        &owner("app.example", Scope::Spending, Some(7), 0),
        NOW,
    )
    .await
    .unwrap();
    grant(
        &storage,
        &owner("late.example", Scope::Wallet, None, NOW + 5),
        NOW,
    )
    .await
    .unwrap();
    fold(&storage, &["gone.example".to_string()], NOW)
        .await
        .unwrap();

    let entries = ["app.example", "late.example"].map(String::from);
    let folded = fold(&storage, &entries, NOW + 10).await.unwrap();

    assert_eq!(folded.lines[0].folded, Folded::Granted);
    let Standing::Live(spending) = lookup(&storage, "app.example", Scope::Spending, NOW)
        .await
        .unwrap()
    else {
        panic!("the owner's spending row went");
    };
    assert_eq!((spending.amount, spending.source), (Some(7), Source::Owner));
    assert_eq!(folded.lines[1].folded, Folded::Expired(NOW + 5));
    assert_eq!(folded.unlisted, vec!["gone.example".to_string()]);
    assert_eq!(
        unlisted_text("gone.example"),
        "origin grants: the grant row gone.example is source env, no ALLOWED_ORIGINS entry; \
         revoke it with bsv-wallet origin-revoke gone.example"
    );

    // An entry that is not an originator refuses the fold whole.
    let before = count(&storage, "origin_grants").await;
    let bad = ["new.example", "http://x.example"].map(String::from);
    let e = fold(&storage, &bad, NOW).await.unwrap_err();
    assert!(matches!(refusal(&e), Refused::NotAnOriginator { .. }));
    assert_eq!(count(&storage, "origin_grants").await, before);
}

#[tokio::test]
async fn an_empty_list_on_a_store_without_env_rows_seeds_nothing() {
    let (_dir, storage) = wallet_db().await;
    let folded = fold(&storage, &[], NOW).await.unwrap();
    assert_eq!(
        folded,
        Fold {
            lines: vec![],
            unlisted: vec![],
            seeded: false
        }
    );
    assert_eq!(count(&storage, "origin_grants").await, 0);
}

async fn spent(storage: &StorageSqlx, originator: &str, txid: Option<&str>, sats: i64, at: i64) {
    sqlx::query(
        "INSERT INTO origin_spends (originator, month, txid, reference, satoshis, at) \
         VALUES (?, ?, ?, ?, ?, ?)",
    )
    .bind(originator)
    .bind(month_of(at).unwrap())
    .bind(txid)
    .bind(txid.is_none().then_some("cmVm"))
    .bind(sats)
    .bind(at)
    .execute(storage.writer())
    .await
    .unwrap();
}

#[tokio::test]
async fn the_months_tally_turns_at_the_month_boundary() {
    assert_eq!(month_of(OCT_LAST).unwrap(), "2026-10");
    assert_eq!(month_of(NOV_FIRST).unwrap(), "2026-11");

    let (_dir, storage) = wallet_db().await;
    assert_eq!(month_tally(&storage, "app.example", NOW).await.unwrap(), 0);
    spent(&storage, "app.example", Some("aa"), 1000, OCT_LAST).await;
    spent(&storage, "app.example", None, 300, NOV_FIRST).await;
    spent(&storage, "app.example", Some("bb"), 20, NOW).await;
    spent(&storage, "other.example", Some("cc"), 7, NOW).await;

    assert_eq!(
        month_tally(&storage, "app.example", OCT_LAST)
            .await
            .unwrap(),
        1000
    );
    assert_eq!(
        month_tally(&storage, "app.example:8443", NOV_FIRST)
            .await
            .unwrap(),
        320
    );
    assert_eq!(
        month_tally(&storage, "other.example", NOV_FIRST - 1)
            .await
            .unwrap(),
        0
    );

    // A spend row names its txid (signed) or its reference (deferred).
    let neither = sqlx::query(
        "INSERT INTO origin_spends (originator, month, satoshis, at) \
         VALUES ('app.example', '2026-11', 1, 0)",
    )
    .execute(storage.writer())
    .await;
    assert!(neither.is_err());
}

#[test]
fn the_stored_words_read_back() {
    for scope in [Scope::Wallet, Scope::Spending] {
        assert_eq!(Scope::parse(scope.as_str()).unwrap(), scope);
    }
    for answer in [Answer::Grant, Answer::Once, Answer::Deny] {
        assert_eq!(Answer::parse(answer.as_str()).unwrap(), answer);
    }
    assert_eq!(Source::parse("env").unwrap(), Source::Env);
    assert!(Source::parse("page").is_err());
    assert_eq!(
        key_of(" App.Example:8443 ").unwrap(),
        ("app.example".to_string(), "app.example:8443".to_string())
    );
    assert_eq!(key_of("[::1]:3000").unwrap().0, "[::1]");
}
