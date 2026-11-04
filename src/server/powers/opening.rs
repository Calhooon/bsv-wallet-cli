//! The row's open, in a task of its own (#552): a call's [`Asking`] owned
//! by the task that commits its row and enters its guard, which the
//! caller's drop does not cut; and the test build's pauses: between the
//! two, and before the open (#534).

use crate::store::origin_grants::{Asking, Power};

/// A call's [`Asking`], owned by the task that opens its row.
pub(super) struct Opening {
    originator: String,
    power: Power,
    usage: Option<String>,
    pub(super) call: String,
    reason: Option<String>,
    amount: Option<i64>,
    renewal: bool,
}

impl Opening {
    pub(super) fn of(asking: &Asking<'_>) -> Self {
        Self {
            originator: asking.originator.to_string(),
            power: asking.power.clone(),
            usage: asking.usage.map(str::to_string),
            call: asking.call.to_string(),
            reason: asking.reason.map(str::to_string),
            amount: asking.amount,
            renewal: asking.renewal,
        }
    }

    pub(super) fn asking(&self) -> Asking<'_> {
        Asking {
            originator: &self.originator,
            power: &self.power,
            usage: self.usage.as_deref(),
            call: &self.call,
            reason: self.reason.as_deref(),
            amount: self.amount,
            renewal: self.renewal,
        }
    }
}

/// Where a test build's pause is planted in a held call's way: after
/// `origin_grants::abandoned`'s read and the call's place, before the open
/// (#534's race of a resume that read missed); or between the open's commit
/// and the guard's entry (#552).
#[derive(Clone, Copy)]
pub(crate) enum At {
    Read = 0,
    Open = 1,
}

/// The served build plants nothing: the open runs straight through.
#[cfg(not(test))]
#[derive(Default)]
pub(super) struct Planted(());

/// The served build's pause: none.
#[cfg(not(test))]
pub(super) fn paused(held: &super::Held, at: At) -> std::future::Ready<()> {
    let (Planted(()), _) = (held.planted(), at);
    std::future::ready(())
}

/// The test build's pauses, one per [`At`], each taken by the next call
/// that reaches it.
#[cfg(test)]
#[derive(Default)]
pub(super) struct Planted(std::sync::Mutex<[Option<Pause>; 2]>);

/// A planted pause: the call signals `reached` at its place and waits on
/// `release` before it goes on.
#[cfg(test)]
#[derive(Clone, Default)]
pub(crate) struct Pause {
    pub(crate) reached: std::sync::Arc<tokio::sync::Notify>,
    pub(crate) release: std::sync::Arc<tokio::sync::Notify>,
}

/// The planted pause at `at`, if any: signalled, then waited out.
#[cfg(test)]
pub(super) async fn paused(held: &super::Held, at: At) {
    let taken = held
        .planted()
        .0
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)[at as usize]
        .take();
    if let Some(pause) = taken {
        pause.reached.notify_one();
        pause.release.notified().await;
    }
}

#[cfg(test)]
impl super::Held {
    /// Plant a pause for the next call that opens a row.
    pub(crate) fn pause_after_open(&self) -> Pause {
        self.pause_at(At::Open)
    }

    /// Plant a pause for the next held call, after its read of the
    /// abandoned row and its place, before its open (#534).
    pub(crate) fn pause_after_read(&self) -> Pause {
        self.pause_at(At::Read)
    }

    fn pause_at(&self, at: At) -> Pause {
        let pause = Pause::default();
        self.planted()
            .0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)[at as usize] = Some(pause.clone());
        pause
    }
}
