use anyhow::{anyhow, bail, Context, Result};
use bsv_sdk::primitives::{to_hex, PrivateKey};
use bsv_sdk::script::templates::P2PKH;
use bsv_sdk::transaction::Beef;
use bsv_sdk::wallet::{
    CreateActionArgs, CreateActionOptions, CreateActionOutput, InternalizeActionArgs,
    InternalizeOutput, WalletInterface, WalletPayment,
};

use crate::brc29;
use crate::context::WalletContext;

/// Where a `send` pays (#38). `to` is a P2PKH address (either chain's
/// prefix), or a 33-byte public key in hex when it is this wallet's own
/// identity key or deposit key. The destination is the wallet's own when its
/// locking script is the deposit script (`brc29::deposit_script`): the
/// BRC-29 derivation `address` prints, which `fund` and `receive` take in.
/// Any other key is refused: a payment to a stranger's key needs a
/// remittance this command does not hand over.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Destination {
    pub locking_script: Vec<u8>,
    /// The output is this wallet's own BRC-29 coin: kept, not relinquished.
    pub own: bool,
}

pub(crate) fn destination(root_key: &PrivateKey, to: &str) -> Result<Destination> {
    let deposit = brc29::deposit_script(root_key)?;
    if to.len() == 66 && to.chars().all(|c| c.is_ascii_hexdigit()) {
        let key = to.to_ascii_lowercase();
        let identity = hex::encode(root_key.public_key().to_compressed());
        let (_, deposit_pub) = brc29::deposit_keypair(root_key)?;
        if key == identity || key == hex::encode(deposit_pub.to_compressed()) {
            return Ok(Destination {
                locking_script: deposit,
                own: true,
            });
        }
        bail!(
            "send pays a P2PKH address; a public key is taken only when it is this \
             wallet's own identity or deposit key, and {to} is neither"
        );
    }
    let locking_script = P2PKH::lock_from_address(to)?.to_binary();
    let own = locking_script == deposit;
    Ok(Destination {
        locking_script,
        own,
    })
}

/// The payment output. A payment to another address leaves the wallet:
/// `relinquish`. A payment to itself carries no tag and no basket; the
/// internalize after the broadcast makes it a coin of the default basket.
pub(crate) fn payment_output(to: &Destination, satoshis: u64) -> CreateActionOutput {
    CreateActionOutput {
        locking_script: to.locking_script.clone(),
        satoshis,
        output_description: "P2PKH send".to_string(),
        basket: None,
        custom_instructions: None,
        tags: (!to.own).then(|| vec!["relinquish".to_string()]),
    }
}

pub async fn run(
    ctx: &WalletContext,
    address: &str,
    satoshis: u64,
    allow_pending: crate::spend_gate::AllowPending,
    from: &[(String, u32)],
) -> Result<()> {
    let to = destination(&ctx.root_key, address)?;
    // A self-payment's second step is an internalize, whose BEEF proofs need
    // headers: with no header service it is refused before anything is built,
    // so no output is broadcast that the wallet could not keep.
    if to.own && ctx.wallet.services().chaintracks.is_none() {
        bail!(
            "a send to this wallet's own address keeps its sats by internalizing the \
             payment, which needs the header service (CHAINTRACKS_URL); none is \
             configured, so nothing was sent"
        );
    }
    let address = if to.own {
        brc29::deposit_address(&ctx.root_key, ctx.chain)?
    } else {
        address.to_string()
    };
    let address = address.as_str();

    let description = format!("Send {} sats to {}", satoshis, &address[..8]);
    let args = CreateActionArgs {
        description: description.clone(),
        input_beef: None,
        inputs: Some(vec![]),
        outputs: Some(vec![payment_output(&to, satoshis)]),
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
        crate::spend_gate::create_action_from(
            &ctx.wallet,
            args,
            "bsv-wallet-cli",
            &ctx.gate().with_allow_pending(allow_pending),
            from,
        )
        .await,
        ctx.json_output,
    )?;

    let txid = result
        .txid
        .context("create_action returned no txid for a signed, sent spend")?;
    let txid_hex = to_hex(&txid);

    // Fail loudly if the broadcast was silently dropped. `create_action` returns
    // Ok with a txid even when ARC rejected the tx with error 465 ("fee too low"
    // on a deep unconfirmed BEEF), because the toolbox misclassifies a 465 as a
    // transient service error. Confirm the tx actually reached the network before
    // reporting success, so a phantom (never-mined) txid is never printed + exit 0.
    ctx.verifier()
        .verify(&txid_hex)
        .await
        .into_send_result(&txid_hex)?;

    // Build AtomicBEEF hex from result for direct wallet-to-wallet transfers
    let beef_hex = result.beef.as_ref().and_then(|beef_bytes| {
        Beef::from_binary(beef_bytes)
            .ok()
            .and_then(|mut beef| beef.to_binary_atomic(&txid_hex).ok().map(hex::encode))
    });

    // The reference's second step (#38): the wallet internalizes its own
    // payment with the deposit address's remittance (the prefix, the suffix
    // and the anyone key as sender, as `fund` does), so the output joins the
    // default basket as a spendable coin. The toolbox merges into the action
    // it already holds: nothing is broadcast again.
    let kept = if to.own {
        let beef = beef_hex
            .as_deref()
            .ok_or_else(|| anyhow!("send {txid_hex}: no BEEF to keep the payment with"))?;
        Some(
            keep_own_payment(
                ctx,
                beef,
                &txid_hex,
                &to.locking_script,
                satoshis,
                &description,
            )
            .await?,
        )
    } else {
        None
    };

    if ctx.json_output {
        let mut obj = serde_json::json!({ "txid": txid_hex });
        if let Some(vout) = kept {
            obj["kept"] = serde_json::json!({ "vout": vout });
        }
        if let Some(ref bh) = beef_hex {
            obj["beef"] = serde_json::Value::String(bh.clone());
        }
        println!("{}", obj);
    } else {
        println!("Sent {} satoshis to {}", satoshis, address);
        println!("TxID: {}", txid_hex);
        if let Some(vout) = kept {
            println!(
                "Kept: output {vout} is this wallet's own address, back in its balance (fee only)"
            );
        }
        if let Some(ref bh) = beef_hex {
            println!("BEEF: {}", bh);
        }
        println!("View: https://whatsonchain.com/tx/{}", txid_hex);
    }

    Ok(())
}

