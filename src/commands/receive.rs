use anyhow::{anyhow, Context, Result};
use bsv_sdk::transaction::{Beef, Transaction};
use bsv_wallet_toolbox::services::GetBeefResult;
use bsv_wallet_toolbox::{Chain, WalletServices};
use std::future::Future;

use crate::brc29;
use crate::commands::fund;
use crate::context::WalletContext;

/// WhatsOnChain's API base: the explorer behind the three break-glass
/// reads the commands still make themselves (Rule 28: C2 here, C4 in
/// `sync`, C7 in the spend probe). Every other chain question goes through
/// the wallet's services or the header service.
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

    let services = ctx.wallet.services();
    let (beef_bytes, resolved_vout) =
        fetch_beef_and_vout(&client, base, txid, vout, &deposit_script, || {
            second_courier(services, txid)
        })
        .await?;

    let accepted = fund::internalize_beef(ctx, &beef_bytes, resolved_vout).await?;
    Ok((resolved_vout, accepted))
}

/// The BEEF of `txid`, and the output of it that pays us: `vout` when the
/// caller named one, else read from the BEEF's own transaction.
///
/// Break-glass (Rule 28, C2): a COURIER. A payment to a bare address
/// arrives with no BEEF from its sender, so no row, header or proof of ours
/// holds this answer and it is fetched. What is fetched is self-verifying:
/// the BEEF is internalized, its transaction hashes to the txid and its
/// merkle paths meet the header service's roots, so the courier is trusted
/// for nothing but delivery. Two couriers: WhatsOnChain's BEEF route first
/// (it alone carries an unmined transaction's ancestors), and behind it
/// `second` (the toolbox's `get_beef`). A fault at the first falls through;
/// when neither delivers, the error says what each said ("could not
/// look"), and nothing is concluded about the payment.
async fn fetch_beef_and_vout<S, SF>(
    client: &reqwest::Client,
    base: &str,
    txid: &str,
    vout: Option<u32>,
    deposit_script: &[u8],
    second: S,
) -> Result<(Vec<u8>, u32)>
where
    S: FnOnce() -> SF,
    SF: Future<Output = Result<Vec<u8>>>,
{
    let beef_bytes = match first_courier(client, base, txid).await {
        Ok(bytes) => bytes,
        Err(first) => {
            tracing::warn!(
                marker = "break_glass_beef_courier",
                txid = %txid,
                error = %first,
                "the first BEEF courier did not deliver; asking the second"
            );
            second().await.map_err(|second| {
                anyhow!(
                    "no courier delivered a BEEF for {txid} (could not look): \
                     WhatsOnChain: {first}; the wallet's services: {second}"
                )
            })?
        }
    };

    let resolved_vout = match vout {
        Some(v) => v,
        None => vout_paying(&beef_bytes, txid, deposit_script)?,
    };
    Ok((beef_bytes, resolved_vout))
}

/// WhatsOnChain's `/tx/{txid}/beef`: the transaction with its proofs, hex.
///
/// Break-glass (Rule 28, C2): a courier. Nothing we hold answers "the BEEF
/// of a payment nobody handed us"; what this returns is checked against
/// the header service when it is internalized, so the explorer is trusted
/// for delivery alone, and `second_courier` stands behind it.
async fn first_courier(client: &reqwest::Client, base: &str, txid: &str) -> Result<Vec<u8>> {
    let beef_hex = client
        .get(format!("{}/tx/{}/beef", base, txid))
        .send()
        .await
        .with_context(|| format!("WoC BEEF fetch failed for {}", txid))?
        .error_for_status()?
        .text()
        .await?;
    Ok(hex::decode(beef_hex.trim())?)
}

/// The second courier: the toolbox's `get_beef`, which carries the bytes
/// from either explorer (hashed to the txid) and a merkle path only after
/// its root met the header service.
async fn second_courier<V: WalletServices>(services: &V, txid: &str) -> Result<Vec<u8>> {
    let found = services
        .get_beef(txid, &[])
        .await
        .map_err(|e| anyhow!("{e}"))?;
    beef_from(found)
}

fn beef_from(found: GetBeefResult) -> Result<Vec<u8>> {
    found.beef.ok_or_else(|| {
        anyhow!(
            "{}",
            found
                .error
                .unwrap_or_else(|| "no BEEF and no reason given".to_string())
        )
    })
}

