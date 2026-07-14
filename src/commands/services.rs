use anyhow::Result;
use bsv_wallet_toolbox::{Chain, Services, WalletServices};

use crate::cli::Cli;
use crate::services_env;

pub async fn run(cli: &Cli) -> Result<()> {
    let chain = if cli.testnet {
        Chain::Test
    } else {
        Chain::Main
    };

    // Shared env-driven services config — see services_env.rs.
    let services = {
        let opts = services_env::services_options_from_env(chain, &cli.db)?;
        Services::with_options(chain, opts)?
    };

    let chain_name = match chain {
        Chain::Main => "mainnet",
        Chain::Test => "testnet",
    };

    match services.get_height().await {
        Ok(height) => {
            if cli.json {
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
            if cli.json {
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
