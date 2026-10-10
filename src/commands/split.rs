use anyhow::Result;
use bsv_sdk::primitives::to_hex;
use bsv_sdk::script::templates::P2PKH;
use bsv_sdk::transaction::Beef;
use bsv_sdk::wallet::{
    Counterparty, CreateActionArgs, CreateActionOptions, CreateActionOutput, InternalizeActionArgs,
    InternalizeOutput, KeyDeriver, ListOutputsArgs, Protocol, SecurityLevel, WalletInterface,
    WalletPayment,
};

use crate::context::WalletContext;

const DEFAULT_DERIVATION_PREFIX: &str = "SfKxPIJNgdI=";
const DEFAULT_DERIVATION_SUFFIX: &str = "NaGLC6fMH50=";
const BRC29_PROTOCOL: &str = "3241645161d8";

pub async fn run(ctx: &WalletContext, count: u32) -> Result<()> {
    if count < 2 {
        anyhow::bail!("Split count must be at least 2");
    }

    // Get current outputs to find total balance
    let list = ctx
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
                limit: None,
                offset: None,
                seek_permission: None,
            },
            "bsv-wallet-cli",
        )
        .await?;

    let total_sats: u64 = list.outputs.iter().map(|o| o.satoshis).sum();
    let utxo_count = list.outputs.len();

    if utxo_count == 0 || total_sats == 0 {
        anyhow::bail!(
            "No UTXOs to split (balance: {} sats, {} UTXOs)",
            total_sats,
            utxo_count
        );
    }

    // Derive the wallet's own address locking script (same logic as address.rs)
    let wallet_deriver = KeyDeriver::new(Some(ctx.root_key.clone()));
    let (_, anyone_pubkey) = KeyDeriver::anyone_key();
    let protocol = Protocol::new(SecurityLevel::Counterparty, BRC29_PROTOCOL);
    let key_id = format!(
        "{} {}",
        DEFAULT_DERIVATION_PREFIX, DEFAULT_DERIVATION_SUFFIX
    );
    let derived_pubkey = wallet_deriver.derive_public_key(
        &protocol,
        &key_id,
        &Counterparty::Other(anyone_pubkey),
        true,
    )?;
    let address = derived_pubkey.to_address();
    let lock = P2PKH::lock_from_address(&address)?;
    let lock_bytes = lock.to_binary();

    // Dynamic fee reserve based on tx size estimate.
    //
    // The old hardcoded 200-sat reserve was fine for small splits (count<=10,
    // few inputs) but silently failed for larger splits or wallets with many
    // existing UTXOs — e.g. 50 outputs from 8 inputs exceeded 200 sats of fees
    // and the toolbox rejected with `Insufficient funds: need X, have Y` where
    // Y was exactly (total_sats - 200). Seen during DolphinSense fleet
    // captain split 50 on 2026-04-15.
    //
    // Estimate follows the usual BSV P2PKH tx footprint:
    //   inputs:  148 bytes each (sig + pubkey + outpoint + scriptlen + seq)
    //   outputs:  34 bytes each (value + scriptlen + P2PKH script)
    //   overhead: ~10 bytes (version + locktime + count varints)
    //
    // Fee rate: we use 1 sat/byte as a generous cap (BSV miners typically
    // accept 0.5 sat/byte or lower). A 1 sat/byte estimate overshoots by ~2x
    // which is fine — worst case a few unused sats accumulate on the wallet.
    // With this headroom, a split from 15+ inputs to 100+ outputs still fits.
    //
    // Minimum floor of 500 so tiny splits don't race into dust territory.
    let estimated_tx_bytes: u64 = (utxo_count as u64 * 148) + (count as u64 * 34) + 10;
    let fee_reserve: u64 = estimated_tx_bytes.max(500);
    if total_sats <= fee_reserve {
        anyhow::bail!(
            "Balance too low to split ({} sats, need > {} for fees at {}b estimate)",
            total_sats,
            fee_reserve,
            estimated_tx_bytes
        );
    }
    let available = total_sats - fee_reserve;
    let per_output = available / count as u64;
    if per_output < 1 {
        anyhow::bail!(
            "Cannot create {} outputs from {} available sats",
            count,
            available
        );
    }

    let (_, anyone_pubkey_for_sender) = KeyDeriver::anyone_key();
    let sender_identity_key = anyone_pubkey_for_sender.to_hex();

    // Build outputs.
    //
    // CRITICAL: `basket: None` (NOT Some("default")).
    //
    // If we put these into a basket during create_action, the toolbox stores
    // them with `change = 0, provided_by = "you"` because it sees the wallet
    // creating intentional outputs into its own basket. After the subsequent
    // internalize_action("wallet payment"), the merge path does NOT promote
    // them to change=1 (internalize only updates change=1 when converting an
    // "external" output into a received payment, not when a basketed output
    // is re-registered).
    //
    // With `basket: None`, create_action treats them as unbasketed external
    // outputs during the signed-tx construction. Then internalize_action
    // with protocol "wallet payment" imports them with change=1, which is
    // what makes them visible to the coin selector in subsequent
    // createAction calls (e.g. overlay registration, LLM x402 payments).
    //
    // The `relinquish` tag is kept as informational — it doesn't affect
    // selection, but labels the outputs for human-readable diagnostics.
    //
    // See dolphinmilkshake/experiments/E21-0-stage.md for the full
    // investigation that led here.
    let outputs: Vec<CreateActionOutput> = (0..count)
        .map(|_| CreateActionOutput {
            locking_script: lock_bytes.clone(),
            satoshis: per_output,
            output_description: "split output".to_string(),
            basket: None,
            custom_instructions: None,
            tags: Some(vec!["relinquish".to_string()]),
        })
        .collect();

    let args = CreateActionArgs {
        description: format!("Split into {} UTXOs ({} sats each)", count, per_output),
        input_beef: None,
        inputs: Some(vec![]),
        outputs: Some(outputs),
        lock_time: None,
        version: None,
        labels: Some(vec!["split".to_string()]),
        options: Some(CreateActionOptions {
            randomize_outputs: Some(false),
            sign_and_process: Some(true),
            no_send: Some(false),
            ..Default::default()
        }),
    };

    // A coin whose transaction drew no accepting word is not selected.
    crate::spend_guard::run(ctx.wallet.storage().pool()).await?;
    let result = ctx.wallet.create_action(args, "bsv-wallet-cli").await?;

    let txid = result.txid.expect("Expected txid from split transaction");
    let txid_hex = to_hex(&txid);

    // Self-internalize each split output with wallet payment protocol
    // so the wallet stores proper derivation info and can sign for them.
    if let Some(beef_bytes) = &result.beef {
        if let Ok(mut beef) = Beef::from_binary(beef_bytes) {
            if let Ok(atomic_bytes) = beef.to_binary_atomic(&txid_hex) {
                let internalize_outputs: Vec<InternalizeOutput> = (0..count)
                    .map(|i| InternalizeOutput {
                        output_index: i,
                        protocol: "wallet payment".to_string(),
                        payment_remittance: Some(WalletPayment {
                            derivation_prefix: DEFAULT_DERIVATION_PREFIX.to_string(),
                            derivation_suffix: DEFAULT_DERIVATION_SUFFIX.to_string(),
                            sender_identity_key: sender_identity_key.clone(),
                        }),
                        insertion_remittance: None,
                    })
                    .collect();

                ctx.wallet
                    .internalize_action(
                        InternalizeActionArgs {
                            tx: atomic_bytes,
                            outputs: internalize_outputs,
                            description: "Self-internalize split outputs".to_string(),
                            labels: Some(vec!["split".to_string()]),
                            seek_permission: None,
                        },
                        "bsv-wallet-cli",
                    )
                    .await?;
            }
        }
    }

    if ctx.json_output {
        println!(
            "{}",
            serde_json::json!({
                "txid": txid_hex,
                "outputs": count,
                "satoshisPerOutput": per_output
            })
        );
    } else {
        println!("Split into {} UTXOs ({} sats each)", count, per_output);
        println!("TxID: {}", txid_hex);
        println!("View: https://whatsonchain.com/tx/{}", txid_hex);
    }

    Ok(())
}
