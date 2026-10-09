//! `tracker-tick`: one pass of the transaction tracker (see `tracker_host`).

use anyhow::Result;

use crate::context::WalletContext;
use crate::services_env;
use crate::tracker_host::{self, SystemClock, TickOptions};

pub async fn run(ctx: &WalletContext) -> Result<()> {
    // With no header service no proof can be checked, so no word can move
    // to `mined`: a pass that proves nothing exits non-zero.
    services_env::require_chain_tracker_to_prove(
        "tracker-tick",
        ctx.wallet.services().chaintracks.is_some(),
    )?;
    let report = tracker_host::tick(
        ctx.wallet.storage(),
        ctx.wallet.services(),
        &SystemClock,
        &TickOptions::from_env(),
    )
    .await?;
    if ctx.json_output {
        println!("{}", serde_json::to_string(&report)?);
        return Ok(());
    }
    println!("{}", report.summary());
    for txid in &report.mined {
        println!("  mined:     {txid}");
    }
    for txid in &report.no_proof {
        println!("  no proof:  {txid} (asked again after its pause)");
    }
    for txid in &report.moved {
        println!("  moved:     {txid} (its stored path left the active chain)");
    }
    for line in &report.stored {
        println!("  storage:   {line}");
    }
    for line in &report.faults {
        println!("  fault:     {line}");
    }
    Ok(())
}
