//! The server's held calls (#414 L5b, #534): the open rows they wait on,
//! the places under the cap (W14, a held call or an abandoned row its last
//! call passed its place to), the guard of a held call ([`Waiting`]) and the
//! disconnect's task over the brief reconnect's window ([`disconnected`]),
//! started from the gate's one spawn door ([`detach`]).

use std::collections::HashMap;
use std::future::Future;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use tokio::sync::{watch, Notify};
use tracing::Instrument;

use super::opening::Planted;
use super::{ABANDON_WINDOW, IN_ALL, PER_ORIGINATOR};
use crate::server::handlers::WalletState;
use crate::server::owner_door;
use crate::store::origin_grants::{self as grants, PendingRow};

/// The server's held calls: each open row a held call waits on (its
/// originator's key, its [`Notify`], how many calls wait on it), the places
/// under the cap per originator and in all (a held call, or an abandoned
/// row its last call passed its place to, #534), the brief reconnect's
/// window and the shutdown.
#[derive(Clone)]
pub struct Held(Arc<Inner>);

struct Inner {
    map: Mutex<Map>,
    shut: watch::Sender<bool>,
    window: Duration,
    planted: Planted,
}

#[derive(Default)]
struct Map {
    rows: HashMap<i64, Row>,
    calls: HashMap<String, usize>,
    all: usize,
    /// The abandoned rows holding a place, by id, with their originator's
    /// key: the place their last call passed them when it left by
    /// disconnect (#534).
    kept: HashMap<i64, String>,
}

impl Map {
    /// The open rows held calls wait on, of `originator`'s key.
    fn of(&self, originator: &str) -> Vec<i64> {
        self.rows
            .iter()
            .filter(|(_, row)| row.originator == originator)
            .map(|(id, _)| *id)
            .collect()
    }

    /// One place of `originator` freed.
    fn free(&mut self, originator: &str) {
        if let Some(n) = self.calls.get_mut(originator) {
            *n -= 1;
            if *n == 0 {
                self.calls.remove(originator);
            }
        }
        self.all -= 1;
    }
}

struct Row {
    originator: String,
    notify: Arc<Notify>,
    waiters: usize,
}

impl Default for Held {
    fn default() -> Self {
        Self::windowed(ABANDON_WINDOW)
    }
}

impl Held {
    fn windowed(window: Duration) -> Self {
        Self(Arc::new(Inner {
            map: Mutex::default(),
            shut: watch::channel(false).0,
            window,
            planted: Planted::default(),
        }))
    }

    /// The held calls with the window's timer shortened to `window`, for
    /// the socket tests (#534): the rows' own reading of the window is
    /// [`ABANDON_WINDOW`]'s, which a shortened timer always ends first.
    #[cfg(test)]
    pub(crate) fn with_window(window: Duration) -> Self {
        Self::windowed(window)
    }

    fn map(&self) -> std::sync::MutexGuard<'_, Map> {
        self.0.map.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// The test build's planted pause in the open (#552); none served.
    pub(super) fn planted(&self) -> &Planted {
        &self.0.planted
    }

    /// The server's shutdown: every held call answers 503
    /// `WALLET_BRIDGE_UNAVAILABLE` and deletes its row, and a call that
    /// would be held is answered so at once.
    pub fn shut_down(&self) {
        self.0.shut.send_replace(true);
    }

    /// The shutdown's watch, for a held call's wait.
    pub(super) fn subscribe(&self) -> watch::Receiver<bool> {
        self.0.shut.subscribe()
    }

    pub(super) fn is_shut(&self) -> bool {
        *self.0.shut.borrow()
    }

    /// Wake every call held on `pending_id` (an answer from this server's
    /// owner's routes, a deletion): it reads its row at once.
    pub fn wake(&self, pending_id: i64) {
        if let Some(row) = self.map().rows.get(&pending_id) {
            row.notify.notify_waiters();
        }
    }

    /// The places held now under the cap, of `originator`'s key and in
    /// all: the held calls and the abandoned rows holding a place.
    #[must_use]
    pub fn counts(&self, originator: &str) -> (usize, usize) {
        let map = self.map();
        (map.calls.get(originator).copied().unwrap_or(0), map.all)
    }

    /// The open rows held calls wait on now, of `originator`'s key.
    #[must_use]
    pub fn rows_of(&self, originator: &str) -> Vec<i64> {
        let mut ids = self.map().of(originator);
        ids.sort_unstable();
        ids
    }

