use anyhow::Result;
use bsv_sdk::wallet::{InternalizeActionArgs, InternalizeOutput, WalletInterface, WalletPayment};

use crate::atomic_beef;
use crate::brc29;
use crate::context::WalletContext;

pub async fn run<S: bsv_wallet_toolbox::WalletServices + Send + Sync + 'static>(
    ctx: &WalletContext<S>,
    beef_hex: &str,
    vout: u32,
) -> Result<()> {
    // With no chain tracker every internalizeAction is refused at the
    // toolbox's door (#509): say so by name before anything is read.
    ctx.require_chain_tracker_to_receive("fund").await?;
    let beef_bytes = hex::decode(beef_hex)?;
    let accepted = internalize_beef(ctx, &beef_bytes, vout).await?;

    if ctx.json_output {
        println!("{}", serde_json::json!({ "accepted": accepted }));
    } else if accepted {
        println!("Transaction internalized successfully");
    } else {
        println!("Transaction was not accepted");
    }

    Ok(())
}

pub(crate) async fn internalize_beef<
    S: bsv_wallet_toolbox::WalletServices + Send + Sync + 'static,
>(
    ctx: &WalletContext<S>,
    beef_bytes: &[u8],
    vout: u32,
) -> Result<bool> {
    let atomic_bytes = atomic_beef::ensure_atomic(beef_bytes)?;

    let (_, anyone_pubkey) = bsv_sdk::wallet::KeyDeriver::anyone_key();
    let sender_identity_key = anyone_pubkey.to_hex();

    let args = InternalizeActionArgs {
        tx: atomic_bytes,
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

    let subject = bsv_wallet_cli::server::post_word::atomic_subject(&args.tx);
    let result = ctx
        .wallet
        .internalize_action(args, Some("bsv-wallet-cli"))
        .await?;
    // The door's word (#645): `fund`, `receive` and `sync` take a body in
    // here, and a body with no proof gets its tracker row, `built`.
    if let Some(txid) = &subject {
        bsv_wallet_cli::tracker_host::door_word(ctx.wallet.storage(), txid).await;
    }
    Ok(result.accepted)
}
