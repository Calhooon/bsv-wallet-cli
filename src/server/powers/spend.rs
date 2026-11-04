//! `createAction`'s spend (#414, the design's L6, section (1), W6 and W7):
//! asked last, inside the `SpendingLock`, against the originator's spending
//! row and the month's tally; held with the lock let go when the owner must
//! answer; its row opened before the toolbox's call, settled on its
//! action, dropped on its failure.

use bsv_sdk::primitives::to_hex;
use bsv_sdk::wallet::CreateActionResult;
use tokio::sync::OwnedMutexGuard;

use super::super::handlers::WalletState;
use super::super::origin_gate::Admitted;
use super::super::SpendingLock;
use super::{admitted_by, clock, refused, revoked, AppError, Asker, ErrorKind, NO_ORIGIN};
use crate::store::origin_grants::{self as grants, Answer, Asking, Flow, Power, Standing};

impl Asker {
    /// `createAction`'s spend of `amount` satoshis (W6: its outputs' sum),
    /// asked inside `lock` (section (1)): the lock's guard, held by the door
    /// until its toolbox call returns and its spend's row is settled, with
    /// the [`Tally`] to open; none for an amount of 0 (no power asked here,
    /// where the reference asks one for the fee) or a seed-listed caller.
    ///
    /// A live spending row admits the call when its ceiling is unset or the
    /// month's tally plus `amount` is at most the ceiling; equal admits.
    /// Past it, or with no row, the call is held (a renewal past a
    /// ceiling) with the lock released; after the answer it takes the lock
    /// again: `once` admits the one call, `grant` is checked again under
    /// the new ceiling, `deny` is 400 `Permission denied.`. A revoked row
    /// refuses at once.
    pub(crate) async fn spend(
        &self,
        call: &str,
        reason: Option<&str>,
        amount: u64,
        lock: &SpendingLock,
    ) -> Result<(OwnedMutexGuard<()>, Option<Tally>), AppError> {
        let Some(admitted) = self.admitted.as_ref() else {
            return Err(AppError::new(ErrorKind::Validation, NO_ORIGIN));
        };
        if amount == 0 || admitted.seed.is_some() {
            return Ok((lock.clone().lock_owned().await, None));
        }
        let amount = i64::try_from(amount).unwrap_or(i64::MAX);
        let tally = Tally {
            originator: admitted.originator.clone(),
            amount,
        };
        loop {
            let guard = lock.clone().lock_owned().await;
            let now = clock()?;
            let Some(renewal) = self.ceiling(admitted, call, amount, now).await? else {
                return Ok((guard, Some(tally)));
            };
            drop(guard);
            let asking = Asking {
                originator: &admitted.originator,
                power: &Power::Spending,
                usage: None,
                call,
                reason,
                amount: Some(amount),
                renewal,
            };
            if self.hold(&asking, now).await? == Answer::Once {
                return Ok((lock.clone().lock_owned().await, Some(tally)));
            }
        }
    }

    /// The spending row's word on `amount` at `now`, read under the lock:
    /// `None` admits; `Some(renewal)` holds (a renewal past a ceiling); a
    /// revoked row refuses.
    async fn ceiling(
        &self,
        admitted: &Admitted,
        call: &str,
        amount: i64,
        now: i64,
    ) -> Result<Option<bool>, AppError> {
        let storage = self.wallet.storage();
        let row = match grants::lookup(storage, &admitted.originator, &Power::Spending, now)
            .await
            .map_err(|e| refused(&e))?
        {
            Standing::Live(row) => row,
            Standing::Revoked(row) => return Err(revoked(admitted, &row, call)),
            Standing::Expired(_) => return Ok(Some(true)),
            Standing::None => return Ok(Some(false)),
        };
        let spent = grants::month_tally(storage, &admitted.originator, now)
            .await
            .map_err(|e| refused(&e))?;
        if row
            .amount
            .is_none_or(|ceiling| spent.saturating_add(amount) <= ceiling)
        {
            admitted_by(admitted, &row, call);
            return Ok(None);
        }
        tracing::info!(
            originator = %admitted.originator,
            grant_id = row.grant_id,
            ceiling = row.amount,
            spent,
            amount,
            call,
            "browser gate: past the spending ceiling, held as a renewal"
        );
        Ok(Some(true))
    }
}

/// An admitted spend the door tallies (section (1), W7): one
/// `origin_spends` row, opened before the toolbox's call so the ceiling
/// holds whatever the store does next, then given the txid on the signed
/// path or the reference on the deferred one; a failed call writes none.
pub(crate) struct Tally {
    originator: String,
    amount: i64,
}

impl Tally {
    /// Open the row under the `SpendingLock`, before the toolbox's call: a
    /// write the store refuses refuses the call by the store's class (the
    /// store busy 503 with `Retry-After`, else the internal class), with
    /// nothing built.
    pub(crate) async fn open(self, wallet: &WalletState) -> Result<OpenSpend, AppError> {
        let now = clock()?;
        let spend_id = grants::open_spend(wallet.storage(), &self.originator, self.amount, now)
            .await
            .map_err(|e| refused(&e))?;
        Ok(OpenSpend {
            tally: self,
            spend_id,
        })
    }
}

/// A spend's open row, settled when the toolbox returns the action and
/// dropped when the call fails.
pub(crate) struct OpenSpend {
    tally: Tally,
    spend_id: i64,
}

impl OpenSpend {
    /// The toolbox returned `action`: the row takes its txid, else its
    /// reference. A write the store refuses leaves the row open, still
    /// counted, logged at warn by name.
    pub(crate) async fn settle(&self, wallet: &WalletState, action: &CreateActionResult) {
        let Tally { originator, amount } = &self.tally;
        let txid = action.txid.as_ref().map(|t| to_hex(t));
        let reference = action
            .signable_transaction
            .as_ref()
            .map(|st| crate::spend_gate::reference_string(&st.reference));
        let flow = match (&txid, &reference) {
            (Some(txid), _) => Flow::Signed(txid),
            (None, Some(reference)) => Flow::Deferred(reference),
            (None, None) => {
                tracing::warn!(originator = %originator, amount, "browser gate: the spend names no txid and no reference; its row stays open");
                return;
            }
        };
        if let Err(e) = grants::settle_spend(wallet.storage(), self.spend_id, flow).await {
            tracing::warn!(originator = %originator, amount, flow = ?flow, error = %e, "browser gate: the spend's row stays open");
        }
    }

    /// The call failed (the toolbox's refusal of any kind, or a broadcast
    /// refused and retired): the row deleted. A delete the store refuses
    /// leaves the row counted, logged at warn by name.
    pub(crate) async fn drop_row(&self, wallet: &WalletState) {
        if let Err(e) = grants::drop_spend(wallet.storage(), self.spend_id).await {
            let Tally { originator, amount } = &self.tally;
            tracing::warn!(originator = %originator, amount, error = %e, "browser gate: the failed spend was not taken from the tally");
        }
    }
}
