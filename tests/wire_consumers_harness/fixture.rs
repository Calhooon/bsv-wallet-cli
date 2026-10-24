//! The fixture, and the normalizer: the wire oracle's rules.

use serde::{Deserialize, Serialize};

use crate::wire_support::{replace_number, Normalizer, Req, Resp, J};

// ---------------------------------------------------------------------------
// The fixture.
// ---------------------------------------------------------------------------

#[derive(Serialize, Deserialize, Clone)]
pub(crate) struct Fixture {
    /// The dependent whose call this is (one of [`CONSUMERS`]).
    pub(crate) consumer: String,
    /// The consumer's own name for the call.
    pub(crate) call: String,
    /// The file in the consumer that sends this shape.
    pub(crate) source: String,
    /// The world the request is served from (see [`World::build`]).
    pub(crate) world: String,
    pub(crate) request: Req,
    pub(crate) response: Resp,
}

/// The response headers that are part of the contract (the wire oracle's
/// list). Every other header is the transport's.
const KEPT_HEADERS: [&str; 6] = [
    "content-type",
    "retry-after",
    "access-control-allow-origin",
    "access-control-allow-credentials",
    "access-control-expose-headers",
    "vary",
];

// ---------------------------------------------------------------------------
// The normalizer: the wire oracle's rules (`docs/WIRE.md`), the values that
// differ from run to run with no change to the code.
// ---------------------------------------------------------------------------

const VOLATILE_KEYS: [(&str, &str); 5] = [
    ("ciphertext", "<ciphertext>"),
    ("encryptedLinkage", "<ciphertext>"),
    ("encryptedLinkageProof", "<ciphertext>"),
    ("ageMinutes", "<minutes>"),
    ("revelationTime", "<timestamp>"),
];

/// The consumers' normalizer: the [`KEPT_HEADERS`], each of the
/// [`VOLATILE_KEYS`] by its placeholder, and [`normalize_text`].
pub(crate) const NORMALIZER: Normalizer = Normalizer {
    kept_headers: &KEPT_HEADERS,
    volatile,
    text: normalize_text,
};

fn volatile(key: &str, _: &J) -> Option<String> {
    VOLATILE_KEYS
        .iter()
        .find(|(k, _)| *k == key)
        .map(|(_, placeholder)| placeholder.to_string())
}

fn normalize_text(s: &str) -> String {
    replace_number(s, " min old", "<minutes>", false)
}
