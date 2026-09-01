use anyhow::{Context, Result};
use bsv_sdk::primitives::PrivateKey;
use bsv_wallet_toolbox::{Chain, Services, StorageSqlx, Wallet, WalletStorageWriter};

use crate::cli::Cli;
use crate::services_env;

pub struct WalletContext {
    pub wallet: Wallet<StorageSqlx, Services>,
    pub identity_key: String,
    pub root_key: PrivateKey,
    pub chain: Chain,
    pub json_output: bool,
    /// Wallet database path (also anchors the persisted callback token).
    pub db_path: String,
}

impl WalletContext {
    pub async fn load(cli: &Cli) -> Result<Self> {
        let root_key_hex =
            std::env::var("ROOT_KEY").context("ROOT_KEY not set. Run `bsv-wallet init` first.")?;
        Self::load_with(&cli.db, &root_key_hex, cli.testnet, cli.json).await
    }

    /// Load a context with an EXPLICIT db + root key — the multi-tenant path
    /// (`serve-fleet`): one process holds many wallets, so per-wallet secrets
    /// cannot ride process env. `load()` above stays the single-wallet door.
    pub async fn load_with(
        db: &str,
        root_key_hex: &str,
        testnet: bool,
        json: bool,
    ) -> Result<Self> {
        let root_key = PrivateKey::from_hex(root_key_hex)?;
        let identity_key = root_key.public_key().to_hex();

        let chain = if testnet { Chain::Test } else { Chain::Main };

        let storage = StorageSqlx::open(db).await?;
        storage.make_available().await?;

        // Shared env-driven services config (CHAINTRACKS_URL, ARC_URL,
        // ARC_MODE=arcade, TAAL keys, callback token) — see services_env.rs.
        let services = {
            let opts = services_env::services_options_from_env(chain, db)?;
            Services::with_options(chain, opts)?
        };

        // Wire ChainTracker into storage for merkle proof validation (Layer 1)
        if let Some(ref ct) = services.chaintracks {
            storage.set_chain_tracker(ct.clone()).await;
        }

        let wallet = Wallet::new(Some(root_key.clone()), storage, services).await?;

        Ok(Self {
            wallet,
            identity_key,
            root_key,
            chain,
            json_output: json,
            db_path: db.to_string(),
        })
    }
}
