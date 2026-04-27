use anyhow::{anyhow, Context, Result};
use bsv_wallet_toolbox::Chain;
use serde::Deserialize;

use crate::brc29;
use crate::commands::fund;
use crate::context::WalletContext;

#[derive(Deserialize)]
struct WocVout {
    n: u32,
    #[serde(rename = "scriptPubKey")]
    script_pub_key: WocScriptPubKey,
}

#[derive(Deserialize)]
struct WocScriptPubKey {
    #[serde(default)]
    addresses: Vec<String>,
}

#[derive(Deserialize)]
struct WocTx {
    vout: Vec<WocVout>,
}

pub async fn run(ctx: &WalletContext, txid: &str, vout: Option<u32>) -> Result<()> {
    let woc_base = match ctx.chain {
        Chain::Main => "https://api.whatsonchain.com/v1/bsv/main",
        Chain::Test => "https://api.whatsonchain.com/v1/bsv/test",
    };

    let client = reqwest::Client::new();

    let resolved_vout = match vout {
        Some(v) => v,
        None => {
            let our_address = brc29::deposit_address(&ctx.root_key, ctx.chain)?;
            let tx: WocTx = client
                .get(format!("{}/tx/{}", woc_base, txid))
                .send()
                .await
                .with_context(|| format!("WoC tx fetch failed for {}", txid))?
                .error_for_status()?
                .json()
                .await?;
            tx.vout
                .iter()
                .find(|v| v.script_pub_key.addresses.iter().any(|a| a == &our_address))
                .map(|v| v.n)
                .ok_or_else(|| {
                    anyhow!(
                        "tx {} has no output to our deposit address {}",
                        txid,
                        our_address
                    )
                })?
        }
    };

    let beef_hex = client
        .get(format!("{}/tx/{}/beef", woc_base, txid))
        .send()
        .await
        .with_context(|| format!("WoC BEEF fetch failed for {}", txid))?
        .error_for_status()?
        .text()
        .await?;
    let beef_bytes = hex::decode(beef_hex.trim())?;

    let accepted = fund::internalize_beef(ctx, &beef_bytes, resolved_vout).await?;

    if ctx.json_output {
        println!(
            "{}",
            serde_json::json!({
                "txid": txid,
                "vout": resolved_vout,
                "accepted": accepted,
            })
        );
    } else if accepted {
        println!("Received tx {} vout {}", txid, resolved_vout);
    } else {
        println!("Transaction was not accepted");
    }

    Ok(())
}
