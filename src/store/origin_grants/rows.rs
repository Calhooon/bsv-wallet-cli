//! The rows of the per-origin grants' tables and the readings over them
//! (#414): the words the tables store, the rows, the refusals with their
//! texts, the expiry rule and the key. No SQL here; the statements are the
//! parent module's, behind the store's boundary.

use anyhow::Result;

/// The notice a fold logs when the store holds an env row (section (6)).
pub const SEED_NOTICE: &str = "ALLOWED_ORIGINS is a seed: the grant rows in the store are the \
                               gate; list them with bsv-wallet origins";

/// A grant's scope: `wallet` admits the served doors, `spending` holds
/// `createAction`'s month's ceiling.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scope {
    /// Every served door (the design's W1).
    Wallet,
    /// `createAction`'s spend, under a monthly ceiling.
    Spending,
}

/// Who wrote a grant row.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    /// The owner, by the CLI, the MCP or the inspector.
    Owner,
    /// The fold of an `ALLOWED_ORIGINS` entry ([`super::fold`]).
    Env,
}

/// The owner's answer to a pending row.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Answer {
    /// Grant the row's scope: a grant row is written.
    Grant,
    /// Admit the one spending call, writing no grant (the reference's
    /// `ephemeral`); refused on a `wallet` row.
    Once,
    /// Deny the call.
    Deny,
}

macro_rules! words {
    ($t:ty { $($v:ident => $w:literal),+ }) => {
        impl $t {
            /// The word the tables store.
            #[must_use]
            pub fn as_str(self) -> &'static str {
                match self { $(Self::$v => $w),+ }
            }

            pub(super) fn parse(word: &str) -> Result<Self> {
                match word {
                    $($w => Ok(Self::$v),)+
                    other => anyhow::bail!("{other:?} is not a stored {}", stringify!($t)),
                }
            }
        }
    };
}

words!(Scope { Wallet => "wallet", Spending => "spending" });
words!(Source { Owner => "owner", Env => "env" });
words!(Answer { Grant => "grant", Once => "once", Deny => "deny" });

/// A row of `origin_grants`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GrantRow {
    /// The row's id.
    pub grant_id: i64,
    /// The key: the hostname, lowercased, no port.
    pub originator: String,
    /// The originator as named, with its port.
    pub raw_originator: String,
    /// The row's scope.
    pub scope: Scope,
    /// `spending` only: the month's ceiling in satoshis; `None` is no
    /// ceiling (env rows only).
    pub amount: Option<i64>,
    /// Unix seconds.
    pub granted_at: i64,
    /// Unix seconds; `0` never expires.
    pub expiry: i64,
    /// Unix seconds; `None` is not revoked.
    pub revoked_at: Option<i64>,
    /// Who wrote it.
    pub source: Source,
}

pub(super) type GrantTuple = (
    i64,
    String,
    String,
    String,
    Option<i64>,
    i64,
    i64,
    Option<i64>,
    String,
);

impl GrantRow {
    pub(super) fn from_tuple(t: GrantTuple) -> Result<Self> {
        Ok(Self {
            grant_id: t.0,
            originator: t.1,
            raw_originator: t.2,
            scope: Scope::parse(&t.3)?,
            amount: t.4,
            granted_at: t.5,
            expiry: t.6,
            revoked_at: t.7,
            source: Source::parse(&t.8)?,
        })
    }
}

/// A row of `origin_pending`: an open row is a prompt.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PendingRow {
    /// The row's id.
    pub pending_id: i64,
    /// The key: the hostname, lowercased, no port.
    pub originator: String,
    /// The originator with its port (the reference's `displayOriginator`).
    pub raw_originator: String,
    /// The scope asked for.
    pub scope: Scope,
    /// The door's path of the call that wrote the row (`/listOutputs`).
    pub call: String,
    /// `spending` only: the call's satoshis.
    pub amount: Option<i64>,
    /// An expired grant, or a ceiling the call would pass.
    pub renewal: bool,
    /// Unix seconds.
    pub first_seen: i64,
    /// Unix seconds; `None` is open.
    pub answered_at: Option<i64>,
    /// The owner's answer, once given.
    pub answer: Option<Answer>,
}

