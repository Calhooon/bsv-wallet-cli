use anyhow::Result;
use bsv_sdk::primitives::to_hex;
use bsv_sdk::script::templates::P2PKH;
use bsv_sdk::transaction::Beef;
use bsv_sdk::wallet::{CreateActionArgs, CreateActionOptions, CreateActionOutput, WalletInterface};

use crate::context::WalletContext;

pub async fn run(ctx: &WalletContext, address: &str, satoshis: u64) -> Result<()> {
    let lock = P2PKH::lock_from_address(address)?;

    let args = CreateActionArgs {
        description: format!("Send {} sats to {}", satoshis, &address[..8]),
        input_beef: None,
        inputs: Some(vec![]),
        outputs: Some(vec![CreateActionOutput {
            locking_script: lock.to_binary(),
            satoshis,
            output_description: "P2PKH send".to_string(),
            basket: None,
            custom_instructions: None,
            tags: Some(vec!["relinquish".to_string()]),
        }]),
        lock_time: None,
        version: None,
        labels: Some(vec!["send".to_string()]),
        options: Some(CreateActionOptions {
            randomize_outputs: Some(false),
            accept_delayed_broadcast: Some(false),
            no_send: Some(false),
            sign_and_process: Some(true),
            known_txids: Some(vec![]),
            return_txid_only: Some(false),
            no_send_change: Some(vec![]),
            send_with: Some(vec![]),
            ..Default::default()
        }),
    };

    let result = ctx.wallet.create_action(args, "bsv-wallet-cli").await?;

    let txid = result.txid.expect("Expected txid in result");
    let txid_hex = to_hex(&txid);

    // Fail loudly if the broadcast was silently dropped. `create_action` returns
    // Ok with a txid even when ARC rejected the tx with error 465 ("fee too low"
    // on a deep unconfirmed BEEF), because the toolbox misclassifies a 465 as a
    // transient service error. Confirm the tx actually reached the network before
    // reporting success, so a phantom (never-mined) txid is never printed + exit 0.
    crate::broadcast_verify::BroadcastVerifier::from_env(ctx.chain)
        .verify(&txid_hex)
        .await
        .into_send_result(&txid_hex)?;

    // Build AtomicBEEF hex from result for direct wallet-to-wallet transfers
    let beef_hex = result.beef.as_ref().and_then(|beef_bytes| {
        Beef::from_binary(beef_bytes)
            .ok()
            .and_then(|mut beef| beef.to_binary_atomic(&txid_hex).ok().map(hex::encode))
    });

    if ctx.json_output {
        let mut obj = serde_json::json!({ "txid": txid_hex });
        if let Some(ref bh) = beef_hex {
            obj["beef"] = serde_json::Value::String(bh.clone());
        }
        println!("{}", obj);
    } else {
        println!("Sent {} satoshis to {}", satoshis, address);
        println!("TxID: {}", txid_hex);
        if let Some(ref bh) = beef_hex {
            println!("BEEF: {}", bh);
        }
        println!("View: https://whatsonchain.com/tx/{}", txid_hex);
    }

    Ok(())
}
