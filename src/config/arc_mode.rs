//! The broadcaster mode (#300): Arcade by default, classic the one opt-out.

use super::{ConfigError, Raw, Reader, Source};

#[cfg(test)]
mod tests;

/// The broadcaster mode (`ARC_MODE`, or the older `ARCADE` when it is
/// unset).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArcMode {
    /// Arcade V2, the default (the owner's word of 2026-10-27, #300: "arc is
    /// old af... we use arcade now for everyting").
    Arcade,
    /// The toolbox's classic ARC ladder, deprecated: "arc is just deprecated,
    /// arcade is the standard" (the owner's wording of 2026-10-27). Kept for
    /// the hermetic stub, which answers classic ARC only, until #299 gives it
    /// an Arcade face; never a default or a failover.
    Classic,
}

impl ArcMode {
    /// The mode from `ARC_MODE` and `ARCADE`, each blank as unset.
    /// `ARC_MODE` wins when set: `arcade` or `classic` (any case), any other
    /// value refused by name. Unset, `ARCADE` decides: `1`, `true` or `yes`
    /// (any case) is Arcade, `0`, `false` or `no` classic, any other value
    /// refused by name. Both unset, or a refused value, is Arcade.
    pub(super) fn read(raw: &Reader) -> Self {
        let mode = match raw.given("ARC_MODE") {
            Some(mode) => match mode.to_ascii_lowercase().as_str() {
                "arcade" => Ok(Some(Self::Arcade)),
                "classic" => Ok(Some(Self::Classic)),
                _ => Err(ConfigError {
                    variable: "ARC_MODE",
                    value: mode.to_string(),
                    expected: "a broadcaster mode: `arcade` (the default) or `classic` \
                               (deprecated, the hermetic stub's until #299)",
                }),
            },
            None => match raw.given("ARCADE").map(str::to_ascii_lowercase).as_deref() {
                None => Ok(None),
                Some("1" | "true" | "yes") => Ok(Some(Self::Arcade)),
                Some("0" | "false" | "no") => Ok(Some(Self::Classic)),
                Some(_) => Err(ConfigError {
                    variable: "ARCADE",
                    value: raw.get("ARCADE").unwrap_or_default().to_string(),
                    expected: "a broadcaster switch: `1`, `true` or `yes` (Arcade, the \
                               default) or `0`, `false` or `no` (classic); prefer ARC_MODE",
                }),
            },
        };
        raw.kept(mode).unwrap_or(Self::Arcade)
    }
}

/// Where the mode in effect came from: `ARC_MODE`'s, or, unset, `ARCADE`'s.
pub(super) fn source(raw: &Raw) -> Source {
    if raw.get("ARC_MODE").is_some() || raw.get("ARCADE").is_some() {
        Source::Env
    } else {
        Source::Default
    }
}
