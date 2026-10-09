use anyhow::{anyhow, Context, Result};
use bsv_sdk::transaction::{Beef, Transaction};
use bsv_wallet_toolbox::Chain;

use crate::brc29;
use crate::commands::fund;
use crate::context::WalletContext;

pub fn woc_base(chain: Chain) -> &'static str {
    match chain {
        Chain::Main => "https://api.whatsonchain.com/v1/bsv/main",
        Chain::Test => "https://api.whatsonchain.com/v1/bsv/test",
    }
}

pub async fn run(ctx: &WalletContext, txid: &str, vout: Option<u32>) -> Result<()> {
    let (resolved_vout, accepted) = receive_txid(ctx, txid, vout).await?;

    if ctx.json_output {
        println!(
            "{}",
            serde_json::json!({
                "txid": txid,
                "vout": resolved_vout,
                "accepted": accepted,
            })
        );
    } else if accepted {
        println!("Received tx {} vout {}", txid, resolved_vout);
    } else {
        println!("Transaction was not accepted");
    }

    Ok(())
}

pub(crate) async fn receive_txid(
    ctx: &WalletContext,
    txid: &str,
    vout: Option<u32>,
) -> Result<(u32, bool)> {
    let base = woc_base(ctx.chain);
    let client = reqwest::Client::new();
    let deposit_script = brc29::deposit_script(&ctx.root_key)?;

    let (beef_bytes, resolved_vout) =
        fetch_beef_and_vout(&client, base, txid, vout, &deposit_script).await?;

    let accepted = fund::internalize_beef(ctx, &beef_bytes, resolved_vout).await?;
    Ok((resolved_vout, accepted))
}

/// The BEEF of `txid`, and the output of it that pays us: `vout` when the
/// caller named one, else read from the BEEF's own transaction.
async fn fetch_beef_and_vout(
    client: &reqwest::Client,
    base: &str,
    txid: &str,
    vout: Option<u32>,
    deposit_script: &[u8],
) -> Result<(Vec<u8>, u32)> {
    let beef_hex = client
        .get(format!("{}/tx/{}/beef", base, txid))
        .send()
        .await
        .with_context(|| format!("WoC BEEF fetch failed for {}", txid))?
        .error_for_status()?
        .text()
        .await?;
    let beef_bytes = hex::decode(beef_hex.trim())?;

    let resolved_vout = match vout {
        Some(v) => v,
        None => vout_paying(&beef_bytes, txid, deposit_script)?,
    };
    Ok((beef_bytes, resolved_vout))
}

/// Which output of `txid` pays `deposit_script`, read from the transaction
/// the BEEF carries (Rule 28, C3, a deletion): the BEEF we just fetched
/// holds the transaction's own bytes, so its outputs are matched here and no
/// explorer is asked for a decoded copy of them.
fn vout_paying(beef_bytes: &[u8], txid: &str, deposit_script: &[u8]) -> Result<u32> {
    let beef = Beef::from_binary(beef_bytes).context("the BEEF does not parse")?;
    let held = beef
        .find_txid(txid)
        .ok_or_else(|| anyhow!("the BEEF does not carry transaction {}", txid))?;
    let tx = match (held.tx(), held.raw_tx()) {
        (Some(tx), _) => tx.clone(),
        (None, Some(raw)) => Transaction::from_binary(raw)
            .with_context(|| format!("transaction {} in the BEEF does not parse", txid))?,
        (None, None) => return Err(anyhow!("the BEEF carries only the txid of {}", txid)),
    };
    tx.outputs
        .iter()
        .position(|o| o.locking_script.to_binary() == deposit_script)
        .map(|n| n as u32)
        .ok_or_else(|| {
            anyhow!(
                "tx {} has no output to our deposit script {}",
                txid,
                hex::encode(deposit_script)
            )
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{p2pkh, raw_tx, txid_of, Fixture};
    use bsv_sdk::transaction::{Beef, MerklePath};

    /// C3 (Rule 28): which output pays us is read from the transaction the
    /// BEEF carries. Red at the base, with a local fixture in the explorer's
    /// place: a second request, `/tx/{txid}`, for a decoded copy of the
    /// outputs, and no answer when that request failed.
    #[tokio::test]
    async fn the_output_index_is_read_from_the_beefs_own_transaction() {
        let ours = p2pkh([0xdb; 20]);
        let raw = raw_tx(
            &[(&"11".repeat(32), 0)],
            &[(500, p2pkh([0x01; 20])), (1000, ours.clone())],
        );
        let txid = txid_of(&raw);
        let mut beef = Beef::new();
        let bump = beef.merge_bump(MerklePath::from_coinbase_txid(&txid, 900_000));
        beef.merge_raw_tx(raw, Some(bump));
        let beef_route = format!("/tx/{txid}/beef");
        let tx_route = format!("/tx/{txid}");
        let explorer = Fixture::start(&[(&beef_route, 200, &beef.to_hex())], 500).await;

        let got =
            fetch_beef_and_vout(&reqwest::Client::new(), &explorer.base, &txid, None, &ours).await;

        assert_eq!(explorer.count(&tx_route), 0, "{:?}", explorer.hits());
        let (_, vout) = got.expect("the BEEF carries the transaction");
        assert_eq!(vout, 1);
        assert_eq!(explorer.hits(), vec![beef_route.clone()]);

        // A named output is taken as named.
        let (_, vout) = fetch_beef_and_vout(
            &reqwest::Client::new(),
            &explorer.base,
            &txid,
            Some(0),
            &ours,
        )
        .await
        .unwrap();
        assert_eq!(vout, 0);

        // A transaction that pays us nothing is an error, not output 0.
        let err = vout_paying(&beef.to_binary(), &txid, &p2pkh([0x77; 20])).unwrap_err();
        assert!(
            err.to_string().contains("no output to our deposit script"),
            "{err}"
        );
        let err = vout_paying(&beef.to_binary(), &"cd".repeat(32), &ours).unwrap_err();
        assert!(
            err.to_string().contains("does not carry transaction"),
            "{err}"
        );
    }
}
