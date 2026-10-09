//! `gift-inspect` — see a time-locked gift and prove it's yours & openable,
//! WITHOUT claiming or broadcasting anything.
//!
//! Confirms the covenant is locked to this wallet's deposit key, shows the unlock
//! time + an estimated claimable block height (accounting for BSV's median-time-past
//! lag), and proves the wallet's signature satisfies the covenant by building (but
//! not broadcasting) the claim.

use anyhow::{anyhow, Result};
use bsv_sdk::primitives::bsv::sighash::parse_transaction;
use bsv_wallet_cli::gift::claim::build_claim_tx;
use bsv_wallet_cli::gift::covenant::parse_locking_script;
use bsv_wallet_toolbox::{Chain, WalletServices};

use crate::context::WalletContext;

/// How many headers the median time past is taken over: the tip and its
/// ten predecessors. The node's rule, read from the sibling, not restated:
/// bsv-script-lean `docs/BLOCK-CONSENSUS.md` section 2.2 (`nMedianTimeSpan`
/// = 11, fewer where the chain is shorter; bitcoin-sv v1.2.2
/// `block_index.h:720-737`).
const MEDIAN_TIME_SPAN: u32 = 11;

/// The chain's clock as the header service holds it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ChainInfo {
    /// The tip header's height.
    blocks: u64,
    /// The median of the times of the tip and its ten predecessors.
    mediantime: u64,
}

/// The tip height and the median time past, from the header service (Rule
/// 28, C13, a deletion): the tip header and the ten before it are headers
/// we already hold a service for, so no explorer is asked. With no header
/// service the answer is an error naming it, never an explorer's number.
async fn chain_clock<V: WalletServices>(services: &V) -> Result<ChainInfo> {
    let tip = services
        .get_chain_tip_header()
        .await
        .map_err(|e| anyhow!("the header service gave no tip header (CHAINTRACKS_URL): {e}"))?;
    let mut times = vec![tip.time];
    let first = tip.height.saturating_sub(MEDIAN_TIME_SPAN - 1);
    for height in first..tip.height {
        let header = services
            .get_header_for_height(height)
            .await
            .map_err(|e| anyhow!("the header service gave no header at height {height}: {e}"))?;
        times.push(
            header_time(&header)
                .ok_or_else(|| anyhow!("the header at height {height} is not 80 bytes"))?,
        );
    }
    Ok(ChainInfo {
        blocks: tip.height as u64,
        mediantime: median(times) as u64,
    })
}

/// The time field of an 80-byte block header (bytes 68 to 71, little-endian).
fn header_time(header: &[u8]) -> Option<u32> {
    if header.len() != 80 {
        return None;
    }
    Some(u32::from_le_bytes(header[68..72].try_into().ok()?))
}

/// The median as the node takes it: sort, then the element at `len / 2`.
fn median(mut times: Vec<u32>) -> u32 {
    times.sort_unstable();
    times[times.len() / 2]
}

fn woc_base(chain: Chain) -> &'static str {
    match chain {
        Chain::Test => "https://api.whatsonchain.com/v1/bsv/test",
        _ => "https://api.whatsonchain.com/v1/bsv/main",
    }
}

fn fmt_ts(ts: u64) -> String {
    chrono::DateTime::from_timestamp(ts as i64, 0)
        .map(|d| d.to_rfc3339())
        .unwrap_or_else(|| ts.to_string())
}

