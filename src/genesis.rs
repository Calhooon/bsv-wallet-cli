//! The Genesis height `internalizeAction` judges a received spend by, and
//! the one question this host asks of a BEEF before it posts a transaction
//! it will then record through that door.
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
//! which bsv-rs does not publish, so it is carried here.
//!
//! The chain. The door is told the `Chain` of the toolbox `Wallet` that
//! calls it (`Wallet::internalize_action`, `src/wallet/wallet.rs:2450`
//! `with_caller_network(self.chain.as_str(), ..)`), so `split` and
//! `gift-claim` pass that same `Chain`, read from the wallet itself
//! (`Wallet::chain`, `wallet.rs:464`), never the CLI's `--testnet` flag
//! (`WalletContext::chain`): host and door agree on every wallet however
//! it was built. This CLI builds its wallets with `Wallet::new`, which
//! gives the toolbox's default chain, mainnet (`wallet.rs:365-387`), so
//! today a `--testnet` wallet's door and this host both judge by
//! mainnet's height. A test at the boundary through the door
//! (`the_door_refuses_a_coin_proven_one_below_genesis_and_takes_one_at_it`)
//! holds the two numbers together, and a `--testnet` context's coin at
//! 700,000 holds the two chains together
//! (`a_testnet_context_holds_by_the_chain_its_door_judges_by`,
//! `a_testnet_contexts_claim_is_asked_the_chain_its_door_judges_by`).

use bsv_sdk::script::ProtocolEra;
use bsv_sdk::transaction::beef_stream::{display_hex, Element};
use bsv_sdk::transaction::BeefStream;
use bsv_wallet_toolbox::Chain;
use std::collections::HashMap;

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
    /// The Genesis height of the chain asked.
    pub genesis_height: u32,
}

/// The earliest transaction, in the BEEF's order, of `beef` (BEEF or
/// AtomicBEEF bytes) that the BEEF does not prove (no BUMP of its own, its
/// txid in no BUMP) with an input whose source the BEEF proves at a height
/// below `chain`'s Genesis height; `None` when there is none, or when the bytes do not read (the
/// door's own steps refuse those). The Genesis rule alone: the scripts are
/// the door's to run.
pub fn unproven_spend_of_pre_genesis_coin(beef: &[u8], chain: Chain) -> Option<PreGenesisSpend> {
    let genesis = genesis_height(chain);
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
