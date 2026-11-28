//! The spend probe's roads under R9 (#660): what the wallet library's
//! `Services::is_utxo_with_spender` answers for each missing binding, as a
//! `MockWalletServices` answers it. The host builds no `WalletServices` of
//! its own outside tests (`context.rs`, `daemon_tenant.rs`, `tick.rs` and
//! `services.rs` construct the library's `Services`), so these are the
//! answers its roads read. At toolbox =0.14.0-beta.10 the library binds a
//! named spender in `bind_spender` (`src/services/services.rs:1715-1749`:
//! its bytes from `get_raw_tx`, hashed by each provider, must spend the
//! outpoint) and confirms it only on `get_merkle_path`'s path, whose root
//! it computes from the spender's txid and asks of the header service
//! (`:2057-2240`); `is_utxo_with_spender` (`:2902-2960`) maps the outcome.

use bsv_wallet_toolbox::services::mock::{MockErrorKind, MockResponse, MockWalletServices};
use bsv_wallet_toolbox::services::{BoundSpender, UtxoVerdict};

/// The spender every case names.
pub(crate) fn spender() -> String {
    "98".repeat(32)
}

/// One case: its name, the library's answer, and whether a coin may move.
pub(crate) struct Case {
    pub(crate) name: &'static str,
    pub(crate) honest: bool,
    answer: fn() -> MockResponse<(UtxoVerdict, Option<BoundSpender>)>,
}

impl Case {
    /// The services answering this case on every probe.
    pub(crate) fn services(&self) -> MockWalletServices {
        MockWalletServices::builder()
            .is_utxo_with_spender_response((self.answer)())
            .build()
    }
}

/// A named spender its bytes could not bear out: could not look.
fn could_not_look() -> MockResponse<(UtxoVerdict, Option<BoundSpender>)> {
    MockResponse::Success((UtxoVerdict::Unknown, None))
}

/// A spender bound by its bytes whose path was dropped: unconfirmed.
fn unconfirmed() -> MockResponse<(UtxoVerdict, Option<BoundSpender>)> {
    MockResponse::Success((
        UtxoVerdict::SpentHint,
        Some(BoundSpender {
            txid: spender(),
            confirmed: false,
        }),
    ))
}

/// The five plants and the honest case.
pub(crate) fn cases() -> Vec<Case> {
    vec![
        // (i) the named spender's bytes do not hash to its txid: no bytes
        // are held, `Unreadable`, could not look.
        Case {
            name: "(i) bytes that do not hash to the spender's txid",
            honest: false,
            answer: could_not_look,
        },
        // (ii) bytes that hash right and spend another outpoint:
        // `Unreadable`, could not look.
        Case {
            name: "(ii) bytes that spend another outpoint",
            honest: false,
            answer: could_not_look,
        },
        // (iii) the path's leaf is another transaction of the block: the
        // root from the spender's txid cannot be computed, the path is
        // dropped, the spender is bound by bytes alone, unconfirmed.
        Case {
            name: "(iii) a path whose leaf is another transaction",
            honest: false,
            answer: unconfirmed,
        },
        // (iv) a root our headers do not carry: the path is dropped,
        // unconfirmed.
        Case {
            name: "(iv) a root our headers do not carry",
            honest: false,
            answer: unconfirmed,
        },
        // (v) a provider's fault in place of an answer.
        Case {
            name: "(v) a provider's fault",
            honest: false,
            answer: || {
                MockResponse::Error(MockErrorKind::ServiceError, "planted fault".to_string())
            },
        },
        // The honest case: bytes, outpoint and root all check.
        Case {
            name: "the spender's own bytes, outpoint and root",
            honest: true,
            answer: || {
                MockResponse::Success((
                    UtxoVerdict::Spent,
                    Some(BoundSpender {
                        txid: spender(),
                        confirmed: true,
                    }),
                ))
            },
        },
    ]
}
