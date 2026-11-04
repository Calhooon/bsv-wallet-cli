//! The switch (#414, the design's L5b, `docs/design/origin-grants.md`
//! sections (1) and (2), under the owner's rulings W1 (a), W4 (a) and W15
//! (i) of 2026-11-03): a door admits a page for each power it asks by the
//! owner's grant of that power to its origin, read from the store.
//!
//! Each handler names its powers after its arguments parse ([`Ask`], the
//! design's door-to-kind map) and calls [`Asker::ask`] before its toolbox
//! call and outside the `SpendingLock`; `createAction`'s spend is asked
//! last, inside the lock ([`Asker::spend`], the design's L6). The powers are
//! asked one at a time, in the reference's order (ts-stack `edf6e03`
//! `WalletPermissionsManager.ts` 4305-4321): for each, the originator's rows
//! are read
//! (`store::origin_grants::lookup`):
//!
//! - **live**: admitted, the next power is asked;
//! - **revoked**: 400 `ERR_PERMISSION_DENIED` `The user has denied the
//!   request for permission.` at once, no pending row (W3);
//! - **none, or expired**: the call opens or joins the power's pending row
//!   and is **held** until the owner answers it (`grant` asks the next
//!   power, `deny` is 400 `Permission denied.`), the row is deleted (a
//!   disconnect's deny, the same 400), or the server shuts down (503
//!   `WALLET_BRIDGE_UNAVAILABLE`, W13).
//!
//! A seed-listed originator (`ALLOWED_ORIGINS`, the design's question 6) is
//! admitted before any power is asked, as (b) admits it; a protocol power
//! at security level 0 is never asked (W15, BRC-100 line 65); an
//! admin-reserved name is refused at once with the reference's words.
//!
//! The spend (section (1), W6, W7): a call's amount is its outputs' sum, and
//! a live spending row admits it when the month's tally (`origin_spends`,
//! this originator, this UTC month) plus the amount is at most the row's
//! ceiling; past it the call is held as a renewal. The check runs inside the
//! `SpendingLock`, so two calls from one page cannot both pass a ceiling one
//! of them fills; a held spend releases the lock while it waits, takes it
//! again after the answer and checks again (`once` admits the one call, no
//! row written). The door tallies the admitted spend before it lets the lock
//! go (`spend::Tally`).
//!
//! A held call waits with no bound, reading its row every 250 ms (the CLI
//! and the inspector answer from other processes) and waking at once on
//! the server's own answers (a [`Notify`] per open row in [`Held`]). When
//! its caller closes the connection, the call's future is dropped and its
//! guard ([`Waiting`]) takes a snapshot of the originator's open rows under
//! [`Held`]'s mutex and deletes them (`origin_grants::abandon`), answering
//! every other call held on them 400 `Permission denied.` (W4 (a)). The
//! held calls are capped (W14): at most [`PER_ORIGINATOR`] per originator
//! and [`IN_ALL`] in all; past either, 429 `ERR_PERMISSION_PENDING_LIMIT`
//! at once.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use axum::extract::FromRequestParts;
use axum::http::request::Parts;
use bsv_sdk::wallet::{Counterparty, Protocol, SecurityLevel};
use tokio::sync::{watch, Notify};

use super::error::{AppError, Kind as ErrorKind};
use super::handlers::WalletState;
use super::open_bounds::{Class, OpenBounds};
use super::origin_gate::{Admitted, DENIED, NO_ORIGIN};
use super::probes::Probes;
use crate::store::origin_grants::{self as grants, Answer, Asking, Power, Refused, Standing};

mod spend;

/// At most this many held calls per originator (the key, the hostname).
pub const PER_ORIGINATOR: usize = 2;
/// At most this many held calls in all, so at least two of a browser's six
/// connections to the wallet stay free for granted pages (W14).
pub const IN_ALL: usize = 4;
/// How often a held call reads its row.
pub const READ_EVERY: Duration = Duration::from_millis(250);

