//! `origins`, `origin-grant`, `origin-deny` and `origin-revoke` (#414, the
//! design's L2, `docs/design/origin-grants.md` section (4)): the owner's
//! listing and answers over the per-origin grant rows, written through the
//! store's module `origin_grants` and nothing else. The served doors still
//! admit by `ALLOWED_ORIGINS` (the design's L5 is the switch); a running
//! server's held calls, once it reads these rows, see an answer at their
//! next read.

use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{bail, Result};
use bsv_wallet_cli::store::origin_grants::{
    self as grants, expired, month_of, when, Answer, GrantRow, Granting, PendingRow, Refused,
    Reply, Scope, Source,
};
use bsv_wallet_toolbox::StorageSqlx;
use serde::Serialize;

use crate::context::WalletContext;

/// The refusal of a spending grant without its ceiling, naming the flag.
const AMOUNT_NEEDED: &str = "the spending scope needs --amount SATS, the month's ceiling in \
                             satoshis (or --once on a pending spending request)";

/// What `origin-grant` was asked, read from its arguments before the
/// wallet opens.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Grant {
    /// Answer pending row `pending_id`: `grant`, or `once` alone.
    Answer {
        /// The row's id.
        pending_id: i64,
        /// `once`: admit the one spending call, no grant written.
        once: bool,
        /// A spending grant's ceiling.
        amount: Option<i64>,
        /// Seconds from now; `None` never expires.
        expires: Option<i64>,
    },
    /// Grant `originator` directly.
    Direct {
        /// The originator as named.
        originator: String,
        /// The scope granted.
        scope: Scope,
        /// A spending grant's ceiling.
        amount: Option<i64>,
        /// Seconds from now; `None` never expires.
        expires: Option<i64>,
    },
}

impl Grant {
    /// The arguments of `origin-grant`, checked.
    ///
    /// # Errors
    ///
    /// `--scope` or `--once` out of place, a spending grant without
    /// `--amount`, an `--expires` that is not seconds or `never`.
    pub fn read(
        target: &str,
        scope_word: Option<&str>,
        amount: Option<u64>,
        expires: Option<&str>,
        once: bool,
    ) -> Result<Self> {
        let amount = amount.map(i64::try_from).transpose()?;
        let expires = expires.map(seconds).transpose()?.flatten();
        if let Some(pending_id) = pending_of(target) {
            if scope_word.is_some() {
                bail!("a pending request names its own scope; --scope is for a direct grant");
            }
            return Ok(Self::Answer {
                pending_id: pending_id?,
                once,
                amount,
                expires,
            });
        }
        if once {
            bail!("--once answers a pending spending request (#7); a direct grant is never once");
        }
        let scope = scope_word.map_or(Ok(Scope::Wallet), scope)?;
        if scope == Scope::Spending && amount.is_none() {
            bail!(AMOUNT_NEEDED);
        }
        Ok(Self::Direct {
            originator: target.to_string(),
            scope,
            amount,
            expires,
        })
    }
}

/// `--expires`: `never` is `None`, else whole seconds from now, 1 or more.
fn seconds(word: &str) -> Result<Option<i64>> {
    if word == "never" {
        return Ok(None);
    }
    match word.parse::<i64>() {
        Ok(s) if s > 0 => Ok(Some(s)),
        _ => bail!("--expires takes seconds from now (1 or more) or never, not {word:?}"),
    }
}

/// A pending row's id as named (`#7`, or `7`); `None` when the word is
/// not one (an originator).
fn pending_of(word: &str) -> Option<Result<i64>> {
    let (marked, digits) = match word.split_at_checked(1) {
        Some(("#", rest)) => (true, rest),
        _ => (false, word),
    };
    let numeric = !digits.is_empty() && digits.bytes().all(|b| b.is_ascii_digit());
    (marked || numeric).then(|| {
        digits
            .parse::<i64>()
            .map_err(|_| anyhow::anyhow!("{word:?} is not a pending request's id (#7)"))
    })
}

/// `origin-deny`'s argument: a pending row's id (`#7`, or `7`).
///
/// # Errors
///
/// A word that is not one.
pub fn pending_id(word: &str) -> Result<i64> {
    pending_of(word).unwrap_or_else(|| bail!("{word:?} is not a pending request's id (#7)"))
}

