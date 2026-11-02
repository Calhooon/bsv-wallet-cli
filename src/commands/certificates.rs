use crate::context::WalletContext;
use anyhow::Result;

/// An explicit, repeatable audit: opening a wallet never runs it implicitly.
/// The toolbox reads every stored certificate past a failure and answers the
/// counts; a failure exits non-zero (#430). Each failing certificate is named
/// (type, serial number, certifier) only by the toolbox's warn line on stderr;
/// naming it in the answer waits on toolbox #585.
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
    if tally.failed > 0 {
        anyhow::bail!(
            "{} of {} stored certificates failed signature verification",
            tally.failed,
            tally.checked
        );
    }
    Ok(())
}
