use anyhow::Result;
use bsv_wallet_toolbox::{ListOutputsArgs, WalletInterface};

use crate::context::WalletContext;

pub async fn run(ctx: &WalletContext, basket: Option<&str>, tag: Option<&str>) -> Result<()> {
    let args = ListOutputsArgs {
        basket: basket.unwrap_or("default").to_string(),
        tags: tag.map(|t| vec![t.to_string()]),
        tag_query_mode: None,
        include: None,
        include_custom_instructions: None,
        include_tags: Some(true),
        include_labels: Some(true),
        limit: Some(100),
        offset: None,
        seek_permission: None,
    };

    let result = ctx
        .wallet
        .list_outputs(args, Some("bsv-wallet-cli"))
        .await?;

    if ctx.json_output {
        println!("{}", serde_json::to_string_pretty(&result)?);
    } else {
        println!(
            "{} outputs (total: {})",
            result.outputs.len(),
            result.total_outputs
        );
        for output in &result.outputs {
            println!(
                "  {} | {} sats | spendable: {}",
                output.outpoint, output.satoshis, output.spendable
            );
        }
    }

    Ok(())
}
