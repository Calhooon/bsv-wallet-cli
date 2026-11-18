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

pub async fn run<S: bsv_wallet_toolbox::WalletServices + Send + Sync + 'static>(
    ctx: &WalletContext<S>,
    count: u32,
    allow_pending: crate::spend_gate::AllowPending,
) -> Result<()> {
    // The split's outputs are recorded by internalizeAction after the
    // broadcast; with no chain tracker every internalizeAction is refused
    // (toolbox 0.13.4), so a split would be posted and its outputs never
    // recorded. Refuse by name before any coin is selected or posted.
    ctx.require_chain_tracker_to_receive("split").await?;
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

    // The coins split does not spend (bsv-wallet-toolbox-rs 0.13.7): a coin
    // whose creator the wallet's own proof record places below the chain's
    // Genesis height. The chain is the wallet's own (`Wallet::chain`), the
    // one its `internalizeAction` passes to the door, never `ctx.chain`: a
    // wallet built with `Wallet::new` carries the toolbox's default chain
    // whatever `--testnet` says, and the hold and the door must agree. The
    // outputs are sized from the other coins; when those cannot fund a
    // split, it is sized as before and the gate refuses it by name, the
    // coin listed under `held` (nothing signed, nothing posted).
    let before_genesis =
        crate::spend_gate::pre_genesis_coins(ctx.wallet.storage(), ctx.wallet.chain()).await?;
    let all_sats: u64 = list.outputs.iter().map(|o| o.satoshis).sum();
    let all_count = list.outputs.len();
    let (left_sats, left_count) = list
        .outputs
        .iter()
        .filter(|o| {
            let outpoint = o.outpoint.to_string();
            !before_genesis.iter().any(|held| held.outpoint == outpoint)
        })
        .fold((0u64, 0usize), |(s, n), o| (s + o.satoshis, n + 1));
    let (total_sats, utxo_count) =
        if left_count < all_count && sized(left_sats, left_count, count).is_none() {
            (all_sats, all_count)
        } else {
            (left_sats, left_count)
        };

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
        crate::spend_gate::create_action_for_split(
            &ctx.wallet,
            args,
            "bsv-wallet-cli",
            &crate::spend_gate::GateOptions::from_env().with_allow_pending(allow_pending),
            &before_genesis,
        )
        .await,
        ctx.json_output,
    )?;

    let txid = result.txid.expect("Expected txid from split transaction");
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
        let mut answer = split_json(&txid_hex, count, per_output, self_internalized);
        if !before_genesis.is_empty() {
            answer["notSpent"] = serde_json::Value::Array(
                before_genesis
                    .iter()
                    .map(|held| {
                        serde_json::json!({
                            "outpoint": held.outpoint,
                            "satoshis": held.satoshis,
                            "reason": held.reason,
                        })
                    })
                    .collect(),
            );
        }
        println!("{answer}");
    } else {
        println!("Split into {} UTXOs ({} sats each)", count, per_output);
        println!("TxID: {}", txid_hex);
        println!("View: https://whatsonchain.com/tx/{}", txid_hex);
        if !self_internalized {
            println!("Warning: the split outputs were not self-internalized (see the log)");
        }
        for held in &before_genesis {
            println!(
                "Not spent: {} {} sats: {}",
                held.outpoint, held.satoshis, held.reason
            );
        }
    }

    Ok(())
}

