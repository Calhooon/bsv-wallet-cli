//! The rows as the owner reads them (section (4)): each grant row with the
//! month's tally of a spending row, the pending rows, and their JSON in
//! the section's keys and order, times in unix seconds as the rows hold
//! them. `bsv-wallet origins --json` prints it and the owner's door
//! (`GET /origins`) answers it, so the two places read one shape.

use anyhow::Result;
use bsv_wallet_toolbox::StorageSqlx;
use serde::Serialize;

use super::{month_tally, Answer, GrantRow, PendingRow, Scope};

/// Rows to show: grant rows, each spending row with the month's tally,
/// and pending rows.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Shown {
    /// Each grant row, with the satoshis its originator spent this UTC
    /// month when the row is a `spending` row (`None` for `wallet`).
    pub grants: Vec<(GrantRow, Option<i64>)>,
    /// The pending rows.
    pub pending: Vec<PendingRow>,
}

impl Shown {
    /// `rows` with the month of `now`'s tally read for each spending row,
    /// and `pending`.
    ///
    /// # Errors
    ///
    /// The store's error.
    pub async fn of(
        storage: &StorageSqlx,
        rows: Vec<GrantRow>,
        pending: Vec<PendingRow>,
        now: i64,
    ) -> Result<Self> {
        let mut grants = Vec::with_capacity(rows.len());
        for g in rows {
            let spent = match g.scope {
                Scope::Spending => Some(month_tally(storage, &g.originator, now).await?),
                Scope::Wallet => None,
            };
            grants.push((g, spent));
        }
        Ok(Self { grants, pending })
    }

    /// The rows as JSON: `{"grants": [...], "pending": [...]}`.
    #[must_use]
    pub fn json(&self) -> ShownJson<'_> {
        let grants = self.grants.iter().map(|(g, spent)| GrantJson {
            originator: &g.originator,
            raw_originator: &g.raw_originator,
            scope: g.scope.as_str(),
            amount: g.amount,
            spent_this_month: *spent,
            granted_at: g.granted_at,
            expiry: g.expiry,
            revoked_at: g.revoked_at,
            source: g.source.as_str(),
        });
        let pending = self.pending.iter().map(|p| PendingJson {
            pending_id: p.pending_id,
            originator: &p.originator,
            raw_originator: &p.raw_originator,
            scope: p.scope.as_str(),
            call: &p.call,
            amount: p.amount,
            renewal: p.renewal,
            first_seen: p.first_seen,
            answered_at: p.answered_at,
            answer: p.answer.map(Answer::as_str),
        });
        ShownJson {
            grants: grants.collect(),
            pending: pending.collect(),
        }
    }
}

/// A grant row in JSON, the keys of section (4) in its order.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct GrantJson<'a> {
    originator: &'a str,
    raw_originator: &'a str,
    scope: &'a str,
    amount: Option<i64>,
    spent_this_month: Option<i64>,
    granted_at: i64,
    expiry: i64,
    revoked_at: Option<i64>,
    source: &'a str,
}

/// A pending row in JSON, the keys of section (4) in its order.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct PendingJson<'a> {
    pending_id: i64,
    originator: &'a str,
    raw_originator: &'a str,
    scope: &'a str,
    call: &'a str,
    amount: Option<i64>,
    renewal: bool,
    first_seen: i64,
    answered_at: Option<i64>,
    answer: Option<&'a str>,
}

/// [`Shown`] as JSON: `{"grants": [...], "pending": [...]}`.
#[derive(Serialize)]
pub struct ShownJson<'a> {
    grants: Vec<GrantJson<'a>>,
    pending: Vec<PendingJson<'a>>,
}
