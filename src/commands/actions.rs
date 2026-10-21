use anyhow::Result;
use bsv_wallet_toolbox::{ListActionsArgs, WalletInterface};

use crate::context::WalletContext;

/// The word a person reads for an action's status. `unproven` (sent, its
/// proof not stored yet, and again after a reorg took its proof away) reads
/// "confirming": ruling 7b (b) of 2026-10-20 20:1xZ (bsv-stack-lean
/// `docs/DECISIONS-2026-10-19.md` section 8c), a reorg never alarms. Every
/// other status reads as its wire word. The `--json` output keeps the wire
/// word (MetaNet Client's `listActions` shape).
pub fn status_word(wire: &str) -> &str {
    match wire {
        "unproven" => "confirming",
        other => other,
    }
}

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
    let rules = crate::spend_gate::action_rules(ctx.wallet.storage()).await?;

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
            // A template not yet signed is listed with an all-zero txid.
            let rule = if action.txid == [0u8; 32] {
                let status = serde_json::to_value(action.status)?;
                rules.by_template.get(&crate::spend_gate::template_key(
                    status.as_str().unwrap_or_default(),
                    &action.description,
                    action.satoshis,
                ))
            } else {
                rules.by_txid.get(&hex::encode(action.txid))
            }
            .map(|r| format!(" | under {r}"))
            .unwrap_or_default();
            let status = serde_json::to_value(action.status)?;
            println!(
                "  {} | {} | {} | {} sats | {}{}",
                direction,
                hex::encode(&action.txid[..4]),
                status_word(status.as_str().unwrap_or_default()),
                action.satoshis,
                action.description,
                rule
            );
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::status_word;
    use bsv_sdk::wallet::ActionStatus;

    /// Ruling 7b (b): an `unproven` action, demoted by a reorg or not yet
    /// proven, reads "confirming"; the other words are the wire's.
    #[test]
    fn an_unproven_action_reads_confirming() {
        let wire = |s: ActionStatus| {
            serde_json::to_value(s)
                .unwrap()
                .as_str()
                .unwrap()
                .to_string()
        };
        assert_eq!(status_word(&wire(ActionStatus::Unproven)), "confirming");
        assert_eq!(status_word(&wire(ActionStatus::Completed)), "completed");
        assert_eq!(status_word(&wire(ActionStatus::Sending)), "sending");
        assert_eq!(status_word(&wire(ActionStatus::Failed)), "failed");
    }
}
