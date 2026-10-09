use anyhow::Result;
use bsv_sdk::primitives::{PrivateKey, PublicKey};
use bsv_sdk::wallet::{Counterparty, KeyDeriver, Protocol, SecurityLevel};
use bsv_wallet_toolbox::Chain;

pub const PROTOCOL: &str = "3241645161d8";
pub const DEFAULT_DERIVATION_PREFIX: &str = "SfKxPIJNgdI=";
pub const DEFAULT_DERIVATION_SUFFIX: &str = "NaGLC6fMH50=";

/// Derive the wallet's BRC-29 deposit keypair (the key behind `deposit_address`).
///
/// Funds paid to this key's P2PKH address are internalized as a payment to
/// the deposit key (`fund`, `receive`), and `gift-claim` records its own
/// claim the same way, so a time-locked gift locked to `deposit_pubkey`
/// becomes normal, spendable wallet balance once claimed. Returns `(private, public)`
/// where `public == private.public_key()` (asserted in tests).
pub fn deposit_keypair(root_key: &PrivateKey) -> Result<(PrivateKey, PublicKey)> {
    let deriver = KeyDeriver::new(Some(root_key.clone()));
    let (_, anyone_pubkey) = KeyDeriver::anyone_key();
    let protocol = Protocol::new(SecurityLevel::Counterparty, PROTOCOL);
    let key_id = format!(
        "{} {}",
        DEFAULT_DERIVATION_PREFIX, DEFAULT_DERIVATION_SUFFIX
    );
    let priv_key =
        deriver.derive_private_key(&protocol, &key_id, &Counterparty::Other(anyone_pubkey))?;
    let pub_key = priv_key.public_key();
    Ok((priv_key, pub_key))
}

pub fn deposit_address(root_key: &PrivateKey, chain: Chain) -> Result<String> {
    let deriver = KeyDeriver::new(Some(root_key.clone()));
    let (_, anyone_pubkey) = KeyDeriver::anyone_key();
    let protocol = Protocol::new(SecurityLevel::Counterparty, PROTOCOL);
    let key_id = format!(
        "{} {}",
        DEFAULT_DERIVATION_PREFIX, DEFAULT_DERIVATION_SUFFIX
    );
    let derived = deriver.derive_public_key(
        &protocol,
        &key_id,
        &Counterparty::Other(anyone_pubkey),
        true,
    )?;
    Ok(match chain {
        Chain::Test => derived.to_address_with_prefix(0x6f),
        Chain::Main => derived.to_address(),
    })
}

/// The locking script of the deposit address: P2PKH over the deposit key.
/// The same on either chain (the address prefix is not in the script).
pub fn deposit_script(root_key: &PrivateKey) -> Result<Vec<u8>> {
    let (_, deposit_pub) = deposit_keypair(root_key)?;
    let mut script = vec![0x76, 0xa9, 0x14];
    script.extend_from_slice(&deposit_pub.hash160());
    script.extend_from_slice(&[0x88, 0xac]);
    Ok(script)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The deposit keypair's public key MUST hash to the same address that
    /// `deposit_address` (and `sync`) use — otherwise a gift locked to the
    /// deposit pubkey would land somewhere the wallet doesn't watch.
    #[test]
    fn deposit_keypair_matches_deposit_address() {
        // Obviously-synthetic test key (0x01 x32) — NOT anyone's wallet. The
        // identity asserted below holds for any valid root key.
        let root = PrivateKey::from_hex(&"01".repeat(32)).unwrap();
        let (priv_key, pub_key) = deposit_keypair(&root).unwrap();
        assert_eq!(
            priv_key.public_key().to_compressed(),
            pub_key.to_compressed()
        );
        assert_eq!(
            pub_key.to_address(),
            deposit_address(&root, Chain::Main).unwrap()
        );
    }

    /// The deposit script is the script of the deposit address: the output
    /// `receive` matches in a BEEF is the one a payer to the address made.
    #[test]
    fn deposit_script_is_the_script_of_the_deposit_address() {
        let root = PrivateKey::from_hex(&"01".repeat(32)).unwrap();
        let from_address = bsv_sdk::script::templates::P2PKH::lock_from_address(
            &deposit_address(&root, Chain::Main).unwrap(),
        )
        .unwrap()
        .to_binary();
        assert_eq!(deposit_script(&root).unwrap(), from_address);
    }
}
