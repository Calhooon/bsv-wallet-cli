//! Prepared, unsent actions at the command line (#673).
//!
//! A `noSend` action is signed and never sent: its transaction may be
//! handed to someone who posts it, or sent later by the caller that
//! prepared it. Until it is sent or aborted it keeps the coins its inputs
//! spend reserved, and nothing ages it out. `actions` marks each one with
//! its age, its txid, its reference and the satoshis its inputs hold, and
//! totals them; `actions --prepared` lists them alone; `abort-action`
//! aborts the one action it names through this host's `abortAction`
//! (`server::handlers::abort_guarded`, the served door's own guard), which
//! never aborts a transaction a broadcaster took.
//!
//! What an abort is and is not: it frees this wallet's coins and writes
//! the action `failed`, so this wallet never sends it. It does not unsay a
//! transaction: a prepared transaction whose bytes were handed to someone
//! else can still be posted by them, and once it is, the coins this wallet
//! freed are spent by it. Abort only a payment that was not handed over.

use crate::store::{self, HeldBy};
use anyhow::{anyhow, Result};
use bsv_sdk::wallet::AbortActionArgs;
use serde_json::{json, Value};

use crate::context::WalletContext;

/// The originator the command line's wallet calls name.
const ORIGINATOR: &str = "bsv-wallet-cli";

/// What an abort is and is not, as the commands print it.
pub const ABORT_IS: &str = "An abort frees this wallet's coins and the wallet never sends \
     the action; a prepared transaction whose bytes were handed to someone else can still \
     be posted by them, so abort only a payment that was not handed over";

/// The prepared, unsent actions of the wallet's user, every one (no page).
pub async fn read(ctx: &WalletContext) -> Result<Vec<HeldBy>> {
    let user = crate::spend_gate::UserCoins::from_wallet(&ctx.wallet).await?;
    store::prepared_actions(ctx.wallet.storage().pool(), user.user_id()).await
}

/// Seconds since `created_at` (unix seconds), never below zero.
pub fn age_seconds(created_at: i64, now: i64) -> i64 {
    (now - created_at).max(0)
}

/// An age as a person reads it: `45s`, `12m`, `3h 5m`, `2d 4h`.
pub fn age_words(seconds: i64) -> String {
    let (d, h, m) = (
        seconds / 86_400,
        seconds % 86_400 / 3_600,
        seconds % 3_600 / 60,
    );
    match (d, h, m) {
        (0, 0, 0) => format!("{seconds}s"),
        (0, 0, m) => format!("{m}m"),
        (0, h, m) => format!("{h}h {m}m"),
        (d, h, _) => format!("{d}d {h}h"),
    }
}

/// `n` and the noun, singular for one.
fn count(n: i64, one: &str, many: &str) -> String {
    format!("{n} {}", if n == 1 { one } else { many })
}

/// A prepared action's line under its listing: what marks it.
pub fn mark(p: &HeldBy, now: i64) -> String {
    format!(
        "      prepared, not sent: holds {} sats in {}, {} old | txid {} | reference {}",
        p.held_satoshis,
        count(p.held_inputs, "input", "inputs"),
        age_words(age_seconds(p.created_at, now)),
        p.txid,
        p.reference
    )
}

/// The total line of `prepared`, when there is one or more.
pub fn total(prepared: &[HeldBy]) -> Option<String> {
    if prepared.is_empty() {
        return None;
    }
    let held: i64 = prepared.iter().map(|p| p.held_satoshis).sum();
    Some(format!(
        "{} {} {held} sats: `bsv-wallet actions --prepared` lists them, \
         `bsv-wallet abort-action <txid>` frees one. {ABORT_IS}",
        count(
            prepared.len() as i64,
            "prepared, unsent action",
            "prepared, unsent actions"
        ),
        if prepared.len() == 1 { "holds" } else { "hold" },
    ))
}

/// The fields `actions --json` adds to a prepared action's object, under
/// `prepared`: its reference, when it was made (unix seconds), its age in
/// seconds, and the inputs it holds.
pub fn added(p: &HeldBy, now: i64) -> Value {
    json!({
        "reference": p.reference,
        "createdAt": p.created_at,
        "ageSeconds": age_seconds(p.created_at, now),
        "heldInputs": p.held_inputs,
        "heldSatoshis": p.held_satoshis,
    })
}

/// The object `actions --json` adds beside `actions`, as `preparedTotal`:
/// how many prepared actions the store holds and the satoshis they hold.
pub fn added_total(prepared: &[HeldBy]) -> Value {
    json!({
        "actions": prepared.len(),
        "heldSatoshis": prepared.iter().map(|p| p.held_satoshis).sum::<i64>(),
    })
}

