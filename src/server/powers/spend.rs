//! `createAction`'s spend (#414, the design's L6, section (1), W6 and W7):
//! asked last, inside the `SpendingLock`, against the originator's spending
//! row and the month's tally; held with the lock let go when the owner must
//! answer; tallied by the door once the toolbox returns the action.

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
    /// until its toolbox call returns and its spend is tallied, with the
    /// [`Tally`] to write; none for an amount of 0 (no power asked here,
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

/// An admitted spend the door tallies once the toolbox returns its action
/// (section (1), W7): one `origin_spends` row, the txid on the signed path,
/// the reference on the deferred one; a failed call writes none.
pub(crate) struct Tally {
    originator: String,
    amount: i64,
}

impl Tally {
    /// Write the row, before the door lets the `SpendingLock` go. A write
    /// the store refuses is logged at warn by name and the door's answer
    /// stands: the action is already built, and a refusal would invite the
    /// page to spend again.
    pub(crate) async fn write(&self, wallet: &WalletState, action: &CreateActionResult) {
        let txid = action.txid.as_ref().map(|t| to_hex(t));
        let reference = action
            .signable_transaction
            .as_ref()
            .map(|st| crate::spend_gate::reference_string(&st.reference));
        let flow = match (&txid, &reference) {
            (Some(txid), _) => Flow::Signed(txid),
            (None, Some(reference)) => Flow::Deferred(reference),
            (None, None) => {
                tracing::warn!(originator = %self.originator, amount = self.amount, "browser gate: the spend names no txid and no reference; it is not tallied");
                return;
            }
        };
        let written = match super::super::owner_door::now() {
            Ok(now) => {
                grants::spent(wallet.storage(), &self.originator, flow, self.amount, now).await
            }
            Err(e) => Err(e),
        };
        if let Err(e) = written {
            tracing::warn!(originator = %self.originator, amount = self.amount, flow = ?flow, error = %e, "browser gate: the spend was not tallied");
        }
    }
}