/// The fee reserve and the satoshis per output of a split of `count`
/// outputs from `total` satoshis in `coins` coins, or `None` when they cannot
/// fund one (the checks `run` refuses by).
fn sized(total: u64, coins: usize, count: u32) -> Option<(u64, u64)> {
    let reserve = ((coins as u64 * 148) + (count as u64 * 34) + 10).max(500);
    if coins == 0 || total <= reserve {
        return None;
    }
    let per_output = (total - reserve) / count as u64;
    (per_output >= 1).then_some((reserve, per_output))
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
            "bsv-wallet-cli",
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

    // ---- the coin mined before Genesis (bsv-wallet-toolbox-rs 0.13.7) ----

    use crate::context::WalletContext;
    use crate::spend_gate::{GateError, GateOptions, PRE_GENESIS_PREFIX, PRE_GENESIS_WHY};
    use bsv_wallet_toolbox::services::BlockHeader;
    use bsv_wallet_toolbox::{Chain, MonitorStorage};

    /// A header at `height` whose merkle root is `root` (a one-transaction
    /// block), its hash computed when `hashed`.
    fn header(height: u32, root: &str, hashed: bool) -> BlockHeader {
        let mut h = BlockHeader {
            version: 1,
            previous_hash: "00".repeat(32),
            merkle_root: root.to_string(),
            time: 1_700_000_000,
            bits: 0x1d00_ffff,
            nonce: 0,
            hash: String::new(),
            height,
        };
        if hashed {
            let mut hash = bsv_sdk::primitives::hash::sha256d(&h.to_binary()).to_vec();
            hash.reverse();
            h.hash = hex::encode(hash);
        }
        h
    }

    /// A mainnet wallet of `root` holding one coin per `(height, satoshis)`:
    /// a payment to its deposit script mined at `height`, its BUMP in the
    /// BEEF, taken in through the toolbox's own `internalizeAction` as
    /// `fund` takes one, and its proof then recorded by the toolbox
    /// (`ingest_merkle_proof`, the proof gate raised as the monitor raises
    /// it): the coin's transaction `completed`, its `proven_txs` row at
    /// `height`. The mock's header service carries each block and a tip ten
    /// blocks above the highest; the storage's chain tracker carries each
    /// root; the broadcaster is the toolbox's mock. The coins' txids.
    async fn wallet_with_mined(
        root: &PrivateKey,
        coins: &[(u32, u64)],
    ) -> (WalletContext<MockWalletServices>, Vec<String>) {
        wallet_with_mined_as(root, coins, Chain::Main).await
    }

    /// [`wallet_with_mined`], its context's `chain` set to `chain` as
    /// `WalletContext::load_with` sets it from `--testnet`; the wallet is
    /// built with `Wallet::new` as `load_with` builds it.
    async fn wallet_with_mined_as(
        root: &PrivateKey,
        coins: &[(u32, u64)],
        chain: Chain,
    ) -> (WalletContext<MockWalletServices>, Vec<String>) {
        let storage = StorageSqlx::in_memory().await.unwrap();
        storage
            .migrate("split", &root.public_key().to_hex())
            .await
            .unwrap();
        storage.make_available().await.unwrap();
        let top = coins.iter().map(|(h, _)| *h).max().unwrap_or(900_000);
        let mut tracker = crate::test_support::TestChainTracker::new(top + 10);
        let services = MockWalletServices::builder().build();
        let wallet = Wallet::new(Some(root.clone()), storage, services)
            .await
            .unwrap();
        wallet
            .storage()
            .set_max_acceptable_proof_height(top + 10)
            .await
            .unwrap();
        let script = crate::brc29::deposit_script(root).unwrap();
        let anyone = KeyDeriver::anyone_key().1.to_hex();
        let mut txids = Vec::new();
        for (i, (height, satoshis)) in coins.iter().enumerate() {
            let raw = crate::test_support::raw_tx(
                &[(&format!("{:02x}", 0x51 + i).repeat(32), 0)],
                &[(*satoshis, script.clone())],
            );
            let txid = crate::test_support::txid_of(&raw);
            tracker.add_root(*height, txid.clone());
            wallet
                .storage()
                .set_chain_tracker(std::sync::Arc::new(tracker.clone()))
                .await;
            let path = bsv_sdk::transaction::MerklePath::from_coinbase_txid(&txid, *height);
            let mut beef = Beef::new();
            let bump = beef.merge_bump(path.clone());
            beef.merge_raw_tx(raw, Some(bump));
            let accepted = wallet
                .internalize_action(
                    InternalizeActionArgs {
                        tx: beef.to_binary_atomic(&txid).unwrap(),
                        outputs: vec![InternalizeOutput {
                            output_index: 0,
                            protocol: "wallet payment".to_string(),
                            payment_remittance: Some(WalletPayment {
                                derivation_prefix: DEFAULT_DERIVATION_PREFIX.to_string(),
                                derivation_suffix: DEFAULT_DERIVATION_SUFFIX.to_string(),
                                sender_identity_key: anyone.clone(),
                            }),
                            insertion_remittance: None,
                        }],
                        description: "a mined payment".to_string(),
                        labels: None,
                        seek_permission: None,
                    },
                    "split.local",
                )
                .await
                .unwrap();
            assert!(accepted.accepted);
            let outcome = wallet
                .storage()
                .ingest_merkle_proof(
                    &txid,
                    &path.to_binary(),
                    *height,
                    &"bb".repeat(32),
                    Some(&txid),
                )
                .await
                .unwrap();
            assert!(
                format!("{outcome:?}").starts_with("Ingested"),
                "{outcome:?}"
            );
            wallet
                .services()
                .set_header_for_height(header(*height, &txid, false));
            txids.push(txid);
        }
        wallet
            .services()
            .set_tip_header(header(top + 10, &"0e".repeat(32), true));
        let ctx = WalletContext {
            wallet,
            identity_key: root.public_key().to_hex(),
            root_key: root.clone(),
            chain,
            json_output: false,
            db_path: ":memory:".into(),
        };
        (ctx, txids)
    }

    /// (actions, unspent satoshis of the default basket).
    async fn ledger(ctx: &WalletContext<MockWalletServices>) -> (i64, i64) {
        sqlx::query_as(
            "SELECT (SELECT COUNT(*) FROM transactions), \
                    (SELECT COALESCE(SUM(o.satoshis), 0) FROM outputs o \
                     JOIN output_baskets b ON o.basket_id = b.basket_id AND b.name = 'default' \
                     WHERE o.spendable = 1 AND o.spent_by IS NULL)",
        )
        .fetch_one(ctx.wallet.storage().pool())
        .await
        .unwrap()
    }

    /// The action that spent the coin `txid`.0, if any.
    async fn spender(ctx: &WalletContext<MockWalletServices>, txid: &str) -> Option<i64> {
        sqlx::query_scalar("SELECT spent_by FROM outputs WHERE txid = ? AND vout = 0")
            .bind(txid)
            .fetch_one(ctx.wallet.storage().pool())
            .await
            .unwrap()
    }

    /// The wallet's only coin is proven below Genesis (600,000): `split`
    /// refuses by name with the gate's own class (`INSUFFICIENT_FUNDS`), the
    /// coin listed under `held` with its reason and the message ending in
    /// the sentence on why and what works. The broadcaster saw no post, no
    /// action row is left, the balance is unmoved.
    #[tokio::test]
    async fn a_split_of_a_coin_mined_before_genesis_is_refused_by_name_with_nothing_posted() {
        let root = PrivateKey::random();
        let (ctx, txids) = wallet_with_mined(&root, &[(600_000, 50_000)]).await;
        let before = ledger(&ctx).await;
        assert_eq!(before.1, 50_000);

        let err = run(&ctx, 2, crate::spend_gate::AllowPending::Off)
            .await
            .expect_err("refused");
        let Some(GateError::Refused(refusal)) = err.downcast_ref::<GateError>() else {
            panic!("not the gate's refusal: {err}");
        };
        assert_eq!(refusal.code(), "INSUFFICIENT_FUNDS");
        assert_eq!(refusal.offered_satoshis, 0);
        assert_eq!(refusal.held.len(), 1, "{refusal}");
        assert_eq!(refusal.held[0].outpoint, format!("{}.0", txids[0]));
        assert_eq!(
            refusal.held[0].reason,
            format!(
                "{PRE_GENESIS_PREFIX}coin height 600000 is before Genesis height 620538; split does not offer it"
            )
        );
        let message = refusal.to_string();
        assert!(message.starts_with("Insufficient funds: "), "{message}");
        assert!(
            message.ends_with(&format!("\n{PRE_GENESIS_WHY}")),
            "{message}"
        );
        assert_eq!(
            refusal.to_json()["held"][0]["reason"],
            refusal.held[0].reason
        );

        assert_eq!(ctx.wallet.services().call_count("post_beef"), 0);
        assert_eq!(
            ledger(&ctx).await,
            before,
            "no action row, the balance unmoved"
        );
        assert_eq!(spender(&ctx, &txids[0]).await, None);
    }

    /// One coin one block below Genesis (620,537, the larger) and one at it
    /// (620,538): the split spends the coin at the Genesis height alone,
    /// its outputs are recorded through the door, and the old coin is left
    /// unspent.
    #[tokio::test]
    async fn a_split_spends_the_coin_at_genesis_and_leaves_the_one_below() {
        let root = PrivateKey::random();
        let (ctx, txids) = wallet_with_mined(&root, &[(620_537, 50_000), (620_538, 30_000)]).await;
        assert_eq!(
            crate::spend_gate::pre_genesis_coins(ctx.wallet.storage(), Chain::Main)
                .await
                .unwrap()
                .into_iter()
                .map(|held| (held.outpoint, held.satoshis))
                .collect::<Vec<_>>(),
            vec![(format!("{}.0", txids[0]), 50_000)]
        );

        run(&ctx, 2, crate::spend_gate::AllowPending::Off)
            .await
            .expect("the split spends the coin at Genesis");
        assert_eq!(ctx.wallet.services().call_count("post_beef"), 1);
        assert_eq!(spender(&ctx, &txids[0]).await, None, "the old coin left");
        assert!(
            spender(&ctx, &txids[1]).await.is_some(),
            "the coin at Genesis spent"
        );
        let recorded: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM outputs WHERE transaction_id = \
             (SELECT spent_by FROM outputs WHERE txid = ? AND vout = 0) \
             AND spendable = 1 AND satoshis = 14750",
        )
        .bind(&txids[1])
        .fetch_one(ctx.wallet.storage().pool())
        .await
        .unwrap();
        assert_eq!(
            recorded, 2,
            "the split's two outputs recorded through the door"
        );
    }

    /// A context built as `--testnet` builds it (`WalletContext::load_with`:
    /// `ctx.chain` is `Chain::Test`, the wallet `Wallet::new`'s), its one
    /// coin proven at 700,000: after mainnet's Genesis height (620,538),
    /// before testnet's (1,344,302). The door judges by the wallet's own
    /// chain, which `Wallet::new` makes the toolbox's default, mainnet, and
    /// takes the split; the hold asks the same chain and offers the coin.
    /// Asked `ctx.chain`, the hold would keep a coin the door takes.
    #[tokio::test]
    async fn a_testnet_context_holds_by_the_chain_its_door_judges_by() {
        let root = PrivateKey::random();
        let (ctx, txids) = wallet_with_mined_as(&root, &[(700_000, 30_000)], Chain::Test).await;
        assert_eq!(ctx.chain, Chain::Test);
        assert_eq!(ctx.wallet.chain(), Chain::Main, "the door's chain");
        assert_eq!(
            crate::spend_gate::pre_genesis_coins(ctx.wallet.storage(), ctx.wallet.chain())
                .await
                .unwrap(),
            vec![],
            "the hold offers what the door takes"
        );
        assert_eq!(
            crate::spend_gate::pre_genesis_coins(ctx.wallet.storage(), ctx.chain)
                .await
                .unwrap()
                .len(),
            1,
            "the flag's chain would hold it"
        );

        run(&ctx, 2, crate::spend_gate::AllowPending::Off)
            .await
            .expect("the split spends the coin at 700,000");
        assert_eq!(ctx.wallet.services().call_count("post_beef"), 1);
        assert!(spender(&ctx, &txids[0]).await.is_some(), "the coin spent");
        let recorded: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM outputs WHERE transaction_id = \
             (SELECT spent_by FROM outputs WHERE txid = ? AND vout = 0) \
             AND spendable = 1 AND satoshis = 14750",
        )
        .bind(&txids[0])
        .fetch_one(ctx.wallet.storage().pool())
        .await
        .unwrap();
        assert_eq!(
            recorded, 2,
            "the split's two outputs recorded through the door"
        );
    }

    /// A send of 1,000 sats to a stranger, signed; posted unless `no_send`.
    fn send_args(no_send: bool) -> CreateActionArgs {
        CreateActionArgs {
            description: "a send".to_string(),
            input_beef: None,
            inputs: None,
            outputs: Some(vec![CreateActionOutput {
                locking_script: crate::test_support::p2pkh([9; 20]),
                satoshis: 1_000,
                output_description: "to someone".to_string(),
                basket: None,
                custom_instructions: None,
                tags: None,
            }]),
            lock_time: None,
            version: None,
            labels: None,
            options: Some(CreateActionOptions {
                sign_and_process: Some(true),
                no_send: Some(no_send),
                ..Default::default()
            }),
        }
    }

    /// The hold is `split`'s alone: the gate every other command uses (a
    /// `send`'s, signed, not sent) offers the coin mined before Genesis and
    /// the spend signs.
    #[tokio::test]
    async fn a_send_of_the_coin_mined_before_genesis_is_not_held() {
        let root = PrivateKey::random();
        let (ctx, txids) = wallet_with_mined(&root, &[(600_000, 50_000)]).await;
        let made = crate::spend_gate::create_action(
            &ctx.wallet,
            send_args(true),
            "split.local",
            &GateOptions::default(),
        )
        .await
        .expect("a send is not held for this reason");
        assert!(made.tx.is_some(), "signed");
        assert!(spender(&ctx, &txids[0]).await.is_some());
        assert_eq!(ctx.wallet.services().call_count("post_beef"), 0, "not sent");
    }

    /// The change of a posted `send` of the coin mined before Genesis, not
    /// mined yet: a split of it would carry the send unproven, spending a
    /// coin the send's parent's proof record places at 600,000, so every
    /// change coin of the send is held with that reason and the split is
    /// refused with nothing more posted. (Without the hold this split is
    /// posted and its self-internalize refused.)
    #[tokio::test]
    async fn a_split_of_the_unmined_change_of_a_send_of_such_a_coin_is_refused_by_name() {
        let root = PrivateKey::random();
        let (ctx, _) = wallet_with_mined(&root, &[(600_000, 50_000)]).await;
        let sent = crate::spend_gate::create_action(
            &ctx.wallet,
            send_args(false),
            "split.local",
            &GateOptions::default(),
        )
        .await
        .expect("the send is posted");
        let send_txid = bsv_sdk::primitives::to_hex(&sent.txid.expect("signed"));
        assert_eq!(ctx.wallet.services().call_count("post_beef"), 1);
        let before = ledger(&ctx).await;

        let err = run(&ctx, 2, crate::spend_gate::AllowPending::Off)
            .await
            .expect_err("refused");
        let Some(GateError::Refused(refusal)) = err.downcast_ref::<GateError>() else {
            panic!("not the gate's refusal: {err}");
        };
        assert_eq!(refusal.code(), "INSUFFICIENT_FUNDS");
        let change: Vec<_> = refusal
            .held
            .iter()
            .filter(|c| c.outpoint.starts_with(&send_txid))
            .collect();
        assert!(!change.is_empty(), "{refusal}");
        for coin in &change {
            assert_eq!(
                coin.reason,
                format!(
                    "{PRE_GENESIS_PREFIX}{send_txid} is not mined yet and spends a coin of height 600000, before Genesis height 620538; split does not offer it"
                )
            );
        }
        assert!(refusal.to_string().ends_with(PRE_GENESIS_WHY));
        assert_eq!(
            ctx.wallet.services().call_count("post_beef"),
            1,
            "nothing more posted"
        );
        assert_eq!(ledger(&ctx).await, before);
    }
}
