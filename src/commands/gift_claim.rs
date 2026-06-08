//! `gift-claim` — claim a time-locked BSV gift after its unlock time.
//!
//! Fetches the deposit tx, confirms the covenant is locked to this wallet's key,
//! refuses if still locked (unless `--force` to pre-broadcast), builds the signed
//! claim (covenant unlock + fee input), and broadcasts via WhatsOnChain.

use anyhow::{anyhow, Result};
use bsv_sdk::primitives::bsv::sighash::parse_transaction;
use bsv_wallet_cli::gift::claim::build_claim_tx;
use bsv_wallet_cli::gift::covenant::parse_locking_script;
use bsv_wallet_toolbox::Chain;

use crate::context::WalletContext;

fn woc_base(chain: Chain) -> &'static str {
    match chain {
        Chain::Test => "https://api.whatsonchain.com/v1/bsv/test",
        _ => "https://api.whatsonchain.com/v1/bsv/main",
    }
}

pub async fn run(ctx: &WalletContext, txid: &str, force: bool) -> Result<()> {
    let base = woc_base(ctx.chain);
    let client = reqwest::Client::new();

    // 1. fetch the deposit transaction
    let raw_hex = client
        .get(format!("{base}/tx/{txid}/hex"))
        .send()
        .await?
        .error_for_status()
        .map_err(|e| anyhow!("could not fetch deposit {txid} from WhatsOnChain: {e}"))?
        .text()
        .await?;
    let deposit_raw = hex::decode(raw_hex.trim())
        .map_err(|e| anyhow!("WhatsOnChain returned non-hex for {txid}: {e}"))?;

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

    // 5. broadcast via WhatsOnChain
    let resp = client
        .post(format!("{base}/tx/raw"))
        .json(&serde_json::json!({ "txhex": plan.claim_raw_hex }))
        .send()
        .await?;
    let status = resp.status();
    let body = resp.text().await?;
    if !status.is_success() {
        return Err(anyhow!("broadcast rejected ({status}): {}", body.trim()));
    }
    let claim_txid = body.trim().trim_matches('"');

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
                "broadcast": "whatsonchain",
            })
        );
    } else {
        let pre = now < params.lock_until;
        println!("🔓 Gift claim broadcast");
        println!("   amount:     {} sats → your address", plan.covenant.amount);
        println!("   fee:        {} sats (change {} sats back to you)", plan.fee, plan.change);
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
