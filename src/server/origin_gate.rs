//! The served doors' browser gate, #411's step (b) by the owner's word
//! of 2026-11-01: every call carries an `Origin`, the originator a door
//! judges is derived from it, and only the originators `ALLOWED_ORIGINS`
//! names are admitted. The per-origin grant in the store is #414's.
//!
//! The key, by the owner's word of 2026-11-02: the reference's originator
//! and its hostname scope (ts-stack edf6e03 `WalletPermissionsManager.ts`,
//! written `WPM`). An originator is a domain name with an optional port,
//! without a scheme (`prepareOriginator`, `WPM:5750-5795`); its grants are
//! the lowercased hostname's, every port of a host sharing one
//! (`WPM:5702-5729`).
//!
//! The desktop client's lines this follows (bsv-desktop bcb6003):
//! `src/onWalletReady.ts:45-71` reads the originator from `Origin` (an
//! unparseable one is none), and `:82-88` answers a call without one
//! `400 {"message": "Origin header is required"}`; an origin its owner
//! refuses is its permission manager's `The user has denied the request for
//! permission.` (`WPM:1348-1349`, code `ERR_PERMISSION_DENIED`), served 400
//! by the same handler. Here the list is the owner's word, so an originator
//! it does not name is refused in that shape without a prompt.
//!
//! Where this is stricter than the client, by the ruling's rule 1 and the
//! stack's charter section 13 (`lean/BrowserGate.lean` `gate`): the legacy
//! `Originator` header never stands in for a missing `Origin` (the client
//! falls back to it, `onWalletReady.ts:59-68`), and an `Originator` header
//! or a body's `originator` naming another originator than the `Origin`'s
//! is refused. The bearer (`AUTH_TOKEN`) is checked ahead of the gate and
//! never admits an origin.

use std::sync::Arc;

use axum::extract::Request;
use axum::http::{HeaderMap, HeaderValue, Method};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use tower_http::cors::{AllowHeaders, AllowMethods, AllowOrigin, CorsLayer};

use super::error::{AppError, Kind};

/// The route the gate does not judge: Arcade's webhook, server to server,
/// sends no `Origin` and is authenticated by the callback token.
const EXEMPT: &str = "/arc-callback";

/// The client's text for a call without an `Origin`.
pub(crate) const NO_ORIGIN: &str = "Origin header is required";

/// The client's text for an origin its owner refused.
pub(crate) const DENIED: &str = "The user has denied the request for permission.";

/// The originators the served doors admit (`ALLOWED_ORIGINS`): each a
/// host with an optional port, lowercased, no scheme; empty admits none.
#[derive(Clone, Debug, Default)]
pub struct AllowedOrigins(Arc<[String]>);

impl AllowedOrigins {
    /// The list of originators (`localhost`, `app.example:8443`),
    /// lowercased; `ALLOWED_ORIGINS` is checked at start by the same rule
    /// (`config::read`).
    ///
    /// # Panics
    ///
    /// On an entry that is not an originator (a scheme, a path, a name that
    /// is not a hostname), with the rule it breaks: an entry is never
    /// stripped to its host.
    pub fn new<I: IntoIterator<Item = S>, S: Into<String>>(originators: I) -> Self {
        Self(
            originators
                .into_iter()
                .map(|entry| {
                    let entry = entry.into();
                    crate::config::originator::checked(&entry)
                        .unwrap_or_else(|rule| panic!("{entry:?} is not {rule}"))
                })
                .collect(),
        )
    }

    /// The entry admitting `originator`, looked up as the reference looks
    /// up a grant (`prepareOriginator`'s lookup values, `WPM:5786-5794`):
    /// the originator as named, then its hostname, then any entry whose
    /// hostname is the originator's (the reference's untagged fallback,
    /// accepted when the grant's domain normalizes to the hostname,
    /// `WPM:2966`, `:3267`). So a port never narrows an entry: every port
    /// of a listed host is admitted.
    fn admits(&self, originator: &str) -> Option<&str> {
        let host = hostname_of(originator)?;
        let find = |hit: &dyn Fn(&str) -> bool| self.0.iter().find(|e| hit(e)).map(String::as_str);
        find(&|e| e == originator)
            .or_else(|| find(&|e| e == host))
            .or_else(|| find(&|e| hostname_of(e).as_deref() == Some(host.as_str())))
    }

