//! The rows as the owner reads them (section (4)): each grant row with the
//! month's tally of a spending row, the pending rows with their prompt's
//! text, and their JSON in the section's keys and order, times in unix
//! seconds as the rows hold them. `bsv-wallet origins --json` prints it
//! and the owner's door (`GET /origins`) answers it, so the two places
//! read one shape.

use anyhow::Result;
use bsv_wallet_toolbox::StorageSqlx;
use serde::Serialize;

use super::{lookup, month_tally, Answer, GrantRow, Kind, PendingRow, Power, Standing};

/// Rows to show: grant rows, each spending row with the month's tally,
/// and pending rows, each with its prompt's text.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Shown {
    /// Each grant row, with the satoshis its originator spent this UTC
    /// month when the row is a `spending` row (`None` for another kind).
    pub grants: Vec<(GrantRow, Option<i64>)>,
    /// The pending rows, each with [`PendingRow::asked`]'s text.
    pub pending: Vec<(PendingRow, String)>,
}

impl Shown {
    /// `rows` with the month of `now`'s tally read for each spending row,
    /// and `pending` with its prompt's text (a spending renewal's with the
    /// originator's ceiling and tally).
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
            let spent = match g.kind {
                Kind::Spending => Some(month_tally(storage, &g.originator, now).await?),
                _ => None,
            };
            grants.push((g, spent));
        }
        let mut asked = Vec::with_capacity(pending.len());
        for p in pending {
            let past = if p.kind == Kind::Spending && p.renewal {
                past(storage, &p.originator, now).await?
            } else {
                None
            };
            let text = p.asked(past);
            asked.push((p, text));
        }
        Ok(Self {
            grants,
            pending: asked,
        })
    }

    /// The rows as JSON: `{"grants": [...], "pending": [...]}`.
    #[must_use]
    pub fn json(&self) -> ShownJson<'_> {
        let grants = self.grants.iter().map(|(g, spent)| GrantJson {
            originator: &g.originator,
            raw_originator: &g.raw_originator,
            kind: g.kind.as_str(),
            key: &g.key,
            amount: g.amount,
            spent_this_month: *spent,
            granted_at: g.granted_at,
            expiry: g.expiry,
            revoked_at: g.revoked_at,
        });
        let pending = self.pending.iter().map(|(p, asked)| PendingJson {
            pending_id: p.pending_id,
            originator: &p.originator,
            raw_originator: &p.raw_originator,
            kind: p.kind.as_str(),
            key: &p.key,
            usage: p.usage.as_deref(),
            asked,
            call: &p.call,
            reason: p.reason.as_deref(),
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

/// The ceiling of `originator`'s spending row and the month's tally, when
/// it holds one.
async fn past(storage: &StorageSqlx, originator: &str, now: i64) -> Result<Option<(i64, i64)>> {
    let row = match lookup(storage, originator, &Power::Spending, now).await? {
        Standing::Live(r) | Standing::Revoked(r) | Standing::Expired(r) => r,
        Standing::None => return Ok(None),
    };
    let Some(ceiling) = row.amount else {
        return Ok(None);
    };
    Ok(Some((
        ceiling,
        month_tally(storage, originator, now).await?,
    )))
}

/// A grant row in JSON, the keys of section (4) in its order.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct GrantJson<'a> {
    originator: &'a str,
    raw_originator: &'a str,
    kind: &'a str,
    key: &'a str,
    amount: Option<i64>,
    spent_this_month: Option<i64>,
    granted_at: i64,
    expiry: i64,
    revoked_at: Option<i64>,
}

/// A pending row in JSON, the keys of section (4) in its order.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct PendingJson<'a> {
    pending_id: i64,
    originator: &'a str,
    raw_originator: &'a str,
    kind: &'a str,
    key: &'a str,
    usage: Option<&'a str>,
    asked: &'a str,
    call: &'a str,
    reason: Option<&'a str>,
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
