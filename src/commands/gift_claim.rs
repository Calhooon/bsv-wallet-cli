//! `gift-claim` — claim a time-locked BSV gift after its unlock time.
//!
//! Fetches the deposit tx, confirms the covenant is locked to this wallet's key,
//! refuses if still locked (unless `--force` to pre-broadcast), builds the signed
//! claim (covenant unlock + fee input), and broadcasts through the wallet's
//! own broadcasters (Rule 28, C11: the deposit's bytes through
//! `Services::get_raw_tx`, the claim through `post_beef`; see `gift_chain`).
//!
//! The claim is tracked from the moment it is handed over, and what the
//! command prints is the tracker's word (E2). With `--force` before the
//! lock time a broadcaster may take the claim (`announced`) or call it not
//! final; the second is not a failure: the claim is kept and announced
//! again at the lock time by the tracker's pass.

use anyhow::{anyhow, Result};
use bsv_sdk::primitives::bsv::sighash::parse_transaction;
use bsv_sdk::wallet::WalletInterface;
use bsv_wallet_cli::gift::claim::build_claim_tx;
use bsv_wallet_cli::gift::covenant::parse_locking_script;

use crate::commands::gift_chain;
use crate::context::WalletContext;
use crate::tracker_host::{SystemClock, TickOptions};

/// What became of the wallet's own record of its claim (E4).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Recorded {
    /// The door took the claim's outputs: they are the wallet's balance.
    Yes,
    /// The claim is posted and the door did not take its outputs: what it
    /// said. The coin is the wallet's on the chain and is not in its
    /// balance yet; the tracker's row keeps the record, and the pass makes
    /// it once the claim is mined (#638).
    NotYet { said: String },
    /// Held as not final: no broadcaster took the claim, so nothing is
    /// recorded before one does. The row keeps the record with the claim.
    WhenTaken,
}

/// What `gift-claim` did, as the command prints it: the claim's plan, what
/// became of it at the broadcasters and in the tracker, and what became of
/// the wallet's record of it.
#[derive(Debug, Clone)]
pub(crate) struct ClaimReport {
    pub claim_txid: String,
    pub deposit_txid: String,
    pub amount: u64,
    pub fee: u64,
    pub change: u64,
    pub lock_until: u64,
    /// The unlock time as a person reads it.
    pub unlocks: String,
    pub outcome: gift_chain::ClaimOutcome,
    pub recorded: Recorded,
    /// The claim's outputs that pay this wallet, lowest index at the head:
    /// what `receive` takes in.
    pub pays: Vec<u32>,
}

impl ClaimReport {
    /// The commands that take the claim's coin in by hand once the claim is
    /// mined, one per output that pays this wallet: `receive` finds the
    /// lowest such output by itself and is told every other.
    pub(crate) fn receive_commands(&self) -> Vec<String> {
        self.pays
            .iter()
            .enumerate()
            .map(|(place, vout)| match place {
                0 => format!("bsv-wallet receive {}", self.claim_txid),
                _ => format!("bsv-wallet receive {} --vout {vout}", self.claim_txid),
            })
            .collect()
    }

    /// Why the claim's coin is not in the balance yet and what takes it in,
    /// or none when it is recorded.
    pub(crate) fn record_hint(&self) -> Option<String> {
        let by_hand = self.receive_commands().join("` and `");
        match &self.recorded {
            Recorded::Yes => None,
            Recorded::NotYet { said } => Some(format!(
                "the claim is posted and its coin is not in your balance yet: {} ({said}); the wallet keeps the claim's record and makes it once the claim is mined: `daemon` and `serve` do that in their tracker pass, and with neither running `bsv-wallet tracker-tick` does; by hand, once the claim is mined, run `{by_hand}`",
                self.why_not_yet()
            )),
            Recorded::WhenTaken => Some(format!(
                "the claim is held as not final and nothing is recorded before a broadcaster takes it; the wallet keeps the claim's record and makes it then, in the tracker pass of `daemon` and `serve` or by `bsv-wallet tracker-tick`; if the claim is mined and still not in your balance, run `{by_hand}`"
            )),
        }
    }

    /// What stood between the posted claim and its record.
    fn why_not_yet(&self) -> String {
        if self.outcome.deposit_proven {
            "the wallet could not record it".to_string()
        } else {
            format!(
                "no checked proof of the gift's deposit {} was to be had, so the wallet could not record the claim",
                self.deposit_txid
            )
        }
    }