    /// The abandoned rows holding a place now (#534), by id.
    #[must_use]
    pub fn kept(&self) -> Vec<i64> {
        let mut ids: Vec<i64> = self.map().kept.keys().copied().collect();
        ids.sort_unstable();
        ids
    }

    /// A held call's place under the cap, or the limit it would pass.
    pub(super) fn place(&self, originator: &str) -> Result<Place, &'static str> {
        let mut map = self.map();
        if map.calls.get(originator).copied().unwrap_or(0) >= PER_ORIGINATOR {
            return Err("2 held calls per originator");
        }
        if map.all >= IN_ALL {
            return Err("4 held calls in all");
        }
        *map.calls.entry(originator.to_string()).or_default() += 1;
        map.all += 1;
        Ok(Place {
            held: self.clone(),
            originator: originator.to_string(),
            passed: false,
        })
    }

    /// The place the abandoned row `pending_id` holds, taken over by the
    /// call that rejoins it (#534): no new place is asked.
    pub(super) fn adopt(&self, pending_id: i64) -> Option<Place> {
        let originator = self.map().kept.remove(&pending_id)?;
        Some(Place {
            held: self.clone(),
            originator,
            passed: false,
        })
    }

    /// The place the row `pending_id` holds freed, if it holds one: the
    /// row deleted, or a call holding a place of its own held on it.
    pub(super) fn release(&self, pending_id: i64) {
        let mut map = self.map();
        if let Some(originator) = map.kept.remove(&pending_id) {
            map.free(&originator);
        }
    }

    /// Rows deleted: each one's place freed and its calls woken.
    fn ended(&self, rows: &[PendingRow]) {
        for row in rows {
            self.release(row.pending_id);
            self.wake(row.pending_id);
        }
    }

    /// A call waits on `pending_id`: its row's [`Notify`].
    fn wait_on(&self, pending_id: i64, originator: &str) -> Arc<Notify> {
        let mut map = self.map();
        // #534: a place the row still holds is this call's now, freed
        // under the same lock as the call's entry.
        if let Some(kept) = map.kept.remove(&pending_id) {
            map.free(&kept);
        }
        let row = map.rows.entry(pending_id).or_insert_with(|| Row {
            originator: originator.to_string(),
            notify: Arc::new(Notify::new()),
            waiters: 0,
        });
        row.waiters += 1;
        row.notify.clone()
    }

    /// A call stops waiting on `pending_id`; with `snapshot`, the
    /// originator's open rows taken under the same lock first (the
    /// disconnect). When `keep` and the call was the row's last, its place
    /// passes to the row (#534), and the answer is `true`.
    fn leave(&self, pending_id: i64, snapshot: Option<&str>, keep: bool) -> (Vec<i64>, bool) {
        let mut map = self.map();
        let ids = snapshot.map_or_else(Vec::new, |originator| map.of(originator));
        let mut passed = false;
        if let Some(row) = map.rows.get_mut(&pending_id) {
            row.waiters -= 1;
            if row.waiters == 0 {
                let row = map.rows.remove(&pending_id);
                if keep && !map.kept.contains_key(&pending_id) {
                    if let Some(row) = row {
                        map.kept.insert(pending_id, row.originator);
                        passed = true;
                    }
                }
            }
        }
        (ids, passed)
    }
}

/// A held call's place under the cap, freed when the call ends, unless it
/// passed to its abandoned row.
pub(super) struct Place {
    held: Held,
    originator: String,
    passed: bool,
}

impl Drop for Place {
    fn drop(&mut self) {
        if !self.passed {
            self.held.map().free(&self.originator);
        }
    }
}

/// A held call's guard (the design's `Abandon`): when the call ends with
/// an answer it is disarmed and only stops waiting; when its future is
/// dropped instead (the caller closed its connection), it takes the
/// snapshot of the originator's open rows under [`Held`]'s mutex before it
/// returns, passes its place to its row when it was the row's last call
/// (a spending row excepted), and runs the disconnect in a task
/// ([`disconnected`]).
pub(super) struct Waiting {
    pub(super) held: Held,
    pub(super) wallet: WalletState,
    pub(super) originator: String,
    pub(super) pending_id: i64,
    notify: Arc<Notify>,
    /// The row is not a spending row: it waits out a window (#534).
    windowed: bool,
    armed: bool,
    place: Place,
}

