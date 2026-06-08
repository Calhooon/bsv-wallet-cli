//! `gift-send` — pay someone a time-locked BSV gift.
//!
//! Funds a TimeLockedGift covenant locked to the recipient's public key + unlock
//! time, plus a small recipient-owned P2PKH "fee UTXO" so the recipient can pay
//! the claim's miner fee without needing any pre-existing funds. The recipient
//! can see and prove ownership of the gift immediately (`gift-inspect`) but the
//! network refuses to confirm any spend until the unlock time (`gift-claim`).

use anyhow::{anyhow, Result};
use bsv_sdk::primitives::{hash160, to_hex};
use bsv_sdk::transaction::Beef;
use bsv_sdk::wallet::{CreateActionArgs, CreateActionOptions, CreateActionOutput, WalletInterface};
use bsv_wallet_cli::gift::covenant::build_locking_script;

use crate::context::WalletContext;

/// Parse `--unlock` as a relative offset (`+3mo`, `+90d`, `+1y`, `+2w`, `+6h`,
/// `+30min`), a unix timestamp, `YYYY-MM-DD` (midnight UTC), or RFC-3339.
fn parse_unlock(s: &str) -> Result<u64> {
    let s = s.trim();
    if let Some(ts) = parse_relative(s) {
        return Ok(ts);
    }
    if let Ok(ts) = s.parse::<u64>() {
        return Ok(ts);
    }
    if let Ok(d) = chrono::NaiveDate::parse_from_str(s, "%Y-%m-%d") {
        return Ok(d.and_hms_opt(0, 0, 0).unwrap().and_utc().timestamp() as u64);
    }
    if let Ok(dt) = chrono::DateTime::parse_from_rfc3339(s) {
        return Ok(dt.timestamp() as u64);
    }
    Err(anyhow!(
        "could not parse --unlock '{s}' (use e.g. +3mo, +90d, a YYYY-MM-DD date, or a unix timestamp)"
    ))
}

/// Relative offset from now: `+3mo` / `90d` / `1y` / `2w` / `6h` / `30min`.
fn parse_relative(s: &str) -> Option<u64> {
    let s = s.trim_start_matches('+').to_lowercase();
    let split = s.find(|c: char| !c.is_ascii_digit())?;
    if split == 0 {
        return None;
    }
    let n: u32 = s[..split].parse().ok()?;
    let now = chrono::Utc::now();
    let dt = match &s[split..] {
        "min" | "mins" | "minute" | "minutes" => now + chrono::Duration::minutes(n as i64),
        "h" | "hr" | "hrs" | "hour" | "hours" => now + chrono::Duration::hours(n as i64),
        "d" | "day" | "days" => now + chrono::Duration::days(n as i64),
        "w" | "wk" | "wks" | "week" | "weeks" => now + chrono::Duration::weeks(n as i64),
        "mo" | "mos" | "month" | "months" => now.checked_add_months(chrono::Months::new(n))?,
        "y" | "yr" | "yrs" | "year" | "years" => now.checked_add_months(chrono::Months::new(n * 12))?,
        _ => return None,
    };
    Some(dt.timestamp() as u64)
}

/// Build a standard P2PKH locking script for a compressed pubkey.
fn p2pkh_script(pubkey: &[u8]) -> Vec<u8> {
    let h = hash160(pubkey);
    let mut s = Vec::with_capacity(25);
    s.extend_from_slice(&[0x76, 0xa9, 0x14]); // OP_DUP OP_HASH160 <20>
    s.extend_from_slice(&h);
    s.extend_from_slice(&[0x88, 0xac]); // OP_EQUALVERIFY OP_CHECKSIG
    s
}

