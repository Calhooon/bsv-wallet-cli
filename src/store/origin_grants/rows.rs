//! The rows of the per-origin grants' tables and the readings over them
//! (#414): the words the tables store, the rows, the refusals with their
//! texts, the expiry rule and the key. No SQL here; the statements are the
//! parent module's, behind the store's boundary.

use anyhow::Result;

use super::power::Power;

/// A grant's kind: the reference's four permission kinds
/// (`WalletPermissionsManager.ts:418-423` at ts-stack `edf6e03`), each
/// row one power of its kind (the design's W1 (a)).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Kind {
    /// A protocol at security level 1 or 2 with its counterparty; a label
    /// rides it as `action label <label>`.
    Protocol,
    /// An output basket.
    Basket,
    /// A certificate type from a verifier, with the fields revealed.
    Certificate,
    /// `createAction`'s spend, under a monthly ceiling.
    Spending,
}

/// The owner's answer to a pending row.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Answer {
    /// Grant the row's power: a grant row is written.
    Grant,
    /// Admit the one spending call, writing no grant (the reference's
    /// `ephemeral`); refused on a row of another kind.
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

words!(Kind { Protocol => "protocol", Basket => "basket", Certificate => "certificate",
              Spending => "spending" });
words!(Answer { Grant => "grant", Once => "once", Deny => "deny" });

impl Kind {
    /// The four kinds, in the order the tables list them.
    pub const ALL: [Self; 4] = [
        Self::Protocol,
        Self::Basket,
        Self::Certificate,
        Self::Spending,
    ];

    /// A kind as the owner names it (`--kind`, a route's `kind`).
    ///
    /// # Errors
    ///
    /// A word that is not one of the four.
    pub fn named(word: &str) -> Result<Self> {
        Self::parse(word).map_err(|_| {
            anyhow::anyhow!("{word:?} is not a kind (protocol, basket, certificate, spending)")
        })
    }
}

/// A row of `origin_grants`: one power of one originator.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GrantRow {
    /// The row's id.
    pub grant_id: i64,
    /// The key: the hostname, lowercased, no port.
    pub originator: String,
    /// The originator as named, with its port.
    pub raw_originator: String,
    /// The power's kind.
    pub kind: Kind,
    /// The power's instance, in its canonical text ([`Power::key`]).
    pub key: String,
    /// `spending` only: the month's ceiling in satoshis.
    pub amount: Option<i64>,
    /// Unix seconds.
    pub granted_at: i64,
    /// Unix seconds; `0` never expires.
    pub expiry: i64,
    /// Unix seconds; `None` is not revoked.
    pub revoked_at: Option<i64>,
}

pub(super) type GrantTuple = (
    i64,
    String,
    String,
    String,
    String,
    Option<i64>,
    i64,
    i64,
    Option<i64>,
);

impl GrantRow {
    pub(super) fn from_tuple(t: GrantTuple) -> Result<Self> {
        Ok(Self {
            grant_id: t.0,
            originator: t.1,
            raw_originator: t.2,
            kind: Kind::parse(&t.3)?,
            key: t.4,
            amount: t.5,
            granted_at: t.6,
            expiry: t.7,
            revoked_at: t.8,
        })
    }

