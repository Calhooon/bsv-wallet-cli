use anyhow::Result;
use bsv_sdk::primitives::PrivateKey;
use bsv_sdk::wallet::{Counterparty, KeyDeriver, Protocol, SecurityLevel};
use bsv_wallet_toolbox::Chain;

pub const PROTOCOL: &str = "3241645161d8";
pub const DEFAULT_DERIVATION_PREFIX: &str = "SfKxPIJNgdI=";
pub const DEFAULT_DERIVATION_SUFFIX: &str = "NaGLC6fMH50=";

pub fn deposit_address(root_key: &PrivateKey, chain: Chain) -> Result<String> {
    let deriver = KeyDeriver::new(Some(root_key.clone()));
    let (_, anyone_pubkey) = KeyDeriver::anyone_key();
    let protocol = Protocol::new(SecurityLevel::Counterparty, PROTOCOL);
    let key_id = format!("{} {}", DEFAULT_DERIVATION_PREFIX, DEFAULT_DERIVATION_SUFFIX);
    let derived = deriver.derive_public_key(&protocol, &key_id, &Counterparty::Other(anyone_pubkey), true)?;
    Ok(match chain {
        Chain::Test => derived.to_address_with_prefix(0x6f),
        Chain::Main => derived.to_address(),
    })
}
