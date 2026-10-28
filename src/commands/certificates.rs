use crate::context::WalletContext;
use anyhow::Result;

/// An explicit, repeatable audit: opening a wallet never runs it implicitly.
pub async fn run(ctx: &WalletContext) -> Result<()> {
    let tally = ctx.wallet.reverify_stored_certificates().await?;
    if ctx.json_output {
        println!(
            "{}",
            serde_json::json!({"checked": tally.checked, "failed": tally.failed})
        );
    } else {
        println!(
            "Certificates checked: {}, failed signature verification: {}",
            tally.checked, tally.failed
        );
        println!("No certificates changed. Certificates issued before toolbox #203 need re-issue.");
    }
    Ok(())
}
