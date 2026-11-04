//! The rows as the owner reads them (section (4)): each grant row read once
//! at `now` ([`GrantShown`]: its state, and a spending row's ceiling, the
//! month's tally and whether the tally has reached the ceiling), the
//! pending rows with their prompt's text, and their JSON in the section's
//! keys and order, times in unix seconds as the rows hold them.
//! `bsv-wallet origins` prints it (text and `--json`), the owner's door
//! (`GET /origins`, and the MCP's `list_origins` through it) and the
//! inspector (`GET /api/origins`) answer it, so every place reads one
//! reading (#533). A listing carries the probe record too (#539,
//! `probes`); the answer to a grant, a deny or a revoke does not.

use anyhow::Result;
use bsv_wallet_toolbox::StorageSqlx;
use serde::Serialize;

use super::{
    expired, lookup, month_of, month_tally, Answer, GrantRow, Kind, Listing, PendingRow, Power,
    ProbeRow, Standing,
};

/// A grant row's state at a time, the word every place prints (#533):
/// `revoked` once stamped, else `expired` past its expiry ([`expired`]),
/// else `granted`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GrantState {
    /// Live: the row admits its power.
    Granted,
    /// Past its expiry and not revoked: a call is held as a renewal.
    Expired,
    /// Stamped revoked: a call is refused.
    Revoked,
}

impl GrantState {
    /// The state of `row` at `now`.
    #[must_use]
    pub fn of(row: &GrantRow, now: i64) -> Self {
        if row.revoked_at.is_some() {
            Self::Revoked
        } else if expired(row.expiry, now) {
            Self::Expired
        } else {
            Self::Granted
        }
    }

    /// The word: `granted`, `expired` or `revoked`.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Granted => "granted",
            Self::Expired => "expired",
            Self::Revoked => "revoked",
        }
    }
}

/// A grant row as every place reads it (#533), computed once from the row
/// and `now`: its state, and for a `spending` row the month, the
/// satoshis its originator spent in it and whether that tally has reached
/// the row's ceiling.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GrantShown {
    /// The row as the store holds it.
    pub row: GrantRow,
    /// [`GrantState::of`] the row at `now`.
    pub state: GrantState,
    /// `spending` alone: the UTC month of `now`, `YYYY-MM` ([`month_of`]).
    pub month: Option<String>,
    /// `spending` alone: the month's tally ([`month_tally`]).
    pub spent_this_month: Option<i64>,
    /// `spending` alone: the tally is at or past the ceiling, so the gate
    /// admits no further spend of a satoshi this month (it admits a call
    /// when the tally plus its amount is at most the ceiling); `false`
    /// for a row with no ceiling.
    pub ceiling_reached: Option<bool>,
}

impl GrantShown {
    /// `row` read at `now`, with `spent`, the month's tally of a spending
    /// row (ignored for another kind).
    ///
    /// # Errors
    ///
    /// A `now` outside chrono's range.
    pub fn read(row: GrantRow, spent: Option<i64>, now: i64) -> Result<Self> {
        let state = GrantState::of(&row, now);
        let (month, spent_this_month, ceiling_reached) = if row.kind == Kind::Spending {
            let spent = spent.unwrap_or(0);
            let reached = row.amount.is_some_and(|ceiling| spent >= ceiling);
            (Some(month_of(now)?), Some(spent), Some(reached))
        } else {
            (None, None, None)
        };
        Ok(Self {
            row,
            state,
            month,
            spent_this_month,
            ceiling_reached,
        })
    }
}

/// Rows to show: grant rows, each read at a time ([`GrantShown`]), and
/// pending rows, each with its prompt's text.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Shown {
    /// Each grant row, read once at the listing's `now`.
    pub grants: Vec<GrantShown>,
    /// The pending rows, each with [`PendingRow::asked`]'s text.
    pub pending: Vec<(PendingRow, String)>,
    /// The probe record's rows, on a listing alone (#539).
    pub probes: Option<Vec<ProbeRow>>,
}

impl Shown {
    /// `rows` each read at `now` ([`GrantShown::read`], the month of
    /// `now`'s tally read for each spending row), and `pending` with its prompt's text (a spending renewal's with the
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
            grants.push(GrantShown::read(g, spent, now)?);
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
            probes: None,
        })
    }

    /// A listing's rows ([`Self::of`]) with its probe record.
    ///
    /// # Errors
    ///
    /// The store's error.
    pub async fn listed(storage: &StorageSqlx, listing: Listing, now: i64) -> Result<Self> {
        let shown = Self::of(storage, listing.grants, listing.pending, now).await?;
        Ok(Self {
            probes: Some(listing.probes),
            ..shown
        })
    }

    /// The rows as JSON: `{"grants": [...], "pending": [...]}`, and
    /// `"probes": [...]` on a listing.
    #[must_use]
    pub fn json(&self) -> ShownJson<'_> {
        let grants = self.grants.iter().map(|shown| {
            let g = &shown.row;
            GrantJson {
                originator: &g.originator,
                raw_originator: &g.raw_originator,
                kind: g.kind.as_str(),
                key: &g.key,
                state: shown.state.as_str(),
                amount: g.amount,
                spent_this_month: shown.spent_this_month,
                ceiling_reached: shown.ceiling_reached,
                granted_at: g.granted_at,
                expiry: g.expiry,
                revoked_at: g.revoked_at,
            }
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
        let probes = self.probes.as_ref().map(|rows| {
            rows.iter()
                .map(|p| ProbeJson {
                    originator: &p.originator,
                    raw_originator: &p.raw_originator,
                    door: &p.door,
                    power: p.power.as_deref(),
                    count: p.count,
                    first_seen: p.first_seen,
                    last_seen: p.last_seen,
                })
                .collect()
        });
        ShownJson {
            grants: grants.collect(),
            pending: pending.collect(),
            probes,
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
    state: &'a str,
    amount: Option<i64>,
    spent_this_month: Option<i64>,
    ceiling_reached: Option<bool>,
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

/// A probe row in JSON (#539), times in unix seconds.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ProbeJson<'a> {
    originator: &'a str,
    raw_originator: &'a str,
    door: &'a str,
    power: Option<&'a str>,
    count: i64,
    first_seen: i64,
    last_seen: i64,
}

/// [`Shown`] as JSON: `{"grants": [...], "pending": [...]}`, and
/// `"probes": [...]` on a listing.
#[derive(Serialize)]
pub struct ShownJson<'a> {
    grants: Vec<GrantJson<'a>>,
    pending: Vec<PendingJson<'a>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    probes: Option<Vec<ProbeJson<'a>>>,
}
