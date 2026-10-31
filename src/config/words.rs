//! The words a setting is shown and refused in, shared by both binaries'
//! configurations: [`Secret`], which prints `<set>`; [`ConfigError`], a
//! refusal by name; [`shown_url`], a URL with its credentials `<set>`; and
//! [`Raw`], the variables as read. `bsv-wallet-mcp`'s `mcp/src/config.rs`
//! includes this file by its path (#171), so the companion prints and
//! refuses its settings in the main crate's words without depending on the
//! crate; it uses `std` alone, and reads no environment (each crate's
//! config module does, the architecture rule A7).

use std::borrow::Cow;
use std::collections::BTreeMap;
use std::fmt;

/// A key or a token: its `Debug` and `Display` print `<set>`, so a log
/// line, a panic or `bsv-wallet config` never carries it.
#[derive(Clone, PartialEq, Eq)]
pub struct Secret(String);

impl Secret {
    /// A secret holding `value`.
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    /// The value itself, for the one place that sends it.
    #[must_use]
    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for Secret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("<set>")
    }
}

impl fmt::Display for Secret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("<set>")
    }
}

/// A setting refused at start: the variable, the value it holds (a URL as
/// [`shown_url`] shows it), and the shape it must have. Never built for a
/// [`Secret`], whose value is not parsed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfigError {
    /// The variable's name.
    pub variable: &'static str,
    /// The value it holds (a URL's credentials `<set>`).
    pub value: String,
    /// The shape it must have: a fixed phrase, or a library's own words
    /// for a value it validates (`TRUST_SETTINGS`, the wallet settings
    /// manager's refusal, #331).
    pub expected: Cow<'static, str>,
}

impl fmt::Display for ConfigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{} is {:?}, which is not {}: set it so, or unset it for the default",
            self.variable, self.value, self.expected
        )
    }
}

impl std::error::Error for ConfigError {}

/// A URL as it may be shown: its userinfo and its query (or fragment) are
/// `<set>`, its scheme, host, port and path as set. Read as text, so a
/// value that does not parse is shown the same way (a credential in a URL
/// is never printed, #80).
#[must_use]
pub fn shown_url(value: &str) -> String {
    let (scheme, rest) = value
        .split_once("://")
        .map_or(("", value), |(scheme, rest)| (scheme, rest));
    let (authority, tail) = rest.split_at(rest.find(['/', '?', '#']).unwrap_or(rest.len()));
    let mut shown = String::new();
    if !scheme.is_empty() {
        shown.push_str(scheme);
        shown.push_str("://");
    }
    match authority.rfind('@') {
        Some(at) => {
            shown.push_str("<set>@");
            shown.push_str(&authority[at + 1..]);
        }
        None => shown.push_str(authority),
    }
    match tail.find(['?', '#']) {
        Some(q) => {
            shown.push_str(&tail[..q]);
            shown.push_str(&tail[q..=q]);
            shown.push_str("<set>");
        }
        None => shown.push_str(tail),
    }
    shown
}

/// The environment as it was read at start, by name: what each setting is
/// parsed from, what `Source` and the fleet's uniformity check read. Its
/// `Debug` prints how many variables it holds, never a name or a value.
#[derive(Clone, Default)]
pub(crate) struct Raw(BTreeMap<String, String>);

impl fmt::Debug for Raw {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Raw({} variables)", self.0.len())
    }
}

impl Raw {
    pub(crate) fn read(vars: impl IntoIterator<Item = (String, String)>) -> Self {
        Self(vars.into_iter().collect())
    }

    /// The value as set, when it is set.
    pub(crate) fn get(&self, name: &str) -> Option<&str> {
        self.0.get(name).map(String::as_str)
    }
}