/// The reference's per-request denial (`denyPermission`,
/// `WalletPermissionsManager.ts:1199`): the owner's deny, or the row gone.
pub const PERMISSION_DENIED: &str = "Permission denied.";
/// The cap's refusal.
pub const PENDING_LIMIT: &str =
    "Too many permission requests from this site are waiting for the owner's answer.";
/// The shutdown's answer to a held call, the MetaNet Client's text
/// (`electron/httpServer.ts:277-281` at bsv-desktop `b791cb2`).
pub const SHUTTING_DOWN: &str = "WALLET_BRIDGE_UNAVAILABLE: HTTP server shutting down";

/// The server's held calls: each open row a held call waits on (its
/// originator's key, its [`Notify`], how many calls wait on it), the held
/// calls per originator and in all, and the shutdown.
#[derive(Clone)]
pub struct Held(Arc<Inner>);

struct Inner {
    map: Mutex<Map>,
    shut: watch::Sender<bool>,
    /// The test's pause between the open's commit and the guard's entry
    /// (#552), taken by the next call that opens a row.
    #[cfg(test)]
    pause: Mutex<Option<Pause>>,
}

/// A planted pause (#552): the call signals `reached` once its row is
/// committed and waits on `release` before its guard is entered.
#[cfg(test)]
#[derive(Clone, Default)]
pub(crate) struct Pause {
    pub(crate) reached: Arc<Notify>,
    pub(crate) release: Arc<Notify>,
}

#[derive(Default)]
struct Map {
    rows: HashMap<i64, Row>,
    calls: HashMap<String, usize>,
    all: usize,
}

struct Row {
    originator: String,
    notify: Arc<Notify>,
    waiters: usize,
}

impl Default for Held {
    fn default() -> Self {
        Self(Arc::new(Inner {
            map: Mutex::default(),
            shut: watch::channel(false).0,
            #[cfg(test)]
            pause: Mutex::default(),
        }))
    }
}

impl Held {
    fn map(&self) -> std::sync::MutexGuard<'_, Map> {
        self.0.map.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// The server's shutdown: every held call answers 503
    /// `WALLET_BRIDGE_UNAVAILABLE` and deletes its row, and a call that
    /// would be held is answered so at once.
    pub fn shut_down(&self) {
        self.0.shut.send_replace(true);
    }

    fn is_shut(&self) -> bool {
        *self.0.shut.borrow()
    }

    /// Plant a pause for the next call that opens a row (#552).
    #[cfg(test)]
    pub(crate) fn pause_after_open(&self) -> Pause {
        let pause = Pause::default();
        *self.0.pause.lock().unwrap_or_else(PoisonError::into_inner) = Some(pause.clone());
        pause
    }

    /// The planted pause, if any: signalled, then waited out.
    #[cfg(test)]
    async fn paused(&self) {
        let pause = self
            .0
            .pause
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take();
        if let Some(pause) = pause {
            pause.reached.notify_one();
            pause.release.notified().await;
        }
    }

    /// Wake every call held on `pending_id` (an answer from this server's
    /// owner's routes, a deletion): it reads its row at once.
    pub fn wake(&self, pending_id: i64) {
        if let Some(row) = self.map().rows.get(&pending_id) {
            row.notify.notify_waiters();
        }
    }

    /// The calls held now, of `originator`'s key and in all.
    #[must_use]
    pub fn counts(&self, originator: &str) -> (usize, usize) {
        let map = self.map();
        (map.calls.get(originator).copied().unwrap_or(0), map.all)
    }

    /// The open rows held calls wait on now, of `originator`'s key.
    #[must_use]
    pub fn rows_of(&self, originator: &str) -> Vec<i64> {
        let mut ids: Vec<i64> = self
            .map()
            .rows
            .iter()
            .filter(|(_, row)| row.originator == originator)
            .map(|(id, _)| *id)
            .collect();
        ids.sort_unstable();
        ids
    }