/// Which output of `txid` pays `deposit_script`, read from the transaction
/// the BEEF carries (Rule 28, C3, a deletion): the BEEF we just fetched
/// holds the transaction's own bytes, so its outputs are matched here and no
/// explorer is asked for a decoded copy of them.
fn vout_paying(beef_bytes: &[u8], txid: &str, deposit_script: &[u8]) -> Result<u32> {
    crate::atomic_beef::refuse_invalid_bytes(beef_bytes)?;
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
    use bsv_wallet_toolbox::services::mock::MockWalletServices;

    /// A second courier that must not be reached.
    async fn no_second_courier() -> Result<Vec<u8>> {
        panic!("the second courier was asked while the first delivered")
    }

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

        let got = fetch_beef_and_vout(
            &reqwest::Client::new(),
            &explorer.base,
            &txid,
            None,
            &ours,
            no_second_courier,
        )
        .await;

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
            no_second_courier,
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

    /// C2 (Rule 28): the BEEF courier has a second courier behind it. Red
    /// at the base: WhatsOnChain's fault was the command's error.
    #[tokio::test]
    async fn a_courier_fault_falls_through_to_the_second_courier() {
        let txid = "ab".repeat(32);
        let explorer = Fixture::start(&[], 500).await;
        let services = MockWalletServices::new();
        let got = fetch_beef_and_vout(
            &reqwest::Client::new(),
            &explorer.base,
            &txid,
            Some(0),
            &p2pkh([0xdb; 20]),
            || second_courier(&services, &txid),
        )
        .await;
        assert!(
            got.is_ok(),
            "one courier's fault ended the command: {got:?}"
        );
        assert_eq!(explorer.total(), 1, "the first courier was asked once");
        assert_eq!(services.call_count("get_beef"), 1);
    }

    /// The second courier is not asked while the first delivers.
    #[tokio::test]
    async fn the_second_courier_is_not_asked_while_the_first_delivers() {
        let txid = "ab".repeat(32);
        let route = format!("/tx/{txid}/beef");
        let explorer = Fixture::start(&[(&route, 200, "0100beef")], 500).await;
        let services = MockWalletServices::new();
        let (bytes, _) = fetch_beef_and_vout(
            &reqwest::Client::new(),
            &explorer.base,
            &txid,
            Some(0),
            &p2pkh([0xdb; 20]),
            || second_courier(&services, &txid),
        )
        .await
        .unwrap();
        assert_eq!(bytes, vec![0x01, 0x00, 0xbe, 0xef]);
        assert_eq!(services.call_count("get_beef"), 0);
    }

    /// Neither courier delivering is "could not look", with what each said.
    #[tokio::test]
    async fn no_courier_delivering_is_could_not_look_with_both_reasons() {
        let txid = "ab".repeat(32);
        let explorer = Fixture::start(&[], 503).await;
        let err = fetch_beef_and_vout(
            &reqwest::Client::new(),
            &explorer.base,
            &txid,
            Some(0),
            &p2pkh([0xdb; 20]),
            || async { Err(anyhow!("both explorers down")) },
        )
        .await
        .unwrap_err()
        .to_string();
        assert!(err.contains("could not look"), "{err}");
        assert!(err.contains("503"), "{err}");
        assert!(err.contains("both explorers down"), "{err}");

        // The toolbox's "no BEEF" carries its reason through.
        let none = GetBeefResult {
            name: "Services".to_string(),
            txid: txid.clone(),
            beef: None,
            has_proof: false,
            error: Some("Transaction not found".to_string()),
        };
        assert_eq!(
            beef_from(none).unwrap_err().to_string(),
            "Transaction not found"
        );
    }

    /// The courier is named at its site, in the rule's words.
    #[test]
    fn the_beef_courier_is_named_at_the_site() {
        let source = include_str!("receive.rs");
        let code = source.split("#[cfg(test)]").next().unwrap();
        assert!(code.contains("Break-glass (Rule 28, C2): a COURIER"));
    }

    /// The courier's BEEF is a stranger's bytes: a transaction with no input
    /// is refused at its offset before an output is matched, and a cut BEEF
    /// at the field that ran out.
    #[test]
    fn the_couriers_beef_is_refused_for_invalid_bytes_at_their_offset() {
        let ours = p2pkh([0xdb; 20]);
        let mut raw = vec![1, 0, 0, 0, 0, 1];
        raw.extend_from_slice(&1000u64.to_le_bytes());
        raw.push(ours.len() as u8);
        raw.extend_from_slice(&ours);
        raw.extend_from_slice(&[0, 0, 0, 0]);
        let mut h = bsv_sdk::primitives::sha256d(&raw).to_vec();
        h.reverse();
        let txid = hex::encode(h);
        let mut beef = Beef::new();
        let bump = beef.merge_bump(MerklePath::from_coinbase_txid(&txid, 900_000));
        beef.merge_raw_tx(raw.clone(), Some(bump));
        let bytes = beef.to_binary();
        let at = bytes.windows(raw.len()).position(|w| w == raw).unwrap();

        let text = vout_paying(&bytes, &txid, &ours)
            .expect_err("a transaction with no input is invalid bytes")
            .to_string();
        assert!(
            text.contains(&format!("Invalid BEEF at byte {at}")) && text.contains("NoInputs"),
            "{text}"
        );

        let text = vout_paying(&bytes[..bytes.len() - 6], &txid, &ours)
            .expect_err("cut bytes")
            .to_string();
        assert!(
            text.contains("Invalid BEEF at byte") && text.contains("Truncated"),
            "{text}"
        );
    }
}
