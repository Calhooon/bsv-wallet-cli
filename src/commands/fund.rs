use anyhow::Result;
use bsv_sdk::wallet::{InternalizeActionArgs, InternalizeOutput, WalletInterface, WalletPayment};

use crate::brc29;
use crate::context::WalletContext;

pub async fn run(ctx: &WalletContext, beef_hex: &str, vout: u32) -> Result<()> {
    let beef_bytes = hex::decode(beef_hex)?;

    let (_, anyone_pubkey) = bsv_sdk::wallet::KeyDeriver::anyone_key();
    let sender_identity_key = anyone_pubkey.to_hex();

    let args = InternalizeActionArgs {
        tx: beef_bytes,
        outputs: vec![InternalizeOutput {
            output_index: vout,
            protocol: "wallet payment".to_string(),
            payment_remittance: Some(WalletPayment {
                derivation_prefix: brc29::DEFAULT_DERIVATION_PREFIX.to_string(),
                derivation_suffix: brc29::DEFAULT_DERIVATION_SUFFIX.to_string(),
                sender_identity_key,
            }),
            insertion_remittance: None,
        }],
        description: "Internalize external funding".to_string(),
        labels: Some(vec!["funding".to_string()]),
        seek_permission: None,
    };

    let result = ctx
        .wallet
        .internalize_action(args, "bsv-wallet-cli")
        .await?;

    if ctx.json_output {
        println!("{}", serde_json::json!({ "accepted": result.accepted }));
    } else if result.accepted {
        println!("Transaction internalized successfully");
    } else {
        println!("Transaction was not accepted");
    }

    Ok(())
}