    /// A held call's place under the cap, or the limit it would pass.
    fn place(&self, originator: &str) -> Result<Place, &'static str> {
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
        })
    }

    /// A call waits on `pending_id`: its row's [`Notify`].
    fn wait_on(&self, pending_id: i64, originator: &str) -> Arc<Notify> {
        let mut map = self.map();
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
    /// disconnect's deny).
    fn leave(&self, pending_id: i64, snapshot: Option<&str>) -> Vec<i64> {
        let mut map = self.map();
        let ids = snapshot.map_or_else(Vec::new, |originator| {
            map.rows
                .iter()
                .filter(|(_, row)| row.originator == originator)
                .map(|(id, _)| *id)
                .collect()
        });
        if let Some(row) = map.rows.get_mut(&pending_id) {
            row.waiters -= 1;
            if row.waiters == 0 {
                map.rows.remove(&pending_id);
            }
        }
        ids
    }
}

/// A held call's place under the cap, freed when the call ends.
struct Place {
    held: Held,
    originator: String,
}

impl Drop for Place {
    fn drop(&mut self) {
        let mut map = self.held.map();
        if let Some(n) = map.calls.get_mut(&self.originator) {
            *n -= 1;
            if *n == 0 {
                map.calls.remove(&self.originator);
            }
        }
        map.all -= 1;
    }
}

/// A held call's guard (the design's `Abandon`): when the call ends with
/// an answer it is disarmed and only stops waiting; when its future is
/// dropped instead (the caller closed its connection), it takes the
/// snapshot of the originator's open rows under [`Held`]'s mutex before it
/// returns and deletes them in a task, waking every call held on one.
struct Waiting {
    held: Held,
    wallet: WalletState,
    originator: String,
    pending_id: i64,
    armed: bool,
    _place: Place,
}

impl Waiting {
    fn answered(mut self) {
        self.armed = false;
    }
}

impl Drop for Waiting {
    fn drop(&mut self) {
        let snapshot = self.armed.then_some(self.originator.as_str());
        let ids = self.held.leave(self.pending_id, snapshot);
        if !self.armed {
            return;
        }
        let (held, wallet, originator) = (
            self.held.clone(),
            self.wallet.clone(),
            self.originator.clone(),
        );
        let Ok(runtime) = tokio::runtime::Handle::try_current() else {
            tracing::warn!(originator = %originator, ids = ?ids, "browser gate: a disconnect outside the runtime; the rows wait for the restart's sweep");
            return;
        };
        runtime.spawn(async move { withdraw(&held, &wallet, &originator, &ids).await });
    }
}

/// Delete the open rows `ids` of `originator` and wake every call held on
/// one: the disconnect's deny and the shutdown's.
async fn withdraw(held: &Held, wallet: &WalletState, originator: &str, ids: &[i64]) {
    match grants::abandon(wallet.storage(), originator, ids).await {
        Ok(rows) => {
            for row in rows {
                held.wake(row.pending_id);
            }
        }
        Err(e) => {
            tracing::warn!(originator = %originator, ids = ?ids, error = %e, "browser gate: the withdrawn rows were not deleted; the restart's sweep deletes them");
        }
    }
}

/// One power a door asks, as its handler names it from its arguments.
pub(crate) struct Ask {
    power: Result<Power, Refused>,
    usage: Option<&'static str>,
    /// A level-0 protocol's key, for the probe record (#539).
    level_zero: Option<String>,
}

impl Ask {
    fn of(power: Result<Power, Refused>, usage: &'static str) -> Self {
        Self {
            power,
            usage: Some(usage),
            level_zero: None,
        }
    }

    /// A protocol at the call's level with its counterparty (`None` is
    /// `self` at level 2), for `usage` (`signing`, `encrypting`, `hmac`,
    /// `publicKey`).
    pub(crate) fn protocol(
        level: u8,
        name: &str,
        counterparty: Option<&str>,
        usage: &'static str,
    ) -> Self {
        let power = Power::protocol(false, level, name, counterparty);
        let level_zero = match power {
            Err(Refused::LevelZero) => Power::level_zero_key(name),
            _ => None,
        };
        Self {
            level_zero,
            ..Self::of(power, usage)
        }
    }