pub async fn run(
    ctx: &WalletContext,
    recipient_pubkey_hex: &str,
    satoshis: u64,
    unlock: &str,
    fee_utxo: u64,
) -> Result<()> {
    let pubkey = hex::decode(recipient_pubkey_hex.trim())
        .map_err(|e| anyhow!("recipient pubkey not valid hex: {e}"))?;
    if pubkey.len() != 33 || !(pubkey[0] == 0x02 || pubkey[0] == 0x03) {
        return Err(anyhow!(
            "recipient must be a 33-byte compressed pubkey (02/03 prefix); got {} bytes",
            pubkey.len()
        ));
    }
    let lock_until = parse_unlock(unlock)?;
    if lock_until < 500_000_000 {
        return Err(anyhow!(
            "unlock {lock_until} is below 500000000 — it would be read as a block height, not a time"
        ));
    }
    // Unlock times are a 4-byte on-chain number (valid through 2038-01-19). Dates
    // past that use a 5-byte encoding whose claim path is unit-tested but not yet
    // mainnet-proven — warn loudly rather than silently create an untested gift.
    if lock_until > 2_147_483_647 {
        eprintln!(
            "⚠️  WARNING: unlock is past 2038-01-19, which uses a time encoding whose claim path is \
             not yet mainnet-proven. Do not lock real value past Jan 2038. Continuing anyway."
        );
    }
    if satoshis < 1 {
        return Err(anyhow!("gift amount must be at least 1 satoshi"));
    }
    if fee_utxo < 1000 {
        return Err(anyhow!(
            "fee-utxo {fee_utxo} is too small to cover the ~1.4KB claim's fee (use >= 1500)"
        ));
    }

    let covenant = build_locking_script(&pubkey, lock_until, satoshis)
        .map_err(|e| anyhow!("failed to build covenant: {e}"))?;
    let fee_lock = p2pkh_script(&pubkey);

    let unlock_human = chrono::DateTime::from_timestamp(lock_until as i64, 0)
        .map(|d| d.to_rfc3339())
        .unwrap_or_else(|| lock_until.to_string());

    let args = CreateActionArgs {
        description: format!("Time-locked gift of {satoshis} sats until {lock_until}"),
        input_beef: None,
        inputs: Some(vec![]),
        outputs: Some(vec![
            // output 0: the covenant (the gift itself)
            CreateActionOutput {
                locking_script: covenant,
                satoshis,
                output_description: "timelock gift covenant".to_string(),
                basket: None,
                custom_instructions: None,
                tags: Some(vec!["timelock-gift".to_string()]),
            },
            // output 1: recipient-owned fee UTXO so the claim self-funds its fee
            CreateActionOutput {
                locking_script: fee_lock,
                satoshis: fee_utxo,
                output_description: "timelock gift claim fee".to_string(),
                basket: None,
                custom_instructions: None,
                tags: Some(vec!["timelock-gift".to_string()]),
            },
        ]),
        lock_time: None,
        version: None,
        labels: Some(vec!["timelock-gift".to_string()]),
        options: Some(CreateActionOptions {
            randomize_outputs: Some(false), // covenant MUST stay at vout 0
            accept_delayed_broadcast: Some(false),
            no_send: Some(false),
            sign_and_process: Some(true),
            known_txids: Some(vec![]),
            return_txid_only: Some(false),
            no_send_change: Some(vec![]),
            send_with: Some(vec![]),
            ..Default::default()
        }),
    };

    let result = ctx.wallet.create_action(args, "bsv-wallet-cli").await?;
    let txid = result.txid.ok_or_else(|| anyhow!("wallet returned no txid"))?;
    let txid_hex = to_hex(&txid);

    let beef_hex = result.beef.as_ref().and_then(|b| {
        Beef::from_binary(b)
            .ok()
            .and_then(|mut beef| beef.to_binary_atomic(&txid_hex).ok().map(hex::encode))
    });

    if ctx.json_output {
        let mut obj = serde_json::json!({
            "txid": txid_hex,
            "covenantVout": 0,
            "feeVout": 1,
            "amount": satoshis,
            "feeUtxo": fee_utxo,
            "lockUntil": lock_until,
            "recipient": recipient_pubkey_hex,
        });
        if let Some(b) = &beef_hex {
            obj["beef"] = serde_json::Value::String(b.clone());
        }
        println!("{obj}");
    } else {
        println!("🎁 Time-locked gift sent");
        println!("   amount:     {satoshis} sats (covenant @ vout 0)");
        println!("   fee utxo:   {fee_utxo} sats (recipient-owned @ vout 1, self-funds the claim)");
        println!("   recipient:  {recipient_pubkey_hex}");
        println!("   unlocks:    {unlock_human}  ({lock_until})");
        println!("   TxID:       {txid_hex}");
        println!("   View:       https://whatsonchain.com/tx/{txid_hex}");
        println!();
        println!("   The recipient can verify it now:  bsv-wallet gift-inspect {txid_hex}");
        println!("   …and claim it after unlock:        bsv-wallet gift-claim {txid_hex}");
        if let Some(b) = &beef_hex {
            println!("   BEEF: {b}");
        }
    }

    Ok(())
}
