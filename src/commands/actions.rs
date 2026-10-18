use anyhow::Result;
use bsv_wallet_toolbox::{ListActionsArgs, WalletInterface};

use crate::context::WalletContext;

pub async fn run(ctx: &WalletContext, label: Option<&str>) -> Result<()> {
    let args = ListActionsArgs {
        labels: label.map(|l| vec![l.to_string()]).unwrap_or_default(),
        label_query_mode: None,
        include_labels: Some(true),
        include_inputs: None,
        include_input_source_locking_scripts: None,
        include_input_unlocking_scripts: None,
        include_outputs: None,
        include_output_locking_scripts: None,
        limit: Some(100),
        offset: None,
        seek_permission: None,
    };

    let result = ctx.wallet.list_actions(args, "bsv-wallet-cli").await?;
    // The rule an action named at the door (R1 (d)), as `underRule`.
    let rules = crate::spend_gate::rules_of(ctx.wallet.storage().pool()).await?;

    if ctx.json_output {
        let shown = crate::server::handlers::with_rules(serde_json::to_value(&result)?, &rules);
        println!("{}", serde_json::to_string_pretty(&shown)?);
    } else {
        println!(
            "{} actions (total: {})",
            result.actions.len(),
            result.total_actions
        );
        for action in &result.actions {
            let direction = if action.is_outgoing { "OUT" } else { "IN " };
            let rule = rules
                .get(&hex::encode(action.txid))
                .map(|r| format!(" | under {r}"))
                .unwrap_or_default();
            println!(
                "  {} | {} | {} sats | {}{}",
                direction,
                hex::encode(&action.txid[..4]),
                action.satoshis,
                action.description,
                rule
            );
        }
    }

    Ok(())
}