    /// A door's parsed protocol and counterparty (`self` when it names
    /// none, the key in hex), for `usage`.
    pub(crate) fn of_protocol(
        p: &Protocol,
        counterparty: Option<&Counterparty>,
        usage: &'static str,
    ) -> Self {
        let level = match p.security_level {
            SecurityLevel::Silent => 0,
            SecurityLevel::App => 1,
            SecurityLevel::Counterparty => 2,
        };
        let named = counterparty.map(|c| match c {
            Counterparty::Self_ => "self".to_string(),
            Counterparty::Anyone => "anyone".to_string(),
            Counterparty::Other(pk) => pk.to_hex(),
        });
        Self::protocol(level, &p.protocol_name, named.as_deref(), usage)
    }

    /// A protocol the manager names at level 1 (`identity key retrieval`,
    /// `certificate list`, `identity resolution`).
    pub(crate) fn named(name: &str, usage: &'static str) -> Self {
        Self::protocol(1, name, None, usage)
    }

    /// An action label, for `apply` or `list`.
    pub(crate) fn label(label: &str, usage: &'static str) -> Self {
        Self::of(Power::label(label), usage)
    }

    /// A basket, for `insertion`, `listing` or `removal`.
    pub(crate) fn basket(basket: &str, usage: &'static str) -> Self {
        Self::of(Power::basket(basket), usage)
    }

    /// `acquireCertificate`'s or `relinquishCertificate`'s protocol over
    /// its type.
    pub(crate) fn certificate_protocol(relinquish: bool, cert_type: &str) -> Self {
        Self::of(
            Power::certificate_protocol(relinquish, cert_type),
            "generic",
        )
    }

    /// `proveCertificate`'s disclosure.
    pub(crate) fn certificate(
        privileged: bool,
        cert_type: &str,
        verifier: &str,
        fields: &[String],
    ) -> Self {
        Self::of(
            Power::certificate(privileged, cert_type, verifier, fields),
            "disclosure",
        )
    }

    /// `revealCounterpartyKeyLinkage`'s protocol, level 2 always.
    pub(crate) fn counterparty_linkage(counterparty: &str, verifier: &str) -> Self {
        Self::of(
            Power::counterparty_linkage(counterparty, verifier),
            "linkageRevelation",
        )
    }

    /// `revealSpecificKeyLinkage`'s protocol, level 2 always; `key_id` is
    /// the call's when its protocol is at level 2, else `all`.
    pub(crate) fn specific_linkage(name: &str, key_id: Option<&str>, verifier: &str) -> Self {
        Self::of(
            Power::specific_linkage(name, key_id, verifier),
            "linkageRevelation",
        )
    }
}

/// A door's view of the gate: the wallet, the held calls, the probe
/// record's queue, the open doors' bound and the admitted `Origin` (none on
/// a route the gate does not judge).
pub struct Asker {
    wallet: WalletState,
    held: Held,
    probes: Probes,
    bounds: OpenBounds,
    admitted: Option<Admitted>,
}

impl FromRequestParts<WalletState> for Asker {
    type Rejection = std::convert::Infallible;

    async fn from_request_parts(
        parts: &mut Parts,
        wallet: &WalletState,
    ) -> Result<Self, Self::Rejection> {
        Ok(Self {
            wallet: wallet.clone(),
            held: parts.extensions.get::<Held>().cloned().unwrap_or_default(),
            probes: parts
                .extensions
                .get::<Probes>()
                .cloned()
                .unwrap_or_default(),
            bounds: parts
                .extensions
                .get::<OpenBounds>()
                .cloned()
                .unwrap_or_default(),
            admitted: parts.extensions.get::<Admitted>().cloned(),
        })
    }
}

