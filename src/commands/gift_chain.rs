//! What the gift commands ask of the chain, through the wallet's services
//! (Rule 28, C11 and C12): a foreign deposit's bytes, and the broadcast of
//! the claim. Neither command holds an explorer base or an HTTP client.
//!
//! The claim is a transaction this wallet built, so the tracker holds it
//! from the moment it is handed over (E2, the rulings of 2026-10-09): what
//! the broadcaster answers is a hint, and the word the user sees is the
//! tracker's.

use anyhow::{anyhow, Result};
use bsv_sdk::transaction::{Beef, MerklePath, Transaction};
use bsv_tracker::{Clock, Hint, HintStatus};
use bsv_wallet_toolbox::{StorageSqlx, WalletServices};

use crate::tracker_host::{self, PostAnswer, Reannounce, TickOptions};

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

/// The claim, posted: its txid, the BEEF that carried it, and what the
/// broadcasters answered.
pub(crate) struct ClaimPost {
    pub claim_txid: String,
    pub beef: Vec<u8>,
    pub answer: PostAnswer,
}

/// Post the claim through the wallet's broadcasters (`post_beef`: the
/// ladder the wallet's own sends use, with its broadcast memory), never a
/// post of our own to an explorer. An error is "could not post"; a refusal
/// is an answer.
///
/// The BEEF is the deposit and the claim. The deposit carries its merkle
/// proof when the header service checks one for it (`get_merkle_path`
/// returns only a checked proof); a deposit not mined yet goes as bytes.
pub(crate) async fn broadcast_claim<V: WalletServices>(
    services: &V,
    deposit_raw: &[u8],
    claim_raw: &[u8],
) -> Result<ClaimPost> {
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
    let beef = beef.to_binary();

    let results = services
        .post_beef(&beef, std::slice::from_ref(&claim_txid))
        .await
        .map_err(|e| anyhow!("broadcast failed: {e}"))?;
    Ok(ClaimPost {
        claim_txid,
        beef,
        answer: tracker_host::read_post(&results),
    })
}

/// What became of a claim handed to the broadcasters.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ClaimOutcome {
    /// The broadcaster that accepted it, if one did.
    pub broadcaster: Option<String>,
    /// The tracker's word for the claim.
    pub word: String,
    /// The tracker's pending re-ask, if any.
    pub reask: Option<String>,
    /// When the host acts on the claim next: the lock time, for a claim
    /// handed over before it.
    pub held_until: Option<u64>,
    /// What a broadcaster said when it called the claim not final.
    pub not_final: Option<String>,
}

