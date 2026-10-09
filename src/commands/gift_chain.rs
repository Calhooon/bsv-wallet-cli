//! What the gift commands ask of the chain, through the wallet's services
//! (Rule 28, C11 and C12): a foreign deposit's bytes, and the broadcast of
//! the claim. Neither command holds an explorer base or an HTTP client.

use anyhow::{anyhow, Result};
use bsv_sdk::transaction::{Beef, MerklePath, Transaction};
use bsv_wallet_toolbox::WalletServices;

/// The bytes of the deposit transaction `txid`.
///
/// Break-glass (Rule 28): the deposit is a stranger's transaction, so no
/// row, header or proof of ours holds its bytes; they are asked of the
/// explorers, through `Services::get_raw_tx`. The answer checks itself (the
/// toolbox hashes the bytes to the txid asked for), a second explorer
/// stands behind the first, and "no explorer has it" is kept apart from
/// "could not look".
pub(crate) async fn deposit_bytes<V: WalletServices>(services: &V, txid: &str) -> Result<Vec<u8>> {
    let found = services
        .get_raw_tx(txid, false)
        .await
        .map_err(|e| anyhow!("could not look for deposit {txid}: {e}"))?;
    if found.is_not_found() {
        return Err(anyhow!(
            "no such transaction: every explorer asked says it has no {txid}"
        ));
    }
    found.raw_tx.ok_or_else(|| {
        anyhow!(
            "could not look for deposit {txid}: {}",
            found
                .error
                .unwrap_or_else(|| "no explorer gave an answer".to_string())
        )
    })
}