/// The open doors' and level 0's answer with no row (W15, ruled (i) by the
/// owner on 2026-11-03): a page with a parsed `Origin` and an agreeing
/// originator, past the seed, is answered at `door` (an open door's path,
/// or a door's level-0 protocol, `power` its key) with no grant read or
/// written, within the bound of its `class` (#541): past it, 429
/// `ERR_RATE_LIMITED` at once, and the call is neither answered nor
/// counted. An answer is counted in the probe record off the call's path
/// (#539, [`Probes::note`]): the call never waits on it.
pub(crate) fn open(
    admitted: &Admitted,
    probes: &Probes,
    bounds: &OpenBounds,
    door: &str,
    class: Class,
    power: Option<&str>,
) -> Result<(), AppError> {
    // #541: the bound is checked here, after the allow-list and before the door
    bounds.check(&admitted.originator, door, class)?;
    tracing::info!(
        origin = %admitted.origin,
        originator = %admitted.originator,
        door,
        power,
        "browser gate: open, no row (W15)"
    );
    probes.note(&admitted.originator, door, power);
    Ok(())
}

/// A row function's error at a door: the store busy by its type, a revoked
/// power the client's denial, any other refusal of the module the caller's
/// 400, else the store's fault.
fn refused(e: &anyhow::Error) -> AppError {
    if let Some(busy) = AppError::store_busy(e) {
        return busy;
    }
    match e.downcast_ref::<Refused>() {
        Some(Refused::Revoked { .. }) => AppError::new(ErrorKind::OriginDenied, DENIED),
        Some(other) => AppError::new(ErrorKind::Validation, other.to_string()),
        None => AppError::new(ErrorKind::Internal, format!("origin grants: {e}")),
    }
}

impl Asker {
    /// Ask `asks` of the owner for the call to `call`, one at a time:
    /// `Ok` when every power is live or answered `grant`,
    /// else the refusal the door serves. `reason` is the call's own words
    /// (its `description`), shown beside the prompt, never the owner's.
    pub(crate) async fn ask(
        &self,
        call: &str,
        reason: Option<&str>,
        asks: Vec<Ask>,
    ) -> Result<(), AppError> {
        let Some(admitted) = self.admitted.as_ref() else {
            return Err(AppError::new(ErrorKind::Validation, NO_ORIGIN));
        };
        if admitted.seed.is_some() {
            // The seed admits every door before any power (question 6).
            return Ok(());
        }
        for ask in asks {
            let power = match ask.power {
                Ok(power) => power,
                Err(Refused::LevelZero) => {
                    let power = ask.level_zero.as_deref();
                    open(
                        admitted,
                        &self.probes,
                        &self.bounds,
                        call,
                        Class::LevelZero,
                        power,
                    )?;
                    continue;
                }
                Err(Refused::AdminOnly(text)) => {
                    tracing::info!(originator = %admitted.originator, call, refused = %text, "browser gate: admin-reserved");
                    return Err(AppError::new(ErrorKind::OriginDenied, text));
                }
                Err(other) => return Err(AppError::new(ErrorKind::Validation, other.to_string())),
            };
            self.one(admitted, call, reason, &power, ask.usage).await?;
        }
        Ok(())
    }

    /// A crypto door's one protocol power ([`Ask::of_protocol`]).
    pub(crate) async fn protocol(
        &self,
        call: &str,
        p: &Protocol,
        counterparty: Option<&Counterparty>,
        usage: &'static str,
    ) -> Result<(), AppError> {
        let ask = Ask::of_protocol(p, counterparty, usage);
        self.ask(call, None, vec![ask]).await
    }

    /// One power: live, revoked, or held for the owner.
    async fn one(
        &self,
        admitted: &Admitted,
        call: &str,
        reason: Option<&str>,
        power: &Power,
        usage: Option<&'static str>,
    ) -> Result<(), AppError> {
        let now = clock()?;
        let storage = self.wallet.storage();
        let renewal = match grants::lookup(storage, &admitted.originator, power, now)
            .await
            .map_err(|e| refused(&e))?
        {
            Standing::Live(row) => {
                admitted_by(admitted, &row, call);
                return Ok(());
            }
            Standing::Revoked(row) => return Err(revoked(admitted, &row, call)),
            Standing::Expired(_) => true,
            Standing::None => false,
        };
        let asking = Asking {
            originator: &admitted.originator,
            power,
            usage,
            call,
            reason,
            amount: None,
            renewal,
        };
        self.hold(&asking, now).await.map(|_| ())
    }