/// Hand the claim to the broadcasters and to the tracker.
///
/// - Accepted: the broadcaster's acceptance is a hint and the word is
///   `announced`. Before the lock time no proof can exist, so nothing is
///   asked until then.
/// - Called not final, with `force`, before the lock time: not a failure.
///   The refusal is a hint; the claim is kept with its BEEF and announced
///   again at the lock time by the tracker's pass (`serve`'s loop, or
///   `tracker-tick`).
/// - Any other refusal: an error that says what they said.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn announce_claim<V: WalletServices, C: Clock>(
    storage: &StorageSqlx,
    services: &V,
    clock: &C,
    opts: &TickOptions,
    deposit_raw: &[u8],
    claim_raw: &[u8],
    lock_until: u64,
    force: bool,
) -> Result<ClaimOutcome> {
    let post = broadcast_claim(services, deposit_raw, claim_raw).await?;
    let now = clock.now();
    let early = now < lock_until;
    let held_until = early.then_some(lock_until);
    match post.answer {
        PostAnswer::Accepted { provider } => {
            let hint = Hint::new(format!("{provider}|accepted"), HintStatus::Accepted, now);
            let (word, reask) = tracker_host::heard(
                storage,
                clock,
                opts,
                &post.claim_txid,
                hint,
                held_until,
                None,
            )
            .await?;
            Ok(ClaimOutcome {
                broadcaster: Some(provider),
                word,
                reask,
                held_until,
                not_final: None,
            })
        }
        PostAnswer::NotFinal { provider, said } if force && early => {
            let hint = Hint::new(
                format!("{provider}|rejected"),
                HintStatus::Rejected {
                    reason: said.clone(),
                },
                now,
            );
            let again = Reannounce {
                beef: post.beef,
                at: lock_until,
            };
            let (word, reask) = tracker_host::heard(
                storage,
                clock,
                opts,
                &post.claim_txid,
                hint,
                held_until,
                Some(again),
            )
            .await?;
            Ok(ClaimOutcome {
                broadcaster: None,
                word,
                reask,
                held_until,
                not_final: Some(format!("{provider}: {said}")),
            })
        }
        PostAnswer::NotFinal { provider, said } => Err(anyhow!(
            "broadcast rejected by every broadcaster ({provider}: {said})"
        )),
        PostAnswer::Refused { said } => {
            Err(anyhow!("broadcast rejected by every broadcaster ({said})"))
        }
    }
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

        let post = broadcast_claim(&services, &deposit, &claim).await.unwrap();
        assert_eq!(
            post.answer,
            PostAnswer::Accepted {
                provider: "MockProvider".to_string()
            }
        );
        assert_eq!(post.claim_txid, claim_txid);
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
        let post = broadcast_claim(&services, &deposit, &claim).await.unwrap();
        assert!(matches!(post.answer, PostAnswer::Accepted { .. }));
        assert_eq!(services.call_count("get_merkle_path"), 1);
        assert_eq!(services.call_count("post_beef"), 1);
    }

    struct At(u64);
    impl Clock for At {
        fn now(&self) -> u64 {
            self.0
        }
    }

    async fn storage() -> StorageSqlx {
        use bsv_wallet_toolbox::WalletStorageWriter;
        let storage = StorageSqlx::in_memory().await.unwrap();
        storage
            .migrate("gift-tests", &("02".to_string() + &"ab".repeat(32)))
            .await
            .unwrap();
        storage.make_available().await.unwrap();
        storage
    }

    fn opts() -> TickOptions {
        TickOptions {
            age_threshold: 600,
            max_asks: 20,
        }
    }

    /// E2 (the rulings of 2026-10-09): a forced claim a broadcaster calls
    /// not final is not a failed claim. The refusal is a hint; the tracker
    /// holds the claim and the host announces it again at the lock time.
    /// Red at the base: `broadcast rejected by every broadcaster (Arcade:
    /// 476 non-final)`, and nothing kept.
    #[tokio::test]
    async fn a_forced_claim_called_not_final_is_tracked_and_re_asked_at_the_lock_time() {
        let (deposit, claim) = deposit_and_claim();
        let claim_txid = txid_of(&claim);
        let lock = 2_000_000_000u64;
        let not_final = || {
            MockWalletServices::builder()
                .post_beef_response(MockResponse::Success(vec![error_post_beef_result(
                    "Arcade",
                    "476 non-final",
                )]))
                .build()
        };
        let storage = storage().await;
        let services = not_final();
        let outcome = announce_claim(
            &storage,
            &services,
            &At(lock - 3600),
            &opts(),
            &deposit,
            &claim,
            lock,
            true,
        )
        .await
        .unwrap();
        assert_eq!(
            outcome,
            ClaimOutcome {
                broadcaster: None,
                word: "built".to_string(),
                reask: Some("hint:Arcade|rejected".to_string()),
                held_until: Some(lock),
                not_final: Some("Arcade: 476 non-final".to_string()),
            }
        );
        // The tracker holds it, in the wallet's own storage.
        assert_eq!(
            tracker_host::stored_word(&storage, &claim_txid)
                .await
                .unwrap()
                .map(|w| w.0),
            Some("built".to_string())
        );
        // The host announces it again at the lock time, not before.
        let early = tracker_host::tick(&storage, &services, &At(lock - 1), &opts())
            .await
            .unwrap();
        assert!(early.not_final.is_empty(), "{early:?}");
        assert_eq!(services.call_count("post_beef"), 1);
        let at_lock = tracker_host::tick(&storage, &services, &At(lock), &opts())
            .await
            .unwrap();
        assert_eq!(at_lock.not_final, vec![claim_txid.clone()]);
        assert_eq!(services.call_count("post_beef"), 2);

        // Without --force, and past the lock time, "not final" is a refusal.
        for (now, force) in [(lock - 3600, false), (lock + 1, true)] {
            let storage = self::storage().await;
            let err = announce_claim(
                &storage,
                &not_final(),
                &At(now),
                &opts(),
                &deposit,
                &claim,
                lock,
                force,
            )
            .await
            .unwrap_err();
            assert!(err.to_string().contains("Arcade: 476 non-final"), "{err}");
            assert_eq!(
                tracker_host::stored_word(&storage, &claim_txid)
                    .await
                    .unwrap(),
                None
            );
        }
    }

    /// A forced claim a broadcaster accepts is `announced`, the tracker's
    /// word, and nothing is asked about it before its lock time.
    #[tokio::test]
    async fn an_accepted_claim_is_announced_and_held_until_its_lock_time() {
        let (deposit, claim) = deposit_and_claim();
        let claim_txid = txid_of(&claim);
        let lock = 2_000_000_000u64;
        let storage = storage().await;
        let services = MockWalletServices::builder()
            .get_merkle_path_response(MockResponse::Success(GetMerklePathResult {
                name: Some("Services".to_string()),
                merkle_path: None,
                header: None,
                error: None,
                notes: vec![],
            }))
            .build();
        let outcome = announce_claim(
            &storage,
            &services,
            &At(lock - 7200),
            &opts(),
            &deposit,
            &claim,
            lock,
            true,
        )
        .await
        .unwrap();
        assert_eq!(outcome.broadcaster.as_deref(), Some("MockProvider"));
        assert_eq!(outcome.word, "announced");
        assert_eq!(outcome.held_until, Some(lock));
        // One call for the deposit's proof at the post; none for the claim
        // an hour later, well past the age threshold.
        assert_eq!(services.call_count("get_merkle_path"), 1);
        let report = tracker_host::tick(&storage, &services, &At(lock - 3600), &opts())
            .await
            .unwrap();
        assert_eq!((report.tracked, report.asked), (1, 0), "{report:?}");
        let report = tracker_host::tick(&storage, &services, &At(lock), &opts())
            .await
            .unwrap();
        assert_eq!(report.asked, 1);
        assert_eq!(report.no_proof, vec![claim_txid]);
    }

    /// Every broadcaster refusing is an error that says what they said.
    #[tokio::test]
    async fn a_refused_claim_is_an_error_naming_the_refusal() {
        let (deposit, claim) = deposit_and_claim();
        let services = MockWalletServices::builder()
            .post_beef_response(MockResponse::Success(vec![error_post_beef_result(
                "Arcade",
                "fee too low",
            )]))
            .build();
        let storage = storage().await;
        let err = announce_claim(
            &storage,
            &services,
            &At(2_000_000_000),
            &opts(),
            &deposit,
            &claim,
            1_900_000_000,
            true,
        )
        .await
        .unwrap_err();
        assert!(err.to_string().contains("Arcade: fee too low"), "{err}");
    }
}
