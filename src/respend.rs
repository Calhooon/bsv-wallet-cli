use anyhow::{bail, Result};
use bsv_sdk::primitives::PrivateKey;
use bsv_wallet_toolbox::{StorageSqlx, WalletServices};

pub async fn build_and_post(
    _storage: &StorageSqlx,
    _services: &dyn WalletServices,
    _root: &PrivateKey,
    _txid: &str,
) -> Result<String> {
    bail!("respend act absent at CLI 0.7.3")
}

pub async fn settle_proven(_storage: &StorageSqlx, _services: &dyn WalletServices) -> Result<()> {
    Ok(())
}
