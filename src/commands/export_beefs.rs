use anyhow::{Context, Result};
use bsv_sdk::wallet::{ListOutputsArgs, WalletInterface};
use std::collections::HashSet;
use std::path::PathBuf;

use crate::commands::receive;
use crate::context::WalletContext;

pub async fn run(ctx: &WalletContext, to_dir: PathBuf) -> Result<()> {
    std::fs::create_dir_all(&to_dir)?;

    let txids = unique_unspent_txids(ctx).await?;
    let base = receive::woc_base(ctx.chain);
    let client = reqwest::Client::new();

    let mut written = 0u32;
    let mut skipped = 0u32;
    let mut failed = 0u32;
    let total = txids.len();

    for txid in &txids {
        let path = to_dir.join(format!("{}.beef.hex", txid));
        if path.exists() {
            skipped += 1;
            continue;
        }
        match client
            .get(format!("{}/tx/{}/beef", base, txid))
            .send()
            .await
            .with_context(|| format!("WoC BEEF fetch failed for {}", txid))
            .and_then(|r| r.error_for_status().map_err(Into::into))
        {
            Ok(resp) => match resp.text().await {
                Ok(beef_hex) => {
                    if let Err(e) = std::fs::write(&path, beef_hex.trim()) {
                        eprintln!("write failed for {}: {}", txid, e);
                        failed += 1;
                    } else {
                        written += 1;
                    }
                }
                Err(e) => {
                    eprintln!("body read failed for {}: {}", txid, e);
                    failed += 1;
                }
            },
            Err(e) => {
                eprintln!("fetch failed for {}: {}", txid, e);
                failed += 1;
            }
        }
    }

    if ctx.json_output {
        println!(
            "{}",
            serde_json::json!({
                "to": to_dir.display().to_string(),
                "total_unspent_txs": total,
                "written": written,
                "skipped": skipped,
                "failed": failed,
            })
        );
    } else {
        println!(
            "Exported BEEFs to {}: {} written, {} already present, {} failed (of {} unspent txs)",
            to_dir.display(),
            written,
            skipped,
            failed,
            total
        );
    }

    Ok(())
}

async fn unique_unspent_txids(ctx: &WalletContext) -> Result<HashSet<String>> {
    let mut txids = HashSet::new();
    let mut offset: i32 = 0;
    let limit: u32 = 1000;
    loop {
        let res = ctx
            .wallet
            .list_outputs(
                ListOutputsArgs {
                    basket: "default".to_string(),
                    tags: None,
                    tag_query_mode: None,
                    include: None,
                    include_custom_instructions: None,
                    include_tags: None,
                    include_labels: None,
                    limit: Some(limit),
                    offset: Some(offset),
                    seek_permission: None,
                },
                "bsv-wallet-cli",
            )
            .await?;
        let n = res.outputs.len() as u32;
        for o in &res.outputs {
            // Outpoint string format is "<txid>.<vout>"
            let s = o.outpoint.to_string();
            if let Some((txid, _)) = s.rsplit_once('.') {
                txids.insert(txid.to_string());
            }
        }
        if n < limit {
            break;
        }
        offset += n as i32;
    }
    Ok(txids)
}
