//! The reading of the environment snapshot into settings: [`Raw`], the
//! snapshot by name, and [`Reader`], which parses a setting's shape and
//! keeps every refusal (#80).

use std::cell::RefCell;
use std::str::FromStr;

use super::{shown_url, ConfigError, Raw, Secret};

/// The environment being parsed into its settings: the snapshot, and every
/// setting refused so far. A refused setting reads as unset (its default),
/// so the reading goes on and every refusal is found in one pass.
pub(crate) struct Reader<'a> {
    raw: &'a Raw,
    refused: RefCell<Vec<ConfigError>>,
}

impl<'a> Reader<'a> {
    pub(crate) fn new(raw: &'a Raw) -> Self {
        Self {
            raw,
            refused: RefCell::new(Vec::new()),
        }
    }

    /// The settings refused, in the order they were read.
    pub(crate) fn into_refused(self) -> Vec<ConfigError> {
        self.refused.into_inner()
    }

    /// `result`'s value, or `None` with its refusal kept.
    pub(super) fn kept<T>(&self, result: Result<Option<T>, ConfigError>) -> Option<T> {
        result.unwrap_or_else(|e| {
            self.refused.borrow_mut().push(e);
            None
        })
    }

    /// The value as set, when it is set.
    pub(crate) fn get(&self, name: &str) -> Option<&'a str> {
        self.raw.get(name)
    }

    /// The value when it is set and not blank, trimmed.
    pub(crate) fn given(&self, name: &str) -> Option<&'a str> {
        self.get(name).map(str::trim).filter(|v| !v.is_empty())
    }

    /// The value as set, when it is set and not empty (blank is kept).
    pub(crate) fn non_empty(&self, name: &str) -> Option<&'a str> {
        self.get(name).filter(|v| !v.is_empty())
    }

    pub(crate) fn secret(&self, name: &str) -> Option<Secret> {
        self.non_empty(name).map(Secret::new)
    }

    /// A key: the value trimmed, when it is set and not blank.
    pub(crate) fn key(&self, name: &str) -> Option<Secret> {
        self.given(name).map(Secret::new)
    }

    /// A switch: `on(value)` when set, `unset` when not.
    pub(crate) fn switch(&self, name: &str, unset: bool, on: fn(&str) -> bool) -> bool {
        self.get(name).map_or(unset, on)
    }

    /// A number: unset or blank is `None`, anything else must parse.
    pub(crate) fn number<T: FromStr>(
        &self,
        name: &'static str,
        expected: &'static str,
    ) -> Option<T> {
        self.kept(
            self.given(name)
                .map(|v| v.parse().map_err(|_| refused(name, v, expected)))
                .transpose(),
        )
    }

    /// A number with a floor: as [`Reader::number`], and a value below
    /// `floor` is refused too (`expected` names the floor), never clamped.
    pub(crate) fn number_from<T: FromStr + PartialOrd>(
        &self,
        name: &'static str,
        expected: &'static str,
        floor: T,
    ) -> Option<T> {
        self.kept(
            self.given(name)
                .map(|v| match v.parse() {
                    Ok(n) if n >= floor => Ok(n),
                    _ => Err(refused(name, v, expected)),
                })
                .transpose(),
        )
    }

    /// A URL: unset or blank is `None`, anything else must be an http or
    /// https URL with a host. The value is kept as set, trimmed.
    pub(crate) fn url(&self, name: &'static str) -> Option<String> {
        self.kept(self.given(name).map(|v| checked_url(name, v)).transpose())
    }

    /// A URL whose empty value means something (off): unset is `None`, set
    /// is the value trimmed, which must be a URL unless it is blank.
    pub(crate) fn present_url(&self, name: &'static str) -> Option<String> {
        self.kept(match self.get(name).map(str::trim) {
            Some(v) if !v.is_empty() => checked_url(name, v).map(Some),
            other => Ok(other.map(str::to_string)),
        })
    }

    /// A list of exact origins (`scheme://host[:port]`, comma-separated):
    /// unset or blank is none. Each entry, trimmed, must be an http or https
    /// origin written as a browser sends it (no path, query, fragment or
    /// userinfo, no wildcard, the default port and upper case left out); any
    /// other refuses the whole list, so a typo never admits less or more
    /// than the operator wrote (#411).
    pub(crate) fn origins(&self, name: &'static str) -> Vec<String> {
        let Some(value) = self.given(name) else {
            return Vec::new();
        };
        let checked: Result<Vec<String>, ConfigError> = value
            .split(',')
            .map(str::trim)
            .filter(|entry| !entry.is_empty())
            .map(|entry| checked_origin(name, entry))
            .collect();
        self.kept(checked.map(Some)).unwrap_or_default()
    }

    /// An API base: as [`Reader::url`], the value without its trailing
    /// slashes.
    pub(crate) fn base(&self, name: &'static str) -> Option<String> {
        self.url(name).as_deref().map(without_trailing_slashes)
    }

    /// An API base whose empty value means something (left out): as
    /// [`Reader::present_url`], the value without its trailing slashes.
    pub(crate) fn present_base(&self, name: &'static str) -> Option<String> {
        self.present_url(name)
            .as_deref()
            .map(without_trailing_slashes)
    }
}

fn without_trailing_slashes(value: &str) -> String {
    value.trim_end_matches('/').to_string()
}

fn refused(variable: &'static str, value: &str, expected: &'static str) -> ConfigError {
    ConfigError {
        variable,
        value: value.to_string(),
        expected: expected.into(),
    }
}

/// An origin as a browser serializes it in its `Origin` header: the entry
/// must equal its own serialization, so `http://localhost/`, `*`,
/// `localhost` and `HTTP://Localhost:80` are refused by name.
fn checked_origin(name: &'static str, entry: &str) -> Result<String, ConfigError> {
    match url::Url::parse(entry) {
        Ok(u)
            if matches!(u.scheme(), "http" | "https")
                && u.has_host()
                && !entry.contains('*')
                && u.origin().ascii_serialization() == entry =>
        {
            Ok(entry.to_string())
        }
        _ => Err(refused(
            name,
            entry,
            "exact origins (scheme://host[:port], comma-separated, no wildcard)",
        )),
    }
}

fn checked_url(name: &'static str, value: &str) -> Result<String, ConfigError> {
    match url::Url::parse(value) {
        Ok(u) if matches!(u.scheme(), "http" | "https") && u.has_host() => Ok(value.to_string()),
        _ => Err(refused(name, &shown_url(value), "an http or https URL")),
    }
}
