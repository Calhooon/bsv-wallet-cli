use anyhow::Result;

use crate::context::WalletContext;

pub async fn run(ctx: &WalletContext, txid: &str) -> Result<()> {
    // The toolbox omits failed status lookups from its answer list. Demand
    // an answer from every configured broadcaster, including failovers.
    let services = ctx.wallet.services();
    let mut sources = vec![bsv_wallet_toolbox::PROVIDER_TAAL_ARC];
    if services.arcade.is_some() {
        sources.push(bsv_wallet_toolbox::PROVIDER_ARCADE_V2);
    }
    if services.arc_gorillapool.is_some() {
        sources.push(bsv_wallet_toolbox::PROVIDER_GORILLAPOOL_ARC);
    }
    let replacement = crate::respend::build_and_post(
        ctx.wallet.storage(),
        services,
        &ctx.root_key,
        txid,
        &sources,
    )
    .await?;
    // Posted: the request's word is read tolerantly, never the error.
    let request = crate::respend::request_word(ctx.wallet.storage(), &replacement).await;
    let word = crate::server::post_word::post_word(ctx.wallet.storage(), &replacement).await;
    if ctx.json_output {
        println!(
            "{}",
            serde_json::json!({"phantom": txid, "replacement": replacement, "request": request, "broadcast": word})
        );
    } else {
        println!("Replacement: {replacement}");
        println!("Request: {request}");
        println!("The original stays tracked until a competing spend's proof checks against our headers.");
    }
    Ok(())
}