/// Internalize `txid`'s output paying `script` as a wallet payment to the
/// deposit key; returns its vout. A failure names the transaction (it is on
/// the network) and the `fund` command that keeps it later.
async fn keep_own_payment(
    ctx: &WalletContext,
    beef_hex: &str,
    txid_hex: &str,
    script: &[u8],
    satoshis: u64,
    description: &str,
) -> Result<u32> {
    let atomic = hex::decode(beef_hex)?;
    let tx = Beef::from_binary(&atomic)
        .ok()
        .and_then(|beef| beef.find_atomic_transaction(txid_hex))
        .ok_or_else(|| anyhow!("send {txid_hex}: the BEEF does not hold the transaction"))?;
    let vout = tx
        .outputs
        .iter()
        .position(|o| o.locking_script.to_binary() == script && o.satoshis == Some(satoshis))
        .ok_or_else(|| anyhow!("send {txid_hex}: no output pays this wallet's address"))?
        as u32;
    let (_, anyone_pubkey) = bsv_sdk::wallet::KeyDeriver::anyone_key();
    let args = InternalizeActionArgs {
        tx: atomic,
        outputs: vec![InternalizeOutput {
            output_index: vout,
            protocol: "wallet payment".to_string(),
            payment_remittance: Some(WalletPayment {
                derivation_prefix: brc29::DEFAULT_DERIVATION_PREFIX.to_string(),
                derivation_suffix: brc29::DEFAULT_DERIVATION_SUFFIX.to_string(),
                sender_identity_key: anyone_pubkey.to_hex(),
            }),
            insertion_remittance: None,
        }],
        // The same words as the action's: the merge rewrites its description.
        description: description.to_string(),
        labels: None,
        seek_permission: None,
    };
    let accepted = ctx
        .wallet
        .internalize_action(args, Some("bsv-wallet-cli"))
        .await
        .map(|r| r.accepted)
        .map_err(|e| e.to_string());
    match accepted {
        Ok(true) => Ok(vout),
        Ok(false) => Err(anyhow!("the wallet did not accept its own payment")),
        Err(e) => Err(anyhow!(e)),
    }
    .with_context(|| {
        format!(
            "send {txid_hex} is broadcast (do not send again), but its output to this \
             wallet was not kept; keep it with `bsv-wallet fund <BEEF> --vout {vout}`, \
             the BEEF being {beef_hex}"
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use bsv_wallet_toolbox::Chain;

    // Obviously-synthetic keys (0x01 x32, 0x02 x32), never anyone's wallet.
    fn root() -> PrivateKey {
        PrivateKey::from_hex(&"01".repeat(32)).unwrap()
    }
    fn stranger() -> PrivateKey {
        PrivateKey::from_hex(&"02".repeat(32)).unwrap()
    }

    /// The wallet's own BRC-29 address, on either chain's prefix, its
    /// identity key and its deposit key are each its own: the payment is
    /// kept, untagged, to the deposit script.
    #[test]
    fn own_address_and_own_keys_are_recognized_and_kept() {
        let root = root();
        let deposit = brc29::deposit_script(&root).unwrap();
        let (_, deposit_pub) = brc29::deposit_keypair(&root).unwrap();
        for to in [
            brc29::deposit_address(&root, Chain::Main).unwrap(),
            brc29::deposit_address(&root, Chain::Test).unwrap(),
            hex::encode(root.public_key().to_compressed()),
            hex::encode(root.public_key().to_compressed()).to_ascii_uppercase(),
            hex::encode(deposit_pub.to_compressed()),
        ] {
            let d = destination(&root, &to).unwrap();
            assert!(d.own, "{to} is this wallet's own");
            assert_eq!(d.locking_script, deposit, "{to} pays the deposit script");
            let out = payment_output(&d, 1_000);
            assert_eq!(out.tags, None, "{to}: a self-payment is never relinquished");
            assert_eq!(out.basket, None);
            assert_eq!(out.locking_script, deposit);
        }
    }

    /// Another wallet's BRC-29 address, and this wallet's identity key's own
    /// P2PKH address (not a BRC-29 derivation the wallet can sign for), are
    /// foreign: still tagged `relinquish`, as before #38.
    #[test]
    fn a_foreign_address_is_relinquished() {
        let root = root();
        for to in [
            brc29::deposit_address(&stranger(), Chain::Main).unwrap(),
            root.public_key().to_address(),
        ] {
            let d = destination(&root, &to).unwrap();
            assert!(!d.own, "{to} is not this wallet's deposit address");
            assert_eq!(
                payment_output(&d, 1_000).tags,
                Some(vec!["relinquish".to_string()])
            );
        }
    }

    /// A stranger's public key is refused by name: no output is built.
    #[test]
    fn a_foreign_key_is_refused() {
        let key = hex::encode(stranger().public_key().to_compressed());
        let e = destination(&root(), &key).unwrap_err().to_string();
        assert!(e.contains("neither") && e.contains(&key), "{e}");
    }
}