/// A `--scope` word as the module's scope.
///
/// # Errors
///
/// A word other than `wallet` or `spending` (clap refuses it first).
pub fn scope(word: &str) -> Result<Scope> {
    match word {
        "wallet" => Ok(Scope::Wallet),
        "spending" => Ok(Scope::Spending),
        other => bail!("{other:?} is not a scope (wallet, spending)"),
    }
}

fn now() -> Result<i64> {
    Ok(i64::try_from(
        SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs(),
    )?)
}

/// One grant row's line (section (4)): `granted`, `expired` or
/// `revoked`; a spending row with its ceiling and the month's tally.
fn grant_line(g: &GrantRow, spent: Option<i64>, now: i64) -> String {
    let head = |state: &str| format!("{state}  {}  {:<8}", g.originator, g.scope.as_str());
    if let Some(at) = g.revoked_at {
        return format!("{}  at {}", head("revoked"), when(at));
    }
    let state = if expired(g.expiry, now) {
        "expired"
    } else {
        "granted"
    };
    let middle = match g.scope {
        Scope::Wallet => format!("since {}", when(g.granted_at)),
        Scope::Spending => {
            let ceiling = g
                .amount
                .map_or_else(|| "no ceiling".to_string(), |a| format!("{a}/month"));
            let month = month_of(now).unwrap_or_default();
            format!("{ceiling} ({} spent {month})", spent.unwrap_or(0))
        }
    };
    let expires = if g.expiry == 0 {
        "never".to_string()
    } else {
        when(g.expiry)
    };
    format!(
        "{}  {middle}  expires {expires}  source {}",
        head(state),
        g.source.as_str()
    )
}

/// One pending row's line (section (4)): `pending` while open, `answered`
/// with its answer under `--all`.
fn pending_line(p: &PendingRow) -> String {
    let state = if p.answered_at.is_some() {
        "answered"
    } else {
        "pending"
    };
    let mut parts = vec![
        state.to_string(),
        format!("#{}", p.pending_id),
        p.raw_originator.clone(),
        p.scope.as_str().to_string(),
        p.call.clone(),
    ];
    if let Some(a) = p.amount {
        parts.push(format!("{a} sat"));
    }
    if p.renewal {
        parts.push("renewal".to_string());
    }
    parts.push(format!("seen {}", when(p.first_seen)));
    if let (Some(at), Some(answer)) = (p.answered_at, p.answer) {
        parts.push(format!("{} at {}", answer.as_str(), when(at)));
    }
    parts.join("  ")
}

/// A grant row in `--json`, the keys of section (4) in its order.
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

/// A pending row in `--json`, the keys of section (4) in its order.
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

#[derive(Serialize)]
struct ListingJson<'a> {
    grants: Vec<GrantJson<'a>>,
    pending: Vec<PendingJson<'a>>,
}

/// Rows to print: grant rows with the month's tally of a spending row,
/// and pending rows.
#[derive(Default)]
struct Shown {
    grants: Vec<(GrantRow, Option<i64>)>,
    pending: Vec<PendingRow>,
}

impl Shown {
    /// `grants` with the month's tally read for each spending row.
    async fn of(
        storage: &StorageSqlx,
        rows: Vec<GrantRow>,
        pending: Vec<PendingRow>,
        now: i64,
    ) -> Result<Self> {
        let mut grants = Vec::with_capacity(rows.len());
        for g in rows {
            let spent = match g.scope {
                Scope::Spending => Some(grants::month_tally(storage, &g.originator, now).await?),
                Scope::Wallet => None,
            };
            grants.push((g, spent));
        }
        Ok(Self { grants, pending })
    }

    fn json(&self) -> ListingJson<'_> {
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
        ListingJson {
            grants: grants.collect(),
            pending: pending.collect(),
        }
    }

    fn print(&self, json: bool, now: i64) -> Result<()> {
        if json {
            println!("{}", serde_json::to_string_pretty(&self.json())?);
            return Ok(());
        }
        for (g, spent) in &self.grants {
            println!("{}", grant_line(g, *spent, now));
        }
        for p in &self.pending {
            println!("{}", pending_line(p));
        }
        Ok(())
    }
}