/// Broadcast the claim through the wallet's broadcasters (`post_beef`: the
/// ladder the wallet's own sends use, with its broadcast memory), never a
/// post of our own to an explorer. Returns the provider that accepted it.
///
/// The BEEF is the deposit and the claim. The deposit carries its merkle
/// proof when the header service checks one for it (`get_merkle_path`
/// returns only a checked proof); a deposit not mined yet goes as bytes.
pub(crate) async fn broadcast_claim<V: WalletServices>(
    services: &V,
    deposit_raw: &[u8],
    claim_raw: &[u8],
) -> Result<String> {
    let deposit_txid = Transaction::from_binary(deposit_raw)
        .map_err(|e| anyhow!("parse deposit: {e}"))?
        .id();
    let claim_txid = Transaction::from_binary(claim_raw)
        .map_err(|e| anyhow!("parse claim: {e}"))?
        .id();

    let proof = match services.get_merkle_path(&deposit_txid, false).await {
        Ok(found) => found
            .merkle_path
            .and_then(|hex| MerklePath::from_hex(&hex).ok()),
        Err(_) => None,
    };
    let mut beef = Beef::new();
    match proof {
        Some(path) => {
            let index = beef.merge_bump(path);
            beef.merge_raw_tx(deposit_raw.to_vec(), Some(index));
        }
        None => {
            beef.merge_raw_tx(deposit_raw.to_vec(), None);
        }
    }
    beef.merge_raw_tx(claim_raw.to_vec(), None);

    let results = services
        .post_beef(&beef.to_binary(), std::slice::from_ref(&claim_txid))
        .await
        .map_err(|e| anyhow!("broadcast failed: {e}"))?;
    if let Some(accepted) = results.iter().find(|r| r.is_success()) {
        return Ok(accepted.name.clone());
    }
    let said: Vec<String> = results
        .iter()
        .map(|r| {
            let detail = r
                .error
                .clone()
                .or_else(|| r.txid_results.iter().find_map(|t| t.data.clone()))
                .unwrap_or_else(|| r.status.clone());
            format!("{}: {}", r.name, detail)
        })
        .collect();
    Err(anyhow!(
        "broadcast rejected by every broadcaster ({})",
        if said.is_empty() {
            "none answered".to_string()
        } else {
            said.join("; ")
        }
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{p2pkh, raw_tx, txid_of};
    use bsv_wallet_toolbox::services::mock::{
        error_post_beef_result, MockErrorKind, MockResponse, MockWalletServices,
    };
    use bsv_wallet_toolbox::services::{GetMerklePathResult, GetRawTxResult};

    /// C11, C12 (Rule 28): the gift commands hold no explorer base and no
    /// HTTP client of their own. Red at the base: both fetched
    /// `/tx/{txid}/hex` from WhatsOnChain directly, and `gift-claim` posted
    /// its transaction to WhatsOnChain's `/tx/raw`.
    #[test]
    fn the_gift_commands_hold_no_explorer_and_no_http_client() {
        for (name, source) in [
            ("gift_claim.rs", include_str!("gift_claim.rs")),
            ("gift_inspect.rs", include_str!("gift_inspect.rs")),
        ] {
            let code = source.split("#[cfg(test)]").next().unwrap();
            for word in ["reqwest", "woc_base", "api.whatsonchain.com", "/tx/raw"] {
                assert!(!code.contains(word), "{name} names `{word}`");
            }
        }
    }

    fn raw_tx_answer(raw_tx: Option<Vec<u8>>, could_not_look: bool) -> MockWalletServices {
        MockWalletServices::builder()
            .get_raw_tx_response(MockResponse::Success(GetRawTxResult {
                name: "Services".to_string(),
                txid: String::new(),
                raw_tx,
                error: could_not_look.then(|| "WhatsOnChain: HTTP 500".to_string()),
                could_not_look,
            }))
            .build()
    }

    /// The deposit's bytes come through `Services::get_raw_tx`, and its two
    /// "no bytes" answers stay apart.
    #[tokio::test]
    async fn the_deposits_bytes_come_through_the_wallets_services() {
        let txid = "ab".repeat(32);
        let held = raw_tx_answer(Some(vec![1, 2, 3]), false);
        assert_eq!(deposit_bytes(&held, &txid).await.unwrap(), vec![1, 2, 3]);
        assert_eq!(held.call_count("get_raw_tx"), 1);

        let nowhere = raw_tx_answer(None, false);
        let err = deposit_bytes(&nowhere, &txid).await.unwrap_err();
        assert!(err.to_string().contains("no such transaction"), "{err}");

        let down = raw_tx_answer(None, true);
        let err = deposit_bytes(&down, &txid).await.unwrap_err();
        assert!(err.to_string().contains("could not look"), "{err}");
        assert!(!err.to_string().contains("no such transaction"), "{err}");

        let fault = MockWalletServices::builder()
            .get_raw_tx_error(MockErrorKind::NetworkError, "down")
            .build();
        let err = deposit_bytes(&fault, &txid).await.unwrap_err();
        assert!(err.to_string().contains("could not look"), "{err}");
    }

    fn deposit_and_claim() -> (Vec<u8>, Vec<u8>) {
        let lock = p2pkh([0xdb; 20]);
        let deposit = raw_tx(
            &[(&"11".repeat(32), 0)],
            &[(1000, lock.clone()), (200, lock.clone())],
        );
        let deposit_txid = txid_of(&deposit);
        let claim = raw_tx(&[(&deposit_txid, 0), (&deposit_txid, 1)], &[(1100, lock)]);
        (deposit, claim)
    }

    /// The claim goes out through the wallet's broadcasters, once, as a
    /// BEEF naming the claim; the accepting provider is reported.
    #[tokio::test]
    async fn the_claim_is_broadcast_through_the_wallets_broadcasters() {
        let (deposit, claim) = deposit_and_claim();
        let claim_txid = txid_of(&claim);
        // No proof of the deposit to be had: it goes as bytes.
        let services = MockWalletServices::builder()
            .get_merkle_path_response(MockResponse::Success(GetMerklePathResult {
                name: Some("Services".to_string()),
                merkle_path: None,
                header: None,
                error: None,
                notes: vec![],
            }))
            .build();

        let provider = broadcast_claim(&services, &deposit, &claim).await.unwrap();
        assert_eq!(provider, "MockProvider");
        let calls = services.call_history();
        let posts: Vec<_> = calls.iter().filter(|c| c.method == "post_beef").collect();
        assert_eq!(posts.len(), 1);
        assert!(
            posts[0].args.iter().any(|a| a.contains(&claim_txid)),
            "the claim is the subject: {:?}",
            posts[0].args
        );
    }

    /// A deposit the header service has a checked proof for carries it.
    #[tokio::test]
    async fn a_mined_deposit_carries_its_checked_proof() {
        let (deposit, claim) = deposit_and_claim();
        let path = MerklePath::from_coinbase_txid(&txid_of(&deposit), 900_000).to_hex();
        let services = MockWalletServices::builder()
            .get_merkle_path_response(MockResponse::Success(GetMerklePathResult {
                name: Some("Services".to_string()),
                merkle_path: Some(path),
                header: None,
                error: None,
                notes: vec![],
            }))
            .build();
        broadcast_claim(&services, &deposit, &claim).await.unwrap();
        assert_eq!(services.call_count("get_merkle_path"), 1);
        assert_eq!(services.call_count("post_beef"), 1);
    }

    /// Every broadcaster refusing is an error that says what they said.
    #[tokio::test]
    async fn a_refused_claim_is_an_error_naming_the_refusal() {
        let (deposit, claim) = deposit_and_claim();
        let services = MockWalletServices::builder()
            .post_beef_response(MockResponse::Success(vec![error_post_beef_result(
                "Arcade",
                "non-final",
            )]))
            .build();
        let err = broadcast_claim(&services, &deposit, &claim)
            .await
            .unwrap_err();
        assert!(err.to_string().contains("Arcade: non-final"), "{err}");
    }
}
