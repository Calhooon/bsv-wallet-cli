//! The brief reconnect's window over the rows (#534, the design's L8 exit
//! tests, section (2)'s "The brief reconnect's window"), with `now` given:
//! the window's two edges, a rejoin from another port of the host, another
//! origin's own row, the window's end against a stale mark, the owner's
//! answer refused while a row is abandoned, and the readers hiding a row
//! past its window when no timer ran. On the fixture of `super::tests`.

use super::tests::{
    ask, basket, count, listing_call, refusal, reply, say, spend_call, wallet_db, NOW,
};
use super::*;

/// The disconnect's time in these tests.
const A: i64 = NOW + 60;

/// `originator`'s listing call opened at `at`.
async fn opened_at(storage: &StorageSqlx, originator: &str, at: i64) -> Pending {
    open_or_join(storage, &listing_call(originator), at)
        .await
        .unwrap()
}

/// The row `id` marked by a disconnect at `at`.
async fn marked_at(storage: &StorageSqlx, originator: &str, id: i64, at: i64) {
    let done = abandon(storage, originator, &[id], at).await.unwrap();
    assert_eq!(
        done.marked.iter().map(|r| r.pending_id).collect::<Vec<_>>(),
        [id]
    );
}

/// back.example's listing row opened at [`NOW`] and left at [`A`].
async fn left_at(storage: &StorageSqlx) -> PendingRow {
    let row = opened_at(storage, "back.example", NOW).await.row;
    marked_at(storage, "back.example", row.pending_id, A).await;
    row
}

/// The row `id` as stored, whatever its window.
async fn stored(storage: &StorageSqlx, id: i64) -> Option<PendingRow> {
    pending(storage, id, i64::MIN / 2).await.unwrap()
}

#[tokio::test]
async fn a_call_one_second_inside_the_window_resumes_the_same_row_and_clears_its_mark() {
    let (_dir, storage) = wallet_db().await;
    let first = left_at(&storage).await;

    let back = opened_at(&storage, "back.example", A + WINDOW - 1).await;

    assert_eq!(back.row.pending_id, first.pending_id, "the same pending_id");
    assert!(back.joined && back.resumed);
    assert_eq!(back.row.abandoned_at, None, "the mark cleared");
    assert_eq!(back.row.first_seen, NOW, "the same prompt");
    assert_eq!(count(&storage, "origin_pending").await, 1);
}

#[tokio::test]
async fn a_call_at_the_windows_end_finds_the_row_gone_and_writes_a_new_one() {
    let (_dir, storage) = wallet_db().await;
    let first = left_at(&storage).await;

    let late = opened_at(&storage, "back.example", A + WINDOW).await;

    assert_ne!(late.row.pending_id, first.pending_id, "a new id");
    assert!(!late.joined && !late.resumed);
    assert_eq!(late.row.first_seen, A + WINDOW);
    assert_eq!(stored(&storage, first.pending_id).await, None, "deleted");
    assert_eq!(count(&storage, "origin_pending").await, 1);
}

#[tokio::test]
async fn a_call_from_another_port_of_the_host_rejoins_the_row() {
    let (_dir, storage) = wallet_db().await;
    let first = left_at(&storage).await;

    let back = opened_at(&storage, "back.example:8080", A + 4).await;

    assert_eq!(back.row.pending_id, first.pending_id);
    assert!(back.resumed);
    assert_eq!(
        back.row.raw_originator, "back.example",
        "the row's name stays"
    );
}

#[tokio::test]
async fn another_origin_asking_the_same_power_gets_its_own_row_and_the_first_stays_abandoned() {
    let (_dir, storage) = wallet_db().await;
    let first = left_at(&storage).await;

    let other = opened_at(&storage, "other.example", A + 2).await;

    assert_ne!(other.row.pending_id, first.pending_id);
    assert!(!other.joined && !other.resumed);
    let kept = stored(&storage, first.pending_id).await.unwrap();
    assert_eq!(
        kept.abandoned_at,
        Some(A),
        "still abandoned, its mark unchanged"
    );
}

#[tokio::test]
async fn the_windows_end_deletes_only_a_row_still_abandoned_with_its_mark() {
    let (_dir, storage) = wallet_db().await;
    let id = left_at(&storage).await.pending_id;
    // Rejoined: the mark cleared, so its timer deletes nothing.
    opened_at(&storage, "back.example", A + 1).await;
    assert!(expire(&storage, "back.example", &[id], A)
        .await
        .unwrap()
        .is_empty());
    // Marked again later: the earlier timer still deletes nothing.
    marked_at(&storage, "back.example", id, A + 3).await;
    assert!(expire(&storage, "back.example", &[id], A)
        .await
        .unwrap()
        .is_empty());
    assert!(stored(&storage, id).await.is_some());
    // Another originator's name never reaches it.
    assert!(expire(&storage, "evil.example", &[id], A + 3)
        .await
        .unwrap()
        .is_empty());

    let gone = expire(&storage, "back.example", &[id], A + 3)
        .await
        .unwrap();

    assert_eq!(gone.iter().map(|r| r.pending_id).collect::<Vec<_>>(), [id]);
    assert_eq!(stored(&storage, id).await, None);
    assert_eq!(count(&storage, "origin_grants").await, 0, "no grant row");
}

