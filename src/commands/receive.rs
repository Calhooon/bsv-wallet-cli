use anyhow::{anyhow, Context, Result};
use bsv_sdk::transaction::{Beef, MerklePath, Transaction};
use bsv_wallet_toolbox::services::GetBeefResult;
use bsv_wallet_toolbox::WalletServices;
use std::future::Future;

use crate::brc29;
use crate::commands::fund;
use crate::context::WalletContext;
use crate::explorers::{Explorers, PROVIDER_BANANABLOCKS, PROVIDER_WHATSONCHAIN};

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
    let explorers = Explorers::for_settings(ctx.chain, &ctx.config.services);
    receive_txid_from(ctx, &explorers, ctx.wallet.services(), txid, vout).await
}

/// Receive `txid` through the courier ladder at `explorers`, with
/// `services` the toolbox's ladders (the proof for BananaBlocks' bytes, and
/// the second courier).
pub(crate) async fn receive_txid_from<V: WalletServices>(
    ctx: &WalletContext,
    explorers: &Explorers,
    services: &V,
    txid: &str,
    vout: Option<u32>,
) -> Result<(u32, bool)> {
    let deposit_script = brc29::deposit_script(&ctx.root_key)?;

    let (beef_bytes, resolved_vout, courier) =
        fetch_beef_and_vout(explorers, services, txid, vout, &deposit_script, || {
            second_courier(services, txid)
        })
        .await?;

    let accepted = fund::internalize_beef(ctx, &beef_bytes, resolved_vout)
        .await
        .map_err(|e| anyhow!("refused tx {txid} from {courier}: {e:#}"))?;
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
/// for nothing but delivery. A ladder of couriers (bsv-wallet-cli #50: each
/// explorer has its own rate limits, so when one fails we go to the next):
/// WhatsOnChain's BEEF route; BananaBlocks' bytes (bound to the txid) with
/// the proof the wallet's services checked against the header service; and
/// behind them `second` (the toolbox's `get_beef`). A fault (a 429 among
/// them) moves to the next rung at once; a delivered body with incomplete
/// ancestry is refused at internalization. When none delivers, the error
/// says what each said ("could not look"), and nothing is concluded about
/// the payment.
async fn fetch_beef_and_vout<V, S, SF>(
    explorers: &Explorers,
    services: &V,
    txid: &str,
    vout: Option<u32>,
    deposit_script: &[u8],
    second: S,
) -> Result<(Vec<u8>, u32, &'static str)>
where
    V: WalletServices,
    S: FnOnce() -> SF,
    SF: Future<Output = Result<Vec<u8>>>,
{
    let mut said: Vec<String> = Vec::new();
    let mut delivered = None;
    match explorers.woc_beef(txid).await {
        Ok(bytes) => delivered = Some((bytes, PROVIDER_WHATSONCHAIN)),
        Err(fault) => said.push(fault),
    }
    if delivered.is_none() && explorers.bananablocks().is_some() {
        match bananablocks_courier(explorers, services, txid).await {
            Ok(bytes) => delivered = Some((bytes, PROVIDER_BANANABLOCKS)),
            Err(fault) => said.push(fault),
        }
    }
    let (beef_bytes, courier) = match delivered {
        Some(found) => found,
        None => {
            tracing::warn!(
                marker = "break_glass_beef_courier",
                txid = %txid,
                error = %said.join("; "),
                "the explorer couriers did not deliver; asking the wallet's services"
            );
            let bytes = second().await.map_err(|second| {
                anyhow!(
                    "no courier delivered a BEEF for {txid} (could not look): \
                     {}; the wallet's services: {second}",
                    said.join("; ")
                )
            })?;
            (bytes, "wallet services")
        }
    };

    let resolved_vout = match vout {
        Some(v) => v,
        None => vout_paying(&beef_bytes, txid, deposit_script)
            .map_err(|e| anyhow!("refused tx {txid} from {courier}: {e:#}"))?,
    };
    Ok((beef_bytes, resolved_vout, courier))
}

/// The BananaBlocks rung: its `/tx/{txid}/hex` bytes (the toolbox's
/// provider binds them to the txid: other bytes are a fault) with the
/// proof the wallet's services return only after its root met the header
/// service. No proof (an unmined payment, whose ancestry only its sender's
/// BEEF carries) is a fault here: the next courier is asked.
async fn bananablocks_courier<V: WalletServices>(
    explorers: &Explorers,
    services: &V,
    txid: &str,
) -> std::result::Result<Vec<u8>, String> {
    let raw = explorers.bananablocks_raw_tx(txid).await?;
    let path = match services.get_merkle_path(txid, false).await {
        Ok(found) => found.merkle_path.ok_or_else(|| {
            format!(
                "{PROVIDER_BANANABLOCKS}: the bytes, but no proof the header service checked ({})",
                found.error.as_deref().unwrap_or("not mined")
            )
        })?,
        Err(e) => {
            return Err(format!(
                "{PROVIDER_BANANABLOCKS}: the bytes, but the proof could not be had: {e}"
            ))
        }
    };
    let path = MerklePath::from_hex(&path)
        .map_err(|e| format!("{PROVIDER_BANANABLOCKS}: the proof does not parse: {e}"))?;
    let mut beef = Beef::new();
    let bump = beef.merge_bump(path);
    beef.merge_raw_tx(raw, Some(bump));
    Ok(beef.to_binary())
}

/// The second courier: the toolbox's `get_beef`, which carries the bytes
/// from its raw-tx ladder (WhatsOnChain, BananaBlocks, Bitails; hashed to
/// the txid) and a merkle path only after its root met the header service.
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
pub(crate) mod tests {
    use super::*;
    use crate::test_support::{p2pkh, raw_tx, txid_of, Fixture, TestChainTracker};
    use bsv_sdk::transaction::{Beef, MerklePath};
    use bsv_wallet_toolbox::services::mock::{MockResponse, MockWalletServices};
    use bsv_wallet_toolbox::services::GetMerklePathResult;
    use bsv_wallet_toolbox::Chain;

    /// The courier ladder with WhatsOnChain its one explorer, at a fixture.
    pub(crate) fn woc_only(base: &str) -> Explorers {
        Explorers::at(Some(base), None, None)
    }

    /// Services whose merkle path ladder returns `path` (a checked proof).
    fn proving(path: Option<String>) -> MockWalletServices {
        MockWalletServices::builder()
            .get_merkle_path_response(MockResponse::Success(GetMerklePathResult {
                name: Some("Services".to_string()),
                error: path.is_none().then(|| "not mined".to_string()),
                merkle_path: path,
                header: None,
                notes: vec![],
            }))
            .build()
    }

    /// A mined payment to `ours`: its bytes and its proof (BUMP hex).
    fn mined_payment(ours: &[u8]) -> (String, Vec<u8>, String) {
        let raw = raw_tx(&[(&"11".repeat(32), 0)], &[(1000, ours.to_vec())]);
        let txid = txid_of(&raw);
        let path = MerklePath::from_coinbase_txid(&txid, 900_000).to_hex();
        (txid, raw, path)
    }

    pub(crate) async fn stub_context() -> WalletContext {
        use bsv_sdk::primitives::PrivateKey;
        use bsv_wallet_toolbox::{
            Services, ServicesOptions, StorageSqlx, Wallet, WalletStorageWriter,
        };
        let root_key = PrivateKey::from_hex(&format!("{:064x}", 42)).unwrap();
        let storage = StorageSqlx::open(":memory:").await.unwrap();
        storage
            .migrate("stub-refusal-test", &root_key.public_key().to_hex())
            .await
            .unwrap();
        storage.make_available().await.unwrap();
        storage
            .set_chain_tracker(std::sync::Arc::new(TestChainTracker::new(900_001)))
            .await;
        let services = Services::with_options(Chain::Main, ServicesOptions::mainnet()).unwrap();
        let wallet = Wallet::new(Some(root_key.clone()), storage, services)
            .await
            .unwrap();
        WalletContext {
            wallet,
            identity_key: root_key.public_key().to_hex(),
            root_key,
            chain: Chain::Main,
            json_output: true,
            db_path: ":memory:".into(),
            config: std::sync::Arc::default(),
        }
    }

    pub(crate) fn stub(ctx: &WalletContext, salt: u8) -> (String, String) {
        let raw = raw_tx(
            &[(&format!("{salt:02x}").repeat(32), 0)],
            &[(1000, brc29::deposit_script(&ctx.root_key).unwrap())],
        );
        let txid = txid_of(&raw);
        let mut beef = Beef::new();
        beef.merge_raw_tx(raw, None);
        (txid, beef.to_hex())
    }

    /// The courier's bytes (a transaction with one input whose parent the
    /// BEEF does not carry, unproven) are refused as invalid BEEF by the
    /// toolbox's internalize door, the refusal names the txid and the
    /// courier, and nothing is stored. Assert the typed internalize refusal,
    /// then the CLI's context with that Display unchanged (#290, #330).
    #[tokio::test]
    async fn l2_stub_refusal_names_txid_courier_reason_and_stores_nothing() {
        let ctx = stub_context().await;
        let (txid, body) = stub(&ctx, 17);
        let route = format!("/tx/{txid}/beef");
        let courier = Fixture::start(&[(&route, 200, &body)], 500).await;
        let refusal = fund::internalize_beef(&ctx, &hex::decode(&body).unwrap(), 0)
            .await
            .unwrap_err();
        assert!(matches!(
            refusal.downcast_ref::<bsv_sdk::Error>(),
            Some(bsv_sdk::Error::WalletRefusal {
                class: bsv_sdk::WalletRefusalClass::InvalidParameter,
                ..
            })
        ));
        let expected = format!("refused tx {txid} from WhatsOnChain: {refusal}");
        // Exercise both output discovery and an explicitly named output.
        for vout in [None, Some(0)] {
            let error = receive_txid_from(
                &ctx,
                &woc_only(&courier.base),
                &MockWalletServices::new(),
                &txid,
                vout,
            )
            .await
            .unwrap_err();
            assert_eq!(error.to_string(), expected);
            for table in ["transactions", "outputs"] {
                let count: i64 = sqlx::query_scalar(&format!("SELECT COUNT(*) FROM {table}"))
                    .fetch_one(ctx.wallet.storage().pool())
                    .await
                    .unwrap();
                assert_eq!(count, 0, "refusal wrote {table}");
            }
        }
        assert_eq!(courier.hits(), vec![route.clone(), route]);
    }

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
            &woc_only(&explorer.base),
            &MockWalletServices::new(),
            &txid,
            None,
            &ours,
            no_second_courier,
        )
        .await;

        assert_eq!(explorer.count(&tx_route), 0, "{:?}", explorer.hits());
        let (_, vout, _) = got.expect("the BEEF carries the transaction");
        assert_eq!(vout, 1);
        assert_eq!(explorer.hits(), vec![beef_route.clone()]);

        // A named output is taken as named.
        let (_, vout, _) = fetch_beef_and_vout(
            &woc_only(&explorer.base),
            &MockWalletServices::new(),
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
            &woc_only(&explorer.base),
            &MockWalletServices::new(),
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
        let (bytes, _, _) = fetch_beef_and_vout(
            &woc_only(&explorer.base),
            &MockWalletServices::new(),
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

    /// #50: a 429 at WhatsOnChain reaches BananaBlocks in the same call,
    /// with no sleep; BananaBlocks' bytes with the proof the services
    /// checked are the BEEF, and the second courier is not asked. The
    /// closed rung is skipped, without a request, on the next receive.
    #[tokio::test]
    async fn a_woc_429_reaches_bananablocks_at_once_and_its_bytes_carry_the_proof() {
        let ours = p2pkh([0xdb; 20]);
        let (txid, raw, path) = mined_payment(&ours);
        let beef_route = format!("/tx/{txid}/beef");
        let woc =
            Fixture::start_with_headers(&[(&beef_route, 429, "", &[("retry-after", "30")])], 500)
                .await;
        let hex_route = format!("/tx/{txid}/hex");
        let bb = Fixture::start(&[(&hex_route, 200, &hex::encode(&raw))], 404).await;
        let explorers = Explorers::at(Some(&woc.base), Some(&bb.base), None);
        let services = proving(Some(path.clone()));

        for round in 0..2 {
            let started = std::time::Instant::now();
            let (bytes, vout, courier) =
                fetch_beef_and_vout(&explorers, &services, &txid, None, &ours, no_second_courier)
                    .await
                    .unwrap();
            assert!(
                started.elapsed() < std::time::Duration::from_secs(2),
                "slept"
            );
            assert_eq!(courier, "BananaBlocks", "round {round}");
            assert_eq!(vout, 0);
            let beef = Beef::from_binary(&bytes).unwrap();
            let held = beef.find_txid(&txid).expect("the payment");
            assert_eq!(held.raw_tx(), Some(raw.as_slice()));
            assert_eq!(beef.bumps.len(), 1, "the checked proof rides along");
        }
        assert_eq!(woc.count(&beef_route), 1, "the closed rung was asked again");
        assert_eq!(bb.count(&hex_route), 2);
        assert_eq!(services.call_count("get_beef"), 0);
    }

    /// BananaBlocks' bytes that do not hash to the txid are a fault, and
    /// bytes with no proof the header service checked (an unmined payment)
    /// are not a BEEF this rung can hand on: the second courier is asked.
    #[tokio::test]
    async fn the_bananablocks_rung_moves_on_without_bound_bytes_or_a_checked_proof() {
        let ours = p2pkh([0xdb; 20]);
        let (txid, raw, path) = mined_payment(&ours);
        let other = raw_tx(&[(&"22".repeat(32), 0)], &[(5, ours.clone())]);
        let woc = Fixture::start(&[], 503).await;
        let hex_route = format!("/tx/{txid}/hex");

        let wrong = Fixture::start(&[(&hex_route, 200, &hex::encode(&other))], 404).await;
        let services = proving(Some(path));
        let err = fetch_beef_and_vout(
            &Explorers::at(Some(&woc.base), Some(&wrong.base), None),
            &services,
            &txid,
            Some(0),
            &ours,
            || async { Err(anyhow!("the services' ladder: down")) },
        )
        .await
        .unwrap_err()
        .to_string();
        assert!(err.contains("could not look"), "{err}");
        assert!(err.contains("WhatsOnChain: HTTP 503"), "{err}");
        assert!(err.contains("BananaBlocks: "), "{err}");
        assert!(err.contains("the services' ladder: down"), "{err}");
        assert_eq!(services.call_count("get_merkle_path"), 0, "unbound bytes");

        let right = Fixture::start(&[(&hex_route, 200, &hex::encode(&raw))], 404).await;
        let unproven = proving(None);
        let (_, _, courier) = fetch_beef_and_vout(
            &Explorers::at(Some(&woc.base), Some(&right.base), None),
            &unproven,
            &txid,
            Some(0),
            &ours,
            || second_courier(&unproven, &txid),
        )
        .await
        .unwrap();
        assert_eq!(courier, "wallet services");
        assert_eq!(unproven.call_count("get_merkle_path"), 1);
        assert_eq!(unproven.call_count("get_beef"), 1);
    }

    /// The courier ladder from the environment: BananaBlocks on by default.
    #[test]
    fn the_courier_ladder_has_bananablocks_by_default() {
        let explorers = Explorers::configured(Chain::Main, None, None);
        assert!(explorers.bananablocks().is_some());
    }

    /// Neither courier delivering is "could not look", with what each said.
    #[tokio::test]
    async fn no_courier_delivering_is_could_not_look_with_both_reasons() {
        let txid = "ab".repeat(32);
        let explorer = Fixture::start(&[], 503).await;
        let err = fetch_beef_and_vout(
            &woc_only(&explorer.base),
            &MockWalletServices::new(),
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