    /// The `--json` output.
    pub(crate) fn json(&self) -> serde_json::Value {
        serde_json::json!({
            "claimTxid": self.claim_txid,
            "depositTxid": self.deposit_txid,
            "amount": self.amount,
            "fee": self.fee,
            "change": self.change,
            "lockUntil": self.lock_until,
            "broadcast": self.outcome.broadcaster,
            "word": self.outcome.word,
            "reask": self.outcome.reask,
            "heldUntil": self.outcome.held_until,
            "notFinal": self.outcome.not_final,
            "recorded": self.recorded == Recorded::Yes,
            "recordError": match &self.recorded {
                Recorded::NotYet { said } => Some(said.clone()),
                _ => None,
            },
            "recordHeld": self.recorded != Recorded::Yes,
            "recordHint": self.record_hint(),
            "receiveCommands": match self.recorded {
                Recorded::Yes => vec![],
                _ => self.receive_commands(),
            },
        })
    }

    /// The text output.
    pub(crate) fn text(&self) -> String {
        let claim_txid = &self.claim_txid;
        let unlocks = &self.unlocks;
        let mut lines = vec![
            match &self.outcome.not_final {
                None => "🔓 Gift claim broadcast".to_string(),
                Some(_) => "⏳ Gift claim held: not final yet".to_string(),
            },
            format!("   amount:     {} sats → your address", self.amount),
            format!(
                "   fee:        {} sats (change {} sats back to you)",
                self.fee, self.change
            ),
            format!("   unlocks:    {unlocks}"),
            format!("   claim txid: {claim_txid}"),
            format!("   word:       {} (the tracker's)", self.outcome.word),
        ];
        match &self.recorded {
            Recorded::Yes => lines.push(format!(
                "   wallet:     recorded; {} sats are in your balance",
                self.amount + self.change
            )),
            Recorded::NotYet { said } => {
                lines.push(format!(
                    "   wallet:     NOT recorded yet. The claim is posted and its coin is not in your balance:\n               {} ({said}).\n               The wallet keeps the claim's record and makes it once the claim is mined:\n               `daemon` and `serve` do that in their tracker pass; with neither running,\n               run `bsv-wallet tracker-tick` after the claim is mined. Or take it in by hand then:",
                    self.why_not_yet()
                ));
                lines.extend(
                    self.receive_commands()
                        .iter()
                        .map(|command| format!("                 {command}")),
                );
            }
            Recorded::WhenTaken => {
                lines
                    .push("   wallet:     recorded when a broadcaster takes the claim".to_string());
            }
        }
        lines.push(match (&self.outcome.not_final, self.outcome.held_until) {
            (Some(said), _) => format!(
                "\n   A broadcaster called it not final ({said}). The claim is kept and\n   announced again at {unlocks}: `serve` does that by itself, or run\n   `bsv-wallet tracker-tick` after the unlock."
            ),
            (None, Some(_)) => format!(
                "\n   ⏳ Pre-armed before unlock: it sits unconfirmed and confirms once\n      median-time-past passes {unlocks}. Nothing is asked about it before then."
            ),
            (None, None) => "\n   `bsv-wallet tracker-tick` (or `serve`) asks for its proof; the word turns\n   `mined` when the proof checks against the header service.".to_string(),
        });
        lines.join("\n")
    }
}

/// `gift-claim`: claim the gift of deposit `txid`, and print what was done.
pub async fn run<S: bsv_wallet_toolbox::WalletServices + Send + Sync + 'static>(
    ctx: &WalletContext<S>,
    txid: &str,
    force: bool,
) -> Result<()> {
    let report = claim(ctx, txid, force).await?;
    if ctx.json_output {
        println!("{}", report.json());
    } else {
        println!("{}", report.text());
    }
    Ok(())
}