/// `bsv-wallet origins [--all]`: every grant row and the open pending
/// rows (`--all` adds the answered ones).
///
/// # Errors
///
/// The store's error.
pub async fn list(ctx: &WalletContext, all: bool) -> Result<()> {
    let storage = ctx.wallet.storage();
    let now = now()?;
    let rows = grants::listing(storage, all).await?;
    let shown = Shown::of(storage, rows.grants, rows.pending, now).await?;
    shown.print(ctx.json_output, now)
}

/// `bsv-wallet origin-grant`: a direct grant (`source` `owner`), or the
/// answer `grant` or `once` to a pending row; prints the rows written.
///
/// # Errors
///
/// The module's [`Refused`] (the originator's form with the rule it
/// breaks, a row already answered, `once` on a wallet row, an amount or
/// expiry out of place), or the store's error.
pub async fn grant(ctx: &WalletContext, asked: &Grant) -> Result<()> {
    let storage = ctx.wallet.storage();
    let now = now()?;
    let until = |expires: Option<i64>| expires.map_or(0, |s| now.saturating_add(s));
    let (rows, pending) = match asked {
        Grant::Direct {
            originator,
            scope,
            amount,
            expires,
        } => {
            let granting = Granting {
                originator,
                scope: *scope,
                amount: *amount,
                expiry: until(*expires),
                source: Source::Owner,
            };
            (vec![grants::grant(storage, &granting, now).await?], vec![])
        }
        Grant::Answer {
            pending_id,
            once,
            amount,
            expires,
        } => {
            let reply = Reply {
                answer: if *once { Answer::Once } else { Answer::Grant },
                expiry: until(*expires),
                amount: *amount,
            };
            let answered = grants::answer(storage, *pending_id, &reply, now)
                .await
                .map_err(named_amount)?;
            (answered.grant.into_iter().collect(), vec![answered.pending])
        }
    };
    let shown = Shown::of(storage, rows, pending, now).await?;
    shown.print(ctx.json_output, now)
}

/// The module's refusal of a spending grant without its amount, in the
/// flag's words; any other error as it is.
fn named_amount(e: anyhow::Error) -> anyhow::Error {
    match e.downcast_ref::<Refused>() {
        Some(Refused::AmountRequired) => anyhow::anyhow!(AMOUNT_NEEDED),
        _ => e,
    }
}

/// `bsv-wallet origin-deny <#pending-id>`: answers `deny`; prints the row.
///
/// # Errors
///
/// The module's [`Refused`] (no such row, a row already answered), or the
/// store's error.
pub async fn deny(ctx: &WalletContext, pending_id: i64) -> Result<()> {
    let storage = ctx.wallet.storage();
    let now = now()?;
    let reply = Reply {
        answer: Answer::Deny,
        expiry: 0,
        amount: None,
    };
    let answered = grants::answer(storage, pending_id, &reply, now).await?;
    let shown = Shown {
        pending: vec![answered.pending],
        ..Shown::default()
    };
    shown.print(ctx.json_output, now)
}

/// `bsv-wallet origin-revoke <originator> [--scope]`: stamps the live
/// rows (both scopes, or `scope`) and prints each; nothing is deleted.
///
/// # Errors
///
/// The originator holds no such row (exit 1, naming it), the module's
/// [`Refused::NotAnOriginator`], or the store's error.
pub async fn revoke(ctx: &WalletContext, originator: &str, scope: Option<Scope>) -> Result<()> {
    let storage = ctx.wallet.storage();
    let now = now()?;
    let rows = grants::revoke(storage, originator, scope, now).await?;
    if rows.is_empty() {
        let (key, _) = grants::key_of(originator)?;
        let scoped = scope.map_or(String::new(), |s| format!(" of the {} scope", s.as_str()));
        bail!("{key} holds no live grant row{scoped}: nothing to revoke");
    }
    let shown = Shown::of(storage, rows, vec![], now).await?;
    shown.print(ctx.json_output, now)
}

#[cfg(test)]
mod tests;