    /// Hold the call on its power's open row until the owner answers it
    /// (`grant` or `once`, else 400 `Permission denied.`), the row is
    /// deleted, or the server shuts down.
    async fn hold(&self, asking: &Asking<'_>, now: i64) -> Result<Answer, AppError> {
        let (originator, _) = grants::key_of(asking.originator).map_err(|e| refused(&e))?;
        let place = self.held.place(&originator).map_err(|limit| {
            tracing::warn!(originator = %originator, call = asking.call, limit, "browser gate: pending limit");
            AppError::new(ErrorKind::PendingLimit, PENDING_LIMIT)
        })?;
        if self.held.is_shut() {
            return Err(AppError::new(ErrorKind::BridgeUnavailable, SHUTTING_DOWN));
        }
        let storage = self.wallet.storage();
        let pending = grants::open_or_join(storage, asking, now)
            .await
            .map_err(|e| refused(&e))?;
        #[cfg(test)]
        self.held.paused().await;
        let id = pending.row.pending_id;
        tracing::info!(
            originator = %pending.row.originator,
            kind = pending.row.kind.as_str(),
            key = %pending.row.key,
            usage = ?pending.row.usage,
            call = asking.call,
            pending_id = id,
            joined = pending.joined,
            "browser gate: pending"
        );
        let waiting = Waiting {
            held: self.held.clone(),
            wallet: self.wallet.clone(),
            originator,
            pending_id: id,
            armed: true,
            _place: place,
        };
        self.wait(waiting).await
    }

    /// Wait on the row until it is answered, deleted, or the server shuts
    /// down: its row read every [`READ_EVERY`] and at once on its
    /// [`Notify`].
    async fn wait(&self, waiting: Waiting) -> Result<Answer, AppError> {
        let id = waiting.pending_id;
        let notify = self.held.wait_on(id, &waiting.originator);
        let mut shut = self.held.0.shut.subscribe();
        loop {
            if *shut.borrow_and_update() {
                let originator = waiting.originator.clone();
                waiting.answered();
                withdraw(&self.held, &self.wallet, &originator, &[id]).await;
                return Err(AppError::new(ErrorKind::BridgeUnavailable, SHUTTING_DOWN));
            }
            tokio::select! {
                () = notify.notified() => {}
                () = tokio::time::sleep(READ_EVERY) => {}
                _ = shut.changed() => continue,
            }
            let answer = match grants::pending(self.wallet.storage(), id).await {
                Ok(Some(row)) => row.answer,
                Ok(None) => Some(Answer::Deny),
                Err(e) => {
                    tracing::warn!(pending_id = id, error = %e, "browser gate: a held call's read failed; it reads again");
                    continue;
                }
            };
            match answer {
                None => {}
                Some(word @ (Answer::Grant | Answer::Once)) => {
                    waiting.answered();
                    return Ok(word);
                }
                Some(Answer::Deny) => {
                    waiting.answered();
                    return Err(AppError::new(ErrorKind::OriginDenied, PERMISSION_DENIED));
                }
            }
        }
    }
}

/// The time now, unix seconds.
fn clock() -> Result<i64, AppError> {
    super::owner_door::now().map_err(|e| AppError::new(ErrorKind::Internal, e.to_string()))
}

/// A live row admitted the call: logged by name.
fn admitted_by(admitted: &Admitted, row: &grants::GrantRow, call: &str) {
    tracing::info!(
        origin = %admitted.origin,
        originator = %admitted.originator,
        grant_id = row.grant_id,
        kind = row.kind.as_str(),
        key = %row.key,
        call,
        "browser gate: admitted"
    );
}

/// A revoked row refuses at once with the client's grouped denial (W3).
fn revoked(admitted: &Admitted, row: &grants::GrantRow, call: &str) -> AppError {
    tracing::info!(
        originator = %admitted.originator,
        grant_id = row.grant_id,
        kind = row.kind.as_str(),
        key = %row.key,
        call,
        "browser gate: revoked, refused"
    );
    AppError::new(ErrorKind::OriginDenied, DENIED)
}

#[cfg(test)]
#[path = "spending_tests.rs"]
mod spending_tests;
