//! The owner's door (#414, the design's L3, `docs/design/origin-grants.md`
//! section (3)): four host routes over the per-origin grant rows, never
//! BRC-100 doors and never a page's.
//!
//! - `GET /origins`: `{"grants": [...], "pending": [...]}`, every grant
//!   row and the open pending rows with the last [`ANSWERED_SHOWN`]
//!   answered ones, in section (4)'s `--json` shape ([`Shown`]);
//! - `POST /origins/answer`: `{"pendingId", "answer": "grant"|"once"|"deny",
//!   "expiry"?, "amount"?}`;
//! - `POST /origins/grant`: `{"originator", "scope", "expiry"?, "amount"?}`;
//! - `POST /origins/revoke`: `{"originator", "scope"?}`, no scope revoking
//!   both.
//!
//! Each writes through `store::origin_grants` once; an answer is
//! `UPDATE .. WHERE pending_id = ? AND answered_at IS NULL`, so of two
//! places answering one row the loser is told it was already answered
//! (409 `CONFLICT`, the module's [`Refused::AlreadyAnswered`] text). An
//! `expiry` is unix seconds as the rows hold it, `0` or absent never.
//!
//! The admission ([`refusal`]), outside the page gate, the CORS layer and
//! the preflight's gate (the served doors' layers never see these paths):
//! a call is admitted only when it carries no `Origin` header (a browser
//! sends one on every `POST`; a `GET` without one is a navigation or a
//! no-cors read the page cannot read), names a loopback `Host` (against a
//! page rebinding its name to the port), and then carries the bearer when
//! `AUTH_TOKEN` is set (`auth_middleware`, unchanged). A preflight, a call
//! with an `Origin`, and a `Host` beyond loopback are 403 `ACCESS_DENIED`
//! in the words of [`PREFLIGHT`], [`PAGE`] and [`not_loopback`]; a missing
//! or wrong bearer is the bearer's 401 `UNAUTHORIZED`. No answer carries a
//! CORS header.

use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::Result;
use axum::extract::{Request, State};
use axum::http::{header, HeaderMap, Method};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::Router;
use serde::Deserialize;

use super::error::{AppError, Kind};
use super::handlers::json_body::Json;
use super::handlers::WalletState;
use crate::store::origin_grants::{self as grants, Answer, Granting, Refused, Reply, Scope};
use crate::store::origin_grants::{Shown, Source};

/// How many answered pending rows `GET /origins` shows beside the open
/// ones (section (3)).
pub const ANSWERED_SHOWN: i64 = 50;

/// The refusal of a call carrying an `Origin` header.
pub const PAGE: &str = "the owner's routes answer no page: a call carrying an Origin header is \
                        refused (the owner calls them without one, from the command line or the \
                        MCP)";

/// The refusal of a preflight.
pub const PREFLIGHT: &str =
    "the owner's routes answer no preflight: no page reaches them, so none is answered";

/// The refusal of a `Host` beyond loopback, naming it.
#[must_use]
pub fn not_loopback(host: Option<&str>) -> String {
    match host {
        Some(host) => format!(
            "the owner's routes answer only a loopback Host (localhost, 127.0.0.1, [::1]), not \
             {host:?}"
        ),
        None => {
            "the owner's routes answer only a loopback Host, and this call names none".to_string()
        }
    }
}

/// The four routes, each behind the owner's admission and then the
/// bearer; the served doors are the fallback of the router they join.
pub(crate) fn routes() -> Router<WalletState> {
    Router::new()
        .route("/origins", get(list).options(preflight))
        .route("/origins/answer", post(answer).options(preflight))
        .route("/origins/grant", post(grant).options(preflight))
        .route("/origins/revoke", post(revoke).options(preflight))
        // The last layer added runs first: the admission, then the bearer.
        // Route layers: a path that is not one of the four is the doors'.
        .route_layer(middleware::from_fn(super::auth_middleware))
        .route_layer(middleware::from_fn(admit))
}

