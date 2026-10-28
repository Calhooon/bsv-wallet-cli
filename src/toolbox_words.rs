//! The toolbox's refusals that reach this crate as words only (#81).
//!
//! The stack pins are `bsv-rs` =0.9.0-beta.2,
//! `bsv-wallet-toolbox-rs` =0.14.0-beta.2 and `bsv-tracker` =0.9.0-beta.2.
//! The toolbox converts its error at the
//! `WalletInterface` boundary (`impl From<Error> for bsv_rs::Error`, its
//! `src/error.rs`): `StoreBusy` becomes bsv-rs's typed `WalletBusy`, an
//! `SdkError` passes its bsv-rs error through, `ReviewActions` keeps its
//! typed carrier, and every other variant
//! becomes `WalletError(<its Display>)`; its wallet also builds
//! `WalletError(format!(..))` itself. Every served door and the spend gate
//! call the wallet through the trait, so `InsufficientFunds`,
//! `ValidationError`, `InvalidBeef`, `UnspendableCoin` and the rest arrive
//! as a `WalletError`'s text, their variant erased. This module is the one
//! place in the crate where an error's words are read: the served layer
//! (`src/server/error.rs`) and the gate map every typed error by its
//! variant and ask here only for a `WalletError`'s text.
//!
//! [`WORDS`] reads the pinned toolbox's Display prefixes, including the
//! SDK's `SpendRefused` rendering (`spend refused at offset`). The abort
//! door also reads two accepting-word sentences: `records an acceptance
//! and cannot be aborted` and `has a recorded acceptance (`. Only those
//! sentences name Conflict and the accepting-word hint. The common
//! `cannot abort action:` prefix also introduces failed-status, incoming
//! and spent-output refusals; those keep beta's unclassified 400 ERROR
//! without a hint. No retired network-presence sentence is read.
//!
//! Published toolbox 4cfe31ca still erases ordinary refusals at
//! `src/error.rs:352-365`. CLI #330 tracks their typed host map (toolbox
//! #404). Every prefix below and both accepting-abort sentences were read
//! against beta.2's Display and formatting sites: none changed. The four
//! not-found doors, INVALID_BEEF and UNSPENDABLE_COIN keep their served
//! classes until that typed conversion lands; no new text match is added.
//!
//! Two words of the table, `insufficient funds:` and `not found:`, are
//! produced by neither pinned library; they stay
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
/// words are the Display texts at =0.14.0-beta.2 up to their variable
/// fields; they do not overlap, so the order decides nothing. Accepting
/// abort sentences are read separately, never by their common prefix.
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
    if accepting_abort(text) {
        return Erased::Conflict;
    }
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

/// The erased accepting-word refusal at the abort door (#165).
/// CLI #330 tracks replacing this read when the toolbox preserves its type.
#[must_use]
pub fn is_abort_on_chain_evidence(e: &bsv_sdk::Error) -> bool {
    matches!(e, bsv_sdk::Error::WalletError(text) if accepting_abort(text))
}

fn accepting_abort(text: &str) -> bool {
    text.split_once("cannot abort action: ")
        .is_some_and(|(kind, refusal)| {
            kind == "Invalid transaction status: "
                && (refusal.contains("records an acceptance and cannot be aborted")
                    || refusal.contains("has a recorded acceptance ("))
        })
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
    fn only_accepting_abort_sentences_name_conflict_and_a_hint() {
        for (refusal, accepting) in [
            ("action with status 'unproven' records an acceptance and cannot be aborted", true),
            ("transaction aa has a recorded acceptance (tracker word 'seen'); only a transaction never accepted can be retired", true),
            ("action with status 'failed' cannot be aborted", false),
            ("must be an outgoing transaction", false),
            ("transaction has outputs that have been spent", false),
            ("transaction with status 'sending' has no txid, so its stored acceptance cannot be checked", false),
            ("transaction aa has a proven descendant bb, so it is on chain", false),
        ] {
            let text = format!("Invalid transaction status: cannot abort action: {refusal}");
            assert_eq!(class(&text), if accepting { Erased::Conflict } else { Erased::Unread }, "{refusal}");
            assert_eq!(is_abort_on_chain_evidence(&bsv_sdk::Error::WalletError(text)), accepting, "{refusal}");
        }
        assert_eq!(class("Invalid transaction status: cannot retire transaction: transaction aa has a recorded acceptance (tracker word 'seen')"), Erased::Unread);
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
