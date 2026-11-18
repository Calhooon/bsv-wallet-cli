//! The Genesis height `internalizeAction` judges a received spend by.
//!
//! From bsv-wallet-toolbox-rs 0.13.7 the door runs every input of every
//! transaction of a received AtomicBEEF that the BEEF does not prove, and
//! "an input spending a coin proven below the network's Genesis height
//! (mainnet 620,538), or a spend below it, is not judged and is refused"
//! (its CHANGELOG, 0.13.7, Hosts, **The rule**): `input <i> not judged:
//! coin height <h> is before Genesis height 620538.` A command that posts
//! and then records (`split`, `gift-claim`) refuses by name before anything
//! is posted when the record cannot be made (0.18.6's principle).
//!
//! The height. The toolbox's door reads it from `activation_heights`
//! (`src/storage/received_beef/input_scripts.rs:79-89` at 0.13.7), which
//! is `pub(crate)`: not public to a host. For mainnet that function
//! answers bsv-rs's public `ProtocolEra::MAINNET_GENESIS_HEIGHT` (bsv-rs
//! 0.8.13 `src/script/flags.rs:94`), which this host reads too; for
//! testnet it answers the literal `1_344_302` (`input_scripts.rs:85`),
//! which bsv-rs does not publish, so it is carried here. The door is told
//! the wallet's `Chain` (`Wallet::internalize_action`, `src/wallet/wallet.rs:2450`
//! `with_caller_network(self.chain.as_str(), ..)`), the same `Chain` the
//! host passes here. A test at the boundary through the door
//! (`the_door_refuses_a_coin_proven_one_below_genesis_and_takes_one_at_it`)
//! holds the two numbers together.

use bsv_sdk::script::ProtocolEra;
use bsv_wallet_toolbox::Chain;

/// The Genesis height of `chain`, as the toolbox's door judges by it: a
/// coin proven below it is not judged.
pub fn genesis_height(chain: Chain) -> u32 {
    match chain {
        // bsv-rs 0.8.13 `src/script/flags.rs:94`, read by the toolbox's
        // `activation_heights` (`input_scripts.rs:81-84`).
        Chain::Main => ProtocolEra::MAINNET_GENESIS_HEIGHT,
        // bsv-wallet-toolbox-rs 0.13.7
        // `src/storage/received_beef/input_scripts.rs:85`.
        Chain::Test => 1_344_302,
    }
}

#[cfg(test)]
mod tests;
