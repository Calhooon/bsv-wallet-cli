//! The row's open, in a task of its own (#552): a call's [`Asking`] owned
//! by the task that commits its row and enters its guard, which the
//! caller's drop does not cut; and the test build's pause between the two.

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

/// The served build plants nothing: the open runs straight through.
#[cfg(not(test))]
#[derive(Default)]
pub(super) struct Planted(());

/// The served build's pause: none.
#[cfg(not(test))]
pub(super) fn paused(held: &super::Held) -> std::future::Ready<()> {
    let Planted(()) = held.planted();
    std::future::ready(())
}

/// The test build's pause between the open's commit and the guard's
/// entry, taken by the next call that opens a row.
#[cfg(test)]
#[derive(Default)]
pub(super) struct Planted(std::sync::Mutex<Option<Pause>>);

/// A planted pause: the call signals `reached` once its row is committed
/// and waits on `release` before its guard is entered.
#[cfg(test)]
#[derive(Clone, Default)]
pub(crate) struct Pause {
    pub(crate) reached: std::sync::Arc<tokio::sync::Notify>,
    pub(crate) release: std::sync::Arc<tokio::sync::Notify>,
}

/// The planted pause, if any: signalled, then waited out.
#[cfg(test)]
pub(super) async fn paused(held: &super::Held) {
    let taken = held
        .planted()
        .0
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
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
        let pause = Pause::default();
        *self
            .planted()
            .0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(pause.clone());
        pause
    }
}
