use anyhow::Result;
use bsv_wallet_toolbox::MonitorStorage;

use crate::context::WalletContext;

/// One pass of the toolbox's own compaction (the monitor's `CompactBeef`
/// task): stored input BEEFs gain the proofs `proven_txs` now holds and drop
/// the ancestors those proofs make dead weight. A stored proof is attached
/// only once a chain tracker confirmed its root, as the ingest requires; with
/// no tracker (`CHAINTRACKS_URL=off`) a never-checked proof is left out
/// (P0-1c). This command used to be a hand copy that read `proven_txs` by
/// raw SQL and attached every proof unchecked.
pub async fn run(ctx: &WalletContext) -> Result<()> {
    println!("Compacting stored BEEF blobs...");
    let compacted = MonitorStorage::compact_input_beefs(ctx.wallet.storage()).await?;
    println!("Compacted {compacted} stored BEEF(s).");
    Ok(())
}
