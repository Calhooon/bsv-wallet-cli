//! The inspector's origins view (#414, the design's L4,
//! `docs/design/origin-grants.md` section (3), its inspector paragraph):
//! the open pending rows and the grant rows of the store, answered and
//! revoked from the page at `/origins`, over the same rows the owner's
//! door and the CLI read.
//!
//! - `GET /api/origins`: `{"grants": [...], "pending": [...]}`, every
//!   grant row and the open pending rows with the last
//!   [`ANSWERED_SHOWN`] answered ones, section (4)'s JSON ([`Shown`]), as
//!   the owner's `GET /origins` answers it;
//! - `POST /api/origins/answer`: `{"pendingId", "answer":
//!   "grant"|"once"|"deny", "expiry"?, "amount"?}`;
//! - `POST /api/origins/revoke`: `{"originator", "scope"?}`, no scope
//!   revoking both.
//!
//! Each write goes through `store::origin_grants` once, on the
//! inspector's own store handle; an answer is `UPDATE .. WHERE pending_id
//! = ? AND answered_at IS NULL`, so of two places answering one row the
//! loser is told it was already answered (409, the module's
//! [`Refused::AlreadyAnswered`] text).
//!
//! The read route follows the inspector's admission (`inspector_word`).
//! The two write routes admit only the inspector's own page ([`own_page`]):
//! an `Origin` of `http://` and the `Host` it was asked by, never an origin
//! `ALLOWED_ORIGINS` lists or a grant admits, since a granted page must not
//! write a grant.

use axum::{
    extract::{Request, State},
    http::{header, HeaderMap, StatusCode},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use bsv_wallet_cli::server::owner_door::{now, Answering, Revoking, ANSWERED_SHOWN};
use bsv_wallet_cli::store::origin_grants::{self as grants, GrantRow, PendingRow, Refused, Shown};
use std::sync::Arc;

use super::AppState;

/// The refusal of a write from any page but the inspector's own, naming
/// the origin and the rule.
#[must_use]
pub fn not_own_page(origin: Option<&str>, host: &str) -> String {
    let named = origin.map_or("no Origin".to_string(), |o| format!("origin {o}"));
    format!(
        "the inspector's origin answers and revokes are its own page's alone (http://{host}): \
         {named} is refused, whether ALLOWED_ORIGINS lists it or a grant admits it, since a \
         granted page must not write a grant"
    )
}

/// The view's three routes; the two writes behind [`own_page`].
pub(super) fn routes() -> Router<Arc<AppState>> {
    let writes = Router::new()
        .route("/api/origins/answer", post(answer))
        .route("/api/origins/revoke", post(revoke))
        .route_layer(middleware::from_fn(own_page));
    Router::new().route("/api/origins", get(list)).merge(writes)
}

/// Why a write is refused, or `None` when its `Origin` is exactly the
/// inspector's own: `http://` and the `Host` it was asked by.
#[must_use]
pub fn write_refusal(headers: &HeaderMap) -> Option<String> {
    let host = headers
        .get(header::HOST)
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default();
    let origin = headers
        .get(header::ORIGIN)
        .map(|v| v.to_str().unwrap_or("?"));
    match origin {
        Some(o) if !host.is_empty() && o == format!("http://{host}") => None,
        _ => Some(not_own_page(origin, host)),
    }
}

/// The own-page admission of the write routes: [`write_refusal`]'s 403.
async fn own_page(request: Request, next: Next) -> Response {
    let Some(why) = write_refusal(request.headers()) else {
        return next.run(request).await;
    };
    tracing::warn!(path = %request.uri().path(), why = %why, "inspector: write refused");
    refusal(StatusCode::FORBIDDEN, &why)
}

/// The inspector's error shape, `{"error": ...}`, at `status`.
fn refusal(status: StatusCode, why: &str) -> Response {
    (status, Json(serde_json::json!({ "error": why }))).into_response()
}

/// A row function's error as the inspector answers it: a row already
/// answered 409, any other refusal of the module 400 in its words, the
/// store busy 503, else 500.
fn refused(e: &anyhow::Error) -> Response {
    let status = match e.downcast_ref::<Refused>() {
        Some(Refused::AlreadyAnswered { .. }) => StatusCode::CONFLICT,
        Some(_) => StatusCode::BAD_REQUEST,
        None if bsv_wallet_cli::store::is_store_busy(e) => StatusCode::SERVICE_UNAVAILABLE,
        None => StatusCode::INTERNAL_SERVER_ERROR,
    };
    refusal(status, &e.to_string())
}

/// The rows, read with the month's tally, as the listing's JSON.
async fn shown(state: &AppState, rows: Vec<GrantRow>, pending: Vec<PendingRow>) -> Response {
    let read = async { Shown::of(&state.store, rows, pending, now()?).await };
    read.await
        .map_or_else(|e| refused(&e), |rows| Json(rows.json()).into_response())
}

/// `GET /api/origins`.
async fn list(State(state): State<Arc<AppState>>) -> Response {
    match grants::owners_listing(&state.store, ANSWERED_SHOWN).await {
        Ok(rows) => shown(&state, rows.grants, rows.pending).await,
        Err(e) => refused(&e),
    }
}

/// `POST /api/origins/answer`: the answered row, and the grant row a
/// `grant` wrote.
async fn answer(State(state): State<Arc<AppState>>, Json(body): Json<Answering>) -> Response {
    let reply = body.reply();
    let written = async { grants::answer(&state.store, body.pending_id(), &reply, now()?).await };
    match written.await {
        Ok(answered) => {
            tracing::info!(
                pending_id = body.pending_id(),
                answer = reply.answer.as_str(),
                "inspector: answered"
            );
            let rows = answered.grant.into_iter().collect();
            shown(&state, rows, vec![answered.pending]).await
        }
        Err(e) => refused(&e),
    }
}

/// `POST /api/origins/revoke`: the rows stamped; 409 naming the
/// originator when it holds no live row.
async fn revoke(State(state): State<Arc<AppState>>, Json(body): Json<Revoking>) -> Response {
    let originator = body.originator();
    let written = async { grants::revoke(&state.store, originator, body.scope(), now()?).await };
    match written.await {
        Ok(rows) if rows.is_empty() => refusal(StatusCode::CONFLICT, &body.nothing_to_revoke()),
        Ok(rows) => {
            tracing::info!(originator = %originator, rows = rows.len(), "inspector: revoked");
            shown(&state, rows, vec![]).await
        }
        Err(e) => refused(&e),
    }
}