/// Why a call to the owner's routes is refused, or `None` when it is
/// admitted (the bearer is judged after this): a preflight, any `Origin`,
/// then a `Host` that is not loopback.
#[must_use]
pub fn refusal(method: &Method, headers: &HeaderMap) -> Option<String> {
    if method == Method::OPTIONS {
        return Some(PREFLIGHT.to_string());
    }
    if headers.contains_key(header::ORIGIN) {
        return Some(PAGE.to_string());
    }
    let host = headers.get(header::HOST).map(|v| v.to_str().unwrap_or("?"));
    match host {
        Some(name) if loopback(name) => None,
        _ => Some(not_loopback(host)),
    }
}

/// Whether a `Host` names loopback: `localhost`, or an address the bind
/// guard reads as loopback (`127.0.0.0/8`, `::1`, either mapped), with an
/// optional port and nothing else.
fn loopback(host: &str) -> bool {
    if host.bytes().any(|b| b"@/?#\\ ".contains(&b)) {
        return false;
    }
    let Ok(url) = url::Url::parse(&format!("http://{host}")) else {
        return false;
    };
    match url.host() {
        Some(url::Host::Domain(name)) => name == "localhost",
        Some(url::Host::Ipv4(ip)) => ip.is_loopback(),
        Some(url::Host::Ipv6(ip)) => ip.to_canonical().is_loopback(),
        None => false,
    }
}

/// The owner's admission, before the bearer: [`refusal`]'s 403.
async fn admit(request: Request, next: Next) -> Response {
    match refusal(request.method(), request.headers()) {
        None => next.run(request).await,
        Some(why) => {
            tracing::warn!(path = %request.uri().path(), why = %why, "owner's door: refused");
            AppError::new(Kind::AccessDenied, why).into_response()
        }
    }
}

/// A preflight's answer, should one pass the admission: refused.
async fn preflight() -> Response {
    AppError::new(Kind::AccessDenied, PREFLIGHT).into_response()
}

/// A scope as the body names it.
#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(rename_all = "lowercase")]
enum ScopeWord {
    Wallet,
    Spending,
}

impl From<ScopeWord> for Scope {
    fn from(word: ScopeWord) -> Self {
        match word {
            ScopeWord::Wallet => Scope::Wallet,
            ScopeWord::Spending => Scope::Spending,
        }
    }
}

/// An answer as the body names it.
#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(rename_all = "lowercase")]
enum AnswerWord {
    Grant,
    Once,
    Deny,
}

impl From<AnswerWord> for Answer {
    fn from(word: AnswerWord) -> Self {
        match word {
            AnswerWord::Grant => Answer::Grant,
            AnswerWord::Once => Answer::Once,
            AnswerWord::Deny => Answer::Deny,
        }
    }
}

/// `POST /origins/answer`'s body (the inspector's `POST
/// /api/origins/answer` reads the same).
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Answering {
    pending_id: i64,
    answer: AnswerWord,
    #[serde(default)]
    expiry: i64,
    amount: Option<i64>,
}

/// `POST /origins/grant`'s body.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Granted {
    originator: String,
    scope: ScopeWord,
    amount: Option<i64>,
    #[serde(default)]
    expiry: i64,
}

impl Answering {
    /// The pending row answered.
    #[must_use]
    pub fn pending_id(&self) -> i64 {
        self.pending_id
    }

    /// The answer as the module takes it.
    #[must_use]
    pub fn reply(&self) -> Reply {
        Reply {
            answer: self.answer.into(),
            expiry: self.expiry,
            amount: self.amount,
        }
    }
}

/// `POST /origins/revoke`'s body (the inspector's `POST
/// /api/origins/revoke` reads the same).
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Revoking {
    originator: String,
    scope: Option<ScopeWord>,
}

impl Revoking {
    /// The originator named.
    #[must_use]
    pub fn originator(&self) -> &str {
        &self.originator
    }

    /// The scope named; `None` revokes both.
    #[must_use]
    pub fn scope(&self) -> Option<Scope> {
        self.scope.map(Scope::from)
    }

    /// The refusal of a revoke that stamped no row, naming the originator.
    #[must_use]
    pub fn nothing_to_revoke(&self) -> String {
        let key = grants::key_of(&self.originator).map_or(self.originator.clone(), |(k, _)| k);
        let scoped = self
            .scope()
            .map_or(String::new(), |s| format!(" of the {} scope", s.as_str()));
        format!("{key} holds no live grant row{scoped}: nothing to revoke")
    }
}

