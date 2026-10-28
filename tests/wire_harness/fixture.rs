//! The fixture, and the normalizer: the one place a value that must differ
//! from run to run is replaced by its shape.

use serde::{Deserialize, Serialize};

use crate::wire_support::{diff, replace_number, Normalizer, Req, Resp, J};

/// `serde_json::json!` into [`J`], its keys in the order written (the
/// macro's map sorts them; a request is sent as recorded either way).
pub(crate) fn j(v: serde_json::Value) -> J {
    serde_json::from_value(v).expect("json")
}

// ---------------------------------------------------------------------------
// The fixture.
// ---------------------------------------------------------------------------

#[derive(Serialize, Deserialize, Clone)]
pub(crate) struct Fixture {
    /// The world the request is served from (see [`World::build`]).
    pub(crate) world: String,
    pub(crate) request: Req,
    pub(crate) response: Resp,
}

/// The response headers that are part of the contract (the body's type, the
/// retry pause, and every CORS header, the preflight's three included).
/// Every other header is the transport's.
const KEPT_HEADERS: [&str; 9] = [
    "content-type",
    "retry-after",
    "access-control-allow-origin",
    "access-control-allow-credentials",
    "access-control-allow-methods",
    "access-control-allow-headers",
    "access-control-max-age",
    "access-control-expose-headers",
    "vary",
];

// ---------------------------------------------------------------------------
// The normalizer: the one place a value that must differ from run to run is
// replaced by a placeholder. The placeholder keeps the value's shape (its
// JSON type, a byte array's length, a string's length, a number's digit
// count), so a shape change is a byte of difference. Every rule is named in
// docs/WIRE.md.
// ---------------------------------------------------------------------------

/// Keys whose values differ on every run, anywhere in a body.
const VOLATILE_KEYS: [&str; 6] = [
    // AES-GCM under a fresh random IV: a new ciphertext each call.
    "ciphertext",
    // A key linkage revealed is encrypted for the verifier the same way.
    "encryptedLinkage",
    "encryptedLinkageProof",
    // A held coin's age, from its row's time (half an hour before the
    // world was built) to the clock's now.
    "ageMinutes",
    // The clock's now, when a linkage is revealed.
    "revelationTime",
    // The reference of a signable transaction: the toolbox draws it at
    // random.
    "reference",
];

/// The placeholder for a volatile value: its shape, never its bytes.
fn shape(v: &J) -> String {
    match v {
        J::Null => "null".to_string(),
        J::Bool(_) => "<bool>".to_string(),
        J::Num(n) => match (n.as_u64(), n.as_i64()) {
            (Some(u), _) => format!("<uint:{}>", u.to_string().len()),
            (None, Some(i)) => format!("<int:{}>", i.unsigned_abs().to_string().len()),
            _ => "<float>".to_string(),
        },
        J::Str(s) => {
            if chrono::DateTime::parse_from_rfc3339(s).is_ok() {
                "<rfc3339>".to_string()
            } else if s.len() % 2 == 0 && !s.is_empty() && s.bytes().all(|b| b.is_ascii_hexdigit())
            {
                format!("<hex:{}>", s.len())
            } else {
                format!("<string:{}>", s.chars().count())
            }
        }
        J::Arr(items) => {
            let bytes = items
                .iter()
                .all(|i| matches!(i, J::Num(n) if n.as_u64().is_some_and(|b| b <= 255)));
            if bytes {
                format!("<bytes:{}>", items.len())
            } else {
                format!("<array:{}>", items.len())
            }
        }
        J::Obj(fields) => format!("<object:{}>", fields.len()),
    }
}

/// The oracle's normalizer: the [`KEPT_HEADERS`], each of the
/// [`VOLATILE_KEYS`] by its [`shape`], and [`normalize_text`].
pub(crate) const NORMALIZER: Normalizer = Normalizer {
    kept_headers: &KEPT_HEADERS,
    volatile,
    text: normalize_text,
};

fn volatile(key: &str, parent: Option<&str>, v: &J) -> Option<String> {
    (VOLATILE_KEYS.contains(&key) || parent == Some("keyringForVerifier")).then(|| shape(v))
}

pub(crate) fn normalize(v: J) -> J {
    NORMALIZER.json(v)
}

/// Inside any string: the local stubs' ports (a fresh port each run), a
/// held coin's age in a refusal's listing (`<n> min old`), and the
/// platform's errno in a refused connection's words (`os error <n>`).
pub(crate) fn normalize_text(s: &str) -> String {
    let s = replace_number(s, "127.0.0.1:", "<port>", true);
    let s = replace_number(&s, "os error ", "<errno>", true);
    replace_number(&s, " min old", "<minutes>", false)
}

/// A strict difference: the fixture's request and the line diff of the
/// recorded response against the served one.
pub(crate) fn answered_differently(file: &str, fixture: &Fixture, got: &Resp) -> String {
    let want = serde_json::to_string_pretty(&fixture.response).unwrap();
    let got = serde_json::to_string_pretty(got).unwrap();
    format!(
        "{file}: {} {} answered differently (- fixture, + served)\n{}",
        fixture.request.method,
        fixture.request.path,
        diff(&want, &got)
    )
}
