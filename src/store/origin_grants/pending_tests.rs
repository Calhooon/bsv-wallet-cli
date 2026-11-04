//! The listings, the disconnect's deny (`abandon`), the restart's sweep
//! (`sweep_open`) and the prompt's text over the rows (#414, the design's
//! L5a exit test), on the fixture of `super::tests`.

use super::tests::{
    ask, basket, call, count, every_kind, granted, listing_call, protocol, say, spend_call,
    wallet_db, NOW,
};
use super::*;
use crate::test_support::Markers;

#[tokio::test]
async fn the_listing_holds_every_grant_by_kind_and_key_and_the_open_rows_unless_all() {
    let (_dir, storage) = wallet_db().await;
    granted(&storage, "b.example", &Power::Spending).await;
    granted(&storage, "b.example", &basket("zeta")).await;
    granted(&storage, "b.example", &basket("alpha")).await;
    granted(&storage, "a.example", &protocol("todo list")).await;
    revoke(&storage, "a.example", &Reach::Every, NOW)
        .await
        .unwrap();
    let open = ask(&storage, &listing_call("c.example:5173")).await;
    let shut = ask(&storage, &spend_call("b.example", 20)).await;
    say(&storage, shut.pending_id, Answer::Deny, None)
        .await
        .unwrap();

    let rows = listing(&storage, false, NOW).await.unwrap();
    let keys: Vec<_> = rows
        .grants
        .iter()
        .map(|g| {
            (
                g.originator.as_str(),
                g.kind,
                g.key.as_str(),
                g.revoked_at.is_some(),
            )
        })
        .collect();
    assert_eq!(
        keys,
        [
            (
                "a.example",
                Kind::Protocol,
                r#"[false,2,"todo list","self"]"#,
                true
            ),
            ("b.example", Kind::Basket, r#"["alpha"]"#, false),
            ("b.example", Kind::Basket, r#"["zeta"]"#, false),
            ("b.example", Kind::Spending, "[]", false),
        ]
    );
    assert_eq!(rows.pending, std::slice::from_ref(&open));
    let every = listing(&storage, true, NOW).await.unwrap();
    let ids: Vec<_> = every.pending.iter().map(|p| p.pending_id).collect();
    assert_eq!(ids, [open.pending_id, shut.pending_id]);
}

#[tokio::test]
async fn the_owners_listing_holds_the_open_rows_and_the_last_answered() {
    let (_dir, storage) = wallet_db().await;
    granted(&storage, "a.example", &basket("todo items")).await;
    let mut shut = Vec::new();
    for (i, at) in [(1, NOW + 3), (2, NOW + 1), (3, NOW + 2)] {
        let row = ask(&storage, &spend_call("b.example", i)).await;
        let reply = Reply {
            answer: Answer::Deny,
            expiry: 0,
            amount: None,
        };
        answer(&storage, row.pending_id, &reply, at).await.unwrap();
        shut.push(row.pending_id);
    }
    let open = ask(&storage, &listing_call("c.example")).await;

    let rows = owners_listing(&storage, 2, NOW).await.unwrap();
    assert_eq!(rows.grants.len(), 1);
    let ids: Vec<_> = rows.pending.iter().map(|p| p.pending_id).collect();
    // The two answered last (the first and the third), and the open row.
    assert_eq!(ids, [shut[0], shut[2], open.pending_id]);
    let none = owners_listing(&storage, 0, NOW).await.unwrap();
    assert_eq!(none.pending, std::slice::from_ref(&open));
}

/// The disconnect (W4 (a), #534): among its snapshot, the open spending
/// rows deleted at once, every other open row marked at the disconnect's
/// time; an answered row, a row the snapshot does not name and another
/// originator's row kept; no grant written.
#[tokio::test]
async fn abandon_deletes_its_listed_spending_rows_marks_the_others_and_keeps_the_rest() {
    let (_dir, storage) = wallet_db().await;
    let (markers, _guard) = Markers::capture();
    let mut listed = Vec::new();
    for (power, usage, _) in every_kind() {
        listed.push(ask(&storage, &call("gone.example:5173", &power, usage, "/x")).await);
    }
    // Answered before the disconnect: kept.
    let answered = ask(&storage, &spend_call("gone.example", 7)).await;
    say(&storage, answered.pending_id, Answer::Deny, None)
        .await
        .unwrap();
    // Opened after the snapshot by a live call of the same page: kept.
    let later = ask(
        &storage,
        &call("gone.example", &basket("later"), None, "/x"),
    )
    .await;
    // Another originator's open row of the same id list: kept.
    let other = ask(&storage, &listing_call("other.example")).await;
    let mut ids: Vec<i64> = listed.iter().map(|r| r.pending_id).collect();
    ids.extend([answered.pending_id, other.pending_id]);

    let done = abandon(&storage, "gone.example:9999", &ids, NOW + 5)
        .await
        .unwrap();

    let ids_of = |rows: &[PendingRow]| rows.iter().map(|r| r.pending_id).collect::<Vec<_>>();
    let (spending, others): (Vec<PendingRow>, Vec<PendingRow>) =
        listed.into_iter().partition(|r| r.kind == Kind::Spending);
    assert_eq!(ids_of(&done.deleted), ids_of(&spending), "spending deleted");
    assert_eq!(
        ids_of(&done.marked),
        ids_of(&others),
        "every other kind marked"
    );
    assert!(done.marked.iter().all(|r| r.abandoned_at == Some(NOW + 5)));
    let left = listing(&storage, true, NOW + 5).await.unwrap().pending;
    let mut want = ids_of(&others);
    want.extend([answered.pending_id, later.pending_id, other.pending_id]);
    want.sort_unstable();
    assert_eq!(ids_of(&left), want);
    let marked: Vec<i64> = left
        .iter()
        .filter(|r| r.abandoned_at.is_some())
        .map(|r| r.pending_id)
        .collect();
    assert_eq!(marked, ids_of(&others), "the rest unmarked");
    assert_eq!(
        count(&storage, "origin_grants").await,
        0,
        "no grant written"
    );
    let lines = markers.named("origin_grants_abandoned");
    assert_eq!(lines.len(), 1);
    assert_eq!(lines[0].message.as_deref(), Some("browser gate: abandoned"));
    assert_eq!(lines[0].fields["originator"], "gone.example");
    assert!(lines[0].fields["rows"].contains("spending []"));
    let lines = markers.named("origin_grants_disconnected");
    assert_eq!(lines.len(), 1);
    assert_eq!(
        lines[0].message.as_deref(),
        Some("browser gate: disconnected")
    );
    assert!(lines[0].fields["rows"].contains(r#"basket ["todo items"]"#));
    assert_eq!(lines[0].fields["until"], (NOW + 5 + WINDOW).to_string());
    // A second disconnect keeps the earlier mark and marks nothing.
    let again = abandon(&storage, "gone.example", &ids, NOW + 7)
        .await
        .unwrap();
    assert_eq!(again, Abandoned::default());
    let kept = pending(&storage, others[0].pending_id, NOW + 7)
        .await
        .unwrap();
    assert_eq!(kept.unwrap().abandoned_at, Some(NOW + 5));
    assert_eq!(
        abandon(&storage, "gone.example", &[], NOW).await.unwrap(),
        Abandoned::default()
    );
}

#[tokio::test]
async fn sweep_open_deletes_every_open_row_and_keeps_the_answered() {
    let (_dir, storage) = wallet_db().await;
    let (markers, _guard) = Markers::capture();
    for (power, usage, _) in every_kind() {
        ask(&storage, &call("a.example", &power, usage, "/x")).await;
    }
    ask(&storage, &listing_call("b.example")).await;
    let answered = ask(&storage, &spend_call("c.example", 1)).await;
    say(&storage, answered.pending_id, Answer::Deny, None)
        .await
        .unwrap();

    assert_eq!(sweep_open(&storage).await.unwrap(), 6);

    let left = listing(&storage, true, NOW).await.unwrap().pending;
    assert_eq!(left.len(), 1);
    assert_eq!(left[0].pending_id, answered.pending_id);
    let lines = markers.named("origin_grants_swept");
    assert_eq!(
        lines[0].message.as_deref(),
        Some(
            "origin grants: 6 open requests from a previous run withdrawn; their callers are gone"
        )
    );
    assert_eq!(sweep_open(&storage).await.unwrap(), 0);
}

#[tokio::test]
async fn the_shown_rows_carry_the_prompts_words() {
    let (_dir, storage) = wallet_db().await;
    let listing_row = ask(&storage, &listing_call("new.example:5173")).await;
    let signing = protocol("todo list");
    let sign = ask(
        &storage,
        &call("new.example", &signing, Some("signing"), "/x"),
    )
    .await;
    granted(&storage, "app.example", &Power::Spending).await;
    let mut past = spend_call("app.example", 2500);
    past.renewal = true;
    let renewal = ask(&storage, &past).await;
    let plain = ask(&storage, &spend_call("new.example", 2500)).await;

    let rows = vec![listing_row, sign, renewal, plain];
    let shown = Shown::of(&storage, vec![], rows, NOW).await.unwrap();
    let words: Vec<&str> = shown.pending.iter().map(|(_, a)| a.as_str()).collect();
    assert_eq!(
        words,
        [
            r#"list the basket "todo items""#,
            r#"use protocol "todo list" (level 2) with counterparty self for signing"#,
            "spend 2500 satoshis past this month's ceiling of 60000 (0 spent)",
            "spend 2500 satoshis",
        ]
    );
    let json = serde_json::to_value(shown.json()).unwrap();
    assert_eq!(json["pending"][0]["usage"], "listing");
    assert_eq!(
        json["pending"][0]["asked"],
        r#"list the basket "todo items""#
    );
    assert_eq!(json["pending"][2]["reason"], "Buy a coffee");
}
