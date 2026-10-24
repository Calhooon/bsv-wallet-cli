//! The reading of the environment snapshot into settings: [`Raw`], the
//! snapshot by name, and [`Reader`], which parses a setting's shape and
//! keeps every refusal (#80).

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::fmt;
use std::str::FromStr;

use super::{shown_url, ConfigError, Secret};

/// The environment as it was read at start, by name: what each group
/// parses, what [`Source`] and the fleet's uniformity check read. Its
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
    fn kept<T>(&self, result: Result<Option<T>, ConfigError>) -> Option<T> {
        result.unwrap_or_else(|e| {
            self.refused.borrow_mut().push(e);
            None
        })
    }

    /// The value as set, when it is set.
    pub(crate) fn get(&self, name: &str) -> Option<&'a str> {
        self.raw.0.get(name).map(String::as_str)
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
}

fn refused(variable: &'static str, value: &str, expected: &'static str) -> ConfigError {
    ConfigError {
        variable,
        value: value.to_string(),
        expected,
    }
}

fn checked_url(name: &'static str, value: &str) -> Result<String, ConfigError> {
    match url::Url::parse(value) {
        Ok(u) if matches!(u.scheme(), "http" | "https") && u.has_host() => Ok(value.to_string()),
        _ => Err(refused(name, &shown_url(value), "an http or https URL")),
    }
}
