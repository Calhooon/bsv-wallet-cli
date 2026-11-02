//! The served doors' browser gate, #411's step (b) by the owner's word
//! of 2026-11-01: every call carries an `Origin`, the originator a door
//! judges is derived from it, and only the origins `ALLOWED_ORIGINS` names
//! are admitted. The per-origin grant in the store is #414's.
//!
//! The desktop client's lines this follows (bsv-desktop bcb6003):
//! `src/onWalletReady.ts:45-71` reads the originator from `Origin` (an
//! unparseable one is none), and `:82-88` answers a call without one
//! `400 {"message": "Origin header is required"}`; an origin its owner
//! refuses is its permission manager's `The user has denied the request for
//! permission.` (ts-stack edf6e03 `WalletPermissionsManager.ts:1348-1349`,
//! code `ERR_PERMISSION_DENIED`), served 400 by the same handler. Here the
//! list is the owner's word, so an origin it does not name is refused in
//! that shape without a prompt.
//!
//! Where this is stricter than the client, by the ruling's rule 1 and the
//! stack's charter section 13 (`lean/BrowserGate.lean` `gate`): the legacy
//! `Originator` header never stands in for a missing `Origin` (the client
//! falls back to it, `onWalletReady.ts:59-68`), and an `Originator` header
//! or a body's `originator` naming another host than the `Origin` is
//! refused. The bearer (`AUTH_TOKEN`) is checked ahead of the gate and
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

/// The exact origins the served doors admit (`ALLOWED_ORIGINS`), as a
/// browser serializes them; empty admits none.
#[derive(Clone, Debug, Default)]
pub struct AllowedOrigins(Arc<[String]>);

impl AllowedOrigins {
    /// The list, each origin as written (`ALLOWED_ORIGINS` is checked at
    /// start: `config::read`).
    pub fn new<I: IntoIterator<Item = S>, S: Into<String>>(origins: I) -> Self {
        Self(origins.into_iter().map(Into::into).collect())
    }

    fn admits(&self, origin: &str) -> bool {
        self.0.iter().any(|o| o == origin)
    }

    /// The CORS layer over the list: a preflight or an answer carries
    /// `Access-Control-Allow-Origin` only for a listed origin, which it
    /// echoes; the methods and headers are mirrored as before (#138).
    pub(crate) fn cors(&self) -> CorsLayer {
        let listed: Vec<HeaderValue> = self
            .0
            .iter()
            .filter_map(|o| HeaderValue::from_str(o).ok())
            .collect();
        // A listed page reads `Retry-After` on a 503 STORE_BUSY (#44).
        CorsLayer::new()
            .allow_credentials(true)
            .allow_headers(AllowHeaders::mirror_request())
            .allow_methods(AllowMethods::mirror_request())
            .allow_origin(AllowOrigin::list(listed))
            .expose_headers([axum::http::header::RETRY_AFTER])
    }
}

impl super::ServerConfig {
    /// The default settings with the browser gate admitting `origins`, as
    /// `ALLOWED_ORIGINS` would (the tests' and the harnesses' servers).
    #[allow(dead_code)] // the binary reads `ALLOWED_ORIGINS`; the tests build it
    #[must_use]
    pub fn admitting<I: IntoIterator<Item = S>, S: Into<String>>(origins: I) -> Self {
        Self {
            allowed_origins: AllowedOrigins::new(origins),
            ..Self::default()
        }
    }
}

/// The gate's refusals (`BrowserGate.Word` under (b): an origin not listed
/// is refused, never pending); a call admitted is its `Origin`.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Refused {
    /// The call names no parseable `Origin`.
    NoOrigin,
    /// The `Originator` header names another host than the `Origin`.
    Disagrees { origin: String, claimed: String },
    /// The `Origin` is not in `ALLOWED_ORIGINS`.
    NotListed { origin: String },
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
            Refused::NotListed { origin } => {
                tracing::info!(origin = %origin, "browser gate: origin not in ALLOWED_ORIGINS, refused");
                AppError::new(Kind::OriginDenied, DENIED)
            }
        }
        .into_response()
    }
}

/// The host a name is judged by, as the client derives it (`new
/// URL(x).host`: the host and a port other than the scheme's default); a
/// name without a scheme is read as `http://`, as the client reads the
/// `Originator` header.
fn host_of(name: &str) -> Option<String> {
    let candidate = if name.contains("://") {
        name.to_string()
    } else {
        format!("http://{name}")
    };
    let url = url::Url::parse(&candidate).ok()?;
    let host = url.host_str()?;
    Some(match url.port() {
        Some(port) => format!("{host}:{port}"),
        None => host.to_string(),
    })
}

/// The `Origin` header, when it is one a URL parser reads with a host.
fn origin_of(headers: &HeaderMap) -> Option<&str> {
    let origin = headers.get("origin")?.to_str().ok()?;
    url::Url::parse(origin).ok()?.host_str()?;
    Some(origin)
}

