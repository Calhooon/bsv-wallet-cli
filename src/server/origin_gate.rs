//! The served doors' browser gate: #411's step (b), a required `Origin`,
//! and #414's switch (the design's L5b, `docs/design/origin-grants.md`
//! sections (1), (2) and (6)): a door admits a page for each power it asks
//! by the owner's grant of that power to its origin, read from the store
//! (`super::powers`).
//!
//! The key, by the owner's word of 2026-11-02: the reference's originator
//! and its hostname scope (ts-stack edf6e03 `WalletPermissionsManager.ts`,
//! written `WPM`). An originator is a domain name with an optional port,
//! without a scheme (`prepareOriginator`, `WPM:5750-5795`); its grants are
//! the lowercased hostname's, every port of a host sharing one
//! (`WPM:5702-5729`).
//!
//! The gate's order, before the body is read: the `Origin` (none is the
//! MetaNet Client's `400 {"message": "Origin header is required"}`,
//! bsv-desktop `src/onWalletReady.ts`), the other fields' agreement, then
//! the seed: an originator `ALLOWED_ORIGINS` lists is admitted at every
//! door and host route during the seed window, as (b) admitted it, with no
//! row (question 6). The host routes answer the owner (no `Origin`, a
//! loopback `Host`, the bearer under `AUTH_TOKEN`) or the seed, never a
//! page's grant; the open doors (W15, ruled (i)) answer any page with a
//! parsed `Origin` and an agreeing originator with no row, within the
//! bound per originator (#541, `super::open_bounds`; past it 429
//! `ERR_RATE_LIMITED`). Every other door asks its powers after its
//! arguments parse (`powers::Asker::ask`).
//!
//! Where this is stricter than the client, by the ruling's rule 1 and the
//! stack's charter section 13 (`lean/BrowserGate.lean` `gate`): the legacy
//! `Originator` header never stands in for a missing `Origin` (the client
//! falls back to it, `onWalletReady.ts:59-68`), and an `Originator` header
//! or a body's `originator` naming another host than the `Origin`'s is
//! refused, compared in the lookup form (the hostname, every port of a
//! host one originator under the key, as the admission compares). The
//! bearer (`AUTH_TOKEN`) is checked ahead of the gate and never admits an
//! origin.

use std::sync::Arc;

use axum::extract::{MatchedPath, Request};
use axum::http::{HeaderMap, HeaderValue, Method};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use tower_http::cors::{AllowHeaders, AllowMethods, AllowOrigin, CorsLayer};

use super::error::{AppError, Kind};
use super::open_bounds::{Class, OpenBounds};
use super::probes::Probes;
pub use crate::config::originator::OriginatorRule;

/// The route the gate does not judge: Arcade's webhook, server to server,
/// sends no `Origin` and is authenticated by the callback token.
const EXEMPT: &str = "/arc-callback";

/// The client's text for a call without an `Origin`.
pub(crate) const NO_ORIGIN: &str = "Origin header is required";

/// The client's text for an origin its owner refused.
pub(crate) const DENIED: &str = "The user has denied the request for permission.";

/// The host routes (not BRC-100): no kind names them, so they answer the
/// owner or the seed, never a page's grant (the design's door-to-kind map).
const HOST_ROUTES: [&str; 4] = [
    "/permission-audit",
    "/permission-audit/reset",
    "/chain-feed",
    "/tx/{txid}",
];

/// The open doors (W15, ruled (i) by the owner on 2026-11-03): answered to
/// a page with a parsed `Origin` and an agreeing originator, with no row,
/// as the reference's manager gates none of them (`WPM:5492-5568`). Each
/// with the methods its route answers (the router's map in `make_router`;
/// `HEAD` rides `GET`): a call by another method is the router's `405`,
/// neither counted nor charged (#556). The test
/// `the_open_doors_methods_are_the_routers` pins the two maps together.
pub(crate) const OPEN_DOORS: [(&str, &[Method]); 6] = [
    ("/isAuthenticated", GET_OR_POST),
    ("/waitForAuthentication", GET_OR_POST),
    ("/getHeight", GET_OR_POST),
    ("/getHeaderForHeight", &[Method::POST]),
    ("/getNetwork", GET_OR_POST),
    ("/getVersion", GET_OR_POST),
];