pub(super) type PendingTuple = (
    i64,
    String,
    String,
    String,
    String,
    Option<i64>,
    bool,
    i64,
    Option<i64>,
    Option<String>,
);

impl PendingRow {
    pub(super) fn from_tuple(t: PendingTuple) -> Result<Self> {
        Ok(Self {
            pending_id: t.0,
            originator: t.1,
            raw_originator: t.2,
            scope: Scope::parse(&t.3)?,
            call: t.4,
            amount: t.5,
            renewal: t.6,
            first_seen: t.7,
            answered_at: t.8,
            answer: t.9.as_deref().map(Answer::parse).transpose()?,
        })
    }
}

/// The rows `bsv-wallet origins` lists ([`super::listing`]).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Listing {
    /// Every grant row, live, expired or revoked.
    pub grants: Vec<GrantRow>,
    /// The pending rows asked for: the open ones, or every one.
    pub pending: Vec<PendingRow>,
}

/// A row function's refusal, told apart by its type
/// (`e.downcast_ref::<Refused>()`); its text is the one served or printed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Refused {
    /// The name is not an originator in the reference's form.
    NotAnOriginator {
        /// The name as given.
        name: String,
        /// The rule it breaks (`crate::config::originator::checked`).
        rule: &'static str,
    },
    /// The originator's row of this scope is revoked: refused at once,
    /// no pending row.
    Revoked {
        /// The key.
        originator: String,
        /// The revoke's unix seconds.
        at: i64,
    },
    /// No pending row has this id.
    NoSuchPending(i64),
    /// The row was answered before: the second answer is refused.
    AlreadyAnswered {
        /// The row's id.
        pending_id: i64,
        /// The first answer.
        answer: Option<Answer>,
        /// Its unix seconds.
        at: i64,
    },
    /// `once` names a `wallet` row.
    OnceOnWallet(i64),
    /// A `spending` grant from the owner, or a spending call, without an
    /// amount.
    AmountRequired,
    /// A `wallet` row with an amount.
    AmountOnWallet,
    /// A `spending` grant with an expiry.
    ExpiryOnSpending,
    /// A negative amount or expiry.
    Negative,
}

impl std::fmt::Display for Refused {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotAnOriginator { name, rule } => write!(f, "{name:?} is not {rule}"),
            Self::Revoked { originator, at } => write!(
                f,
                "the grant row {originator} was revoked by the owner at {}",
                when(*at)
            ),
            Self::NoSuchPending(id) => write!(f, "no pending request #{id}"),
            Self::AlreadyAnswered {
                pending_id,
                answer,
                at,
            } => write!(
                f,
                "pending request #{pending_id} was already answered {} at {}",
                answer.map_or("", Answer::as_str),
                when(*at)
            ),
            Self::OnceOnWallet(id) => write!(
                f,
                "pending request #{id} asks for the wallet scope; once answers a spending \
                 request alone"
            ),
            Self::AmountRequired => {
                f.write_str("a spending grant or call names its amount in satoshis")
            }
            Self::AmountOnWallet => {
                f.write_str("a wallet row carries no amount; the ceiling is the spending scope's")
            }
            Self::ExpiryOnSpending => {
                f.write_str("a spending grant never expires; its ceiling resets each UTC month")
            }
            Self::Negative => f.write_str(
                "an amount is satoshis, zero or more; an expiry is unix seconds, 0 for never",
            ),
        }
    }
}

impl std::error::Error for Refused {}

