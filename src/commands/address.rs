use anyhow::Result;

use crate::brc29;
use crate::context::WalletContext;

pub async fn run(ctx: &WalletContext) -> Result<()> {
    let address = brc29::deposit_address(&ctx.root_key, ctx.chain)?;

    if ctx.json_output {
        println!("{}", serde_json::json!({ "address": address }));
    } else {
        println!("{}", address);
    }

    Ok(())
}
