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
    self as grants, probes, when, when_exact, Answer, GrantShown, Granting, Kind, PendingRow,
    Power, ProbeRow, Reach, Refused, Reply, Shown,
};

use crate::context::WalletContext;

/// The refusal of a spending grant without its ceiling, naming the flag.
const AMOUNT_NEEDED: &str = "the spending kind needs --amount SATS, the month's ceiling in \
                             satoshis (or --once on a pending spending request)";

/// The refusal of a direct grant that names no kind.
const KIND_NEEDED: &str = "a direct grant names its power: --kind protocol|basket|certificate|\
                           spending, with --key (the canonical text bsv-wallet origins prints) \
                           but for spending";

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
    /// Grant `originator` one power directly.
    Direct {
        /// The originator as named.
        originator: String,
        /// The power granted.
        power: Power,
        /// A spending grant's ceiling.
        amount: Option<i64>,
        /// Seconds from now; `None` never expires.
        expires: Option<i64>,
    },
}

impl Grant {
    /// The arguments of `origin-grant`, checked: a direct grant's key read
    /// by its kind's validators and refused naming its canonical text when
    /// it is not that text.
    ///
    /// # Errors
    ///
    /// `--kind`, `--key` or `--once` out of place, a direct grant without
    /// `--kind`, a key [`Power::named`] refuses, a spending grant without
    /// `--amount`, an `--expires` that is not seconds or `never`.
    pub fn read(
        target: &str,
        (kind_word, key): (Option<&str>, Option<&str>),
        amount: Option<u64>,
        expires: Option<&str>,
        once: bool,
    ) -> Result<Self> {
        let amount = amount.map(i64::try_from).transpose()?;
        let expires = expires.map(seconds).transpose()?.flatten();
        if let Some(pending_id) = pending_of(target) {
            if kind_word.is_some() || key.is_some() {
                bail!(
                    "a pending request names its own power; --kind and --key are for a direct \
                     grant"
                );
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
        let Some(word) = kind_word else {
            bail!(KIND_NEEDED);
        };
        let power = Power::named(kind(word)?, key)?;
        if power == Power::Spending && amount.is_none() {
            bail!(AMOUNT_NEEDED);
        }
        Ok(Self::Direct {
            originator: target.to_string(),
            power,
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

/// A `--kind` word as the module's kind.
///
/// # Errors
///
/// A word other than the four (clap refuses it first).
pub fn kind(word: &str) -> Result<Kind> {
    Kind::named(word)
}

/// `origin-revoke`'s reach: `--kind` and `--key` (one power), `--kind`
/// alone, or neither (every row of the originator).
///
/// # Errors
///
/// A word that is not a kind, or a key [`Power::parse`] refuses.
pub fn reach((kind_word, key): (Option<&str>, Option<&str>)) -> Result<Reach> {
    let kind = kind_word.map(kind).transpose()?;
    Ok(Reach::named(kind, key)?)
}

fn now() -> Result<i64> {
    Ok(i64::try_from(
        SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs(),
    )?)
}

/// One grant row's line (section (4)), from its one reading (#533): the
/// state (`granted`, `expired` or `revoked`), the kind and the key; a
/// spending row's ceiling and the month's tally, `at the ceiling` once
/// the tally has reached it; the grant's time and its expiry; the revoke's
/// time on a revoked row.
fn grant_line(g: &GrantShown) -> String {
    let r = &g.row;
    let mut parts = vec![
        g.state.as_str().to_string(),
        r.originator.clone(),
        format!("{:<11}", r.kind.as_str()),
        r.key.clone(),
    ];
    if let (Some(month), Some(spent)) = (&g.month, g.spent_this_month) {
        let ceiling = r
            .amount
            .map_or_else(|| "no ceiling".to_string(), |a| format!("{a}/month"));
        let reached = if g.ceiling_reached == Some(true) {
            ", at the ceiling"
        } else {
            ""
        };
        parts.push(format!("{ceiling} ({spent} spent {month}{reached})"));
    }
    let expires = if r.expiry == 0 {
        "never".to_string()
    } else {
        when(r.expiry)
    };
    parts.push(format!("since {}", when(r.granted_at)));
    parts.push(format!("expires {expires}"));
    if let Some(at) = r.revoked_at {
        parts.push(format!("revoked at {}", when(at)));
    }
    parts.join("  ")
}

/// One pending row's line (section (4)): `pending` while open, `answered`
/// with its answer under `--all`; the power asked in the prompt's words,
/// with the call's reason quoted.
fn pending_line(p: &PendingRow, asked: &str) -> String {
    let state = if p.answered_at.is_some() {
        "answered"
    } else {
        "pending"
    };
    let asked = p
        .reason
        .as_ref()
        .map_or_else(|| asked.to_string(), |r| format!("{asked} {r:?}"));
    let mut parts = vec![
        state.to_string(),
        format!("#{}", p.pending_id),
        p.raw_originator.clone(),
        p.kind.as_str().to_string(),
        p.key.clone(),
        asked,
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
    if let Some(end) = p.withdrawn_at() {
        parts.push(format!(
            "caller disconnected, withdrawn at {} unless the page asks again",
            when_exact(end)
        ));
    }
    parts.join("  ")
}

/// Print `shown`: its JSON, or one line a row.
fn print(shown: &Shown, json: bool) -> Result<()> {
    if json {
        println!("{}", serde_json::to_string_pretty(&shown.json())?);
        return Ok(());
    }
    for g in &shown.grants {
        println!("{}", grant_line(g));
    }
    for (p, asked) in &shown.pending {
        println!("{}", pending_line(p, asked));
    }
    for p in shown.probes.iter().flatten() {
        println!("{}", probe_line(p));
    }
    Ok(())
}

/// One probe row's line (#539): the door, the level-0 power when one was
/// named, the count and the first and last times; the remainder row
/// names the rows it folded.
fn probe_line(p: &ProbeRow) -> String {
    let door = if p.door == probes::REMAINDER {
        "* (older rows folded)".to_string()
    } else {
        p.power
            .as_ref()
            .map_or_else(|| p.door.clone(), |power| format!("{}  {power}", p.door))
    };
    let times = if p.count == 1 { "time" } else { "times" };
    format!(
        "probe  {}  {door}  {} {times}  first {}  last {}",
        p.originator,
        p.count,
        when(p.first_seen),
        when(p.last_seen)
    )
}

/// `bsv-wallet origins [--all]`: every grant row, the open pending rows
/// (`--all` adds the answered ones) and the probe record (#539).
///
/// # Errors
///
/// The store's error.
pub async fn list(ctx: &WalletContext, all: bool) -> Result<()> {
    let storage = ctx.wallet.storage();
    let now = now()?;
    let rows = grants::listing(storage, all, now).await?;
    let shown = Shown::listed(storage, rows, now).await?;
    print(&shown, ctx.json_output)
}

/// `bsv-wallet origin-grant`: a direct grant of one power, or the answer
/// `grant` or `once` to a pending row; prints the rows written.
///
/// # Errors
///
/// The module's [`Refused`] (the originator's form with the rule it
/// breaks, a row already answered, `once` on a row of another kind than
/// spending, an amount or expiry out of place), or the store's error.
pub async fn grant(ctx: &WalletContext, asked: &Grant) -> Result<()> {
    let storage = ctx.wallet.storage();
    let now = now()?;
    let until = |expires: Option<i64>| expires.map_or(0, |s| now.saturating_add(s));
    let (rows, pending) = match asked {
        Grant::Direct {
            originator,
            power,
            amount,
            expires,
        } => {
            let granting = Granting {
                originator,
                power,
                amount: *amount,
                expiry: until(*expires),
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
    print(&shown, ctx.json_output)
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
    let shown = Shown::of(storage, vec![], vec![answered.pending], now).await?;
    print(&shown, ctx.json_output)
}

/// `bsv-wallet origin-revoke <originator> [--kind KIND [--key KEY]]`:
/// stamps the live rows of the power, of the kind, or (neither) of every
/// kind, and prints each; nothing is deleted.
///
/// # Errors
///
/// The originator holds no such row (exit 1, naming it and the power),
/// the module's [`Refused::NotAnOriginator`], or the store's error.
pub async fn revoke(ctx: &WalletContext, originator: &str, reach: &Reach) -> Result<()> {
    let storage = ctx.wallet.storage();
    let now = now()?;
    let rows = grants::revoke(storage, originator, reach, now).await?;
    if rows.is_empty() {
        let (key, _) = grants::key_of(originator)?;
        // A seed-listed originator's admission is its ALLOWED_ORIGINS line,
        // not a row (#414 question 6): the revoke names the line.
        let seed = crate::server::AllowedOrigins::new(ctx.config.server.allowed_origins.clone())
            .ok()
            .and_then(|s| s.seeds(&key).map(str::to_string));
        if let Some(entry) = seed {
            bail!(
                "{key} holds no grant row{}; its admission is the seed's: remove its \
                 ALLOWED_ORIGINS line ({entry}) and restart",
                reach.words()
            );
        }
        bail!(
            "{key} holds no live grant row{}: nothing to revoke",
            reach.words()
        );
    }
    let shown = Shown::of(storage, rows, vec![], now).await?;
    print(&shown, ctx.json_output)
}

#[cfg(test)]
mod tests;
