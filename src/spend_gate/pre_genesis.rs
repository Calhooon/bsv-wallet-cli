//! `split`'s hold of a coin mined before Genesis (#634, the port of the
//! stable line's 0.18.9). `split` posts and then records its outputs
//! through `internalizeAction`, whose door refuses a payment spending a
//! coin mined before Genesis as not judged; the hold keeps the split from
//! posting what it could not record. Every other command's offer is
//! unchanged: only `split` sets [`GateOptions::split_hold`].

use super::{candidates, Result, UserCoins};
use std::collections::{HashMap, HashSet};

/// The reason's head of a coin `split` does not offer.
const PRE_GENESIS_HEAD: &str = "mined before Genesis: ";

/// The sentence a refusal holding such a coin ends with: why, and what
/// works.
pub const PRE_GENESIS_WHY: &str = "split records its outputs through internalizeAction after it posts them, and internalizeAction refuses a payment spending a coin mined before Genesis as not judged, so split does not spend such a coin; nothing was signed or posted. A plain `send` of the coin is not held for this reason.";

/// The most unproven transactions of ours [`creator_before_genesis`] walks
/// for one creator.
const PRE_GENESIS_WALK: usize = 64;

/// A coin `split` does not spend ([`pre_genesis_coins`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PreGenesisCoin {
    /// The store's output id.
    pub output_id: i64,
    /// `txid.vout`.
    pub outpoint: String,
    pub satoshis: u64,
    /// Why, in words: `mined before Genesis: ...; split does not offer it`.
    pub reason: String,
}

/// The coins of the default basket the gate could allocate whose spend the
/// door would refuse as not judged under `genesis` (the Genesis height of
/// the door's own store, [`crate::genesis::door_genesis_height`]), read
/// from the wallet's own records and never an explorer, as the toolbox
/// builds a spend's BEEF from them: the creator's proof record
/// (`proven_txs.height`); with none, a BUMP of the creator's stored BEEF
/// (`transactions.input_beef`) proving it; with neither, the creator not
/// mined: an unproven transaction of its stored BEEF spending a coin that
/// BEEF proves below the height
/// ([`crate::genesis::unproven_spend_of_pre_genesis_coin`]), or an input of
/// it, or of an unproven ancestor of ours, whose source's record proves it
/// below the height (at most [`PRE_GENESIS_WALK`] transactions). A coin its
/// own record holds (frozen, lost) is the gate's already. Reads only.
pub async fn pre_genesis_coins(coins: UserCoins<'_>, genesis: u32) -> Result<Vec<PreGenesisCoin>> {
    let pool = coins.storage.pool();
    let mut by_creator: HashMap<String, Option<String>> = HashMap::new();
    let mut held = Vec::new();
    for c in candidates(coins)
        .await?
        .into_iter()
        .filter(|c| c.record_hold().is_none())
    {
        let reason = match by_creator.get(&c.txid) {
            Some(reason) => reason.clone(),
            None => {
                let reason = creator_before_genesis(pool, &c.txid, genesis).await?;
                by_creator.insert(c.txid.clone(), reason.clone());
                reason
            }
        };
        if let Some(reason) = reason {
            held.push(PreGenesisCoin {
                output_id: c.output_id,
                outpoint: format!("{}.{}", c.txid, c.vout),
                satoshis: c.satoshis,
                reason,
            });
        }
    }
    Ok(held)
}

/// Why a spend of an output of `txid` would not be judged under `genesis`,
/// from the wallet's own records ([`pre_genesis_coins`]), or `None`.
async fn creator_before_genesis(
    pool: &sqlx::SqlitePool,
    txid: &str,
    genesis: u32,
) -> Result<Option<String>> {
    if let Some(height) = proof_height(pool, txid).await? {
        return Ok((height < u64::from(genesis)).then(|| {
            format!(
                "{PRE_GENESIS_HEAD}coin height {height} is before Genesis height {genesis}; split does not offer it"
            )
        }));
    }
    let unmined = |spender: &str, height: u64| {
        format!(
            "{PRE_GENESIS_HEAD}{spender} is not mined yet and spends a coin of height {height}, before Genesis height {genesis}; split does not offer it"
        )
    };
    // The creator is not mined: the BEEF of a spend of it carries it and
    // its unproven ancestry down to the proofs, as the toolbox's walk
    // builds it (its stored BEEF, else its inputs' records).
    let mut walk = vec![txid.to_string()];
    let mut seen = HashSet::new();
    while let Some(t) = walk.pop() {
        if !seen.insert(t.clone()) || seen.len() > PRE_GENESIS_WALK {
            continue;
        }
        let Some((Some(raw), input_beef)) = crate::store::stored_tx(pool, &t).await? else {
            continue;
        };
        if let Some(mut beef) =
            input_beef.and_then(|bytes| bsv_sdk::transaction::Beef::from_binary(&bytes).ok())
        {
            if beef.find_txid(&t).is_none() && beef.merge_raw_tx(raw.clone(), None).is_err() {
                continue;
            }
            if let Some(spend) = beef.to_binary().ok().and_then(|bytes| {
                crate::genesis::unproven_spend_of_pre_genesis_coin(&bytes, genesis)
            }) {
                return Ok(Some(unmined(&spend.txid, spend.coin_height)));
            }
        }
        let Ok(tx) = bsv_sdk::transaction::Transaction::from_binary(&raw) else {
            continue;
        };
        for input in tx.inputs() {
            let Some(prev) = input.source_txid.as_ref().map(|t| t.to_ascii_lowercase()) else {
                continue;
            };
            match proof_height(pool, &prev).await? {
                Some(height) if height < u64::from(genesis) => {
                    return Ok(Some(unmined(&t, height)));
                }
                Some(_) => {}
                None => walk.push(prev),
            }
        }
    }
    Ok(None)
}

/// The height the wallet's own records prove `txid` at: its proof record
/// (`proven_txs.height`), else a BUMP of its stored BEEF that proves it.
async fn proof_height(pool: &sqlx::SqlitePool, txid: &str) -> Result<Option<u64>> {
    if let Some((_, height)) = crate::store::stored_proof(pool, txid).await? {
        return Ok(Some(height.max(0).cast_unsigned()));
    }
    Ok(crate::store::stored_tx(pool, txid)
        .await?
        .and_then(|(_, beef)| beef)
        .and_then(|bytes| bsv_sdk::transaction::Beef::from_binary(&bytes).ok())
        .and_then(|beef| {
            beef.bumps
                .iter()
                .find(|b| b.contains(txid))
                .map(|b| u64::from(b.block_height))
        }))
}