/// `actions --prepared`: the prepared, unsent actions alone, every one,
/// oldest first.
pub fn list(prepared: &[HeldBy], now: i64, json_output: bool) -> String {
    if json_output {
        let rows: Vec<Value> = prepared
            .iter()
            .map(|p| {
                json!({
                    "txid": p.txid,
                    "reference": p.reference,
                    "description": p.description,
                    "satoshis": p.satoshis,
                    "createdAt": p.created_at,
                    "ageSeconds": age_seconds(p.created_at, now),
                    "heldInputs": p.held_inputs,
                    "heldSatoshis": p.held_satoshis,
                })
            })
            .collect();
        let held: i64 = prepared.iter().map(|p| p.held_satoshis).sum();
        return serde_json::to_string_pretty(&json!({"prepared": rows, "heldSatoshis": held}))
            .unwrap_or_default();
    }
    let Some(total) = total(prepared) else {
        return "No prepared, unsent actions: no coin is held by one.".to_string();
    };
    let mut out = String::new();
    for p in prepared {
        out.push_str(&format!(
            "  {} | reference {} | {} old | holds {} sats in {} | {} sats | {}\n",
            p.txid,
            p.reference,
            age_words(age_seconds(p.created_at, now)),
            p.held_satoshis,
            count(p.held_inputs, "input", "inputs"),
            p.satoshis,
            p.description
        ));
    }
    out.push_str(&total);
    out
}

/// The time now, in unix seconds.
pub fn now() -> i64 {
    chrono::Utc::now().timestamp()
}

/// `abort-action <txid|reference>`: the one action `name` names, aborted
/// through this host's `abortAction`; never more than that one. Under
/// `--json` a refusal prints the door's body (`{code, message}`) on stdout
/// and exits 1.
pub async fn abort(ctx: &WalletContext, name: &str) -> Result<()> {
    let user = crate::spend_gate::UserCoins::from_wallet(&ctx.wallet).await?;
    let before = store::held_by(ctx.wallet.storage().pool(), user.user_id(), name).await?;
    let args = AbortActionArgs {
        reference: name.to_string(),
    };
    match crate::server::handlers::abort_guarded(&ctx.wallet, args, ORIGINATOR).await {
        Ok(result) => {
            let txid = before.as_ref().map(|b| b.txid.as_str()).unwrap_or(name);
            let reference = before.as_ref().map(|b| b.reference.as_str()).unwrap_or("");
            let (inputs, sats) = before
                .as_ref()
                .map(|b| (b.held_inputs, b.held_satoshis))
                .unwrap_or_default();
            if ctx.json_output {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&json!({
                        "aborted": result.aborted,
                        "txid": txid,
                        "reference": reference,
                        "was": before.as_ref().map(|b| b.status.as_str()),
                        "freedInputs": inputs,
                        "freedSatoshis": sats,
                    }))?
                );
            } else {
                println!(
                    "aborted {txid} (reference {reference}): {} holding {sats} sats, spendable again; \
                     the action is failed and this wallet will not send it. A transaction whose \
                     bytes were handed to someone else can still be posted by them, and then \
                     spends these coins",
                    count(inputs, "input", "inputs"),
                );
            }
            Ok(())
        }
        Err(refused) => {
            if ctx.json_output {
                println!("{}", refused.to_json());
                std::process::exit(1);
            }
            Err(anyhow!("abort-action {name}: {}", refused.served_message()))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_age_reads_in_its_largest_two_units() {
        assert_eq!(age_words(0), "0s");
        assert_eq!(age_words(45), "45s");
        assert_eq!(age_words(12 * 60 + 5), "12m");
        assert_eq!(age_words(3 * 3_600 + 5 * 60), "3h 5m");
        assert_eq!(age_words(2 * 86_400 + 4 * 3_600 + 59), "2d 4h");
        assert_eq!(age_seconds(100, 40), 0);
    }

    fn held(sats: i64) -> HeldBy {
        HeldBy {
            txid: "ab".repeat(32),
            reference: "cmVm".into(),
            status: "nosend".into(),
            description: "a payment".into(),
            satoshis: -1_038,
            created_at: 1_000,
            held_inputs: 1,
            held_satoshis: sats,
        }
    }

    #[test]
    fn the_total_names_the_count_the_hold_and_the_commands() {
        assert_eq!(total(&[]), None);
        let one = total(&[held(100_000)]).unwrap();
        assert!(
            one.starts_with("1 prepared, unsent action holds 100000 sats: "),
            "{one}"
        );
        let two = total(&[held(1), held(2)]).unwrap();
        assert!(
            two.starts_with("2 prepared, unsent actions hold 3 sats: "),
            "{two}"
        );
        for said in [
            "bsv-wallet actions --prepared",
            "bsv-wallet abort-action <txid>",
            ABORT_IS,
        ] {
            assert!(one.contains(said), "{one}");
        }
    }
}