pub async fn run(ctx: &WalletContext, txid: &str) -> Result<()> {
    let base = woc_base(ctx.chain);
    let client = reqwest::Client::new();

    // 1. fetch + parse the deposit covenant
    let raw_hex = client
        .get(format!("{base}/tx/{txid}/hex"))
        .send()
        .await?
        .error_for_status()
        .map_err(|e| anyhow!("could not fetch {txid} from WhatsOnChain: {e}"))?
        .text()
        .await?;
    let deposit_raw = hex::decode(raw_hex.trim())
        .map_err(|e| anyhow!("WhatsOnChain returned non-hex for {txid}: {e}"))?;
    let dep = parse_transaction(&deposit_raw).map_err(|e| anyhow!("parse deposit: {e}"))?;
    let cov_out = dep
        .outputs
        .first()
        .ok_or_else(|| anyhow!("{txid} has no vout 0"))?;
    let params = parse_locking_script(&cov_out.script)
        .map_err(|e| anyhow!("{txid} vout 0 is not a TimeLockedGift covenant: {e}"))?;

    // 2. is it locked to me?
    let (deposit_priv, deposit_pub) = crate::brc29::deposit_keypair(&ctx.root_key)?;
    let locked_to_me = deposit_pub.to_compressed().as_slice() == params.recipient.as_slice();

    // 3. signability proof — build (not broadcast) the claim
    let signable = locked_to_me
        && build_claim_tx(&deposit_raw, &deposit_priv, params.lock_until as u32).is_ok();

    // 4. timing: current height + median-time-past, estimate claimable block
    let info = chain_clock(ctx.wallet.services()).await?;
    let now = chrono::Utc::now().timestamp() as u64;
    let mtp_lag = now.saturating_sub(info.mediantime);
    let claimable_now = info.mediantime >= params.lock_until;
    // a block can include the claim once its MTP >= lockUntil; MTP advances ~1
    // block / ~600s. estimate blocks-to-go + the wall-clock it lands.
    let (est_blocks, est_height, est_wall) = if claimable_now {
        (0u64, info.blocks, now)
    } else {
        let secs = params.lock_until - info.mediantime;
        let blocks = secs.div_ceil(600);
        (blocks, info.blocks + blocks, params.lock_until + mtp_lag)
    };

    if ctx.json_output {
        println!(
            "{}",
            serde_json::json!({
                "txid": txid,
                "lockedToMe": locked_to_me,
                "signable": signable,
                "amount": params.amount,
                "recipient": hex::encode(&params.recipient),
                "lockUntil": params.lock_until,
                "currentHeight": info.blocks,
                "currentMtp": info.mediantime,
                "claimableNow": claimable_now,
                "estClaimableHeight": est_height,
                "estClaimableTime": est_wall,
            })
        );
        return Ok(());
    }

    println!("🎁 Time-locked gift — inspection (nothing claimed, nothing broadcast)");
    println!("   gift txid:      {txid}");
    println!("   amount:         {} sats", params.amount);
    println!("   locked to key:  {}", hex::encode(&params.recipient));
    println!(
        "   that's YOU:     {}",
        if locked_to_me {
            "✅ yes — this gift is yours"
        } else {
            "❌ no — locked to a different key"
        }
    );
    println!(
        "   you can open it:{}",
        if signable {
            " ✅ your signature satisfies the covenant"
        } else if !locked_to_me {
            " ❌ not your key"
        } else {
            " ❌ could not produce a valid claim"
        }
    );
    println!("   unlocks at:     {}", fmt_ts(params.lock_until));
    println!(
        "   chain now:      block {} · median-time-past {} (~{}h behind real time)",
        info.blocks,
        fmt_ts(info.mediantime),
        mtp_lag / 3600
    );
    if claimable_now {
        println!("   status:         🔓 CLAIMABLE NOW");
        println!("\n   Claim it:  bsv-wallet gift-claim {txid}");
        println!(
            "   …then it lands in your wallet; `bsv-wallet sync` + `bsv-wallet send` to spend."
        );
    } else {
        println!(
            "   status:         🔒 LOCKED — claimable ~{} (≈ block {}, ~{} blocks away)",
            fmt_ts(est_wall),
            est_height,
            est_blocks
        );
        println!("\n   Note: the network won't confirm a claim until its block's median-time-past");
        println!(
            "   passes the unlock time, which lags real time by ~{}h.",
            mtp_lag / 3600
        );
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use bsv_wallet_toolbox::services::mock::MockWalletServices;
    use bsv_wallet_toolbox::services::BlockHeader;

    fn header(height: u32, time: u32) -> BlockHeader {
        BlockHeader {
            version: 536870912,
            previous_hash: "0".repeat(64),
            merkle_root: "a".repeat(64),
            time,
            bits: 402917821,
            nonce: 7,
            height,
            hash: "b".repeat(64),
        }
    }

    /// C13 (Rule 28): the tip height and the median time past come from
    /// the header service. Red at the base, with a local fixture in the
    /// explorer's place: one request to `/chain/info`, and its `blocks` and
    /// `mediantime` taken as the chain's clock.
    #[tokio::test]
    async fn the_tip_and_the_median_time_past_come_from_the_header_service() {
        let services = MockWalletServices::new();
        // Times out of order, as a chain's are: the median is not the tip's.
        let times = [100, 400, 200, 900, 300, 800, 500, 700, 600, 1000, 50];
        for (i, time) in times.iter().enumerate() {
            let h = header(900_000 + i as u32, *time);
            if i + 1 == times.len() {
                services.set_tip_header(h);
            } else {
                services.set_header_for_height(h);
            }
        }

        let clock = chain_clock(&services).await.unwrap();
        assert_eq!(
            clock,
            ChainInfo {
                blocks: 900_010,
                mediantime: 500
            }
        );
        assert_eq!(services.call_count("get_chain_tip_header"), 1);
        assert_eq!(services.call_count("get_header_for_height"), 10);
    }

    /// With no tip from the header service there is no clock: an error
    /// naming the setting, never a number from somewhere else.
    #[tokio::test]
    async fn no_header_service_is_no_clock() {
        let services = MockWalletServices::new();
        services.set_tip_unavailable();
        let err = chain_clock(&services).await.unwrap_err();
        assert!(err.to_string().contains("CHAINTRACKS_URL"), "{err}");
    }

    /// The command asks no explorer for the chain's tip. Red at the base.
    #[test]
    fn gift_inspect_asks_no_explorer_for_the_tip() {
        let source = include_str!("gift_inspect.rs");
        let code = source.split("#[cfg(test)]").next().unwrap();
        assert!(!code.contains("chain/info"));
    }

    #[test]
    fn the_median_is_the_middle_of_the_sorted_times() {
        assert_eq!(median(vec![3, 1, 2]), 2);
        assert_eq!(median(vec![7]), 7);
        // A chain younger than eleven blocks: the upper middle, as the node.
        assert_eq!(median(vec![4, 1, 3, 2]), 3);
        assert_eq!(header_time(&[0u8; 79]), None);
    }
}
