//! The brief reconnect's window over the served router (#534, the design's
//! L8 exit tests, section (2)): a page that closes its connection while its
//! call is held and asks again inside the window resumes the same row, from
//! another port of the host too; the owner's answer is refused 409 while the
//! row is abandoned; another origin asking the same power is held on its own
//! row throughout; a call still held on a marked row is denied at the
//! window's end and not before; a dead timer leaves the readers hiding the
//! row past the store's window; the cap counts an abandoned row's place, the
//! joining call takes it over and the window's end frees it. Each over a
//! real socket (`axum::serve` on a bound `TcpListener`), the timer's window
//! shortened.

use std::time::{Duration, Instant};

use super::{
    at, basket, held_listing, listed, open_rows, page_call, page_leaving, raw_listing, read,
    refusal, row_of, until_marked, until_open, window_served, Read, OTHER, PAGE,
};
use crate::server::*;
use crate::store::origin_grants::{self as grants, when_exact};
use tokio::io::AsyncWriteExt;

/// The owner's answer from the owner's route: the status and the body.
async fn owner_answer(
    addr: std::net::SocketAddr,
    id: i64,
    answer: &str,
) -> (u16, serde_json::Value) {
    let body = json!({"pendingId": id, "answer": answer});
    let request = reqwest::Client::new()
        .post(at(addr, "/origins/answer"))
        .json(&body);
    let read = read(request.send().await.unwrap()).await;
    (read.0, read.3)
}

/// The grant rows of `originator`'s key now.
async fn grants_of(wallet: &WalletState, originator: &str) -> usize {
    let listing = listed(wallet.storage(), true).await;
    listing
        .grants
        .iter()
        .filter(|g| g.originator == originator)
        .count()
}

/// The end of a scene: no grant row for `originator`, and page.example
/// holds no place.
async fn settled(wallet: &WalletState, held: &powers::Held, originator: &str) {
    assert_eq!(grants_of(wallet, originator).await, 0, "no grant row");
    assert_eq!(held.counts("page.example"), (0, 0));
}

/// A held call's stream closed by the client, once its row is open: the
/// row's id.
async fn left(wallet: &WalletState, addr: std::net::SocketAddr, origin: &str, name: &str) -> i64 {
    let key = origin.trim_start_matches("http://");
    let before = open_rows(wallet).await.len();
    let mut stream = raw_listing(addr, origin, name).await;
    let id = row_of(&until_open(wallet, before + 1).await, key);
    stream.shutdown().await.unwrap();
    drop(stream);
    id
}

