use anyhow::{Context, Result};
use bsv_sdk::primitives::PrivateKey;
use bsv_wallet_toolbox::{
    ArcConfig, Chain, Monitor, Services, ServicesOptions, StorageSqlx, WalletStorageProvider,
    WalletStorageWriter,
};
use std::sync::Arc;

use crate::cli::Cli;

pub async fn run(cli: &Cli) -> Result<()> {
    let root_key_hex =
        std::env::var("ROOT_KEY").context("ROOT_KEY not set. Run `bsv-wallet init` first.")?;
    let _root_key = PrivateKey::from_hex(&root_key_hex)?;

    let chain = if cli.testnet {
        Chain::Test
    } else {
        Chain::Main
    };

    let storage = StorageSqlx::open(&cli.db).await?;
    storage.make_available().await?;
    sqlx::query("PRAGMA busy_timeout = 500")
        .execute(storage.pool())
        .await
        .ok();

    let services = {
        let mut opts = match chain {
            Chain::Main => ServicesOptions::mainnet(),
            Chain::Test => ServicesOptions::testnet(),
        };
        if let Ok(url) = std::env::var("CHAINTRACKS_URL") {
            opts = opts.with_chaintracks_url(url);
        }
        if let Ok(key) = std::env::var("TAAL_API_KEY") {
            if !key.is_empty() {
                let arc_url = opts.arc_url.clone();
                opts = opts.with_arc(arc_url, Some(ArcConfig::with_api_key(key)));
            }
        }
        Services::with_options(chain, opts)?
    };
    if let Some(ref ct) = services.chaintracks {
        storage.set_chain_tracker(ct.clone()).await;
    }

    let storage_arc = Arc::new(storage);
    let services_arc = Arc::new(services);
    storage_arc.set_services(services_arc.clone());

    let monitor = Monitor::new(storage_arc, services_arc);
    let results = monitor
        .run_once()
        .await
        .map_err(|e| anyhow::anyhow!("{}", e))?;

    let mut entries: Vec<_> = results.into_iter().collect();
    entries.sort_by_key(|(t, _)| format!("{}", t));

    if cli.json {
        let json: serde_json::Map<String, serde_json::Value> = entries
            .iter()
            .map(|(t, r)| {
                (
                    format!("{}", t),
                    serde_json::json!({
                        "processed": r.items_processed,
                        "errors": r.errors,
                    }),
                )
            })
            .collect();
        println!("{}", serde_json::Value::Object(json));
    } else {
        let mut total_proc = 0u32;
        let mut total_err = 0usize;
        for (task, res) in &entries {
            total_proc += res.items_processed;
            total_err += res.errors.len();
            let line = format!(
                "{:<24} processed={:<5} errors={}",
                task.to_string(),
                res.items_processed,
                res.errors.len()
            );
            if !res.errors.is_empty() {
                println!("{}", line);
                for e in &res.errors {
                    println!("    └─ {}", e);
                }
            } else {
                println!("{}", line);
            }
        }
        println!("---");
        println!(
            "total: {} task(s), {} processed, {} error(s)",
            entries.len(),
            total_proc,
            total_err
        );
    }

    Ok(())
}
