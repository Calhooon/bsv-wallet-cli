//! An `ALLOWED_ORIGINS` entry: an originator in the reference's form
//! (#411, the owner's word of 2026-11-02 on the key). ts-stack edf6e03
//! `WalletPermissionsManager.ts` `prepareOriginator` (`:5750-5795`): an
//! originator is a domain name with an optional port, without a URL scheme;
//! credentials, a path, a query or a fragment are refused; the name is a
//! DNS hostname or `localhost`. Its grants are scoped to the lowercased
//! hostname, every port of a host sharing one (`:5702-5729`).

/// The rule an entry is refused by, in the reference's words.
const SCHEME: &str = "an originator: a domain name with an optional port, without a URL scheme \
                      (`localhost`, `app.example:8443`; the reference's prepareOriginator); \
                      this entry carries a scheme";
const PARTS: &str = "an originator: a domain name with an optional port, without credentials, \
                     a path, a query parameter or a fragment (the reference's prepareOriginator)";
const NAME: &str = "an originator: a DNS hostname, `localhost` or an IP address, with an optional \
                    port (the reference's prepareOriginator)";

/// An entry checked as the reference's `prepareOriginator` checks an
/// originator, lowercased; the refusal names the rule. One divergence,
/// named: the reference refuses an IP address (`:5777-5779`), and a list
/// may name one (`127.0.0.1`, `[::1]`), the owner's loopback pages.
///
/// # Errors
///
/// The rule the entry breaks, as the phrase a [`super::ConfigError`]
/// names: a scheme; credentials, a path, a query or a fragment; a name
/// that is not a hostname, or a port that is not one.
pub fn checked(entry: &str) -> Result<String, &'static str> {
    let entry = entry.trim().to_ascii_lowercase();
    if has_scheme(&entry) {
        return Err(SCHEME);
    }
    if entry
        .chars()
        .any(|c| matches!(c, '/' | '?' | '#' | '@' | '\\') || c.is_whitespace())
    {
        return Err(PARTS);
    }
    // A port, when named, is 1 to 5 digits up to 65535 (`localhost:` is
    // refused, not read as `localhost`).
    let port = match entry.rfind([':', ']']) {
        Some(at) if entry.as_bytes()[at] == b':' => Some(&entry[at + 1..]),
        _ => None,
    };
    if port.is_some_and(|p| !(1..=5).contains(&p.len()) || p.parse::<u16>().is_err()) {
        return Err(NAME);
    }
    let url = url::Url::parse(&format!("https://{entry}")).map_err(|_| NAME)?;
    let host = url.host().ok_or(NAME)?;
    let named = match host {
        url::Host::Domain(name) => {
            name == "localhost"
                || name.split('.').all(|label| {
                    let b = label.as_bytes();
                    (1..=63).contains(&b.len())
                        && b[0].is_ascii_alphanumeric()
                        && b[b.len() - 1].is_ascii_alphanumeric()
                        && b.iter().all(|c| c.is_ascii_alphanumeric() || *c == b'-')
                })
        }
        url::Host::Ipv4(_) | url::Host::Ipv6(_) => true,
    };
    if named {
        Ok(entry)
    } else {
        Err(NAME)
    }
}

/// `^[a-zA-Z][a-zA-Z\d+\-.]*:\/\/`, the reference's test for a scheme
/// (`:5758`).
#[must_use]
pub fn has_scheme(name: &str) -> bool {
    let Some((scheme, _)) = name.split_once("://") else {
        return false;
    };
    scheme
        .chars()
        .next()
        .is_some_and(|c| c.is_ascii_alphabetic())
        && scheme
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.'))
}
