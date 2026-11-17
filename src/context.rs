use anyhow::{Context, Result};
use bsv_sdk::primitives::PrivateKey;
use bsv_wallet_toolbox::{
    Chain, Services, StorageSqlx, Wallet, WalletServices, WalletStorageWriter,
};

use crate::cli::Cli;
use crate::services_env;

/// `S` is the services: the toolbox's `Services` in every command (the
/// default); a test hands a mock.
pub struct WalletContext<S: WalletServices + Send + Sync = Services> {
    pub wallet: Wallet<StorageSqlx, S>,
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
        // ARC_MODE, callback token) — see services_env.rs.
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

impl<S: WalletServices + Send + Sync + 'static> WalletContext<S> {
    /// Refuse `command` when the wallet has no chain tracker, before it
    /// fetches, builds, signs or broadcasts anything: from
    /// bsv-wallet-toolbox-rs 0.13.4 every `internalizeAction` of such a
    /// wallet is refused ([`services_env::require_chain_tracker_to_receive`]).
    /// The services' tracker is the one read: `load_with` sets the storage's
    /// tracker only from it, so with `CHAINTRACKS_URL=off` the wallet has
    /// neither, and with a header service it has both.
    pub async fn require_chain_tracker_to_receive(&self, command: &str) -> Result<()> {
        let has_tracker = self.wallet.services().get_chain_tracker().await.is_ok();
        services_env::require_chain_tracker_to_receive(command, has_tracker)
    }
}
