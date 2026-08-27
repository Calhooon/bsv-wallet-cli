use anyhow::{anyhow, Result};
use bsv_sdk::primitives::to_hex;
use bsv_sdk::script::templates::P2PKH;
use bsv_sdk::wallet::{CreateActionArgs, CreateActionOptions, CreateActionOutput, WalletInterface};
use bsv_wallet_toolbox::ListOutputsArgs;

use crate::context::WalletContext;

const FEE_RESERVE_BASE: u64 = 50;
const FEE_RESERVE_PER_INPUT: u64 = 25;

pub async fn run(ctx: &WalletContext, address: &str) -> Result<()> {
    let (balance, n_outputs) = sum_spendable(ctx).await?;
    if n_outputs == 0 {
        return Err(anyhow!("wallet has no spendable outputs"));
    }
    let fee_reserve = FEE_RESERVE_BASE + FEE_RESERVE_PER_INPUT * n_outputs as u64;
    if balance <= fee_reserve {
        return Err(anyhow!(
            "balance {} sats is below fee reserve {} sats — nothing to drain",
            balance,
            fee_reserve
        ));
    }
    let send_amount = balance - fee_reserve;

    let lock = P2PKH::lock_from_address(address)?;
    let args = CreateActionArgs {
        description: format!(
            "Drain {} sats to {}",
            send_amount,
            &address[..8.min(address.len())]
        ),
        input_beef: None,
        inputs: Some(vec![]),
        outputs: Some(vec![CreateActionOutput {
            locking_script: lock.to_binary(),
            satoshis: send_amount,
            output_description: "P2PKH drain".to_string(),
            basket: None,
            custom_instructions: None,
            tags: Some(vec!["relinquish".to_string()]),
        }]),
        lock_time: None,
        version: None,
        labels: Some(vec!["drain".to_string()]),
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
    let txid_hex = to_hex(&result.txid.expect("expected txid in result"));

    // Same loud-failure bar as `send` (2026-08-27): `create_action` returns Ok
    // with a txid even when the broadcast never reached the network (ARC 465
    // misclassified as transient; keyless ARC auth rejection swallowed the
    // same way). `drain` skipping this check is exactly how 24 phantom
    // recovery drains printed clean TxIDs while the network never saw one —
    // a drain that did not propagate must exit nonzero, never print success.
    crate::broadcast_verify::BroadcastVerifier::from_env(ctx.chain)
        .verify(&txid_hex)
        .await
        .into_send_result(&txid_hex)?;

    if ctx.json_output {
        println!(
            "{}",
            serde_json::json!({
                "txid": txid_hex,
                "drained": send_amount,
                "fee_reserved": fee_reserve,
                "inputs": n_outputs,
            })
        );
    } else {
        println!("Drained {} sats to {}", send_amount, address);
        println!("Inputs: {}, fee reserve: {} sats", n_outputs, fee_reserve);
        println!("TxID: {}", txid_hex);
        println!("View: https://whatsonchain.com/tx/{}", txid_hex);
    }

    Ok(())
}

async fn sum_spendable(ctx: &WalletContext) -> Result<(u64, u32)> {
    let mut balance: u64 = 0;
    let mut count: u32 = 0;
    let mut offset: i32 = 0;
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
                    limit: Some(100),
                    offset: Some(offset),
                    seek_permission: None,
                },
                "bsv-wallet-cli",
            )
            .await?;
        for o in &res.outputs {
            balance += o.satoshis;
            count += 1;
        }
        offset += res.outputs.len() as i32;
        if res.outputs.is_empty() || offset >= res.total_outputs as i32 {
            break;
        }
    }
    Ok((balance, count))
}
