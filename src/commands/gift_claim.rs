//! `gift-claim` — claim a time-locked BSV gift after its unlock time.
//!
//! Fetches the deposit tx, confirms the covenant is locked to this wallet's key,
//! refuses if still locked (unless `--force` to pre-broadcast), builds the signed
//! claim (covenant unlock + fee input), and broadcasts through the wallet's
//! own broadcasters (Rule 28, C11: the deposit's bytes through
//! `Services::get_raw_tx`, the claim through `post_beef`; see `gift_chain`).

use anyhow::{anyhow, Result};
use bsv_sdk::primitives::bsv::sighash::parse_transaction;
use bsv_wallet_cli::gift::claim::build_claim_tx;
use bsv_wallet_cli::gift::covenant::parse_locking_script;

use crate::commands::gift_chain;
use crate::context::WalletContext;

pub async fn run(ctx: &WalletContext, txid: &str, force: bool) -> Result<()> {
    // 1. fetch the deposit transaction
    let deposit_raw = gift_chain::deposit_bytes(ctx.wallet.services(), txid).await?;

    // 2. parse the covenant + confirm it's ours
    let dep = parse_transaction(&deposit_raw).map_err(|e| anyhow!("parse deposit: {e}"))?;
    let cov_out = dep
        .outputs
        .first()
        .ok_or_else(|| anyhow!("{txid} has no vout 0"))?;
    let params = parse_locking_script(&cov_out.script)
        .map_err(|e| anyhow!("{txid} vout 0 is not a TimeLockedGift covenant: {e}"))?;

    // Sign + receive with the BRC-29 DEPOSIT key, so the claimed coins land at
    // the wallet's deposit address and become normal spendable balance after sync.
    let (deposit_priv, deposit_pub) = crate::brc29::deposit_keypair(&ctx.root_key)?;
    let our_pub = deposit_pub.to_compressed();
    if our_pub.as_slice() != params.recipient.as_slice() {
        return Err(anyhow!(
            "this gift is locked to {}, which is not your deposit key ({})",
            hex::encode(&params.recipient),
            hex::encode(our_pub)
        ));
    }

    // 3. unlock-time gate
    let now = chrono::Utc::now().timestamp() as u64;
    let unlock_human = chrono::DateTime::from_timestamp(params.lock_until as i64, 0)
        .map(|d| d.to_rfc3339())
        .unwrap_or_else(|| params.lock_until.to_string());
    if now < params.lock_until && !force {
        return Err(anyhow!(
            "🔒 This gift is locked until {unlock_human} ({}). It cannot be claimed yet.\n   \
             (Use --force to pre-broadcast now; it will sit unconfirmed and auto-confirm at unlock.)",
            params.lock_until
        ));
    }

    // 4. build the signed claim (signed by the deposit key)
    let plan = build_claim_tx(&deposit_raw, &deposit_priv, params.lock_until as u32)
        .map_err(|e| anyhow!("failed to build claim: {e}"))?;

    // 5. broadcast through the wallet's broadcasters
    let claim_raw =
        hex::decode(&plan.claim_raw_hex).map_err(|e| anyhow!("the claim is not hex: {e}"))?;
    let broadcaster =
        gift_chain::broadcast_claim(ctx.wallet.services(), &deposit_raw, &claim_raw).await?;
    let claim_txid = plan.claim_txid.as_str();

    if ctx.json_output {
        println!(
            "{}",
            serde_json::json!({
                "claimTxid": claim_txid,
                "depositTxid": plan.deposit_txid,
                "amount": plan.covenant.amount,
                "fee": plan.fee,
                "change": plan.change,
                "lockUntil": plan.covenant.lock_until,
                "broadcast": broadcaster,
            })
        );
    } else {
        let pre = now < params.lock_until;
        println!("🔓 Gift claim broadcast");
        println!(
            "   amount:     {} sats → your address",
            plan.covenant.amount
        );
        println!(
            "   fee:        {} sats (change {} sats back to you)",
            plan.fee, plan.change
        );
        println!("   unlocks:    {unlock_human}");
        println!("   claim txid: {claim_txid}");
        println!("   View:       https://whatsonchain.com/tx/{claim_txid}");
        if pre {
            println!(
                "\n   ⏳ Pre-armed before unlock: it sits unconfirmed and auto-confirms once\n      median-time-past passes {unlock_human}."
            );
        }
    }

    Ok(())
}
