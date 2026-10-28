use anyhow::{Context, Result};
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

pub async fn run(
    ctx: &WalletContext,
    count: u32,
    allow_pending: crate::spend_gate::AllowPending,
) -> Result<()> {
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
            Some("bsv-wallet-cli"),
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

    // The holds a 0.15 process left are healed first, when there are any.
    crate::spend_gate::heal_before_spend(ctx.wallet.storage()).await;
    // A coin whose transaction drew no accepting word is not selected.
    crate::spend_gate::busy_as_json(
        crate::spend_guard::run(ctx.wallet.storage()).await,
        ctx.json_output,
    )?;
    // The spend gate (R1 (b), R3 (b)): only offerable coins are allocated; a
    // refusal lists every coin held back.
    let result = crate::spend_gate::refused_as_json(
        crate::spend_gate::create_action(
            &ctx.wallet,
            args,
            "bsv-wallet-cli",
            &ctx.gate().with_allow_pending(allow_pending),
        )
        .await,
        ctx.json_output,
    )?;

    let txid = result
        .txid
        .context("create_action returned no txid for a signed, sent split")?;
    let txid_hex = to_hex(&txid);

    // Self-internalize each split output with wallet payment protocol
    // so the wallet stores proper derivation info and can sign for them.
    // The split is posted: a fault here is logged, never the command's error.
    let self_internalized = self_internalize(
        &ctx.wallet,
        &txid_hex,
        result.beef.as_deref(),
        count,
        &sender_identity_key,
    )
    .await;

    if ctx.json_output {
        println!(
            "{}",
            split_json(&txid_hex, count, per_output, self_internalized)
        );
    } else {
        println!("Split into {} UTXOs ({} sats each)", count, per_output);
        println!("TxID: {}", txid_hex);
        println!("View: https://whatsonchain.com/tx/{}", txid_hex);
        if !self_internalized {
            println!("Warning: the split outputs were not self-internalized (see the log)");
        }
    }

    Ok(())
}

/// The split's answer under `--json`. The txid is the post's truth; when
/// the self-internalize after it failed, `selfInternalized: false` says so.
fn split_json(
    txid_hex: &str,
    count: u32,
    per_output: u64,
    self_internalized: bool,
) -> serde_json::Value {
    let mut answer = serde_json::json!({
        "txid": txid_hex,
        "outputs": count,
        "satoshisPerOutput": per_output
    });
    if !self_internalized {
        answer["selfInternalized"] = serde_json::Value::Bool(false);
    }
    answer
}

/// Internalize the split's `count` outputs as wallet payments to ourselves,
/// after the broadcast. Every failure is logged with the marker
/// `post_broadcast_split_internalize_failed` (the txid, the error) and
/// answers `false`: the split is posted, its txid is still the answer.
async fn self_internalize<W: WalletInterface + ?Sized>(
    wallet: &W,
    txid_hex: &str,
    beef: Option<&[u8]>,
    count: u32,
    sender_identity_key: &str,
) -> bool {
    let atomic = match beef
        .map(|b| Beef::from_binary(b).and_then(|mut beef| beef.to_binary_atomic(txid_hex)))
    {
        Some(Ok(atomic)) => atomic,
        Some(Err(e)) => {
            tracing::error!(
                marker = "post_broadcast_split_internalize_failed",
                txid = %txid_hex,
                error = %e,
                "split posted; its BEEF could not be made atomic, the outputs are not self-internalized"
            );
            return false;
        }
        None => {
            tracing::error!(
                marker = "post_broadcast_split_internalize_failed",
                txid = %txid_hex,
                error = "no BEEF in the createAction result",
                "split posted; the outputs are not self-internalized"
            );
            return false;
        }
    };
    let outputs: Vec<InternalizeOutput> = (0..count)
        .map(|i| InternalizeOutput {
            output_index: i,
            protocol: "wallet payment".to_string(),
            payment_remittance: Some(WalletPayment {
                derivation_prefix: DEFAULT_DERIVATION_PREFIX.to_string(),
                derivation_suffix: DEFAULT_DERIVATION_SUFFIX.to_string(),
                sender_identity_key: sender_identity_key.to_string(),
            }),
            insertion_remittance: None,
        })
        .collect();
    match wallet
        .internalize_action(
            InternalizeActionArgs {
                tx: atomic,
                outputs,
                description: "Self-internalize split outputs".to_string(),
                labels: Some(vec!["split".to_string()]),
                seek_permission: None,
            },
            Some("bsv-wallet-cli"),
        )
        .await
    {
        Ok(_) => true,
        Err(e) => {
            tracing::error!(
                marker = "post_broadcast_split_internalize_failed",
                txid = %txid_hex,
                error = %e,
                "split posted; self-internalizing its outputs failed"
            );
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::Markers;
    use bsv_sdk::primitives::PrivateKey;
    use bsv_wallet_toolbox::services::mock::MockWalletServices;
    use bsv_wallet_toolbox::{StorageSqlx, Wallet, WalletStorageWriter};

    /// F2 of the #44 review: after the split is posted, a self-internalize
    /// that fails is logged with its marker and the txid, and the answer
    /// still carries the txid, with `selfInternalized: false`.
    #[tokio::test]
    async fn a_failed_self_internalize_after_the_post_still_answers_the_txid() {
        let (markers, _guard) = Markers::capture();
        let root = PrivateKey::random();
        let storage = StorageSqlx::in_memory().await.unwrap();
        storage
            .migrate("split", &root.public_key().to_hex())
            .await
            .unwrap();
        storage.make_available().await.unwrap();
        let wallet = Wallet::new(Some(root), storage, MockWalletServices::builder().build())
            .await
            .unwrap();
        // A split whose BEEF carries no proof and no stored parent: the
        // toolbox refuses to internalize it.
        let raw = crate::test_support::raw_tx(
            &[(&"ab".repeat(32), 0)],
            &[(1_000, crate::test_support::p2pkh([7; 20]))],
        );
        let txid = crate::test_support::txid_of(&raw);
        let tx = bsv_sdk::transaction::Transaction::from_binary(&raw).unwrap();
        let mut beef = Beef::new();
        beef.merge_transaction(tx);
        let beef_bytes = beef.to_binary();
        let anyone = KeyDeriver::anyone_key().1.to_hex();

        for beef in [Some(beef_bytes.as_slice()), Some(&[1u8, 2, 3][..]), None] {
            assert!(!self_internalize(&wallet, &txid, beef, 1, &anyone).await);
        }
        let failed = markers.named("post_broadcast_split_internalize_failed");
        assert_eq!(failed.len(), 3, "{:?}", markers.all());
        for line in &failed {
            assert_eq!(line.txid.as_deref(), Some(txid.as_str()));
            assert!(line.error.is_some());
        }

        let answer = split_json(&txid, 1, 1_000, false);
        assert_eq!(answer["txid"], txid);
        assert_eq!(answer["selfInternalized"], false);
        assert!(split_json(&txid, 1, 1_000, true)
            .get("selfInternalized")
            .is_none());
    }
}
