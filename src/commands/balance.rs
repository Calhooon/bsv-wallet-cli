use anyhow::Result;
use bsv_wallet_toolbox::{ListOutputsArgs, WalletInterface};

use crate::context::WalletContext;

pub async fn run(ctx: &WalletContext) -> Result<()> {
    let mut balance: u64 = 0;
    let mut offset: i32 = 0;

    loop {
        let args = ListOutputsArgs {
            basket: "default".to_string(),
            tags: None,
            tag_query_mode: None,
            include: None,
            include_custom_instructions: None,
            include_tags: None,
            include_labels: None,
            limit: Some(100),
            offset: Some(offset),
            seek_permission: None,
        };

        let result = ctx
            .wallet
            .list_outputs(args, Some("bsv-wallet-cli"))
            .await?;

        for output in &result.outputs {
            balance += output.satoshis;
        }

        offset += result.outputs.len() as i32;
        if result.outputs.is_empty() || offset >= result.total_outputs as i32 {
            break;
        }
    }

    if ctx.json_output {
        println!("{}", serde_json::json!({ "satoshis": balance }));
    } else {
        println!("{} satoshis", balance);
    }

    Ok(())
}

/// The default basket's spendable sum, read from the store: the rows
/// [`run`] counts. The toolbox's `list_outputs` (0.12.0) lists an output when
/// `spendable = 1` and its creating transaction is `completed`, `unproven`,
/// `nosend` or `sending`, in the named basket (`is_deleted = 0`); it asks
/// nothing of `spent_by`, so neither does this. The repair passes read it
/// before and after they write, so the line they print is the balance's
/// own move, never a sum of the rows they touched (#53).
pub(crate) async fn default_basket_sats(pool: &sqlx::SqlitePool) -> Result<u64> {
    let sats: i64 = sqlx::query_scalar(
        "SELECT COALESCE(SUM(o.satoshis), 0) FROM outputs o \
         JOIN transactions t ON t.transaction_id = o.transaction_id \
         JOIN output_baskets b ON b.basket_id = o.basket_id \
         WHERE o.spendable = 1 \
           AND t.status IN ('completed', 'unproven', 'nosend', 'sending') \
           AND b.name = 'default' AND b.is_deleted = 0",
    )
    .fetch_one(pool)
    .await?;
    Ok(sats.max(0) as u64)
}

/// The basket name a repair pass reports a row under; a row in no basket
/// reads `(no basket)`.
pub(crate) const BASKET_NAME_SQL: &str = "COALESCE(\
     (SELECT b.name FROM output_baskets b WHERE b.basket_id = o.basket_id), '(no basket)')";

/// Rows a repair pass moved, per basket: `rows` outputs, `sats` satoshis.
#[derive(Default, Debug, Clone, PartialEq, Eq)]
pub struct BasketTally {
    pub basket: String,
    pub rows: u64,
    pub sats: u64,
}

/// Add one row of `sats` in `basket` to `tallies` (first-seen order).
pub(crate) fn tally(tallies: &mut Vec<BasketTally>, basket: &str, sats: i64) {
    let sats = sats.max(0) as u64;
    match tallies.iter_mut().find(|t| t.basket == basket) {
        Some(t) => {
            t.rows += 1;
            t.sats += sats;
        }
        None => tallies.push(BasketTally {
            basket: basket.to_string(),
            rows: 1,
            sats,
        }),
    }
}

/// The measured line: `Balance (default basket): B1 -> B2 (delta)`, or on a
/// dry run (`after` = `None`) the one read.
pub(crate) fn balance_line(before: u64, after: Option<u64>) -> String {
    match after {
        Some(after) => format!(
            "Balance (default basket): {} -> {} ({:+} sats)",
            before,
            after,
            after as i64 - before as i64
        ),
        None => format!(
            "Balance (default basket): {} sats (dry run: nothing written)",
            before
        ),
    }
}

/// One `<verb>: N rows, S sats in <basket>` line per basket.
pub(crate) fn tally_lines(verb: &str, tallies: &[BasketTally]) -> Vec<String> {
    tallies
        .iter()
        .map(|t| format!("{}: {} rows, {} sats in {}", verb, t.rows, t.sats, t.basket))
        .collect()
}