    /// Whether the list admits an `Origin` header's originator (the CORS
    /// layer's and the inspector's question). Public because the inspector
    /// (`commands::ui`) lives in the binary, across the lib/bin split.
    pub fn admits_origin(&self, origin: &str) -> bool {
        originator_of(origin).is_some_and(|o| self.admits(&o).is_some())
    }

    /// The CORS layer over the list: a preflight or an answer carries
    /// `Access-Control-Allow-Origin` only for an `Origin` whose originator
    /// the list admits, which it echoes; the methods and headers are
    /// mirrored as before (#138).
    pub(crate) fn cors(&self) -> CorsLayer {
        let allowed = self.clone();
        let admits = move |origin: &HeaderValue, _: &axum::http::request::Parts| {
            origin.to_str().is_ok_and(|o| allowed.admits_origin(o))
        };
        // A listed page reads `Retry-After` on a 503 STORE_BUSY (#44).
        CorsLayer::new()
            .allow_credentials(true)
            .allow_headers(AllowHeaders::mirror_request())
            .allow_methods(AllowMethods::mirror_request())
            .allow_origin(AllowOrigin::predicate(admits))
            .expose_headers([axum::http::header::RETRY_AFTER])
    }
}

impl super::ServerConfig {
    /// The default settings with the browser gate admitting `originators`, as
    /// `ALLOWED_ORIGINS` would (the tests' and the harnesses' servers).
    #[allow(dead_code)] // the binary reads `ALLOWED_ORIGINS`; the tests build it
    #[must_use]
    pub fn admitting<I: IntoIterator<Item = S>, S: Into<String>>(originators: I) -> Self {
        Self {
            allowed_origins: AllowedOrigins::new(originators),
            ..Self::default()
        }
    }
}

/// The gate's refusals (`BrowserGate.Word` under (b): an originator not
/// listed is refused, never pending); a call admitted is its `Origin`.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Refused {
    /// The call names no parseable `Origin`.
    NoOrigin,
    /// The `Originator` header names another originator than the `Origin`.
    Disagrees { origin: String, claimed: String },
    /// The `Origin`'s originator is not in `ALLOWED_ORIGINS`.
    NotListed { origin: String, originator: String },
}

impl Refused {
    /// The refusal as the door serves it.
    fn answer(self) -> Response {
        match self {
            Refused::NoOrigin => AppError::new(Kind::Validation, NO_ORIGIN),
            Refused::Disagrees { origin, claimed } => AppError::new(
                Kind::Validation,
                format!(
                    "the Originator header names {claimed}, which disagrees with the Origin \
                     header's {origin}"
                ),
            ),
            Refused::NotListed { origin, originator } => {
                tracing::info!(origin = %origin, originator = %originator, "browser gate: originator not in ALLOWED_ORIGINS, refused");
                AppError::new(Kind::OriginDenied, DENIED)
            }
        }
        .into_response()
    }
}

/// The originator of an `Origin` header, as the client derives it (`new
/// URL(origin).host`, `onWalletReady.ts:51`) in the reference's form: the
/// host lowercased, with its port only when the header names one, never
/// the scheme. `https://App.Example:8443` is `app.example:8443`;
/// `http://localhost` is `localhost`.
pub(crate) fn originator_of(origin: &str) -> Option<String> {
    let url = url::Url::parse(origin).ok()?;
    let host = url.host_str()?.to_ascii_lowercase();
    Some(match url.port() {
        Some(port) => format!("{host}:{port}"),
        None => host,
    })
}

/// The originator another field names, in the same form: a name with a
/// scheme as an `Origin`, one without (the reference's form,
/// `app.example:8443`) read as its host and port.
fn claimed_originator(name: &str) -> Option<String> {
    if crate::config::originator::has_scheme(name) {
        originator_of(name)
    } else {
        originator_of(&format!("http://{name}"))
    }
}

/// An originator's hostname, its port dropped (`WPM:5702-5729`).
fn hostname_of(originator: &str) -> Option<String> {
    url::Url::parse(&format!("http://{originator}"))
        .ok()?
        .host_str()
        .map(str::to_ascii_lowercase)
}

/// The `Origin` header and its originator, when it is one a URL parser
/// reads with a host.
fn origin_of(headers: &HeaderMap) -> Option<(&str, String)> {
    let origin = headers.get("origin")?.to_str().ok()?;
    Some((origin, originator_of(origin)?))
}