#[tokio::test]
async fn a_spending_row_is_deleted_at_the_disconnect_and_never_marked() {
    let (_dir, storage) = wallet_db().await;
    let spend = ask(&storage, &spend_call("back.example", 2500)).await;

    let done = abandon(&storage, "back.example", &[spend.pending_id], A)
        .await
        .unwrap();

    assert_eq!(done.deleted.len(), 1);
    assert!(done.marked.is_empty());
    // The table refuses a mark on a spending row or an answered one.
    for values in [
        "('a', 'a', 'spending', '[]', '/createAction', 5, 0, NULL, NULL, 9)",
        "('a', 'a', 'basket', '[\"x\"]', '/x', NULL, 0, 3, 'deny', 9)",
    ] {
        let sql = format!(
            "INSERT INTO origin_pending (originator, raw_originator, kind, key, call, amount, \
             first_seen, answered_at, answer, abandoned_at) VALUES {values}"
        );
        assert!(
            sqlx::query(&sql).execute(storage.writer()).await.is_err(),
            "{values}"
        );
    }
}

#[tokio::test]
async fn an_answered_row_is_never_marked() {
    let (_dir, storage) = wallet_db().await;
    let row = opened_at(&storage, "back.example", NOW).await.row;
    say(&storage, row.pending_id, Answer::Deny, None)
        .await
        .unwrap();

    let done = abandon(&storage, "back.example", &[row.pending_id], A)
        .await
        .unwrap();

    assert_eq!(done, Abandoned::default());
    assert_eq!(
        stored(&storage, row.pending_id).await.unwrap().abandoned_at,
        None
    );
}

#[tokio::test]
async fn the_owners_answer_is_refused_while_abandoned_and_answers_after_the_rejoin() {
    let (_dir, storage) = wallet_db().await;
    let id = left_at(&storage).await.pending_id;

    for said in [Answer::Grant, Answer::Deny] {
        let e = answer(&storage, id, &reply(said, None), A + 1)
            .await
            .unwrap_err();
        assert_eq!(
            refusal(&e),
            Refused::Abandoned {
                pending_id: id,
                withdrawn_at: A + WINDOW
            }
        );
        assert_eq!(
            e.to_string(),
            format!(
                "pending request #{id} is waiting for its page to reconnect; answer it when the \
                 page asks again, or it is withdrawn at {}",
                when_exact(A + WINDOW)
            )
        );
    }
    assert_eq!(when_exact(A + WINDOW), "2026-11-02T00:01:10Z");
    assert_eq!(count(&storage, "origin_grants").await, 0, "no grant row");
    let kept = stored(&storage, id).await.unwrap();
    assert_eq!((kept.answered_at, kept.abandoned_at), (None, Some(A)));

    opened_at(&storage, "back.example", A + 2).await;
    let answered = answer(&storage, id, &reply(Answer::Grant, None), A + 3)
        .await
        .unwrap();

    assert_eq!(answered.pending.answer, Some(Answer::Grant));
    assert!(answered.grant.is_some());
    assert_eq!(count(&storage, "origin_grants").await, 1);
}

#[tokio::test]
async fn the_readers_hide_a_row_past_its_window_when_no_timer_ran() {
    let (_dir, storage) = wallet_db().await;
    let row = opened_at(&storage, "back.example", NOW).await.row;
    let id = row.pending_id;
    let open = opened_at(&storage, "open.example", NOW).await.row;
    // The disconnect's mark, and a timer that never runs (the process
    // killed between the mark and the window's end).
    marked_at(&storage, "back.example", id, A).await;

    let ids = |l: &Listing| l.pending.iter().map(|r| r.pending_id).collect::<Vec<_>>();
    let inside = A + WINDOW - 1;
    assert_eq!(
        ids(&listing(&storage, true, inside).await.unwrap()),
        [id, open.pending_id]
    );
    assert_eq!(
        ids(&owners_listing(&storage, 50, inside).await.unwrap()),
        [id, open.pending_id]
    );
    let shown = pending(&storage, id, inside).await.unwrap().unwrap();
    assert_eq!(shown.abandoned_at, Some(A));

    let past = A + WINDOW;
    for all in [false, true] {
        assert_eq!(
            ids(&listing(&storage, all, past).await.unwrap()),
            [open.pending_id]
        );
    }
    assert_eq!(
        ids(&owners_listing(&storage, 50, past).await.unwrap()),
        [open.pending_id]
    );
    assert_eq!(pending(&storage, id, past).await.unwrap(), None);
    let e = answer(&storage, id, &reply(Answer::Grant, None), past)
        .await
        .unwrap_err();
    assert_eq!(refusal(&e), Refused::NoSuchPending(id));
    assert_eq!(
        e.to_string(),
        format!("no pending request #{id} (a request is withdrawn when its caller disconnects)")
    );
    assert_eq!(count(&storage, "origin_grants").await, 0);
    // The restart's sweep deletes it, as an open row.
    assert_eq!(sweep_open(&storage).await.unwrap(), 2);
    assert_eq!(stored(&storage, id).await, None);
}

#[tokio::test]
async fn a_row_of_another_power_is_its_own_prompt_inside_the_window() {
    let (_dir, storage) = wallet_db().await;
    let row = left_at(&storage).await;
    let power = basket("another basket");

    let other = open_or_join(
        &storage,
        &super::tests::call("back.example", &power, Some("listing"), "/listOutputs"),
        A + 1,
    )
    .await
    .unwrap();

    assert_ne!(other.row.pending_id, row.pending_id);
    assert!(!other.resumed);
    assert_eq!(
        stored(&storage, row.pending_id).await.unwrap().abandoned_at,
        Some(A)
    );
}