/// The status doors' methods: `GET` (with its `HEAD`) and `POST`.
const GET_OR_POST: &[Method] = &[Method::GET, Method::HEAD, Method::POST];

/// Whether `route` is an open door answering `method`: the call the bound
/// charges and the probe record counts.
fn open_door_answers(route: &str, method: &Method) -> bool {
    OPEN_DOORS
        .iter()
        .any(|(door, methods)| *door == route && methods.contains(method))
}

/// The seed's notice, logged once at start beside each entry (question 6).
pub const SEED_NOTICE: &str = "ALLOWED_ORIGINS is a seed: it admits its listed originators until \
                               it is retired; the owner's grants are the rows, listed by \
                               bsv-wallet origins";

/// The seed (`ALLOWED_ORIGINS`): the originators admitted at every door
/// and host route during the seed window, each a host with an optional
/// port, lowercased, no scheme; empty seeds none.
#[derive(Clone, Debug, Default)]
pub struct AllowedOrigins(Arc<[String]>);

impl AllowedOrigins {
    /// The list of originators (`localhost`, `app.example:8443`),
    /// lowercased; `ALLOWED_ORIGINS` is checked at start by the same rule
    /// (`config::read`).
    ///
    /// # Errors
    ///
    /// On an entry that is not an originator (a scheme, a path, a name that
    /// is not a hostname), the [`OriginatorRule`] it breaks, in the text the
    /// binary refuses `ALLOWED_ORIGINS` with: an entry is never stripped to
    /// its host (#433).
    pub fn new<I: IntoIterator<Item = S>, S: Into<String>>(
        originators: I,
    ) -> Result<Self, OriginatorRule> {
        originators
            .into_iter()
            .map(|entry| crate::config::originator::listed(&entry.into()))
            .collect::<Result<_, _>>()
            .map(Self)
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

    /// The entry seeding `originator` (any name of its host), when one
    /// does: `origin-revoke`'s word that its admission is the seed's.
    pub fn seeds(&self, originator: &str) -> Option<&str> {
        self.admits(originator)
    }

    /// The seed's log at start (question 6): each entry by name with the
    /// notice, or the word that nothing is seeded.
    pub fn log_seed(&self) {
        if self.0.is_empty() {
            tracing::info!(
                marker = "origin_grants_seed",
                "origin grants: ALLOWED_ORIGINS is unset: no originator is seeded; every page is \
                 asked per power"
            );
            return;
        }
        for entry in self.0.iter() {
            tracing::info!(
                marker = "origin_grants_seed",
                entry = %entry,
                "origin grants: ALLOWED_ORIGINS lists {entry}; during the seed window it is \
                 admitted at every door as (b) admits it, with no grant row; its powers are \
                 asked of the owner once the seed is retired (#414 L7)"
            );
        }
        tracing::info!(marker = "origin_grants_seed", "{SEED_NOTICE}");
    }
}

/// The CORS layer: every answer to an `Origin` that parses carries its
/// echo, the held call's and the refusal's included, so a page reads its
/// grant's answer and its denial, as the client's `*` lets it
/// (`electron/httpServer.ts:40-46` at bsv-desktop `b791cb2`); never `*`.
/// The methods and headers are mirrored as before (#138).
pub(crate) fn cors() -> CorsLayer {
    let parses = |origin: &HeaderValue, _: &axum::http::request::Parts| {
        origin.to_str().ok().and_then(originator_of).is_some()
    };
    // A page reads `Retry-After` on a 503 STORE_BUSY (#44) and a 429.
    CorsLayer::new()
        .allow_credentials(true)
        .allow_headers(AllowHeaders::mirror_request())
        .allow_methods(AllowMethods::mirror_request())
        .allow_origin(AllowOrigin::predicate(parses))
        .expose_headers([axum::http::header::RETRY_AFTER])
}

impl super::ServerConfig {
    /// The default settings with the browser gate admitting `originators`, as
    /// `ALLOWED_ORIGINS` would (the tests' and the harnesses' servers).
    ///
    /// # Errors
    ///
    /// The [`OriginatorRule`] an entry breaks, as [`AllowedOrigins::new`].
    #[allow(dead_code)] // the binary reads `ALLOWED_ORIGINS`; the tests build it
    pub fn admitting<I: IntoIterator<Item = S>, S: Into<String>>(
        originators: I,
    ) -> Result<Self, OriginatorRule> {
        Ok(Self {
            allowed_origins: AllowedOrigins::new(originators)?,
            ..Self::default()
        })
    }
}

/// The gate's refusals before any power is asked (`BrowserGate.Word`); a
/// call admitted is its `Origin`.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Refused {
    /// The call names no parseable `Origin`.
    NoOrigin,
    /// The `Originator` header names another originator than the `Origin`.
    Disagrees { origin: String, claimed: String },
    /// A page that is not the seed's at a host route: no kind names one, so
    /// no page's grant reaches it.
    HostRoute {
        origin: String,
        originator: String,
        path: String,
    },
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
            Refused::HostRoute {
                origin,
                originator,
                path,
            } => {
                tracing::info!(origin = %origin, originator = %originator, path = %path, "browser gate: a host route answers the owner or the seed, never a page's grant; refused");
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

/// Whether another field's name agrees with the `Origin`'s originator:
/// both in the lookup form, the lowercased hostname (`prepareOriginator`'s
/// `normalized`, `WPM:5776`, `:5786`), as the admission compares an entry
/// (#448). A key covering every port of a host never splits one caller by
/// its port; a name of another host disagrees.
fn agrees(name: &str, originator: &str) -> bool {
    let claimed = claimed_originator(name).and_then(|c| hostname_of(&c));
    claimed.is_some() && claimed == hostname_of(originator)
}

/// The `Origin` header and its originator, when it is one a URL parser
/// reads with a host.
fn origin_of(headers: &HeaderMap) -> Option<(&str, String)> {
    let origin = headers.get("origin")?.to_str().ok()?;
    Some((origin, originator_of(origin)?))
}

/// The gate on a call's headers: the admitted call, or the refusal. The
/// order is `BrowserGate.gate`'s: the `Origin`, the other field's
/// agreement (by hostname), then the seed, which never refuses: a page it
/// does not list is asked per power.
pub(crate) fn judge(headers: &HeaderMap, allowed: &AllowedOrigins) -> Result<Admitted, Refused> {
    let (origin, originator) = origin_of(headers).ok_or(Refused::NoOrigin)?;
    if let Some(claimed) = headers.get("originator") {
        let claimed = String::from_utf8_lossy(claimed.as_bytes()).into_owned();
        if !agrees(&claimed, &originator) {
            return Err(Refused::Disagrees {
                origin: origin.to_string(),
                claimed,
            });
        }
    }
    Ok(Admitted {
        origin: origin.to_string(),
        seed: allowed.admits(&originator).map(str::to_string),
        originator,
    })
}

/// The admitted call: its `Origin` as sent, its originator, and the seed's
/// entry that admits it during the seed window (none: a page asked per
/// power), for the log, the body's check and the doors' powers.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Admitted {
    pub(crate) origin: String,
    pub(crate) originator: String,
    pub(crate) seed: Option<String>,
}

/// The path a call is routed by: the route's pattern (`/tx/{txid}`), or the
/// path itself where no route matched.
fn route_of(request: &Request) -> &str {
    request
        .extensions()
        .get::<MatchedPath>()
        .map_or_else(|| request.uri().path(), MatchedPath::as_str)
}

/// The gate, after the bearer and before the body is read: the owner at a
/// host route; the `Origin` and the agreement; then the seed (admitted at
/// every door and host route, logged with its entry), a host route a page
/// asks (refused), an open door by a method its route answers (answered
/// within its bound, W15, #541; another method is the router's `405`,
/// uncharged, #556), and every other door (its powers asked by its
/// handler). Inside the CORS
/// layer, so a page reads its refusal.
pub(crate) async fn gate(
    axum::Extension(allowed): axum::Extension<AllowedOrigins>,
    mut request: Request,
    next: Next,
) -> Response {
    let route = route_of(&request).to_string();
    if route == EXEMPT {
        return next.run(request).await;
    }
    let host_route = HOST_ROUTES.contains(&route.as_str());
    if host_route && super::owner_door::refusal(request.method(), request.headers()).is_none() {
        tracing::info!(path = %route, "browser gate: the owner's admission at a host route");
        return next.run(request).await;
    }
    let admitted = match judge(request.headers(), &allowed) {
        Ok(admitted) => admitted,
        Err(refused) => return refused.answer(),
    };
    match &admitted.seed {
        Some(entry) => tracing::info!(
            origin = %admitted.origin,
            originator = %admitted.originator,
            entry = %entry,
            path = %route,
            "browser gate: admitted"
        ),
        None if host_route => {
            return Refused::HostRoute {
                origin: admitted.origin,
                originator: admitted.originator,
                path: route,
            }
            .answer()
        }
        None if open_door_answers(&route, request.method()) => {
            let probes = request.extensions().get::<Probes>().cloned();
            let bounds = request.extensions().get::<OpenBounds>().cloned();
            let open = super::powers::open(
                &admitted,
                &probes.unwrap_or_default(),
                &bounds.unwrap_or_default(),
                &route,
                Class::Open,
                None,
            );
            if let Err(limited) = open {
                return limited.into_response();
            }
        }
        None => {}
    }
    request.extensions_mut().insert(admitted);
    next.run(request).await
}

/// The preflight's gate, outside the CORS layer: an `OPTIONS` with no
/// `Origin` that parses is refused before CORS answers it; every other is
/// answered (a page's opening `application/json` call must pass its
/// preflight to reach its prompt).
pub(crate) async fn preflight(request: Request, next: Next) -> Response {
    if request.method() != Method::OPTIONS || request.uri().path() == EXEMPT {
        return next.run(request).await;
    }
    match origin_of(request.headers()) {
        Some(_) => next.run(request).await,
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
/// admitted `Origin`'s (or is not a string): the refusal. It fails closed: a
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
                .is_some_and(|c| agrees(c, &admitted.originator));
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
    use crate::config::originator;
    use crate::server::ServerConfig;
    use bsv_wallet_toolbox::Chain;

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
        AllowedOrigins::new(["App.Example", "localhost:5173"]).expect("an originator")
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

    /// The switch (#414 L5b): an originator whose hostname the seed does not
    /// name is no refusal at the gate: it is admitted with no seed, and
    /// its door asks its powers of the owner. Unset seeds nothing.
    #[test]
    fn an_originator_the_seed_does_not_name_is_asked_per_power() {
        for (origin, originator) in [
            ("http://new.example", "new.example"),
            ("http://sub.app.example", "sub.app.example"),
            ("http://app.example.evil", "app.example.evil"),
            ("http://127.0.0.1:5173", "127.0.0.1:5173"),
        ] {
            assert_eq!(
                judge(&headers(&[("origin", origin)]), &listed()),
                Ok(Admitted {
                    origin: origin.into(),
                    originator: originator.into(),
                    seed: None,
                })
            );
        }
        let none = AllowedOrigins::default();
        let admitted = judge(&headers(&[("origin", "http://app.example")]), &none);
        assert_eq!(admitted.map(|a| a.seed), Ok(None));
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
            // The stack's disagreeing-originator step 4 (#448): a port
            // beside the name is one originator under the hostname key.
            (
                "http://app.example:8080",
                Some("app.example"),
                "app.example",
            ),
            (
                "http://app.example",
                Some("app.example:8080"),
                "app.example",
            ),
            ("http://localhost:5173", Some("localhost"), "localhost:5173"),
            ("http://localhost", None, "localhost:5173"),
        ] {
            let mut pairs = vec![("origin", origin)];
            pairs.extend(claimed.map(|c| ("originator", c)));
            let admitted = judge(&headers(&pairs), &listed()).expect(origin);
            assert_eq!(admitted.seed.as_deref(), Some(entry), "{origin}");
            assert_eq!(admitted.origin, origin);
        }
        let both = AllowedOrigins::new(["localhost:5173", "localhost"]).expect("an originator");
        let at = |origin| judge(&headers(&[("origin", origin)]), &both).unwrap().seed;
        assert_eq!(
            at("http://localhost:5173").as_deref(),
            Some("localhost:5173")
        );
        assert_eq!(at("http://localhost:3000").as_deref(), Some("localhost"));
    }

    /// The scenario disagreeing-originator: another host in the
    /// `Originator` header is refused, listed or not, before the list; it
    /// is compared in the lookup form, the hostname (#448).
    #[test]
    fn a_disagreeing_originator_is_refused_before_the_list() {
        for (origin, claimed) in [
            ("http://evil.example", "app.example"),
            ("http://app.example", "evil.example"),
            ("http://app.example:8080", "evil.example:8080"),
            ("http://app.example", "sub.app.example"),
            ("http://app.example", "http://evil.example"),
            ("http://app.example", ""),
            ("http://app.example", ":8080"),
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

    /// An entry is an originator or the rule it breaks, returned, never a
    /// panic and never stripped to its host (#433): the text is the
    /// binary's refusal of the same entry in `ALLOWED_ORIGINS`.
    #[test]
    fn an_entry_with_a_scheme_is_refused_by_the_scheme_rule() {
        let e = AllowedOrigins::new(["localhost", " http://localhost "]).unwrap_err();
        assert_eq!(
            (e.entry(), e.rule()),
            ("http://localhost", originator::SCHEME)
        );
        assert_eq!(
            e.to_string(),
            format!(
                "ALLOWED_ORIGINS is \"http://localhost\", which is not {}: \
                 set it so, or unset it for the default",
                originator::SCHEME
            )
        );
        let config = ServerConfig::admitting(["https://app.example"]).err();
        assert_eq!(config.map(|e| e.rule()), Some(originator::SCHEME));
    }

    #[test]
    fn an_entry_with_a_path_is_refused_by_the_parts_rule() {
        let e = AllowedOrigins::new(["localhost/app"]).unwrap_err();
        assert_eq!((e.entry(), e.rule()), ("localhost/app", originator::PARTS));
        assert!(e.to_string().contains("a path"), "{e}");
        let config = ServerConfig::admitting(["app.example", "localhost/"]).err();
        assert_eq!(config.map(|e| e.rule()), Some(originator::PARTS));
    }

    #[test]
    fn an_entry_whose_name_is_not_a_hostname_is_refused_by_the_name_rule() {
        let e = AllowedOrigins::new(["localhost:99999"]).unwrap_err();
        assert_eq!(e.rule(), originator::NAME);
    }

    #[test]
    fn good_entries_are_admitted_lowercased() {
        let list = AllowedOrigins::new([" App.Example ", "localhost:5173", "127.0.0.1"]).unwrap();
        assert_eq!(&*list.0, ["app.example", "localhost:5173", "127.0.0.1"]);
        assert!(ServerConfig::admitting(Vec::<String>::new()).is_ok());
    }

    /// A configuration built in-process, not read from `ALLOWED_ORIGINS`
    /// (dolphinmilk's path, docs/DEPENDENTS.md): its bad entry is returned
    /// by `from_config`, never a panic.
    #[test]
    fn from_config_returns_the_rule_an_in_process_entry_breaks() {
        let mut config = crate::config::Config::default();
        config.server.allowed_origins = vec!["http://localhost:3000".into()];
        let e = ServerConfig::from_config(&Arc::new(config), Chain::Main, None).err();
        assert_eq!(e.map(|e| e.rule()), Some(originator::SCHEME));
    }

    /// A body's originator is compared in the lookup form (#448, fixture
    /// 191): `Origin: https://app.example:8080` with `originator:
    /// app.example` is one originator under the hostname key; another host
    /// on the same port is not.
    #[test]
    fn a_body_originator_agrees_by_hostname() {
        let admitted = Admitted {
            origin: "https://app.example:8080".into(),
            originator: "app.example:8080".into(),
            seed: Some("app.example".into()),
        };
        for body in [
            &br#"{"originator":"app.example"}"#[..],
            br#"{"originator":"app.example:8080"}"#,
            br#"{"originator":"app.example:9090"}"#,
        ] {
            assert!(body_disagrees(&admitted, body).is_none());
        }
        let refused = body_disagrees(&admitted, br#"{"originator":"evil.example:8080"}"#)
            .expect("another host disagrees");
        assert!(format!("{refused:?}").contains(
            "names evil.example:8080, which disagrees with the Origin header's https://app.example:8080"
        ));
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
            seed: Some("app.example".into()),
        };
        for body in [
            &br#"{"keyID":"1"}"#[..],
            br#"{"originator":"app.example"}"#,
            br#"{"originator":"app.example","keyID":"1"}"#,
            br#"{"originator":"APP.example"}"#,
            br#"{"originator":"http://app.example"}"#,
            br#"{"originator":"app.example:8080"}"#,
            br#"{"originator":"https://APP.example:8443"}"#,
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
            br#"{"originator":"evil.example:80"}"#,
            br#"{"originator":"sub.app.example"}"#,
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