/// The gate on a call's headers: the admitted call, or the refusal. The
/// order is `BrowserGate.gate`'s: the `Origin`, the other field's
/// agreement, then the list.
pub(crate) fn judge(headers: &HeaderMap, allowed: &AllowedOrigins) -> Result<Admitted, Refused> {
    let (origin, originator) = origin_of(headers).ok_or(Refused::NoOrigin)?;
    if let Some(claimed) = headers.get("originator") {
        let claimed = String::from_utf8_lossy(claimed.as_bytes()).into_owned();
        if claimed_originator(&claimed).as_ref() != Some(&originator) {
            return Err(Refused::Disagrees {
                origin: origin.to_string(),
                claimed,
            });
        }
    }
    match allowed.admits(&originator) {
        Some(entry) => Ok(Admitted {
            origin: origin.to_string(),
            entry: entry.to_string(),
            originator,
        }),
        None => Err(Refused::NotListed {
            origin: origin.to_string(),
            originator,
        }),
    }
}

/// The admitted call: its `Origin` as sent, its originator, and the entry
/// that admitted it, for the log and the body's check after it is read.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Admitted {
    pub(crate) origin: String,
    pub(crate) originator: String,
    pub(crate) entry: String,
}

/// The gate, after the bearer and before the body is read: a call is
/// admitted, logged at info with its origin, its originator and the entry
/// that admitted it, or refused in the client's shape. Inside the CORS
/// layer, so a listed page reads its refusal.
pub(crate) async fn gate(
    axum::Extension(allowed): axum::Extension<AllowedOrigins>,
    mut request: Request,
    next: Next,
) -> Response {
    if request.uri().path() == EXEMPT {
        return next.run(request).await;
    }
    match judge(request.headers(), &allowed) {
        Ok(admitted) => {
            tracing::info!(
                origin = %admitted.origin,
                originator = %admitted.originator,
                entry = %admitted.entry,
                path = %request.uri().path(),
                "browser gate: admitted"
            );
            request.extensions_mut().insert(admitted);
            next.run(request).await
        }
        Err(refused) => refused.answer(),
    }
}

/// The preflight's gate, outside the CORS layer: an `OPTIONS` from an
/// originator not listed (or with no `Origin`) is refused before CORS
/// answers it, so a preflight is answered only for a listed originator.
pub(crate) async fn preflight(
    axum::Extension(allowed): axum::Extension<AllowedOrigins>,
    request: Request,
    next: Next,
) -> Response {
    if request.method() != Method::OPTIONS || request.uri().path() == EXEMPT {
        return next.run(request).await;
    }
    match origin_of(request.headers()) {
        Some((_, originator)) if allowed.admits(&originator).is_some() => next.run(request).await,
        Some((origin, originator)) => Refused::NotListed {
            origin: origin.to_string(),
            originator,
        }
        .answer(),
        None => Refused::NoOrigin.answer(),
    }
}

/// The refusal of a read body whose `originator` disagrees with the
/// admitted `Origin` (#411), when the gate admitted the call.
pub(crate) fn refuse_body(extensions: &axum::http::Extensions, body: &[u8]) -> Option<Response> {
    let admitted = extensions.get::<Admitted>()?;
    body_disagrees(admitted, body).map(IntoResponse::into_response)
}

/// A body's top-level `originator`, when it names another originator than
/// the admitted `Origin`'s (or is not a string): the refusal. It fails closed: a
/// second `originator` is a disagreement by itself, and so is a body the
/// walk cannot read once it has met one. A body that is not a JSON object
/// names none, and a fault before any `originator` is the door's own (its
/// reader stops at the same byte); either way its door reads it as before.
pub(crate) fn body_disagrees(admitted: &Admitted, body: &[u8]) -> Option<AppError> {
    // Only an object names a field: an array body is a door's positional
    // call.
    let object = body.iter().find(|b| !b.is_ascii_whitespace()) == Some(&b'{');
    if !object {
        return None;
    }
    let refused = |claim: String| {
        AppError::new(
            Kind::Validation,
            format!(
                "the body's originator {claim}, which disagrees with the Origin header's {}",
                admitted.origin
            ),
        )
    };
    let mut claims = Vec::new();
    let mut reader = serde_json::Deserializer::from_slice(body);
    let read = serde::de::DeserializeSeed::deserialize(Originators(&mut claims), &mut reader)
        .and_then(|()| reader.end());
    match (read, claims.as_slice()) {
        (_, []) => None,
        (Err(_), _) | (_, [None]) => Some(refused("cannot be read".into())),
        (Ok(()), [Some(claim)]) => {
            let agrees = claim
                .as_str()
                .is_some_and(|c| claimed_originator(c).as_ref() == Some(&admitted.originator));
            let claim = claim
                .as_str()
                .map_or_else(|| claim.to_string(), str::to_string);
            (!agrees).then(|| refused(format!("names {claim}")))
        }
        (Ok(()), many) => Some(refused(format!("is named {} times", many.len()))),
    }
}