impl Waiting {
    /// The armed guard of a call on `pending_id`, the row entered in
    /// [`Held`]'s map as it is made (#552): from here a drop finds its id
    /// there.
    pub(super) fn enter(
        held: Held,
        wallet: WalletState,
        originator: String,
        pending_id: i64,
        windowed: bool,
        place: Place,
    ) -> Self {
        let notify = held.wait_on(pending_id, &originator);
        Self {
            held,
            wallet,
            originator,
            pending_id,
            notify,
            windowed,
            armed: true,
            place,
        }
    }

    /// The row's [`Notify`], taken at the entry.
    pub(super) fn notify(&self) -> Arc<Notify> {
        self.notify.clone()
    }

    pub(super) fn answered(mut self) {
        self.armed = false;
    }
}

impl Drop for Waiting {
    fn drop(&mut self) {
        let snapshot = self.armed.then_some(self.originator.as_str());
        let keep = self.armed && self.windowed;
        let (ids, passed) = self.held.leave(self.pending_id, snapshot, keep);
        self.place.passed = passed;
        if !self.armed {
            return;
        }
        let (held, wallet, originator, own) = (
            self.held.clone(),
            self.wallet.clone(),
            self.originator.clone(),
            self.pending_id,
        );
        let Ok(runtime) = tokio::runtime::Handle::try_current() else {
            tracing::warn!(originator = %originator, ids = ?ids, "browser gate: a disconnect outside the runtime; the rows wait for the restart's sweep");
            held.release(own);
            return;
        };
        detach(&runtime, async move {
            disconnected(&held, &wallet, &originator, &ids, own).await;
        });
    }
}

/// Run `task` on `runtime` off the caller's future: the gate's one spawn
/// door (rule A6), for the disconnect's task (the mark and the window's
/// end, #534) and the open (#552).
pub(super) fn detach(
    runtime: &tokio::runtime::Handle,
    task: impl Future<Output = ()> + Send + 'static,
) {
    drop(runtime.spawn(task.in_current_span()));
}

/// The disconnect (W4 (a) and #534's window): the snapshot's spending rows
/// deleted and their calls woken (400 `Permission denied.`), its other open
/// rows marked; after the window the rows still abandoned with that mark
/// deleted, their calls woken and their places freed. `own` is the leaving
/// call's row: a place it passed to a row the disconnect did not mark (the
/// row answered or gone meanwhile) is freed at once.
async fn disconnected(held: &Held, wallet: &WalletState, originator: &str, ids: &[i64], own: i64) {
    let storage = wallet.storage();
    let marked = async {
        let now = owner_door::now()?;
        let done = grants::abandon(storage, originator, ids, now).await?;
        anyhow::Ok((done, now))
    };
    let (done, at) = match marked.await {
        Ok(done) => done,
        Err(e) => {
            tracing::warn!(originator = %originator, ids = ?ids, error = %e, "browser gate: the disconnect's rows were not marked; the restart's sweep deletes them");
            held.release(own);
            return;
        }
    };
    for row in &done.deleted {
        held.wake(row.pending_id);
    }
    let marked: Vec<i64> = done.marked.iter().map(|r| r.pending_id).collect();
    if !marked.contains(&own) {
        free_if_gone(held, wallet, own).await;
    }
    if marked.is_empty() {
        return;
    }
    tokio::time::sleep(held.0.window).await;
    match grants::expire(storage, originator, &marked, at).await {
        Ok(rows) => held.ended(&rows),
        Err(e) => {
            tracing::warn!(originator = %originator, ids = ?marked, error = %e, "browser gate: the window's end did not delete its rows; the readers hide them and the restart's sweep deletes them");
        }
    }
    for id in marked {
        free_if_gone(held, wallet, id).await;
    }
}

/// Free the place row `id` holds when the row is no longer open nor
/// abandoned inside its window (answered, or deleted by a reader past its
/// window).
async fn free_if_gone(held: &Held, wallet: &WalletState, id: i64) {
    let Ok(now) = owner_door::now() else {
        return;
    };
    if let Ok(row) = grants::pending(wallet.storage(), id, now).await {
        if row.is_none_or(|r| r.answered_at.is_some()) {
            held.release(id);
        }
    }
}

/// Delete the open rows `ids` of `originator`, abandoned or not, and wake
/// every call held on one: the shutdown's withdrawal.
pub(super) async fn withdraw(held: &Held, wallet: &WalletState, originator: &str, ids: &[i64]) {
    match grants::withdraw(wallet.storage(), originator, ids).await {
        Ok(rows) => held.ended(&rows),
        Err(e) => {
            tracing::warn!(originator = %originator, ids = ?ids, error = %e, "browser gate: the withdrawn rows were not deleted; the restart's sweep deletes them");
        }
    }
}