/// Unix seconds as the CLI prints them (`2026-11-02T19:31Z`, UTC); a time
/// outside chrono's range prints as its number.
#[must_use]
pub fn when(at: i64) -> String {
    chrono::DateTime::from_timestamp(at, 0).map_or_else(
        || at.to_string(),
        |t| t.format("%Y-%m-%dT%H:%MZ").to_string(),
    )
}

/// `expiry > 0 && expiry < now`: `0` never expires, and a row is live up
/// to and including its expiry's second (the reference's reading,
/// `WalletPermissionsManager.ts:2900-2903`).
#[must_use]
pub fn expired(expiry: i64, now: i64) -> bool {
    expiry > 0 && expiry < now
}

/// The calendar month of `at` in UTC, `YYYY-MM` (the reference's
/// `getCurrentMonthYearUTC`): the key of the month's tally.
///
/// # Errors
///
/// A time outside chrono's range.
pub fn month_of(at: i64) -> Result<String> {
    chrono::DateTime::from_timestamp(at, 0)
        .map(|t| t.format("%Y-%m").to_string())
        .ok_or_else(|| anyhow::anyhow!("{at} is not a time"))
}

/// The key of `name` and the name as kept: `name` checked by PR 419's rule
/// and lowercased, and its hostname with the port dropped.
///
/// # Errors
///
/// [`Refused::NotAnOriginator`].
pub fn key_of(name: &str) -> Result<(String, String)> {
    let refused = |rule| Refused::NotAnOriginator {
        name: name.to_string(),
        rule,
    };
    let raw = crate::config::originator::checked(name).map_err(refused)?;
    let host = url::Url::parse(&format!("http://{raw}"))
        .ok()
        .and_then(|u| u.host_str().map(str::to_ascii_lowercase));
    let host =
        host.ok_or_else(|| refused("an originator: a DNS hostname, `localhost` or an IP address"))?;
    Ok((host, raw))
}

/// How a grant row reads at a time: the gate's four answers (section (2)).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Standing {
    /// Admitted.
    Live(GrantRow),
    /// Revoked: refused at once, no pending row. Read before the expiry.
    Revoked(GrantRow),
    /// Past its expiry: held, as a renewal.
    Expired(GrantRow),
    /// No row: held.
    None,
}

impl Standing {
    /// The reading of `row` at `now`.
    #[must_use]
    pub fn of(row: Option<GrantRow>, now: i64) -> Self {
        match row {
            None => Self::None,
            Some(r) if r.revoked_at.is_some() => Self::Revoked(r),
            Some(r) if expired(r.expiry, now) => Self::Expired(r),
            Some(r) => Self::Live(r),
        }
    }
}

/// A grant as asked: the originator (any name of its host), its scope,
/// ceiling, expiry and source.
#[derive(Debug, Clone, Copy)]
pub struct Granting<'a> {
    /// The originator as named; the row's `raw_originator`.
    pub originator: &'a str,
    /// The scope granted.
    pub scope: Scope,
    /// `spending`: the month's ceiling (required of the owner); `wallet`:
    /// `None`.
    pub amount: Option<i64>,
    /// Unix seconds, `0` never; a `spending` grant is `0`.
    pub expiry: i64,
    /// Who grants.
    pub source: Source,
}

impl Granting<'_> {
    /// The expiry rule and the amount's place (sections (1) and (5)).
    pub(super) fn check(&self) -> Result<(), Refused> {
        if self.expiry < 0 || self.amount.is_some_and(|a| a < 0) {
            return Err(Refused::Negative);
        }
        match self.scope {
            Scope::Wallet if self.amount.is_some() => Err(Refused::AmountOnWallet),
            Scope::Spending if self.expiry != 0 => Err(Refused::ExpiryOnSpending),
            Scope::Spending if self.amount.is_none() && self.source == Source::Owner => {
                Err(Refused::AmountRequired)
            }
            _ => Ok(()),
        }
    }
}

