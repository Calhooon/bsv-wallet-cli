//! The probe record's writer (#539, the design's section "The probe
//! record"): the gate counts each answer it gives with no row (an open
//! door, a level-0 protocol, W15 (i)) by sending it to one task, which
//! writes it to `origin_probes` (`store::origin_grants::probes`). The call
//! never waits on the record: [`Probes::note`] is a `try_send` on a bounded
//! queue, and the answer goes out whatever the store does. A failed write
//! is logged and lost, never an error to the page; a full queue drops the
//! answer's count and the next write logs how many were dropped.
//!
//! The task drains what is queued (at most [`BATCH`]) and writes it in one
//! transaction, so a polling page costs one write a batch, not one a call.
//! It ends when the router holding the queue is dropped.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, OnceLock};

use tokio::sync::{mpsc, watch};

use super::handlers::WalletState;
use crate::store::origin_grants::{probes, Probe};

/// The answers the queue holds before one is dropped.
pub const QUEUE: usize = 1024;
/// The answers one write takes.
pub const BATCH: usize = 64;

/// The probe record's queue: unstarted until a router serves it
/// ([`Probes::started`]); clones share one queue.
#[derive(Clone, Default)]
pub struct Probes(Arc<Inner>);

#[derive(Default)]
struct Inner {
    /// The queue, and the address of the wallet whose store its task
    /// writes.
    queue: OnceLock<(mpsc::Sender<Probe>, usize)>,
    counts: Arc<Counts>,
}

/// What the call side and the task share.
struct Counts {
    /// The answers queued and not yet written (or dropped).
    owed: watch::Sender<u64>,
    /// The answers dropped on a full queue since the last write.
    dropped: AtomicU64,
}

impl Default for Counts {
    fn default() -> Self {
        Self {
            owed: watch::channel(0).0,
            dropped: AtomicU64::new(0),
        }
    }
}

impl Counts {
    fn paid(&self, n: u64) {
        self.owed.send_modify(|owed| *owed = owed.saturating_sub(n));
    }
}

impl Probes {
    /// This queue, its task started on `wallet`'s store; a queue already
    /// started on another wallet is not shared, and a fresh one is started
    /// for `wallet`. With no runtime the queue stays unstarted and every
    /// answer is dropped, logged.
    #[must_use]
    pub fn started(&self, wallet: &WalletState) -> Self {
        let address = Arc::as_ptr(wallet) as usize;
        if let Some((_, on)) = self.0.queue.get() {
            if *on == address {
                return self.clone();
            }
            return Self::default().started(wallet);
        }
        let Ok(runtime) = tokio::runtime::Handle::try_current() else {
            tracing::warn!(
                "browser gate: no runtime for the probe record's writer; no answer is recorded"
            );
            return self.clone();
        };
        let (tx, rx) = mpsc::channel(QUEUE);
        if self.0.queue.set((tx, address)).is_err() {
            return self.started(wallet);
        }
        let counts = self.0.counts.clone();
        runtime.spawn(write(wallet.clone(), rx, counts));
        self.clone()
    }

    /// Count an answer `originator` was given at `door` (its path or name;
    /// the slash dropped) with no row, `power` a level-0 protocol's key:
    /// queued, never awaited.
    pub fn note(&self, originator: &str, door: &str, power: Option<&str>) {
        let Some((queue, _)) = self.0.queue.get() else {
            return;
        };
        let Ok(at) = super::owner_door::now() else {
            return;
        };
        let probe = Probe {
            originator: originator.to_string(),
            door: door.trim_start_matches('/').to_string(),
            power: power.map(str::to_string),
            at,
        };
        let counts = &self.0.counts;
        counts.owed.send_modify(|owed| *owed += 1);
        if queue.try_send(probe).is_err() {
            counts.paid(1);
            counts.dropped.fetch_add(1, Ordering::Relaxed);
        }
    }

    /// Wait until every answer queued so far is written (or dropped): the
    /// harnesses' read after a page's calls.
    pub async fn settled(&self) {
        let mut owed = self.0.counts.owed.subscribe();
        // The sender lives in `self`, so the wait ends only at zero.
        let _ = owed.wait_for(|n| *n == 0).await;
    }
}

/// The writer: each batch in one transaction, a failure logged.
async fn write(wallet: WalletState, mut rx: mpsc::Receiver<Probe>, counts: Arc<Counts>) {
    let mut batch = Vec::with_capacity(BATCH);
    while rx.recv_many(&mut batch, BATCH).await > 0 {
        let dropped = counts.dropped.swap(0, Ordering::Relaxed);
        if dropped > 0 {
            tracing::warn!(
                dropped,
                "browser gate: the probe record's queue was full; {dropped} answers were not counted"
            );
        }
        if let Err(e) = probes::record(wallet.storage(), &batch).await {
            tracing::warn!(error = %e, answers = batch.len(), "browser gate: the probe record was not written");
        }
        counts.paid(batch.len() as u64);
        batch.clear();
    }
}