/// Claim the gift of deposit `txid`: build the claim, hand it to the
/// broadcasters and the tracker, and record it in the wallet.
pub(crate) async fn claim<S: bsv_wallet_toolbox::WalletServices + Send + Sync + 'static>(
    ctx: &WalletContext<S>,
    txid: &str,
    force: bool,
) -> Result<ClaimReport> {
    // 0. The wallet records its claim by internalizeAction after the
    // broadcast; with no chain tracker every internalizeAction is refused
    // at the toolbox's door (#509), so the claim would be posted and never
    // recorded. Refuse by name before the deposit is fetched.
    ctx.require_chain_tracker_to_receive("gift-claim").await?;
    // 1. fetch the deposit transaction
    let deposit_raw = gift_chain::deposit_bytes(ctx.wallet.services(), txid).await?;

    // 2. parse the covenant + confirm it's ours
    let dep = parse_transaction(&deposit_raw).map_err(|e| anyhow!("parse deposit: {e}"))?;
    let cov_out = dep
        .outputs
        .first()
        .ok_or_else(|| anyhow!("{txid} has no vout 0"))?;
    let params = parse_locking_script(&cov_out.script)
        .map_err(|e| anyhow!("{txid} vout 0 is not a TimeLockedGift covenant: {e}"))?;

    // Sign + receive with the BRC-29 DEPOSIT key, so the claimed coins land at
    // the wallet's deposit address; the wallet records its own claim below
    // (E4), so they are normal spendable balance with no scan of the chain.
    let (deposit_priv, deposit_pub) = crate::brc29::deposit_keypair(&ctx.root_key)?;
    let our_pub = deposit_pub.to_compressed();
    if our_pub.as_slice() != params.recipient.as_slice() {
        return Err(anyhow!(
            "this gift is locked to {}, which is not your deposit key ({})",
            hex::encode(&params.recipient),
            hex::encode(our_pub)
        ));
    }

    // 3. unlock-time gate
    let now = chrono::Utc::now().timestamp() as u64;
    let unlock_human = chrono::DateTime::from_timestamp(params.lock_until as i64, 0)
        .map(|d| d.to_rfc3339())
        .unwrap_or_else(|| params.lock_until.to_string());
    if now < params.lock_until && !force {
        return Err(anyhow!(
            "🔒 This gift is locked until {unlock_human} ({}). It cannot be claimed yet.\n   \
             (Use --force to pre-broadcast now; it will sit unconfirmed and auto-confirm at unlock.)",
            params.lock_until
        ));
    }

    // 4. build the signed claim (signed by the deposit key)
    let plan = build_claim_tx(&deposit_raw, &deposit_priv, params.lock_until as u32)
        .map_err(|e| anyhow!("failed to build claim: {e}"))?;

    // 5. hand it to the wallet's broadcasters, and to the tracker
    let claim_raw =
        hex::decode(&plan.claim_raw_hex).map_err(|e| anyhow!("the claim is not hex: {e}"))?;
    let pay_script = crate::brc29::deposit_script(&ctx.root_key)?;
    let outcome = gift_chain::announce_claim(
        ctx.wallet.storage(),
        ctx.wallet.services(),
        &SystemClock,
        &TickOptions::from_config(&ctx.config),
        &deposit_raw,
        &claim_raw,
        &pay_script,
        params.lock_until,
        force,
    )
    .await?;

    // 6. the wallet records its own claim (E4): the BEEF is in hand, so
    // nothing scans the chain to learn what this wallet just did. A claim
    // held as not final is recorded when its re-announce is accepted. The
    // tracker's row keeps the record from the post until it is made
    // (#638): a door that declines it now (a deposit not mined yet) leaves
    // it for the pass that sees the claim's own proof.
    let recorded = match outcome.record.clone() {
        Some(record) => match ctx
            .wallet
            .internalize_action(record, Some("bsv-wallet-cli"))
            .await
        {
            Ok(result) if result.accepted => {
                // The record is made: it leaves the tracker's row. A row
                // that could not be written keeps it, and the pass that
                // hands it over again changes nothing.
                if let Err(e) =
                    crate::tracker_host::record_made(ctx.wallet.storage(), &plan.claim_txid).await
                {
                    tracing::warn!(marker = "owed_record_not_cleared", txid = %plan.claim_txid, error = %e, "the claim is recorded and its row still holds the record");
                }
                Recorded::Yes
            }
            Ok(_) => Recorded::NotYet {
                said: "the wallet did not accept its own claim".to_string(),
            },
            Err(e) => Recorded::NotYet {
                said: e.to_string(),
            },
        },
        None => Recorded::WhenTaken,
    };

    Ok(ClaimReport {
        claim_txid: plan.claim_txid,
        deposit_txid: plan.deposit_txid,
        amount: plan.covenant.amount,
        fee: plan.fee,
        change: plan.change,
        lock_until: plan.covenant.lock_until,
        unlocks: unlock_human,
        pays: gift_chain::outputs_paying(&claim_raw, &pay_script)?,
        outcome,
        recorded,
    })
}