/// The gate on a call's headers: the admitted `Origin`, or the refusal.
/// The order is `BrowserGate.gate`'s: the `Origin`, the other field's
/// agreement, then the list.
pub(crate) fn judge(headers: &HeaderMap, allowed: &AllowedOrigins) -> Result<String, Refused> {
    let origin = origin_of(headers).ok_or(Refused::NoOrigin)?;
    if let Some(claimed) = headers.get("originator") {
        let claimed = String::from_utf8_lossy(claimed.as_bytes()).into_owned();
        if host_of(&claimed) != host_of(origin) {
            return Err(Refused::Disagrees {
                origin: origin.to_string(),
                claimed,
            });
        }
    }
    if allowed.admits(origin) {
        Ok(origin.to_string())
    } else {
        Err(Refused::NotListed {
            origin: origin.to_string(),
        })
    }
}

/// The admitted call's origin, for the body's check after it is read.
#[derive(Clone, Debug)]
pub(crate) struct Admitted(pub(crate) String);

/// The gate, after the bearer and before the body is read: a call is
/// admitted, logged at info with its origin, or refused in the client's
/// shape. Inside the CORS layer, so a listed page reads its refusal.
pub(crate) async fn gate(
    axum::Extension(allowed): axum::Extension<AllowedOrigins>,
    mut request: Request,
    next: Next,
) -> Response {
    if request.uri().path() == EXEMPT {
        return next.run(request).await;
    }
    match judge(request.headers(), &allowed) {
        Ok(origin) => {
            tracing::info!(origin = %origin, path = %request.uri().path(), "browser gate: admitted");
            request.extensions_mut().insert(Admitted(origin));
            next.run(request).await
        }
        Err(refused) => refused.answer(),
    }
}

/// The preflight's gate, outside the CORS layer: an `OPTIONS` from an
/// origin not listed (or with none) is refused before CORS answers it, so a
/// preflight is answered only for a listed origin.
pub(crate) async fn preflight(
    axum::Extension(allowed): axum::Extension<AllowedOrigins>,
    request: Request,
    next: Next,
) -> Response {
    if request.method() != Method::OPTIONS || request.uri().path() == EXEMPT {
        return next.run(request).await;
    }
    match origin_of(request.headers()) {
        Some(origin) if allowed.admits(origin) => next.run(request).await,
        Some(origin) => Refused::NotListed {
            origin: origin.to_string(),
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

/// A body's top-level `originator`, when it names another host than the
/// admitted `Origin` (or is not a string): the refusal. It fails closed: a
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
                admitted.0
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
                .is_some_and(|c| host_of(c).is_some() && host_of(c) == host_of(&admitted.0));
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
        AllowedOrigins::new(["http://app.example", "http://localhost:5173"])
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

    /// The scenarios new-origin and revoked-origin under (b): an origin the
    /// list does not name (or no longer names) is refused, the list exact.
    #[test]
    fn an_origin_not_listed_is_refused() {
        for origin in [
            "http://new.example",
            "https://app.example",
            "http://app.example:8080",
            "http://localhost",
            "http://localhost:5174",
        ] {
            assert_eq!(
                judge(&headers(&[("origin", origin)]), &listed()),
                Err(Refused::NotListed {
                    origin: origin.into()
                })
            );
        }
        let none = AllowedOrigins::default();
        assert!(matches!(
            judge(&headers(&[("origin", "http://app.example")]), &none),
            Err(Refused::NotListed { .. })
        ));
    }

    /// The scenario granted-origin: a listed origin is admitted, with or
    /// without an agreeing `Originator` (the reference's Node client sends
    /// the same value in both, HTTPWalletJSON.ts:314).
    #[test]
    fn a_listed_origin_is_admitted_with_an_agreeing_field() {
        for h in [
            headers(&[("origin", "http://app.example")]),
            headers(&[
                ("origin", "http://app.example"),
                ("originator", "app.example"),
            ]),
            headers(&[
                ("origin", "http://app.example"),
                ("originator", "http://app.example"),
            ]),
            headers(&[
                ("origin", "http://localhost:5173"),
                ("originator", "localhost:5173"),
            ]),
        ] {
            assert!(judge(&h, &listed()).is_ok(), "{h:?}");
        }
    }

    /// The scenario disagreeing-originator: another host in the
    /// `Originator` header is refused, listed or not, before the list.
    #[test]
    fn a_disagreeing_originator_is_refused_before_the_list() {
        for (origin, claimed) in [
            ("http://evil.example", "app.example"),
            ("http://app.example", "evil.example"),
            ("http://localhost:5173", "localhost"),
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

    /// A body's originator agrees with the `Origin` or is refused, and the
    /// check fails closed (the review of PR 419): a second `originator` is a
    /// disagreement by itself (serde's derive refused the duplicate and the
    /// check admitted the call), and so is a fault while the walk reads one
    /// or after it has read one. A fault before any is the door's own
    /// (fixture 032).
    #[test]
    fn a_body_originator_must_agree_with_the_origin() {
        let admitted = Admitted("http://app.example".into());
        for body in [
            &br#"{"keyID":"1"}"#[..],
            br#"{"originator":"app.example"}"#,
            br#"{"originator":"app.example","keyID":"1"}"#,
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
