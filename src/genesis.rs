//! The Genesis height `internalizeAction` judges a received spend by, and
//! the one question this host asks of a BEEF before it posts a transaction
//! it will then record through that door (#634, the port of the stable
//! line's 0.18.9).
//!
//! The toolbox's door runs every input of every transaction of a received
//! BEEF that the BEEF does not prove, and an input spending a coin the BEEF
//! proves below the network's Genesis height is not judged and is refused:
//! `input <i> not judged: coin height <h> is before Genesis height 620538.`
//! A command that posts and then records (`split`, `gift-claim`) refuses by
//! name before anything is posted when the record cannot be made (0.18.6's
//! principle).
//!
//! The chain. On this line the door judges by its store's own `chain`
//! setting (bsv-wallet-toolbox-rs 0.14.0-beta.9
//! `src/storage/sqlx/internalize_action.rs:98`, `storage.get_settings().chain`),
//! the word `init`'s `migrate` wrote from `--testnet` (`main`, `test` or
//! `regtest`), never the `Wallet`'s `Chain`. So the host asks the same
//! store ([`door_network`]).
//!
//! The height. The door maps that word to bsv-rs's public
//! `ChainParams` (`beef_verification.rs:28-35` at that pin:
//! `main` mainnet, `test` testnet, `regtest` regtest) and reads its
//! `genesis_height`; [`genesis_height`] reads the same public item. A word
//! the door's policy does not read answers `None`: the door refuses that
//! store's every unproven BEEF by its own words, and this host holds
//! nothing for this rule.

use bsv_sdk::block::ChainParams;
use bsv_sdk::transaction::beef_stream::{display_hex, Element};
use bsv_sdk::transaction::BeefStream;
use bsv_wallet_toolbox::{StorageSqlx, WalletStorageReader};
use std::collections::HashMap;

/// The network word the door of `storage` judges by: its settings'
/// `chain`, as `internalizeAction` reads it.
pub fn door_network(storage: &StorageSqlx) -> String {
    storage.get_settings().chain
}

/// The Genesis height of the network word `network`, as the door judges
/// by it (a coin proven below it is not judged); `None` for a word the
/// door's policy does not read.
pub fn genesis_height(network: &str) -> Option<u32> {
    match network {
        "main" => Some(ChainParams::mainnet().genesis_height),
        "test" => Some(ChainParams::testnet().genesis_height),
        "regtest" => Some(ChainParams::regtest().genesis_height),
        _ => None,
    }
}

/// The Genesis height the door of `storage` judges by.
pub fn door_genesis_height(storage: &StorageSqlx) -> Option<u32> {
    genesis_height(&door_network(storage))
}

/// A transaction of a BEEF that the BEEF does not prove, with an input
/// spending a coin the BEEF proves below the Genesis height: the door
/// refuses that BEEF as not judged.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PreGenesisSpend {
    /// The unproven transaction (display hex).
    pub txid: String,
    /// The input of it that spends the coin.
    pub input: u32,
    /// The transaction whose output it spends (display hex).
    pub source_txid: String,
    /// The height the BEEF's BUMP proves the source at.
    pub coin_height: u64,
    /// The Genesis height asked.
    pub genesis_height: u32,
}

/// The earliest transaction, in the BEEF's order, of `beef` (BEEF or
/// AtomicBEEF bytes) that the BEEF does not prove (no BUMP of its own, its
/// txid in no BUMP) with an input whose source the BEEF proves at a height
/// below `genesis`; `None` when there is none, or when the bytes do not
/// read (the door's own steps refuse those). The Genesis rule alone: the
/// scripts are the door's to run.
pub fn unproven_spend_of_pre_genesis_coin(beef: &[u8], genesis: u32) -> Option<PreGenesisSpend> {
    let mut heights = HashMap::new();
    let mut unproven = Vec::new();
    for element in BeefStream::new(beef) {
        match element.ok()? {
            Element::Bump(bump) => {
                for txid in bump.proven() {
                    heights.entry(*txid).or_insert(bump.block_height);
                }
            }
            Element::Tx {
                txid,
                bump_index: None,
                body,
                ..
            } => unproven.push((txid, body)),
            _ => {}
        }
    }
    unproven
        .into_iter()
        .filter(|(txid, _)| !heights.contains_key(txid))
        .find_map(|(txid, body)| {
            body.inputs.iter().enumerate().find_map(|(i, input)| {
                let height = *heights.get(&input.prev)?;
                (height < u64::from(genesis)).then(|| PreGenesisSpend {
                    txid: display_hex(&txid),
                    input: u32::try_from(i).unwrap_or(u32::MAX),
                    source_txid: display_hex(&input.prev),
                    coin_height: height,
                    genesis_height: genesis,
                })
            })
        })
}

#[cfg(test)]
mod tests;