/// Wait until `held` counts `places` of `originator` and in all.
async fn until_counted(held: &powers::Held, originator: &str, places: (usize, usize)) {
    for _ in 0..500 {
        if held.counts(originator) == places {
            return;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!(
        "{originator}: {places:?} never counted, {:?}",
        held.counts(originator)
    );
}

/// The owner's answer to the abandoned row `id`, refused 409 `CONFLICT` in
/// the store's words naming the window's end; no grant row written.
async fn refused_while_abandoned(
    wallet: &WalletState,
    addr: std::net::SocketAddr,
    id: i64,
    abandoned_at: i64,
) {
    let (status, body) = owner_answer(addr, id, "grant").await;
    let withdrawn = when_exact(abandoned_at + powers::ABANDON_WINDOW_SECS);
    let words = format!(
        "pending request #{id} is waiting for its page to reconnect; answer it when the page \
         asks again, or it is withdrawn at {withdrawn}"
    );
    assert_eq!(
        (status, body["code"].as_str(), body["message"].as_str()),
        (409, Some("CONFLICT"), Some(words.as_str()))
    );
    assert_eq!(grants_of(wallet, "page.example").await, 0, "no grant row");
}

/// The owner grants row `id` and the rejoined call `back` answers once:
/// the door's 200 with its echo, one grant row, the row answered `grant`.
async fn granted_once(
    wallet: &WalletState,
    addr: std::net::SocketAddr,
    id: i64,
    back: tokio::task::JoinHandle<Read>,
) {
    let (status, _) = owner_answer(addr, id, "grant").await;
    assert_eq!(status, 200);
    let back = back.await.unwrap();
    assert_eq!(back.0, 200);
    assert_eq!(back.1.as_deref(), Some("http://page.example:8080"));
    assert!(back.3["outputs"].is_array());
    assert_eq!(grants_of(wallet, "page.example").await, 1);
    let now = owner_door::now().unwrap();
    let answered = grants::pending(wallet.storage(), id, now).await.unwrap();
    assert_eq!(answered.unwrap().answer, Some(grants::Answer::Grant));
}

/// The design's L8 exit test: a held call's stream closed, the owner's
/// answer refused while the row waits, then a call of the same originator
/// from another port of the host inside the window held on the same row and
/// answered once by the owner's grant (the door's 200 with the echo, one
/// grant row, the row answered `grant`); another origin asking the same
/// power held on a row of its own throughout.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_page_that_reconnects_from_another_port_inside_the_window_resumes_its_prompt() {
    let (_dir, wallet, addr, held) = window_served(Duration::from_secs(5)).await;
    let id = left(&wallet, addr, PAGE, "browser gate").await;
    let [row] = until_marked(&wallet, 1).await.try_into().unwrap();
    assert_eq!(row.pending_id, id);
    let abandoned_at = row.abandoned_at.unwrap();
    assert_eq!(held.kept(), [id], "the row holds its call's place");
    assert_eq!(held.counts("page.example"), (1, 1));

    // The owner's answer is refused while the row is abandoned.
    refused_while_abandoned(&wallet, addr, id, abandoned_at).await;

    // Another origin asking the same power: a row of its own.
    let other = held_listing(addr, OTHER, "browser gate");
    let other_id = row_of(&until_open(&wallet, 2).await, "other.example");
    assert_ne!(other_id, id);
    assert_eq!(
        until_marked(&wallet, 1).await[0].pending_id,
        id,
        "still abandoned"
    );

    // The page asks again from another port of its host.
    let back = held_listing(addr, "http://page.example:8080", "browser gate");
    until_marked(&wallet, 0).await;
    let rows = open_rows(&wallet).await;
    assert_eq!(rows.len(), 2, "no new row: {rows:?}");
    assert_eq!(row_of(&rows, "page.example"), id, "the same pending_id");
    until_counted(&held, "page.example", (1, 2)).await;
    assert!(
        held.kept().is_empty(),
        "the joining call took the row's place"
    );

    granted_once(&wallet, addr, id, back).await;
    assert!(!other.is_finished(), "other.example's call is still held");

    let (status, _) = owner_answer(addr, other_id, "deny").await;
    assert_eq!(status, 200);
    assert_eq!(other.await.unwrap().0, 400);
    settled(&wallet, &held, "other.example").await;
}

/// The same with the call after the window: the row deleted at its end,
/// the call a new prompt with a new `pending_id`.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_call_after_the_window_opens_a_new_prompt() {
    let window = Duration::from_millis(800);
    let (_dir, wallet, addr, held) = window_served(window).await;
    let id = left(&wallet, addr, PAGE, "browser gate").await;
    until_marked(&wallet, 1).await;
    for _ in 0..500 {
        if open_rows(&wallet).await.is_empty() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert!(
        open_rows(&wallet).await.is_empty(),
        "deleted at the window's end"
    );
    assert!(held.kept().is_empty(), "its place freed");
    assert_eq!(held.counts("page.example"), (0, 0));

    let again = held_listing(addr, PAGE, "browser gate");
    let again_id = row_of(&until_open(&wallet, 1).await, "page.example");

    assert_ne!(again_id, id, "a new pending_id");
    super::powers_tests::answer(&wallet, again_id, grants::Answer::Deny).await;
    assert_eq!(again.await.unwrap().0, 400);
}

/// A call joined on the row of a call that leaves is still held on the
/// marked row through the window and answered 400 `Permission denied.` at
/// its end, not before; the owner's answer meanwhile is refused.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_call_still_held_on_a_marked_row_is_denied_at_the_windows_end_not_before() {
    let window = Duration::from_millis(1500);
    let (_dir, wallet, addr, held) = window_served(window).await;
    let (mut leaving, id) = page_leaving(&wallet, addr, "browser gate").await;
    let staying = held_listing(addr, PAGE, "browser gate");
    until_counted(&held, "page.example", (2, 2)).await;

    let closed = Instant::now();
    leaving.shutdown().await.unwrap();
    drop(leaving);
    until_marked(&wallet, 1).await;
    // The joined call still holds the row: no place passed to it.
    until_counted(&held, "page.example", (1, 1)).await;
    assert!(held.kept().is_empty());
    assert_eq!(owner_answer(addr, id, "deny").await.0, 409);

    tokio::time::sleep(window / 2).await;
    assert!(!staying.is_finished(), "denied before the window's end");
    let staying = staying.await.unwrap();
    assert!(
        closed.elapsed() >= window,
        "answered {:?} after the close",
        closed.elapsed()
    );
    assert_eq!(
        refusal(&staying),
        (
            400,
            Some("ERR_PERMISSION_DENIED"),
            Some(powers::PERMISSION_DENIED)
        )
    );
    assert_eq!(staying.1.as_deref(), Some(PAGE));
    assert!(open_rows(&wallet).await.is_empty());
    settled(&wallet, &held, "page.example").await;
}

