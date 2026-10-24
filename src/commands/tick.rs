use anyhow::Result;
use bsv_sdk::primitives::PrivateKey;
use bsv_wallet_toolbox::{
    Monitor, Services, StorageSqlx, WalletStorageProvider, WalletStorageWriter,
};
use std::sync::Arc;

use crate::config::Config;
use crate::services_env;

pub async fn run(config: &Config, json: bool) -> Result<()> {
    let _root_key = PrivateKey::from_hex(config.wallet.root_key()?)?;

    let chain = config.chain();
    let db = &config.wallet.db;

    let storage = StorageSqlx::open(db).await?;
    storage.make_available().await?;

    // Shared services config — see services_env.rs.
    let services = {
        let opts = services_env::services_options(&config.services, chain, db)?;
        Services::with_options(chain, opts)?
    };
    // P0-1b: with no tracker every proof is refused; a proving pass that
    // proves nothing exits non-zero rather than print a clean run.
    services_env::require_chain_tracker_to_prove("tick", services.chaintracks.is_some())?;
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

    if json {
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
