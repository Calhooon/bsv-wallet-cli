use anyhow::{anyhow, Result};
use bsv_sdk::primitives::PrivateKey;
use bsv_wallet_toolbox::{Chain, StorageSqlx, WalletStorageWriter};

use crate::brc29;

/// The `.env` lines `init` writes after the key: the trust section (#331),
/// commented, so the file names the knob and its shape. Unset, discovery
/// answers the certificates of the chain's default certifiers.
const TRUST_SETTINGS_NOTE: &str = "\
# TRUST_SETTINGS: discovery's trusted certifiers and threshold, the reference's
# trust settings JSON (#331). Unset: the chain's default certifiers. A
# certifier's description is served cut to 50 bytes, under 5 bytes as
# \"Trusted certifier\", and a missing iconUrl as the reference's favicon.
# TRUST_SETTINGS='{\"trustLevel\":2,\"trustedCertifiers\":[{\"name\":\"...\",\"description\":\"...\",\"identityKey\":\"02...\",\"trust\":3,\"iconUrl\":\"https://...\"}]}'
";

pub async fn run(
    db_path: &str,
    key: Option<&str>,
    force: bool,
    config: &crate::config::Config,
) -> Result<()> {
    if std::path::Path::new(".env").exists() && !force {
        return Err(anyhow!(
            ".env already exists in this directory. Refusing to overwrite — \
             running init here would destroy the existing wallet's ROOT_KEY. \
             If you really want to wipe it, pass --force (and back up .env first)."
        ));
    }

    let private_key = if let Some(hex) = key {
        PrivateKey::from_hex(hex)?
    } else {
        PrivateKey::random()
    };

    let identity_key = private_key.public_key().to_hex();
    let address = brc29::deposit_address(&private_key, Chain::Main)?;

    let storage = StorageSqlx::open_with_options(db_path, config.wallet.storage_options()).await?;
    storage.migrate("bsv-wallet-cli", &identity_key).await?;
    storage.make_available().await?;

    // The wallet's other settings are the environment's (`bsv-wallet
    // config` lists them); discovery's trust section is named here because
    // unset is a choice, the chain's default certifiers (#331).
    let env_content = format!("ROOT_KEY={}\n{TRUST_SETTINGS_NOTE}", private_key.to_hex());
    std::fs::write(".env", &env_content)?;

    println!("Wallet initialized!");
    println!("Identity key: {}", identity_key);
    println!("Address: {}", address);
    println!("Database: {}", db_path);
    println!("Root key saved to .env");

    Ok(())
}