/// Now, in unix seconds, as the rows hold times.
///
/// # Errors
///
/// A clock before the epoch, or past `i64`.
pub fn now() -> Result<i64> {
    Ok(i64::try_from(
        SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs(),
    )?)
}

/// A row function's error as the door answers it: the store busy by its
/// type, a row already answered (or nothing to revoke) 409 `CONFLICT`, any
/// other refusal of the module 400 `VALIDATION_ERROR` in its words, else
/// 500.
fn refused(e: &anyhow::Error) -> Response {
    if let Some(busy) = AppError::store_busy(e) {
        return busy.into_response();
    }
    let kind = match e.downcast_ref::<Refused>() {
        Some(Refused::AlreadyAnswered { .. }) => Kind::Conflict,
        Some(_) => Kind::Validation,
        None => Kind::Internal,
    };
    AppError::new(kind, e.to_string()).into_response()
}

/// The rows written, read with the month's tally, as the listing's JSON.
async fn shown(
    wallet: &WalletState,
    grants: Vec<grants::GrantRow>,
    pending: Vec<grants::PendingRow>,
    now: i64,
) -> Response {
    match Shown::of(wallet.storage(), grants, pending, now).await {
        Ok(shown) => Json(shown.json()).into_response(),
        Err(e) => refused(&e),
    }
}

/// `GET /origins`.
async fn list(State(wallet): State<WalletState>) -> Response {
    let read = async {
        let rows = grants::owners_listing(wallet.storage(), ANSWERED_SHOWN).await?;
        anyhow::Ok((rows, now()?))
    };
    match read.await {
        Ok((rows, now)) => shown(&wallet, rows.grants, rows.pending, now).await,
        Err(e) => refused(&e),
    }
}

/// `POST /origins/answer`: the answered row, and the grant row a `grant`
/// wrote.
async fn answer(State(wallet): State<WalletState>, Json(body): Json<Answering>) -> Response {
    let reply = body.reply();
    let written = async {
        let now = now()?;
        let answered = grants::answer(wallet.storage(), body.pending_id, &reply, now).await?;
        anyhow::Ok((answered, now))
    };
    match written.await {
        Ok((answered, now)) => {
            tracing::info!(
                pending_id = body.pending_id,
                answer = reply.answer.as_str(),
                "owner's door: answered"
            );
            let grants = answered.grant.into_iter().collect();
            shown(&wallet, grants, vec![answered.pending], now).await
        }
        Err(e) => refused(&e),
    }
}

/// `POST /origins/grant`: the owner's direct grant (`source` `owner`).
async fn grant(State(wallet): State<WalletState>, Json(body): Json<Granted>) -> Response {
    let granting = Granting {
        originator: &body.originator,
        scope: body.scope.into(),
        amount: body.amount,
        expiry: body.expiry,
        source: Source::Owner,
    };
    let written = async {
        let now = now()?;
        anyhow::Ok((grants::grant(wallet.storage(), &granting, now).await?, now))
    };
    match written.await {
        Ok((row, now)) => {
            tracing::info!(originator = %row.originator, scope = row.scope.as_str(), "owner's door: granted");
            shown(&wallet, vec![row], vec![], now).await
        }
        Err(e) => refused(&e),
    }
}

/// `POST /origins/revoke`: the rows stamped; 409 naming the originator
/// when it holds no live row (the CLI's exit 1).
async fn revoke(State(wallet): State<WalletState>, Json(body): Json<Revoking>) -> Response {
    let written = async {
        let now = now()?;
        let rows = grants::revoke(wallet.storage(), &body.originator, body.scope(), now).await?;
        anyhow::Ok((rows, now))
    };
    match written.await {
        Ok((rows, _)) if rows.is_empty() => {
            AppError::new(Kind::Conflict, body.nothing_to_revoke()).into_response()
        }
        Ok((rows, now)) => {
            tracing::info!(originator = %body.originator, rows = rows.len(), "owner's door: revoked");
            shown(&wallet, rows, vec![], now).await
        }
        Err(e) => refused(&e),
    }
}
