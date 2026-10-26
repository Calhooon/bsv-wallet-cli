//! The toolbox's refusals that reach this crate as words only (#81).
//!
//! `bsv-wallet-toolbox-rs` =0.14.0-beta.1 converts its error at the
//! `WalletInterface` boundary (`impl From<Error> for bsv_rs::Error`, its
//! `src/error.rs`): `StoreBusy` becomes bsv-rs's typed `WalletBusy`, an
//! `SdkError` passes its bsv-rs error through, and every other variant
//! becomes `WalletError(<its Display>)`; its wallet also builds
//! `WalletError(format!(..))` itself. Every served door and the spend gate
//! call the wallet through the trait, so `InsufficientFunds`,
//! `ValidationError`, `InvalidBeef`, `UnspendableCoin` and the rest arrive
//! as a `WalletError`'s text, their variant erased. This module is the one
//! place in the crate where an error's words are read: the served layer
//! (`src/server/error.rs`) and the gate map every typed error by its
//! variant and ask here only for a `WalletError`'s text.
//!
//! What would type each, to be filed upstream (the lane files nothing):
//!
//! - bsv-rs: a wallet refusal that carries its class (for instance
//!   `WalletError { class, detail }`, the class one of the toolbox's
//!   refusal kinds), since bsv-rs's `Error` is `Clone + Eq` and cannot
//!   hold the toolbox's error itself.
//! - bsv-wallet-toolbox-rs: its `From<Error> for bsv_rs::Error` mapping
//!   each variant [`WORDS`] reads to that class; and the abort refusal on
//!   chain evidence (`InvalidTransactionStatus("cannot abort action: the
//!   network has transaction ..")`, its `abort_action.rs`) a variant of its
//!   own.
//!
//! Two words of the table, `insufficient funds:` and `not found:`, are
//! produced by neither library at the pins (=0.13.0, =0.8.2); they stay
//! as `AppError::classify` had them, so no answer moves.

/// The class of a toolbox refusal, as its words name it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Erased {
    /// `InsufficientFunds`.
    InsufficientFunds,
    /// `AuthenticationRequired`, `InvalidIdentityKey`.
    AuthenticationRequired,
    /// `AccessDenied`.
    AccessDenied,
    /// `NotFound`.
    NotFound,
    /// `UnspendableCoin`.
    UnspendableCoin,
    /// `Duplicate`, `InvalidOperation`.
    Conflict,
    /// `InvalidBeef`.
    InvalidBeef,
    /// `ValidationError`, `InvalidArgument`.
    Validation,
    /// `StorageError`, `DatabaseError`, `Internal`.
    Internal,
    /// `ServiceError`, `NetworkError`, `BroadcastFailed`.
    Service,
    /// Any other words: no class.
    Unread,
}

/// Each word a toolbox Display begins with, and the class it names. The
/// words are the toolbox's `#[error(..)]` texts at =0.13.0 up to their
/// first field; they do not overlap, so the order decides nothing.
pub const WORDS: &[(&str, Erased)] = &[
    ("Insufficient funds:", Erased::InsufficientFunds),
    ("insufficient funds:", Erased::InsufficientFunds),
    ("Authentication required", Erased::AuthenticationRequired),
    ("Invalid identity key:", Erased::AuthenticationRequired),
    ("Access denied:", Erased::AccessDenied),
    ("Entity not found:", Erased::NotFound),
    ("not found:", Erased::NotFound),
    ("Unspendable coin:", Erased::UnspendableCoin),
    ("Duplicate entity:", Erased::Conflict),
    ("Invalid operation:", Erased::Conflict),
    ("Invalid BEEF at byte", Erased::InvalidBeef),
    ("spend refused at offset", Erased::InvalidBeef),
    (
        "Invalid transaction status: cannot abort action:",
        Erased::Conflict,
    ),
    ("Validation error:", Erased::Validation),
    ("Invalid argument:", Erased::Validation),
    ("Storage error:", Erased::Internal),
    ("Database error:", Erased::Internal),
    ("Internal error:", Erased::Internal),
    ("Service error:", Erased::Service),
    ("Network error:", Erased::Service),
    ("Broadcast failed:", Erased::Service),
];

/// The class a `WalletError`'s text names ([`WORDS`]), or
/// [`Erased::Unread`].
#[must_use]
pub fn class(text: &str) -> Erased {
    WORDS
        .iter()
        .find(|(word, _)| text.starts_with(word))
        .map_or(Erased::Unread, |(_, class)| *class)
}

/// The coin an `Unspendable coin: <txid.vout>: ..` refusal names.
#[must_use]
pub fn unspendable_coin(text: &str) -> Option<String> {
    let at = text.find("Unspendable coin: ")? + "Unspendable coin: ".len();
    text[at..]
        .split(':')
        .next()
        .map(str::trim)
        .filter(|c| !c.is_empty())
        .map(str::to_string)
}

/// The toolbox's shortfall, anywhere in the error's words, any case: the
/// gate's cue to list the coins it held.
#[must_use]
pub fn is_insufficient(e: &bsv_sdk::Error) -> bool {
    e.to_string()
        .to_ascii_lowercase()
        .contains("insufficient funds")
}

/// The toolbox's `Unspendable coin:` refusal (409), anywhere in the
/// error's words.
#[must_use]
pub fn is_unspendable(e: &bsv_sdk::Error) -> bool {
    e.to_string().contains("Unspendable coin:")
}

/// The erased accepting-word refusal at the beta.1 abort door (#165).
/// The typed classes replace this word read at beta.2 (bsv-rs #137).
#[must_use]
pub fn is_abort_on_chain_evidence(e: &bsv_sdk::Error) -> bool {
    e.to_string()
        .contains("Invalid transaction status: cannot abort action:")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Each word names its class at the start of the text only.
    #[test]
    fn a_word_names_its_class_at_the_start() {
        for (word, class_of) in WORDS {
            assert_eq!(class(&format!("{word} x")), *class_of, "{word}");
            assert_eq!(class(&format!("wallet error: {word} x")), Erased::Unread);
        }
        assert_eq!(class("anything else"), Erased::Unread);
    }

    #[test]
    fn the_unspendable_coin_is_named() {
        assert_eq!(
            unspendable_coin("Unspendable coin: aa.1: its stored ancestry").as_deref(),
            Some("aa.1")
        );
        assert_eq!(unspendable_coin("Unspendable coin: : x"), None);
        assert_eq!(unspendable_coin("Validation error: x"), None);
    }
}
