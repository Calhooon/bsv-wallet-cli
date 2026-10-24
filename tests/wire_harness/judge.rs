//! The judge: what the corpus must cover.

use std::collections::BTreeSet;
use std::path::Path;

use super::fixture::Fixture;
use super::worlds::WORLDS;

// ---------------------------------------------------------------------------
// The judge: what the corpus must cover.
// ---------------------------------------------------------------------------

/// The 28 BRC-100 calls the router serves.
const ENDPOINTS: [&str; 28] = [
    "/isAuthenticated",
    "/getPublicKey",
    "/createSignature",
    "/createAction",
    "/internalizeAction",
    "/getHeight",
    "/getNetwork",
    "/getVersion",
    "/waitForAuthentication",
    "/getHeaderForHeight",
    "/verifySignature",
    "/encrypt",
    "/decrypt",
    "/createHmac",
    "/verifyHmac",
    "/signAction",
    "/abortAction",
    "/listActions",
    "/listOutputs",
    "/relinquishOutput",
    "/acquireCertificate",
    "/listCertificates",
    "/proveCertificate",
    "/relinquishCertificate",
    "/discoverByIdentityKey",
    "/discoverByAttributes",
    "/revealCounterpartyKeyLinkage",
    "/revealSpecificKeyLinkage",
];

/// The five host routes (not BRC-100).
const HOST_ROUTES: [&str; 5] = [
    "/permission-audit",
    "/permission-audit/reset",
    "/chain-feed",
    "/tx/{txid}",
    "/arc-callback",
];

/// `AppError`'s codes (`src/server/error.rs`) some world produces, by
/// status; each must be answered at least once. With [`UNPRODUCED`] they
/// are every code the source writes ([`app_codes_in_source`]): a code added
/// there without a fixture fails `every_app_error_code_is_judged`.
pub(crate) const APP_CODES: [(u16, &str); 13] = [
    (400, "VALIDATION_ERROR"),
    (400, "INVALID_BEEF"),
    (400, "ERROR"),
    (402, "INSUFFICIENT_FUNDS"),
    (403, "INPUT_NOT_OFFERED"),
    (404, "NOT_FOUND"),
    (409, "UNSPENDABLE_COIN"),
    (409, "CONFLICT"),
    (500, "INTERNAL_ERROR"),
    (501, "PRIVILEGED_NOT_SUPPORTED"),
    (502, "SERVICE_ERROR"),
    (503, "STORE_BUSY"),
    (503, "HEADERS_UNAVAILABLE"),
];

/// The router's own codes (`src/server/mod.rs`), by status; each must be
/// answered at least once.
const ROUTER_CODES: [(u16, &str); 7] = [
    (401, "UNAUTHORIZED"),
    (404, "CALLBACK_DISABLED"),
    (400, "BAD_JSON"),
    (400, "BAD_PAYLOAD"),
    (413, "TOO_LARGE"),
    (500, "ERR_CHAIN_FEED"),
    (500, "ERR_TX_WORD"),
];

/// Every code the corpus judges: a fixture whose code is in neither list is
/// a judgment gap.
fn judged_codes() -> impl Iterator<Item = (u16, &'static str)> {
    APP_CODES.into_iter().chain(ROUTER_CODES)
}

/// `AppError` codes no in-process world produces, each named with why in
/// docs/WIRE.md. A fixture that answers one moves it to [`APP_CODES`].
pub(crate) const UNPRODUCED: [(u16, &str); 3] = [
    (401, "AUTHENTICATION_REQUIRED"),
    (403, "ACCESS_DENIED"),
    (502, "BROADCAST_REJECTED"),
];

/// The `(status, code)` pairs `src/server/error.rs` writes: each
/// `(StatusCode::NAME, "CODE")` of `Kind::answer`, the served layer's one
/// table (#81).
pub(crate) fn app_codes_in_source() -> BTreeSet<(u16, String)> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let path = "src/server/error.rs";
    let source = std::fs::read_to_string(root.join(path)).unwrap_or_else(|e| panic!("{path}: {e}"));
    let status = |name: &str| -> u16 {
        match name {
            "BAD_REQUEST" => 400,
            "UNAUTHORIZED" => 401,
            "PAYMENT_REQUIRED" => 402,
            "FORBIDDEN" => 403,
            "NOT_FOUND" => 404,
            "CONFLICT" => 409,
            "INTERNAL_SERVER_ERROR" => 500,
            "NOT_IMPLEMENTED" => 501,
            "BAD_GATEWAY" => 502,
            "SERVICE_UNAVAILABLE" => 503,
            other => panic!("StatusCode::{other}: not in the scanner's table"),
        }
    };
    let mut found = BTreeSet::new();
    for line in source.lines() {
        let Some((_, rest)) = line.split_once("(StatusCode::") else {
            continue;
        };
        let (name, code) = rest
            .split_once(',')
            .unwrap_or_else(|| panic!("{path}: `{}` has no code", line.trim()));
        let code = code
            .trim()
            .strip_prefix('"')
            .and_then(|c| c.split_once('"'));
        let (code, _) =
            code.unwrap_or_else(|| panic!("{path}: `{}`: the code is not a literal", line.trim()));
        found.insert((status(name.trim()), code.to_string()));
    }
    found
}

fn route_of(path: &str) -> String {
    if path.starts_with("/tx/") {
        "/tx/{txid}".to_string()
    } else {
        path.to_string()
    }
}

pub(crate) fn judge_coverage(fixtures: &[(String, Fixture)]) -> Vec<String> {
    let routes: BTreeSet<String> = fixtures
        .iter()
        .map(|(_, f)| route_of(&f.request.path))
        .collect();
    let codes: BTreeSet<(u16, String)> = fixtures
        .iter()
        .filter_map(|(_, f)| {
            let code = f.response.json.as_ref()?.get("code")?.as_str()?;
            Some((f.response.status, code.to_string()))
        })
        .collect();
    let mut missing = Vec::new();
    for route in ENDPOINTS.iter().chain(HOST_ROUTES.iter()) {
        if !routes.contains(*route) {
            missing.push(format!("no fixture serves {route}"));
        }
    }
    for (status, code) in judged_codes() {
        if !codes.contains(&(status, code.to_string())) {
            missing.push(format!("no fixture answers {status} {code}"));
        }
    }
    for (status, code) in &codes {
        if !judged_codes().any(|c| c == (*status, code.as_str())) {
            let why = if UNPRODUCED.contains(&(*status, code.as_str())) {
                "a world produces it now: move it from UNPRODUCED to APP_CODES"
            } else {
                "neither APP_CODES nor ROUTER_CODES judges it"
            };
            missing.push(format!("a fixture answers {status} {code}: {why}"));
        }
    }
    let worlds: BTreeSet<&str> = fixtures.iter().map(|(_, f)| f.world.as_str()).collect();
    for world in WORLDS {
        if !worlds.contains(world) {
            missing.push(format!("no fixture is served from the `{world}` world"));
        }
    }
    missing
}
