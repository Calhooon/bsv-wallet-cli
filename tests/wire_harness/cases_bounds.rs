//! The open doors' bound (#541, `docs/design/origin-grants.md` section
//! "The open doors' bound"): a page the seed does not list, past the bound
//! of each class, answered 429 `ERR_RATE_LIMITED` naming the door and the
//! window, with `Retry-After` and the echo. Served from the `pages` world
//! by an origin of their own (no earlier fixture spent its allowance);
//! recorded last, so no file before them moves.
use super::owner::{Owner, Spent};
use super::recorder::{get, Recorder};
use crate::wire_support::Req;

/// A page the seed does not list, whose allowance only these fixtures
/// spend.
const PAGE: &str = "http://bound.test";

/// The request with `call` sent `times` times first, each answered.
fn spent(times: u32, call: Req) -> Option<Owner> {
    Some(Owner {
        spent: Some(Spent { times, call }),
        ..Owner::default()
    })
}

pub(crate) async fn record(r: &mut Recorder) {
    // A presence door answers a page once a minute: the second call is
    // 429 with the minute's rest.
    let presence = get("/isAuthenticated").header("origin", PAGE);
    r.case_owned(
        "bound.isAuthenticated.presence",
        "pages",
        presence.clone(),
        spent(1, presence),
    )
    .await;
    // The chain doors, one bound for the four: ten getNetwork calls spend
    // the burst, and getHeight is 429 for the second's rest.
    r.case_owned(
        "bound.getHeight.chain",
        "pages",
        get("/getHeight").header("origin", PAGE),
        spent(10, get("/getNetwork").header("origin", PAGE)),
    )
    .await;
}
