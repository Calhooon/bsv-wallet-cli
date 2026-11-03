//! The move of L1's tables to the four kinds (#414, the design's section
//! (1), "The migration of L1's tables"). L1 keyed a grant by
//! (originator, scope) with the scopes `wallet` and `spending` and a
//! `source` column; SQLite cannot alter a `CHECK`, so the move
//! (`super::move_l1`, its statements behind the store's boundary) renames
//! L1's two tables aside, runs the block, carries each ceilinged
//! `spending` row into the spending kind and drops the old tables, in the
//! one `BEGIN IMMEDIATE` transaction [`super::ensure`] decided it in. This
//! file holds the case, the rows' account and their lines: what is not
//! carried is logged at warn by name, once, after the commit.

use super::{when, OBJECTS};

/// The store's shape, as [`super::ensure`] decides on it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Case {
    /// Every object present, the grants with `kind`: nothing to write.
    Current,
    /// No grants table, or the new shape missing an object: the block.
    Missing,
    /// L1's shape: a grants table without `kind`; `pending` when L1's
    /// pending table is there too.
    L1 {
        /// L1's `origin_pending` is present.
        pending: bool,
    },
}

impl Case {
    /// The case of a store holding the objects `present`, `(type, name)`,
    /// whose grants table has the column `kind` when `kinded`.
    pub(super) fn of(present: &[(String, String)], kinded: bool) -> Self {
        let has = |t: &str, n: &str| present.iter().any(|(pt, pn)| pt == t && pn == n);
        if has("table", "origin_grants") && !kinded {
            Self::L1 {
                pending: has("table", "origin_pending"),
            }
        } else if OBJECTS.iter().all(|(t, n)| has(t, n)) {
            Self::Current
        } else {
            Self::Missing
        }
    }
}

/// A grant row of L1's table, as the move reads it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct L1Row {
    originator: String,
    scope: String,
    amount: Option<i64>,
    granted_at: i64,
    revoked_at: Option<i64>,
    source: String,
}

impl L1Row {
    /// Whether the spending kind takes the row: a `spending` row with its
    /// ceiling (the new `CHECK` refuses a spending row without one).
    fn carried(&self) -> bool {
        self.scope == "spending" && self.amount.is_some()
    }

    /// The row's line at the move, by name.
    fn line(&self) -> String {
        let (o, at) = (&self.originator, when(self.granted_at));
        let revoked = self
            .revoked_at
            .map_or(String::new(), |r| format!(", revoked at {}", when(r)));
        match (self.scope.as_str(), self.amount) {
            ("spending", Some(ceiling)) => format!(
                "origin grants: the L1 spending grant for {o} (granted {at}, {ceiling}/month\
                 {revoked}) is carried as the row {o} spending []"
            ),
            ("spending", None) => format!(
                "origin grants: the L1 spending grant for {o} (granted {at}{revoked}, source {}) \
                 has no ceiling and is not carried: a spending row names its month's ceiling",
                self.source
            ),
            (scope, _) => format!(
                "origin grants: the L1 {scope} grant for {o} (granted {at}{revoked}) names no \
                 power under the four kinds and is not carried; the page is asked per power \
                 from the switch on"
            ),
        }
    }
}

/// L1's grant row as read: originator, scope, amount, granted at,
/// revoked at, source.
pub(super) type L1Tuple = (String, String, Option<i64>, i64, Option<i64>, String);

/// What the move did, logged after its commit ([`Moved::log`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Moved {
    /// L1's grant rows, in their order.
    pub(super) rows: Vec<L1Row>,
    /// L1's pending rows, none carried.
    pub(super) pending: i64,
}

impl Moved {
    /// Each row's line, the pending rows' count and the move's summary,
    /// at warn, by name (`marker` `origin_grants_l1` for a row's line and
    /// the pending count, `origin_grants_moved` for the summary).
    pub(super) fn log(&self) {
        for row in &self.rows {
            tracing::warn!(
                marker = "origin_grants_l1",
                originator = %row.originator,
                "{}",
                row.line()
            );
        }
        if self.pending > 0 {
            tracing::warn!(
                marker = "origin_grants_l1",
                "origin grants: {} L1 pending requests are not carried: no server wrote one, and \
                 an answered one answers a scope that no longer exists",
                self.pending
            );
        }
        let carried = self.rows.iter().filter(|r| r.carried()).count();
        tracing::warn!(
            marker = "origin_grants_moved",
            carried,
            "origin grants: the store's L1 tables moved to the four kinds ({carried} grant rows \
             carried, {} not carried, {} pending requests not carried)",
            self.rows.len() - carried,
            self.pending
        );
    }
}

impl Moved {
    /// The move's account of L1's rows, as read before the move, and the
    /// count of its pending rows.
    pub(super) fn of(rows: Vec<L1Tuple>, pending: i64) -> Self {
        let rows = rows
            .into_iter()
            .map(
                |(originator, scope, amount, granted_at, revoked_at, source)| L1Row {
                    originator,
                    scope,
                    amount,
                    granted_at,
                    revoked_at,
                    source,
                },
            )
            .collect();
        Self { rows, pending }
    }
}
