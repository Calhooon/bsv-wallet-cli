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
use bsv_wallet_toolbox::Chain;
use serde::Deserialize;

use crate::context::WalletContext;

#[derive(Deserialize)]
struct ChainInfo {
    blocks: u64,
    mediantime: u64,
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
    let info: ChainInfo = client
        .get(format!("{base}/chain/info"))
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
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
