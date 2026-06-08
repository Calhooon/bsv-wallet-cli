use anyhow::Result;

use crate::brc29;
use crate::context::WalletContext;

pub async fn run(ctx: &WalletContext) -> Result<()> {
    let address = brc29::deposit_address(&ctx.root_key, ctx.chain)?;
    // The deposit pubkey is what you share to RECEIVE a time-locked gift
    // (`gift-send <pubkey> ...`); the claim lands back here, spendable.
    let (_, deposit_pub) = brc29::deposit_keypair(&ctx.root_key)?;
    let pubkey_hex = hex::encode(deposit_pub.to_compressed());

    if ctx.json_output {
        println!(
            "{}",
            serde_json::json!({ "address": address, "pubkey": pubkey_hex })
        );
    } else {
        println!("{address}");
        println!("pubkey (for receiving gifts): {pubkey_hex}");
    }

    Ok(())
}