/// A dead timer (the window's task never ends): past the store's window
/// the readers hide the row, the call still held on it reads it gone and is
/// answered 400, and the page's next call opens a new prompt.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_readers_hide_a_row_past_its_window_when_the_timer_never_runs() {
    let (_dir, wallet, addr, _held) = window_served(Duration::from_secs(3600)).await;
    let (mut leaving, id) = page_leaving(&wallet, addr, "browser gate").await;
    let staying = held_listing(addr, PAGE, "browser gate");
    tokio::time::sleep(Duration::from_millis(300)).await;
    leaving.shutdown().await.unwrap();
    drop(leaving);
    let [row] = until_marked(&wallet, 1).await.try_into().unwrap();
    let end = row.abandoned_at.unwrap() + powers::ABANDON_WINDOW_SECS;

    let staying = tokio::time::timeout(powers::ABANDON_WINDOW + Duration::from_secs(3), staying)
        .await
        .expect("the held call outlived the window")
        .unwrap();

    assert!(owner_door::now().unwrap() >= end);
    assert_eq!(staying.0, 400);
    assert!(open_rows(&wallet).await.is_empty(), "the listing hides it");
    let listed = get_owner(addr).await;
    assert_eq!(listed["pending"], json!([]), "GET /origins hides it");
    let again = held_listing(addr, PAGE, "browser gate");
    let again_id = row_of(&until_open(&wallet, 1).await, "page.example");
    assert_ne!(again_id, id);
    let stored: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM origin_pending WHERE pending_id = ?")
            .bind(id)
            .fetch_one(wallet.storage().pool())
            .await
            .unwrap();
    assert_eq!(stored, 0, "the next call deleted it");
    super::powers_tests::answer(&wallet, again_id, grants::Answer::Deny).await;
    assert_eq!(again.await.unwrap().0, 400);
}

/// `GET /origins` from the owner's side: the body.
async fn get_owner(addr: std::net::SocketAddr) -> serde_json::Value {
    reqwest::Client::new()
        .get(at(addr, "/origins"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap()
}

/// The cap (W14) with an abandoned row: a page with one abandoned row and
/// one held call is refused 429 on a third; the call that rejoins the row
/// takes its place, so the count stays 2; when it leaves again the place
/// passes back, and the window's end frees it with the held call's.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_cap_counts_an_abandoned_rows_place_and_the_joining_call_takes_it_over() {
    let window = Duration::from_secs(4);
    let (_dir, wallet, addr, held) = window_served(window).await;
    let id = left(&wallet, addr, PAGE, "one").await;
    until_marked(&wallet, 1).await;
    assert_eq!(held.kept(), [id]);
    let two = held_listing(addr, PAGE, "two");
    until_counted(&held, "page.example", (2, 2)).await;

    let third = page_call(addr, PAGE, "/listOutputs", basket("three")).await;
    assert_eq!(
        refusal(&third),
        (
            429,
            Some("ERR_PERMISSION_PENDING_LIMIT"),
            Some(powers::PENDING_LIMIT)
        )
    );
    assert_eq!(
        open_rows(&wallet).await.len(),
        2,
        "the refused call wrote no row"
    );

    // At its 2 places, the page rejoins its abandoned row.
    let mut back = raw_listing(addr, PAGE, "one").await;
    until_marked(&wallet, 0).await;
    until_counted(&held, "page.example", (2, 2)).await;
    assert!(
        held.kept().is_empty(),
        "the joining call took the row's place"
    );
    let rows = open_rows(&wallet).await;
    assert!(
        rows.iter().any(|r| r.pending_id == id),
        "the same row: {rows:?}"
    );

    // It leaves again: the place passes back to the row, and its snapshot
    // marks the page's other row too (W4 (a): every open prompt of the
    // originator); the window's end deletes both, denies the call held on
    // the second and frees every place.
    back.shutdown().await.unwrap();
    drop(back);
    until_marked(&wallet, 2).await;
    assert_eq!(held.kept(), [id]);
    assert_eq!(held.counts("page.example"), (2, 2));
    let two = two.await.unwrap();
    assert_eq!(two.0, 400, "denied at the window's end");
    until_counted(&held, "page.example", (0, 0)).await;
    assert!(held.kept().is_empty(), "freed at the window's end");
    let rows = open_rows(&wallet).await;
    assert!(rows.is_empty(), "the abandoned rows deleted: {rows:?}");
}

/// A kept place is tied to its row's window: under a dead timer (the
/// window's task never ends), once `now >= abandoned_at + W` the row is gone
/// to every reader and its place is too, so the page's next call holds one
/// place and a second is held beside it, never refused 429.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_kept_place_ends_with_its_rows_window_when_the_timer_never_runs() {
    let (_dir, wallet, addr, held) = window_served(Duration::from_secs(3600)).await;
    let id = left(&wallet, addr, PAGE, "one").await;
    let [row] = until_marked(&wallet, 1).await.try_into().unwrap();
    assert_eq!(held.kept(), [id]);
    let end = row.abandoned_at.unwrap() + powers::ABANDON_WINDOW_SECS;
    while owner_door::now().unwrap() < end {
        tokio::time::sleep(Duration::from_millis(100)).await;
    }

    let two = held_listing(addr, PAGE, "two");
    until_open(&wallet, 1).await;
    assert_eq!(
        held.counts("page.example"),
        (1, 1),
        "kept {:?}",
        held.kept()
    );
    let three = held_listing(addr, PAGE, "three");
    until_counted(&held, "page.example", (2, 2)).await;
    assert!(held.kept().is_empty(), "kept {:?}", held.kept());
    let rows = until_open(&wallet, 2).await;
    for row in rows {
        super::powers_tests::answer(&wallet, row.pending_id, grants::Answer::Deny).await;
    }
    assert_eq!(two.await.unwrap().0, 400);
    assert_eq!(three.await.unwrap().0, 400);
    settled(&wallet, &held, "page.example").await;
}

