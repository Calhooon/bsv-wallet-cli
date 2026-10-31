use anyhow::Result;
use bsv_wallet_toolbox::{Chain, Services, WalletServices};

use crate::config::Config;
use crate::services_env;

pub async fn run(config: &Config, json: bool) -> Result<()> {
    let chain = config.chain();

    // Shared services config — see services_env.rs.
    let services = {
        let opts = services_env::services_options(&config.services, chain, &config.wallet.db)?;
        Services::with_options(chain, opts)?
    };

    let chain_name = match chain {
        Chain::Main => "mainnet",
        Chain::Test => "testnet",
        Chain::Regtest => "regtest",
    };

    match services.get_height().await {
        Ok(height) => {
            if json {
                println!(
                    "{}",
                    serde_json::json!({
                        "chain": chain_name,
                        "height": height,
                    })
                );
            } else {
                println!("Chain: {}", chain_name);
                println!("Block height: {}", height);
            }
        }
        Err(e) => {
            if json {
                println!(
                    "{}",
                    serde_json::json!({
                        "chain": chain_name,
                        "error": e.to_string(),
                    })
                );
            } else {
                println!("Chain: {}", chain_name);
                println!("Error fetching height: {}", e);
            }
        }
    }

    Ok(())
}
