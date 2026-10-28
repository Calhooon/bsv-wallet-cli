//! CLI wallet calls retain their existing originator names at the beta.2
//! pin: `Some("bsv-wallet-cli")`, or the existing command/engine-specific
//! name. A request's required header supplies the served door's `Some`.
//! No local caller changes an empty name into `None` (#329, #318, #323).

use std::sync::Arc;

use anyhow::Result;
use bsv_sdk::primitives::PrivateKey;
use bsv_wallet_toolbox::{Chain, Services, StorageSqlx, Wallet, WalletStorageWriter};

use crate::config::Config;
use crate::services_env;

pub struct WalletContext {
    pub wallet: Wallet<StorageSqlx, Services>,
    pub identity_key: String,
    pub root_key: PrivateKey,
    pub chain: Chain,
    pub json_output: bool,
    /// Wallet database path (also anchors the persisted callback token).
    pub db_path: String,
    /// The configuration read at start (`crate::config`).
    pub config: Arc<Config>,
}

impl WalletContext {
    /// The wallet the configuration names: `--db`, `--testnet`, `ROOT_KEY`.
    pub async fn load(config: &Arc<Config>, json: bool) -> Result<Self> {
        let root_key_hex = config.wallet.root_key()?;
        let testnet = config.chain() == Chain::Test;
        Self::load_with(&config.wallet.db, root_key_hex, testnet, json, config).await
    }

    /// Load a context with an EXPLICIT db + root key — the multi-tenant path
    /// (`serve-fleet`): one process holds many wallets, so per-wallet secrets
    /// cannot ride process env. `load()` above stays the single-wallet door.
    pub async fn load_with(
        db: &str,
        root_key_hex: &str,
        testnet: bool,
        json: bool,
        config: &Arc<Config>,
    ) -> Result<Self> {
        let root_key = PrivateKey::from_hex(root_key_hex)?;
        let identity_key = root_key.public_key().to_hex();

        let chain = if testnet { Chain::Test } else { Chain::Main };
        let (storage, services) = open_store(config, chain, db).await?;
        let wallet = Wallet::with_options(
            Some(root_key.clone()),
            storage,
            services,
            config.wallet.options(),
        )
        .await?;

        Ok(Self {
            wallet,
            identity_key,
            root_key,
            chain,
            json_output: json,
            db_path: db.to_string(),
            config: config.clone(),
        })
    }

    /// The spend gate's settings, from the configuration.
    pub fn gate(&self) -> crate::spend_gate::GateOptions {
        crate::spend_gate::GateOptions::from_config(&self.config)
    }

    /// The post-broadcast verifier, from the configuration.
    pub fn verifier(&self) -> crate::broadcast_verify::BroadcastVerifier {
        crate::broadcast_verify::BroadcastVerifier::new(self.chain, &self.config)
    }
}

/// The store at `db`, migrated, and the configured services for `chain`,
/// the services' chain tracker wired into the store for merkle proof
/// validation (Layer 1).
pub async fn open_store(
    config: &Config,
    chain: Chain,
    db: &str,
) -> Result<(StorageSqlx, Services)> {
    let storage = StorageSqlx::open_with_options(db, config.wallet.storage_options()).await?;
    storage.make_available().await?;

    // Shared services config (CHAINTRACKS_URL, ARC_URL, ARC_MODE,
    // callback token) — see services_env.rs.
    let opts = services_env::services_options(&config.services, chain, db)?;
    let services = Services::with_options(chain, opts)?;
    if let Some(ref ct) = services.chaintracks {
        storage.set_chain_tracker(ct.clone()).await;
    }
    Ok((storage, services))
}
