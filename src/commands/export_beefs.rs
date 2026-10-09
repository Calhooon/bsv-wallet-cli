//! `export-beefs`: one BEEF file per transaction that holds an unspent
//! output of ours, built from the wallet's own storage.
//!
//! Rule 28 (C10, a deletion): the wallet already holds each of these
//! transactions and the proofs that carry them (it builds every outgoing
//! BEEF from the same rows), so nothing is asked of an explorer. A
//! transaction the store cannot carry back to a proof is counted as failed
//! and named; it is never fetched from a third party.

use anyhow::{anyhow, Context, Result};
use bsv_sdk::transaction::{Beef, Transaction};
use bsv_sdk::wallet::{ListOutputsArgs, OutputInclude, WalletInterface};
use std::collections::{BTreeSet, HashSet};
use std::path::{Path, PathBuf};

use crate::context::WalletContext;

/// What one export did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Exported {
    /// Transactions holding an unspent output of ours.
    pub total: usize,
    pub written: u32,
    /// Already on disk.
    pub skipped: u32,
    /// The store could not build the BEEF, or the file could not be written.
    pub failed: u32,
}

pub async fn run(ctx: &WalletContext, to_dir: PathBuf) -> Result<()> {
    let done = export(&ctx.wallet, &to_dir).await?;

    if ctx.json_output {
        println!(
            "{}",
            serde_json::json!({
                "to": to_dir.display().to_string(),
                "total_unspent_txs": done.total,
                "written": done.written,
                "skipped": done.skipped,
                "failed": done.failed,
            })
        );
    } else {
        println!(
            "Exported BEEFs to {}: {} written, {} already present, {} failed (of {} unspent txs)",
            to_dir.display(),
            done.written,
            done.skipped,
            done.failed,
            done.total
        );
    }

    Ok(())
}

/// Write `<txid>.beef.hex` under `to_dir` for every transaction holding an
/// unspent output of `wallet`, each BEEF cut from what storage holds.
pub(crate) async fn export<W: WalletInterface>(wallet: &W, to_dir: &Path) -> Result<Exported> {
    std::fs::create_dir_all(to_dir)?;

    let (txids, stored) = unspent_txids_with_their_beef(wallet).await?;

    let mut done = Exported {
        total: txids.len(),
        written: 0,
        skipped: 0,
        failed: 0,
    };

    for txid in &txids {
        let path = to_dir.join(format!("{}.beef.hex", txid));
        if path.exists() {
            done.skipped += 1;
            continue;
        }
        match beef_of(&stored, txid) {
            Ok(beef) => {
                if let Err(e) = std::fs::write(&path, hex::encode(beef)) {
                    eprintln!("write failed for {}: {}", txid, e);
                    done.failed += 1;
                } else {
                    done.written += 1;
                }
            }
            Err(e) => {
                eprintln!("no BEEF from storage for {}: {}", txid, e);
                done.failed += 1;
            }
        }
    }

    Ok(done)
}

/// The BEEF of `txid` alone, cut from `stored`: the transaction, and each
/// unproven ancestor back to the transactions `stored` carries a proof for.
fn beef_of(stored: &Beef, txid: &str) -> Result<Vec<u8>> {
    let mut beef = Beef::new();
    let mut seen: HashSet<String> = HashSet::new();
    let mut pending = vec![txid.to_string()];
    while let Some(next) = pending.pop() {
        if !seen.insert(next.clone()) {
            continue;
        }
        let held = stored
            .find_txid(&next)
            .ok_or_else(|| anyhow!("the wallet's storage does not hold transaction {next}"))?;
        let raw = match (held.raw_tx(), held.tx()) {
            (Some(raw), _) => raw.to_vec(),
            (None, Some(tx)) => tx.to_binary(),
            (None, None) => {
                return Err(anyhow!(
                    "the wallet's storage holds only the txid of {next}"
                ))
            }
        };
        match held.bump_index().and_then(|i| stored.bumps.get(i)) {
            Some(bump) => {
                let index = beef.merge_bump(bump.clone());
                beef.merge_raw_tx(raw, Some(index));
            }
            None => {
                let tx = Transaction::from_binary(&raw)
                    .with_context(|| format!("stored transaction {next} does not parse"))?;
                for input in &tx.inputs {
                    if let Some(source) = &input.source_txid {
                        pending.push(source.clone());
                    }
                }
                beef.merge_raw_tx(raw, None);
            }
        }
    }
    if !beef.is_valid(false) {
        return Err(anyhow!(
            "the stored transactions do not chain back to stored proofs"
        ));
    }
    Ok(beef.to_binary())
}

