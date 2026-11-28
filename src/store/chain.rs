//! The store's chain word against the chain a command runs on (#639).

/// A store opened for one chain whose settings name another (#639): the
/// store's `chain` is the word the toolbox's `internalizeAction` judges a
/// received payment by, and `init` writes it once from `--testnet`, so a
/// wallet built on any other chain would answer one network and judge by
/// another.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChainMismatch {
    /// The database path.
    pub db: String,
    /// The store's settings' `chain` (`main`, `test`, `regtest`).
    pub stored: String,
    /// The chain the command runs on, as the toolbox writes it.
    pub asked: &'static str,
}

/// The one case (#682) where the advice of the plain refusal points the
/// wrong way: a store whose word is `main` opened on testnet. Every
/// release pinning the toolbox at 0.14.0-beta.3 or below wrote `mainnet`
/// (read as `main`) into a `--testnet` store, so running it without the
/// flag would run a testnet wallet against mainnet's services: the
/// refusal names the repair instead.
pub const LEGACY_TESTNET_ADVICE: &str = "a testnet wallet made before the chain word was \
     recorded reads so; if this wallet's coins are testnet's, do not run it without --testnet: \
     run `bsv-wallet --testnet repair-chain` to see the evidence and repair the word";

impl std::fmt::Display for ChainMismatch {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.stored == "main" && self.asked == "test" {
            return write!(
                f,
                "the wallet database {} was created for chain main (its settings' chain) \
                 and this command runs on chain test: {LEGACY_TESTNET_ADVICE}",
                self.db
            );
        }
        let how = match self.stored.as_str() {
            "test" => "with --testnet",
            "main" => "without --testnet",
            _ => "on the chain it was created on",
        };
        write!(
            f,
            "the wallet database {} was created for chain {} (its settings' chain) \
             and this command runs on chain {}: run it {how}",
            self.db, self.stored, self.asked
        )
    }
}

impl std::error::Error for ChainMismatch {}

/// Refuse `storage` (made available) when its settings' `chain` is not
/// `chain`'s word: every command that serves or runs a wallet asks this
/// at open, before anything is built on the store ([`ChainMismatch`]).
///
/// # Errors
///
/// [`ChainMismatch`], naming both words and the flag.
pub fn require_chain(
    storage: &bsv_wallet_toolbox::StorageSqlx,
    db: &str,
    chain: bsv_wallet_toolbox::Chain,
) -> std::result::Result<(), ChainMismatch> {
    use bsv_wallet_toolbox::WalletStorageReader;
    let stored = storage.get_settings().chain;
    if stored == chain.as_str() {
        Ok(())
    } else {
        Err(ChainMismatch {
            db: db.to_string(),
            stored,
            asked: chain.as_str(),
        })
    }
}

/// The same check for a command that opens a file `init` may not have
/// made (`init` over an existing file, the inspector `ui`, #682): a file
/// holding a wallet's settings row is made available and refused by
/// [`require_chain`] when its word is another chain's; a file with no
/// settings row has no chain word to ask. Writes nothing of the store's.
///
/// # Errors
///
/// [`ChainMismatch`], or the store's read fault.
pub async fn require_chain_if_made(
    storage: &bsv_wallet_toolbox::StorageSqlx,
    db: &str,
    chain: bsv_wallet_toolbox::Chain,
) -> anyhow::Result<()> {
    use bsv_wallet_toolbox::WalletStorageWriter;
    if !has_settings_row(storage.pool()).await? {
        return Ok(());
    }
    storage.make_available().await?;
    require_chain(storage, db, chain)?;
    Ok(())
}

/// The newest stored proofs, at most `limit`, as `(txid, height, merkle
/// root)`: the store's own evidence of its chain (`repair-chain`, #682).
/// Reads only.
///
/// # Errors
///
/// The reader's `sqlx::Error`.
pub async fn newest_proofs(
    reader: &sqlx::SqlitePool,
    limit: u32,
) -> anyhow::Result<Vec<(String, i64, String)>> {
    sqlx::query_as(
        "SELECT txid, height, merkle_root FROM proven_txs \
         ORDER BY height DESC, proven_tx_id DESC LIMIT ?",
    )
    .bind(i64::from(limit))
    .fetch_all(reader)
    .await
    .map_err(anyhow::Error::from)
}

/// Does the file hold a wallet's settings row, the row `init`'s `migrate`
/// writes? A file no `init` made has no chain word to ask (#682).
///
/// # Errors
///
/// The reader's `sqlx::Error`.
pub async fn has_settings_row(reader: &sqlx::SqlitePool) -> anyhow::Result<bool> {
    if !super::has_column(reader, "settings", "chain").await? {
        return Ok(false);
    }
    let n: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM settings")
        .fetch_one(reader)
        .await?;
    Ok(n > 0)
}

/// Write the store's chain word, `chain`'s, in one statement on the
/// writer (`repair-chain --execute`, #682). The rows written: one.
///
/// # Errors
///
/// The writer's fault, or a store with no settings row.
pub async fn write_chain_word(
    storage: &bsv_wallet_toolbox::StorageSqlx,
    chain: bsv_wallet_toolbox::Chain,
) -> anyhow::Result<u64> {
    let written = sqlx::query("UPDATE settings SET chain = ? WHERE settings_id = 1")
        .bind(chain.as_str())
        .execute(&mut *super::writer(storage).await?)
        .await?
        .rows_affected();
    if written != 1 {
        anyhow::bail!("the store has no settings row to write the chain word into");
    }
    Ok(written)
}