    /// The row's power, read back from its key.
    ///
    /// # Errors
    ///
    /// A stored key that is not its kind's canonical text.
    pub fn power(&self) -> Result<Power, Refused> {
        Power::parse(self.kind, &self.key)
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
    /// The power's kind.
    pub kind: Kind,
    /// The power asked, as the grant row would hold it.
    pub key: String,
    /// The usage of the call that wrote the row (`signing`, `insertion`,
    /// `listing`, `apply`, ...); `None` for spending.
    pub usage: Option<String>,
    /// The door's path of the call that wrote the row (`/listOutputs`).
    pub call: String,
    /// The call's own reason (its `description` or `privilegedReason`),
    /// the page's words, never the owner's.
    pub reason: Option<String>,
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
    Option<String>,
    String,
    Option<String>,
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
            kind: Kind::parse(&t.3)?,
            key: t.4,
            usage: t.5,
            call: t.6,
            reason: t.7,
            amount: t.8,
            renewal: t.9,
            first_seen: t.10,
            answered_at: t.11,
            answer: t.12.as_deref().map(Answer::parse).transpose()?,
        })
    }

    /// The prompt's text: the power asked, in the words every surface
    /// prints (section (3)). `past` is a spending renewal's ceiling and the
    /// month's tally, when the originator's spending row is known.
    #[must_use]
    pub fn asked(&self, past: Option<(i64, i64)>) -> String {
        if self.kind == Kind::Spending {
            let spend = format!("spend {} satoshis", self.amount.unwrap_or(0));
            return match past {
                Some((ceiling, spent)) if self.renewal => {
                    format!("{spend} past this month's ceiling of {ceiling} ({spent} spent)")
                }
                _ => spend,
            };
        }
        Power::parse(self.kind, &self.key).map_or_else(
            |_| format!("{} {}", self.kind.as_str(), self.key),
            |p| p.asked(self.usage.as_deref()),
        )
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
    /// The originator's row of this power is revoked: refused at once,
    /// no pending row.
    Revoked {
        /// The key.
        originator: String,
        /// The power's kind.
        kind: Kind,
        /// The power's key.
        key: String,
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
    /// `once` names a row of another kind than spending.
    OnceOnKind {
        /// The row's id.
        pending_id: i64,
        /// Its kind.
        kind: Kind,
    },
    /// A `spending` grant from the owner, or a spending call, without an
    /// amount.
    AmountRequired,
    /// A row of another kind than spending with an amount.
    AmountOnKind(Kind),
    /// A `spending` grant with an expiry.
    ExpiryOnSpending,
    /// A negative amount or expiry.
    Negative,
    /// A key text that is not its kind's shape, or a name in it that its
    /// validator refuses.
    BadKey {
        /// The kind named.
        kind: Kind,
        /// The key as given.
        key: String,
        /// Why, in the shape's or the validator's words.
        why: String,
    },
    /// A key that reads as a power but is not written in its canonical
    /// text.
    NotCanonical {
        /// The kind named.
        kind: Kind,
        /// The key as given.
        key: String,
        /// The canonical text of the power it names.
        canonical: String,
    },
    /// An admin-reserved name, in the reference's words.
    AdminOnly(String),
    /// A protocol at security level 0: open access, never a row.
    LevelZero,
    /// A grant of a kind that names its key, without one.
    KeyRequired(Kind),
    /// A key named without its kind.
    KeyWithoutKind,
}

impl Refused {
    /// The words of a key's, a kind's or an amount's refusal; `None` for
    /// the rows' and the answers' refusals ([`std::fmt::Display`]'s).
    fn power_words(&self) -> Option<String> {
        Some(match self {
            Self::AmountRequired => "a spending grant or call names its amount in satoshis".into(),
            Self::AmountOnKind(kind) => format!(
                "a {} row carries no amount; the ceiling is the spending kind's",
                kind.as_str()
            ),
            Self::ExpiryOnSpending => {
                "a spending grant never expires; its ceiling resets each UTC month".into()
            }
            Self::Negative => {
                "an amount is satoshis, zero or more; an expiry is unix seconds, 0 for never".into()
            }
            Self::BadKey { kind, key, why } => {
                format!("{key} is not a {} key: {why}", kind.as_str())
            }
            Self::NotCanonical {
                kind,
                key,
                canonical,
            } => format!(
                "the {} key {key} is not in its canonical form; write it {canonical}",
                kind.as_str()
            ),
            Self::AdminOnly(text) => text.clone(),
            Self::LevelZero => "a protocol at security level 0 is open access (BRC-100: \"Level \
                                0: No restrictions, open access\") and is never a grant row"
                .into(),
            Self::KeyRequired(kind) => format!(
                "a {} grant names its key, the canonical text bsv-wallet origins prints (a \
                 spending grant alone has none)",
                kind.as_str()
            ),
            Self::KeyWithoutKind => {
                "a key names a power of a kind: name the kind with the key".into()
            }
            _ => return None,
        })
    }
}

impl std::fmt::Display for Refused {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if let Some(words) = self.power_words() {
            return f.write_str(&words);
        }
        match self {
            Self::NotAnOriginator { name, rule } => write!(f, "{name:?} is not {rule}"),
            Self::Revoked {
                originator,
                kind,
                key,
                at,
            } => write!(
                f,
                "the grant row {originator} {} {key} was revoked by the owner at {}",
                kind.as_str(),
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
            Self::OnceOnKind { pending_id, kind } => write!(
                f,
                "pending request #{pending_id} asks for a {} power; once answers a spending \
                 request alone",
                kind.as_str()
            ),
            _ => Ok(()),
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

/// How the rows covering a power read at a time: the gate's four answers
/// (section (2)).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Standing {
    /// Admitted.
    Live(GrantRow),
    /// Revoked: refused at once, no pending row.
    Revoked(GrantRow),
    /// Past its expiry: held, as a renewal.
    Expired(GrantRow),
    /// No row: held.
    None,
}

impl Standing {
    /// The reading of `rows`, each covering the power asked, at `now`
    /// (section (1)'s lookup): a live row admits; else a revoked row
    /// refuses; else an expired row is a renewal; else none.
    #[must_use]
    pub fn of(rows: Vec<GrantRow>, now: i64) -> Self {
        let state = |r: &GrantRow| match r.revoked_at {
            Some(_) => 1,
            None if expired(r.expiry, now) => 2,
            None => 0,
        };
        let first = rows.into_iter().min_by_key(|r| (state(r), r.grant_id));
        match first {
            None => Self::None,
            Some(r) => match state(&r) {
                0 => Self::Live(r),
                1 => Self::Revoked(r),
                _ => Self::Expired(r),
            },
        }
    }
}

/// A grant as asked: the originator (any name of its host), the power,
/// its ceiling and expiry.
#[derive(Debug, Clone, Copy)]
pub struct Granting<'a> {
    /// The originator as named; the row's `raw_originator`.
    pub originator: &'a str,
    /// The power granted.
    pub power: &'a Power,
    /// `spending`: the month's ceiling (required); any other kind: `None`.
    pub amount: Option<i64>,
    /// Unix seconds, `0` never; a `spending` grant is `0`.
    pub expiry: i64,
}

impl Granting<'_> {
    /// The expiry rule and the amount's place (sections (1) and (5)).
    pub(super) fn check(&self) -> Result<(), Refused> {
        if self.expiry < 0 || self.amount.is_some_and(|a| a < 0) {
            return Err(Refused::Negative);
        }
        match self.power.kind() {
            Kind::Spending if self.expiry != 0 => Err(Refused::ExpiryOnSpending),
            Kind::Spending if self.amount.is_none() => Err(Refused::AmountRequired),
            Kind::Spending => Ok(()),
            kind if self.amount.is_some() => Err(Refused::AmountOnKind(kind)),
            _ => Ok(()),
        }
    }
}

/// A call asking for the owner's answer.
#[derive(Debug, Clone, Copy)]
pub struct Asking<'a> {
    /// The `Origin`'s originator, with its port.
    pub originator: &'a str,
    /// The power the call needs.
    pub power: &'a Power,
    /// The call's usage (`signing`, `insertion`, `listing`, `apply`, ...);
    /// `None` for spending.
    pub usage: Option<&'a str>,
    /// The door's path (`/listOutputs`).
    pub call: &'a str,
    /// The call's own reason, as the reference passes it.
    pub reason: Option<&'a str>,
    /// `spending`: the call's satoshis (required); any other kind: `None`.
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

/// What a revoke reaches of one originator's live rows (section (3)):
/// every row (the reference's `revokeAllForOriginator`), every row of a
/// kind, or one power's row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Reach {
    /// Every live row of the originator.
    Every,
    /// Every live row of the kind.
    Kind(Kind),
    /// The one power's row.
    Power(Power),
}

impl Reach {
    /// The reach the owner named: a kind and a key (one power), a kind
    /// alone, or neither.
    ///
    /// # Errors
    ///
    /// A key without a kind, or a key [`Power::parse`] refuses.
    pub fn named(kind: Option<Kind>, key: Option<&str>) -> Result<Self, Refused> {
        match (kind, key) {
            (None, None) => Ok(Self::Every),
            (None, Some(_)) => Err(Refused::KeyWithoutKind),
            (Some(kind), None) => Ok(Self::Kind(kind)),
            (Some(kind), Some(key)) => Power::parse(kind, key).map(Self::Power),
        }
    }

    /// The words naming the reach after `holds no live grant row`: none,
    /// ` of the <kind> kind`, or ` for <kind> <key>`.
    #[must_use]
    pub fn words(&self) -> String {
        match self {
            Self::Every => String::new(),
            Self::Kind(kind) => format!(" of the {} kind", kind.as_str()),
            Self::Power(p) => format!(" for {} {}", p.kind().as_str(), p.key()),
        }
    }
}