/// Every txid holding an unspent output in the default basket, and one BEEF
/// holding all of them, as storage builds it (`include: entire
/// transactions`): the same walk over the stored transactions and proofs
/// that builds an outgoing BEEF.
async fn unspent_txids_with_their_beef<W: WalletInterface>(
    wallet: &W,
) -> Result<(BTreeSet<String>, Beef)> {
    let mut txids = BTreeSet::new();
    let mut stored = Beef::new();
    let mut offset: i32 = 0;
    let limit: u32 = 1000;
    loop {
        let res = wallet
            .list_outputs(
                ListOutputsArgs {
                    basket: "default".to_string(),
                    tags: None,
                    tag_query_mode: None,
                    include: Some(OutputInclude::EntireTransactions),
                    include_custom_instructions: None,
                    include_tags: None,
                    include_labels: None,
                    limit: Some(limit),
                    offset: Some(offset),
                    seek_permission: None,
                },
                "bsv-wallet-cli",
            )
            .await?;
        let n = res.outputs.len() as u32;
        for o in &res.outputs {
            // Outpoint string format is "<txid>.<vout>"
            let s = o.outpoint.to_string();
            if let Some((txid, _)) = s.rsplit_once('.') {
                txids.insert(txid.to_string());
            }
        }
        if let Some(bytes) = &res.beef {
            let page = Beef::from_binary(bytes).context("the BEEF storage built does not parse")?;
            stored.merge_beef(&page);
        }
        if n < limit {
            break;
        }
        offset += n as i32;
    }
    Ok((txids, stored))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{p2pkh, raw_tx, txid_of};
    use bsv_sdk::primitives::PrivateKey;
    use bsv_sdk::transaction::{Beef, MerklePath};
    use bsv_wallet_toolbox::services::mock::MockWalletServices;
    use bsv_wallet_toolbox::{StorageSqlx, Wallet, WalletStorageWriter};

    /// A wallet over an in-memory store holding one unspent output whose
    /// transaction the store has proven (a stored merkle path). Returns the
    /// wallet and that transaction's txid.
    async fn wallet_with_a_proven_output() -> (Wallet<StorageSqlx, MockWalletServices>, String) {
        let key = PrivateKey::random();
        let identity = key.public_key().to_hex();
        let storage = StorageSqlx::in_memory().await.unwrap();
        storage
            .migrate("export-beefs-tests", &identity)
            .await
            .unwrap();
        storage.make_available().await.unwrap();
        let (user, _) = storage.find_or_insert_user(&identity).await.unwrap();
        let basket = storage
            .find_or_create_default_basket(user.user_id)
            .await
            .unwrap()
            .basket_id;

        let lock = p2pkh([0xdb; 20]);
        let raw = raw_tx(&[(&"11".repeat(32), 0)], &[(1000, lock.clone())]);
        let txid = txid_of(&raw);
        // A block of one transaction: the path is the txid, the root is the txid.
        let path = MerklePath::from_coinbase_txid(&txid, 900_000).to_binary();
        let now = chrono::Utc::now();

        let proven_tx_id: i64 = sqlx::query_scalar(
            "INSERT INTO proven_txs (txid, height, idx, block_hash, merkle_root, merkle_path, raw_tx, created_at, updated_at) \
             VALUES (?, 900000, 0, ?, ?, ?, ?, ?, ?) RETURNING proven_tx_id",
        )
        .bind(&txid)
        .bind("bb".repeat(32))
        .bind(&txid)
        .bind(&path)
        .bind(&raw)
        .bind(now)
        .bind(now)
        .fetch_one(storage.pool())
        .await
        .unwrap();
        let tx_row = sqlx::query(
            "INSERT INTO transactions (user_id, status, reference, is_outgoing, satoshis, version, lock_time, description, txid, raw_tx, proven_tx_id, created_at, updated_at) \
             VALUES (?, 'completed', 'ref-export', 0, 1000, 1, 0, 'd', ?, ?, ?, ?, ?)",
        )
        .bind(user.user_id)
        .bind(&txid)
        .bind(&raw)
        .bind(proven_tx_id)
        .bind(now)
        .bind(now)
        .execute(storage.pool())
        .await
        .unwrap()
        .last_insert_rowid();
        sqlx::query(
            "INSERT INTO outputs (user_id, transaction_id, basket_id, vout, satoshis, locking_script, txid, type, spendable, change, provided_by, purpose, output_description, created_at, updated_at) \
             VALUES (?, ?, ?, 0, 1000, ?, ?, 'P2PKH', 1, 1, 'storage', 'change', 'c', ?, ?)",
        )
        .bind(user.user_id)
        .bind(tx_row)
        .bind(basket)
        .bind(&lock)
        .bind(&txid)
        .bind(now)
        .bind(now)
        .execute(storage.pool())
        .await
        .unwrap();

        let wallet = Wallet::new(Some(key), storage, MockWalletServices::new())
            .await
            .unwrap();
        (wallet, txid)
    }

    /// C10 (Rule 28): the BEEF of each of our own outputs is built from the
    /// proofs and transactions the wallet's storage already holds. Red at the
    /// base, with a local fixture in the explorer's place: one request per
    /// transaction, and nothing written when the explorer was down.
    #[tokio::test]
    async fn export_beefs_writes_each_beef_from_the_wallets_own_storage() {
        let (wallet, txid) = wallet_with_a_proven_output().await;
        let dir = tempfile::TempDir::new().unwrap();

        let done = export(&wallet, dir.path()).await.unwrap();

        assert_eq!(
            done,
            Exported {
                total: 1,
                written: 1,
                skipped: 0,
                failed: 0
            }
        );
        let hex = std::fs::read_to_string(dir.path().join(format!("{txid}.beef.hex"))).unwrap();
        let mut beef = Beef::from_hex(hex.trim()).unwrap();
        assert!(
            beef.find_txid(&txid).unwrap().has_proof(),
            "the stored proof"
        );
        assert!(beef.is_valid(false));

        // A second run finds the file and writes nothing.
        let again = export(&wallet, dir.path()).await.unwrap();
        assert_eq!((again.written, again.skipped), (0, 1));
    }

    /// C10: the command names no explorer and holds no HTTP client. Red at
    /// the base: `reqwest::Client` and the WhatsOnChain base were here.
    #[test]
    fn export_beefs_names_no_explorer_and_no_http_client() {
        let source = include_str!("export_beefs.rs");
        let code = source.split("#[cfg(test)]").next().unwrap();
        for word in ["reqwest", "woc_base", "whatsonchain", "bitails", "http"] {
            assert!(
                !code.to_ascii_lowercase().contains(word),
                "export-beefs names `{word}`"
            );
        }
    }

    /// An unproven transaction's BEEF carries its ancestors back to the
    /// stored proof, and nothing beside them.
    #[test]
    fn an_unproven_transaction_carries_its_ancestors_back_to_the_stored_proof() {
        let lock = p2pkh([0xdb; 20]);
        let parent = raw_tx(&[(&"11".repeat(32), 0)], &[(1000, lock.clone())]);
        let parent_id = txid_of(&parent);
        let child = raw_tx(&[(&parent_id, 0)], &[(900, lock.clone())]);
        let child_id = txid_of(&child);
        let stranger = raw_tx(&[(&"22".repeat(32), 0)], &[(5, lock)]);
        let stranger_id = txid_of(&stranger);

        let mut stored = Beef::new();
        let bump = stored.merge_bump(MerklePath::from_coinbase_txid(&parent_id, 900_000));
        stored.merge_raw_tx(parent, Some(bump));
        stored.merge_raw_tx(child, None);
        let other = stored.merge_bump(MerklePath::from_coinbase_txid(&stranger_id, 900_001));
        stored.merge_raw_tx(stranger, Some(other));

        let mut beef = Beef::from_binary(&beef_of(&stored, &child_id).unwrap()).unwrap();
        assert!(beef.is_valid(false));
        assert!(beef.find_txid(&parent_id).unwrap().has_proof());
        assert!(!beef.find_txid(&child_id).unwrap().has_proof());
        assert!(beef.find_txid(&stranger_id).is_none());
    }

    /// A transaction storage cannot carry back to a proof is an error
    /// naming what is missing, never a reason to ask a third party.
    #[test]
    fn a_transaction_storage_does_not_hold_is_named_not_fetched() {
        let err = beef_of(&Beef::new(), &"ab".repeat(32)).unwrap_err();
        assert!(
            err.to_string().contains("does not hold transaction"),
            "{err}"
        );
    }
}