/// A call asking for the owner's answer.
#[derive(Debug, Clone, Copy)]
pub struct Asking<'a> {
    /// The `Origin`'s originator, with its port.
    pub originator: &'a str,
    /// The scope the call needs.
    pub scope: Scope,
    /// The door's path (`/listOutputs`).
    pub call: &'a str,
    /// `spending`: the call's satoshis (required); `wallet`: `None`.
    pub amount: Option<i64>,
    /// An expired grant, or a ceiling the call would pass.
    pub renewal: bool,
}

/// A pending row opened or joined.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pending {
    /// The row the call waits on.
    pub row: PendingRow,
    /// The call joined an open row and wrote nothing.
    pub joined: bool,
}

/// The owner's reply to a pending row.
#[derive(Debug, Clone, Copy)]
pub struct Reply {
    /// The answer.
    pub answer: Answer,
    /// A `grant`'s expiry, unix seconds, `0` never (a `spending` grant is
    /// `0`); unread by `once` and `deny`.
    pub expiry: i64,
    /// A `spending` `grant`'s ceiling (required); unread by `once` and
    /// `deny`.
    pub amount: Option<i64>,
}

/// A pending row answered.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Answered {
    /// The row, with its answer stamped.
    pub pending: PendingRow,
    /// `grant`: the grant row written.
    pub grant: Option<GrantRow>,
}

/// What the fold found for one `ALLOWED_ORIGINS` entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Folded {
    /// No `wallet` row: the `wallet` row and a ceiling-free `spending`
    /// row were written, `source` `env`.
    Granted,
    /// A live row, left as it is.
    AlreadyGranted,
    /// A row past its expiry, left as it is: the entry does not renew it.
    Expired(i64),
    /// A revoked row, left as it is: the entry never grants it again.
    Revoked(i64),
}

/// One entry's line of the fold.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FoldLine {
    /// The entry as listed (checked, lowercased).
    pub entry: String,
    /// Its key.
    pub originator: String,
    /// What the fold found.
    pub folded: Folded,
}

impl std::fmt::Display for FoldLine {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let (entry, key) = (&self.entry, &self.originator);
        write!(f, "origin grants: ALLOWED_ORIGINS entry {entry} ")?;
        match self.folded {
            Folded::Granted => write!(
                f,
                "is now the grant row {key} (wallet, spending without a ceiling; source env)"
            ),
            Folded::AlreadyGranted => write!(f, "names the grant row {key}: already granted"),
            Folded::Expired(at) => write!(
                f,
                "names the grant row {key}: expired at {}; the env entry does not renew it",
                when(at)
            ),
            Folded::Revoked(at) => write!(
                f,
                "names the grant row {key}: revoked by the owner at {}; the env entry does not \
                 grant it again",
                when(at)
            ),
        }
    }
}

/// The fold's account: one line per entry, the env rows no entry names,
/// and whether the store holds an env row (the seed notice is logged).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Fold {
    /// One line per entry, in the list's order.
    pub lines: Vec<FoldLine>,
    /// The keys of env rows not revoked whose entry left the list: they
    /// stay granted until the owner revokes them.
    pub unlisted: Vec<String>,
    /// The store holds an env row: [`SEED_NOTICE`] was logged.
    pub seeded: bool,
}

impl Fold {
    /// Log the fold at info by name: each entry's line, each unlisted env
    /// row, and [`SEED_NOTICE`] when the store holds an env row.
    pub fn log(&self) {
        for line in &self.lines {
            tracing::info!(originator = %line.originator, "{line}");
        }
        for key in &self.unlisted {
            tracing::info!(originator = %key, "{}", unlisted_text(key));
        }
        if self.seeded {
            tracing::info!("{SEED_NOTICE}");
        }
    }
}

/// The text logged for an env row whose entry left the list.
#[must_use]
pub fn unlisted_text(originator: &str) -> String {
    format!(
        "origin grants: the grant row {originator} is source env, no ALLOWED_ORIGINS entry; \
         revoke it with bsv-wallet origin-revoke {originator}"
    )
}