/// The walk of a body's top level: each `originator` it meets, `None`
/// until its value is read (a fault inside one leaves it unread). Every
/// other value is skipped unread.
struct Originators<'a>(&'a mut Vec<Option<serde_json::Value>>);

impl<'de> serde::de::DeserializeSeed<'de> for Originators<'_> {
    type Value = ();
    fn deserialize<D: serde::Deserializer<'de>>(self, reader: D) -> Result<(), D::Error> {
        reader.deserialize_map(self)
    }
}

impl<'de> serde::de::Visitor<'de> for Originators<'_> {
    type Value = ();
    fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        f.write_str("a JSON object")
    }
    fn visit_map<A: serde::de::MapAccess<'de>>(self, mut map: A) -> Result<(), A::Error> {
        while let Some(key) = map.next_key::<String>()? {
            if key == "originator" {
                self.0.push(None);
                let claim = map.next_value::<serde_json::Value>()?;
                if let Some(last) = self.0.last_mut() {
                    *last = Some(claim);
                }
            } else {
                map.next_value::<serde::de::IgnoredAny>()?;
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn headers(pairs: &[(&str, &str)]) -> HeaderMap {
        let mut h = HeaderMap::new();
        for (k, v) in pairs {
            h.insert(
                axum::http::HeaderName::from_bytes(k.as_bytes()).unwrap(),
                HeaderValue::from_str(v).unwrap(),
            );
        }
        h
    }

    fn listed() -> AllowedOrigins {
        AllowedOrigins::new(["App.Example", "localhost:5173"])
    }

    /// The key (the owner's word of 2026-11-02): an `Origin` is judged as
    /// its originator, the host lowercased with its port when named and no
    /// scheme, as the reference names one.
    #[test]
    fn an_origin_is_judged_as_its_originator() {
        for (origin, originator) in [
            ("https://App.Example:8443", "app.example:8443"),
            ("http://localhost:5173", "localhost:5173"),
            ("https://app.example", "app.example"),
            ("http://127.0.0.1:3000", "127.0.0.1:3000"),
            ("http://[::1]:3000", "[::1]:3000"),
        ] {
            assert_eq!(originator_of(origin).as_deref(), Some(originator));
        }
    }

    /// The scenario no-origin: no `Origin` is refused whatever the
    /// `Originator` header names; an unparseable `Origin` is none (the
    /// client's `parseOrigin` returns null for it).
    #[test]
    fn no_origin_is_refused_whatever_another_field_names() {
        for h in [
            headers(&[]),
            headers(&[("originator", "app.example")]),
            headers(&[("originator", "http://app.example")]),
            headers(&[("origin", "null")]),
            headers(&[("origin", "not-a-url"), ("originator", "app.example")]),
        ] {
            assert_eq!(judge(&h, &listed()), Err(Refused::NoOrigin), "{h:?}");
        }
    }

    /// The scenarios new-origin and revoked-origin under (b): an originator
    /// whose hostname the list does not name (or no longer names) is
    /// refused.
    #[test]
    fn an_originator_not_listed_is_refused() {
        for (origin, originator) in [
            ("http://new.example", "new.example"),
            ("http://sub.app.example", "sub.app.example"),
            ("http://app.example.evil", "app.example.evil"),
            ("http://127.0.0.1:5173", "127.0.0.1:5173"),
        ] {
            assert_eq!(
                judge(&headers(&[("origin", origin)]), &listed()),
                Err(Refused::NotListed {
                    origin: origin.into(),
                    originator: originator.into()
                })
            );
        }
        let none = AllowedOrigins::default();
        assert!(matches!(
            judge(&headers(&[("origin", "http://app.example")]), &none),
            Err(Refused::NotListed { .. })
        ));
    }

    /// The scenario granted-origin, by the reference's hostname scope: a
    /// listed host is admitted under either scheme and on every port, with
    /// or without an agreeing `Originator` (the reference's Node client
    /// sends the same value in both, HTTPWalletJSON.ts:314). The lookup is
    /// the reference's: the originator as named, then its hostname, then an
    /// entry for another port of the host (`WPM:5786-5794`).
    #[test]
    fn a_listed_host_is_admitted_on_every_port_and_scheme() {
        for (origin, claimed, entry) in [
            ("http://app.example", None, "app.example"),
            ("https://app.example", None, "app.example"),
            ("http://APP.example:8080", None, "app.example"),
            ("http://app.example", Some("app.example"), "app.example"),
            (
                "http://app.example",
                Some("http://app.example"),
                "app.example",
            ),
            (
                "https://app.example:8443",
                Some("App.Example:8443"),
                "app.example",
            ),
            (
                "http://localhost:5173",
                Some("localhost:5173"),
                "localhost:5173",
            ),
            ("http://localhost:5174", None, "localhost:5173"),
            ("http://localhost", None, "localhost:5173"),
        ] {
            let mut pairs = vec![("origin", origin)];
            pairs.extend(claimed.map(|c| ("originator", c)));
            let admitted = judge(&headers(&pairs), &listed()).expect(origin);
            assert_eq!(admitted.entry, entry, "{origin}");
            assert_eq!(admitted.origin, origin);
        }
        let both = AllowedOrigins::new(["localhost:5173", "localhost"]);
        let at = |origin| judge(&headers(&[("origin", origin)]), &both).unwrap().entry;
        assert_eq!(at("http://localhost:5173"), "localhost:5173");
        assert_eq!(at("http://localhost:3000"), "localhost");
    }

    /// The scenario disagreeing-originator: another originator in the
    /// `Originator` header is refused, listed or not, before the list; it
    /// is compared in the `Origin`'s form, host and port.
    #[test]
    fn a_disagreeing_originator_is_refused_before_the_list() {
        for (origin, claimed) in [
            ("http://evil.example", "app.example"),
            ("http://app.example", "evil.example"),
            ("http://localhost:5173", "localhost"),
            ("http://app.example", "app.example:8080"),
            ("http://app.example", ""),
        ] {
            assert!(
                matches!(
                    judge(
                        &headers(&[("origin", origin), ("originator", claimed)]),
                        &listed()
                    ),
                    Err(Refused::Disagrees { .. })
                ),
                "{origin} {claimed}"
            );
        }
    }

    /// An entry is an originator or a programmer's error, never stripped
    /// to its host.
    #[test]
    #[should_panic(expected = "without a URL scheme")]
    fn an_entry_with_a_scheme_is_refused() {
        let _ = AllowedOrigins::new(["http://localhost"]);
    }

    /// A body's originator agrees with the `Origin` or is refused, and the
    /// check fails closed (the review of PR 419): a second `originator` is a
    /// disagreement by itself (serde's derive refused the duplicate and the
    /// check admitted the call), and so is a fault while the walk reads one
    /// or after it has read one. A fault before any is the door's own
    /// (fixture 032).
    #[test]
    fn a_body_originator_must_agree_with_the_origin() {
        let admitted = Admitted {
            origin: "http://app.example".into(),
            originator: "app.example".into(),
            entry: "app.example".into(),
        };
        for body in [
            &br#"{"keyID":"1"}"#[..],
            br#"{"originator":"app.example"}"#,
            br#"{"originator":"app.example","keyID":"1"}"#,
            br#"{"originator":"APP.example"}"#,
            br#"{"originator":"http://app.example"}"#,
            br"[1,2]",
            br#"["evil.example",{"keyID":"1"}]"#,
            b"not json",
            br#"{"data": [1"#,
        ] {
            assert!(
                body_disagrees(&admitted, body).is_none(),
                "{}",
                String::from_utf8_lossy(body)
            );
        }
        for body in [
            &br#"{"originator":"evil.example"}"#[..],
            br#"{"originator":7}"#,
            br#"{"originator":""}"#,
            br#"{"originator":"evil.example","originator":"evil.example"}"#,
            br#"{"originator":"app.example","originator":"evil.example"}"#,
            br#"{"originator":"app.example","keyID":"1","originator":"app.example"}"#,
            br#"{"origin\u0061tor":"app.example","originator":"evil.example"}"#,
            br#"{"originator":["evil.example"]}"#,
            br#"{"originator":{"host":"app.example"}}"#,
            br#"{"originator":null}"#,
            br#"{"originator":"app.example"#,
            br#"{"originator":"app.example","keyID":[1"#,
            br#"{"originator":"app.example"} trailing"#,
        ] {
            assert!(
                body_disagrees(&admitted, body).is_some(),
                "{}",
                String::from_utf8_lossy(body)
            );
        }
    }
}