/// A place passed to a row marked before its last call left ends with the
/// row's earlier window, not the later disconnect's: under a dead timer,
/// page.example's call on "one" leaves and its snapshot marks "two" with
/// it while "two"'s call is still held; that call leaves 3 s later, and
/// once `now >= abandoned_at + W` of the first mark no place is counted.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_place_passed_to_a_row_marked_earlier_ends_with_that_rows_window() {
    let (_dir, wallet, addr, held) = window_served(Duration::from_secs(3600)).await;
    let mut two = raw_listing(addr, PAGE, "two").await;
    until_open(&wallet, 1).await;
    left(&wallet, addr, PAGE, "one").await;
    let marked = until_marked(&wallet, 2).await;
    let end = marked[0].abandoned_at.unwrap() + powers::ABANDON_WINDOW_SECS;
    assert!(marked
        .iter()
        .all(|r| r.abandoned_at == marked[0].abandoned_at));
    tokio::time::sleep(Duration::from_secs(3)).await;
    two.shutdown().await.unwrap();
    drop(two);
    until_counted(&held, "page.example", (2, 2)).await;
    while held.kept().len() < 2 {
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    tokio::time::sleep(Duration::from_millis(300)).await;

    while owner_door::now().unwrap() < end {
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert!(
        owner_door::now().unwrap() < end + 2,
        "measured inside the later disconnect's window"
    );
    assert_eq!(
        held.counts("page.example"),
        (0, 0),
        "kept {:?}",
        held.kept()
    );
    assert!(held.kept().is_empty());
}

/// M16, the race of a resume `abandoned`'s read missed: a second call of
/// the page on the same power reads no abandoned row (the first call is
/// still held on it) and takes a place of its own; a planted pause holds it
/// there while the first call leaves, its place passing to the row as it is
/// marked; then the second call's open resumes the row, and the row's kept
/// place is released as the call enters (one place counted, none kept),
/// never two places held for one prompt.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_resume_the_read_missed_releases_the_rows_kept_place_as_it_enters() {
    let (_dir, wallet, addr, held) = window_served(Duration::from_secs(3600)).await;
    let mut first = raw_listing(addr, PAGE, "one").await;
    let id = row_of(&until_open(&wallet, 1).await, "page.example");
    let pause = held.pause_after_read();
    let second = held_listing(addr, PAGE, "one");
    pause.reached.notified().await;
    assert_eq!(held.counts("page.example"), (2, 2), "two calls, two places");

    first.shutdown().await.unwrap();
    drop(first);
    until_marked(&wallet, 1).await;
    assert_eq!(
        held.kept(),
        [id],
        "the first call's place passed to the row"
    );
    assert_eq!(held.counts("page.example"), (2, 2));
    pause.release.notify_one();
    until_marked(&wallet, 0).await;
    until_counted(&held, "page.example", (1, 1)).await;
    assert!(held.kept().is_empty(), "kept {:?}", held.kept());
    assert_eq!(row_of(&open_rows(&wallet).await, "page.example"), id);

    super::powers_tests::answer(&wallet, id, grants::Answer::Deny).await;
    assert_eq!(second.await.unwrap().0, 400);
    settled(&wallet, &held, "page.example").await;
}
