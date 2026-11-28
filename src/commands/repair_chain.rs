//! `repair-chain` (#682): set a store's chain word to the context's chain,
//! once, by the operator's explicit act, with the store's own evidence.
//!
//! Every release pinning the toolbox at 0.14.0-beta.3 or below wrote the
//! settings' `chain` as `mainnet` (read as `main`) whatever `--testnet`
//! said; since #639 such a testnet store is refused when opened with
//! `--testnet`. The target is the context's chain, the one the flag names
//! (one chain word per wallet): `bsv-wallet --testnet repair-chain` says
//! "this store is a testnet wallet".
//!
//! The store is opened WITHOUT `require_chain`'s refusal; no wallet is built
//! and no service is reached but the header service (`CHAINTRACKS_URL`,
//! through the toolbox's `ChaintracksServiceClient` alone: never an
//! explorer, never a broadcaster). THE EVIDENCE: for the newest stored
//! proofs ([`PROOFS_READ`] at most), the header the target chain's header
//! service holds at each proof's height, its root compared with the
//! proof's own: `agrees` when every proof read was compared and agrees,
//! `disagrees` when any one does not, `none` when the store holds no proof,
//! no header service is configured (`CHAINTRACKS_URL=off`) or any header
//! could not be read (a fault is "could not look", never an answer).
//!
//! What the store can show of its chain by itself: only its stored proofs
//! (a height and a merkle root each) against a header service's header at
//! that height. A store with no proof shows nothing; an output's locking
//! script carries no network byte; a transaction id is the same bytes on
//! either chain.
//!
//! A dry run by default: it writes nothing and says what `--execute` would
//! write. `--execute` writes the word in one statement only when the
//! evidence agrees; when it is `none`, only with `--unchecked` too, and the
//! report says the word was written with no evidence; when it disagrees,
//! the write is refused under every flag, naming the heights. A word that
//! already is the target is said and nothing is written or asked. As
//! `reconcile-outputs` and `cleanup-abandoned` record an act, the report
//! (its lines, or `--json`'s fields) is the record: `applied` says what was
//! written.

use std::future::Future;

use anyhow::{bail, Result};
use bsv_wallet_toolbox::monitor::reorg_ops::merkle_root_of_header;
use bsv_wallet_toolbox::services::ChaintracksServiceClient;
use bsv_wallet_toolbox::{Chain, StorageSqlx, WalletStorageReader, WalletStorageWriter};
use serde::Serialize;

use crate::config::Config;

/// How many of the newest stored proofs the evidence reads.
pub const PROOFS_READ: u32 = 3;

/// Where the evidence's headers come from: the target chain's header
/// service. Production asks `CHAINTRACKS_URL` through the toolbox's client
/// alone; a test hands the toolbox's mock.
pub trait HeaderSource {
    /// The 80-byte header at `height`, or the fault that kept it unread.
    fn header_at(&self, height: u32) -> impl Future<Output = Result<Vec<u8>>> + Send;
}

impl HeaderSource for ChaintracksServiceClient {
    async fn header_at(&self, height: u32) -> Result<Vec<u8>> {
        Ok(self.find_header_for_height(height).await?.to_binary())
    }
}

/// The evidence's verdict.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Verdict {
    /// Every proof read was compared, and every one agrees.
    Agrees,
    /// At least one proof's root is not the header's at its height.
    Disagrees,
    /// Nothing compared it: no proof, no header service, or a fault.
    None,
}

impl Verdict {
    fn word(self) -> &'static str {
        match self {
            Verdict::Agrees => "agrees",
            Verdict::Disagrees => "disagrees",
            Verdict::None => "none",
        }
    }
}

/// One stored proof against the header service's header at its height.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ProofCheck {
    pub txid: String,
    pub height: u32,
    pub stored_root: String,
    /// The header's root, when the header was read.
    pub header_root: Option<String>,
    /// `agrees`, `disagrees`, or `could not look`.
    pub answer: &'static str,
    /// Why the header could not be read.
    pub fault: Option<String>,
}

/// The store's evidence of its chain.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Evidence {
    pub verdict: Verdict,
    /// Why the verdict is `none`, in words.
    pub reason: Option<String>,
    pub proofs: Vec<ProofCheck>,
}

impl Evidence {
    fn none(reason: &str) -> Self {
        Self {
            verdict: Verdict::None,
            reason: Some(reason.to_string()),
            proofs: Vec::new(),
        }
    }

    /// The heights whose proof disagreed.
    #[must_use]
    pub fn disagreeing_heights(&self) -> Vec<u32> {
        self.proofs
            .iter()
            .filter(|p| p.answer == "disagrees")
            .map(|p| p.height)
            .collect()
    }
}

/// The evidence of `proofs` (`(txid, height, root)`) against `headers`, or
/// `none` when `headers` is absent (no header service is configured).
pub async fn evidence<H: HeaderSource>(
    proofs: &[(String, i64, String)],
    headers: Option<&H>,
) -> Evidence {
    if proofs.is_empty() {
        return Evidence::none("the store holds no proof");
    }
    let Some(headers) = headers else {
        return Evidence::none("no header service is configured (CHAINTRACKS_URL=off)");
    };
    let mut checks = Vec::with_capacity(proofs.len());
    for (txid, height, stored_root) in proofs {
        let Ok(height) = u32::try_from(*height) else {
            checks.push(ProofCheck {
                txid: txid.clone(),
                height: 0,
                stored_root: stored_root.clone(),
                header_root: None,
                answer: "could not look",
                fault: Some(format!("a stored height out of range: {height}")),
            });
            continue;
        };
        let (header_root, fault) = match headers.header_at(height).await {
            Ok(bytes) => match merkle_root_of_header(&bytes) {
                Some(root) => (Some(root), None),
                None => (
                    None,
                    Some(format!("a header of {} bytes, not 80", bytes.len())),
                ),
            },
            Err(e) => (None, Some(format!("{e:#}"))),
        };
        let answer = match &header_root {
            Some(root) if root.eq_ignore_ascii_case(stored_root) => "agrees",
            Some(_) => "disagrees",
            None => "could not look",
        };
        checks.push(ProofCheck {
            txid: txid.clone(),
            height,
            stored_root: stored_root.clone(),
            header_root,
            answer,
            fault,
        });
    }
    let (verdict, reason) = if checks.iter().any(|c| c.answer == "disagrees") {
        (Verdict::Disagrees, None)
    } else if checks.iter().all(|c| c.answer == "agrees") {
        (Verdict::Agrees, None)
    } else {
        (
            Verdict::None,
            Some("the header service could not be asked for every proof read".to_string()),
        )
    };
    Evidence {
        verdict,
        reason,
        proofs: checks,
    }
}

/// What the command did, or on a dry run what `--execute` would do.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Outcome {
    /// The stored word already is the target: nothing written or asked.
    Already,
    /// A dry run: `--execute` would write the word.
    WouldWrite,
    /// A dry run: `--execute` would write the word only with `--unchecked`.
    WouldWriteOnlyUnchecked,
    /// A dry run: `--execute` would be refused (the evidence disagrees).
    WouldRefuse,
    /// Written, with the evidence agreeing.
    Written,
    /// Written with no evidence (`--unchecked`).
    WrittenUnchecked,
    /// Refused: the evidence disagrees, or it is `none` without
    /// `--unchecked`.
    Refused,
}

/// The decision, from the words, the verdict and the flags alone.
#[must_use]
pub fn decide(
    stored: &str,
    target: &str,
    verdict: Option<Verdict>,
    execute: bool,
    unchecked: bool,
) -> Outcome {
    if stored == target {
        return Outcome::Already;
    }
    match (verdict.unwrap_or(Verdict::None), execute, unchecked) {
        (Verdict::Disagrees, false, _) => Outcome::WouldRefuse,
        (Verdict::Agrees, false, _) | (Verdict::None, false, true) => Outcome::WouldWrite,
        (Verdict::None, false, false) => Outcome::WouldWriteOnlyUnchecked,
        (Verdict::Agrees, true, _) => Outcome::Written,
        (Verdict::None, true, true) => Outcome::WrittenUnchecked,
        (Verdict::Disagrees | Verdict::None, true, _) => Outcome::Refused,
    }
}

/// The command's report: its lines, or `--json`'s fields.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Report {
    pub db: String,
    /// The store's chain word as read (a legacy `mainnet` reads `main`).
    pub stored: String,
    /// The context's chain: the word `--execute` writes.
    pub target: &'static str,
    /// `None` when the word already is the target (nothing was asked).
    pub evidence: Option<Evidence>,
    pub execute: bool,
    pub unchecked: bool,
    pub outcome: Outcome,
    /// The word was written.
    pub applied: bool,
    /// Why `--execute` was, or would be, refused.
    pub refusal: Option<String>,
}

fn refusal(outcome: Outcome, target: &str, evidence: Option<&Evidence>) -> Option<String> {
    let evidence = evidence?;
    match (outcome, evidence.verdict) {
        (Outcome::Refused | Outcome::WouldRefuse, Verdict::Disagrees) => {
            let heights: Vec<String> = evidence
                .disagreeing_heights()
                .iter()
                .map(u32::to_string)
                .collect();
            Some(format!(
                "repair-chain refused: the stored proofs at height {} do not carry the root of \
                 chain {target}'s header at that height; this store is not shown to be chain \
                 {target}'s, and no flag writes the word against its evidence",
                heights.join(", ")
            ))
        }
        (Outcome::Refused | Outcome::WouldWriteOnlyUnchecked, Verdict::None) => Some(format!(
            "repair-chain refused: no evidence ({}); --execute --unchecked writes the word {target} \
             with no evidence",
            evidence.reason.as_deref().unwrap_or("none")
        )),
        _ => None,
    }
}

/// The repair over an opened `storage` (made available, its word NOT
/// asked) on `target`, the evidence's headers from `headers`. Writes only
/// on `Written` or `WrittenUnchecked`, in one statement.
///
/// # Errors
///
/// A store read or the write's fault. A refusal is a report, not an error
/// ([`Report::refusal`]); [`run`] exits non-zero on it.
pub async fn repair<H: HeaderSource>(
    storage: &StorageSqlx,
    db: &str,
    target: Chain,
    headers: Option<&H>,
    execute: bool,
    unchecked: bool,
) -> Result<Report> {
    let stored = storage.get_settings().chain;
    let evidence = if stored == target.as_str() {
        None
    } else {
        let proofs = crate::store::chain::newest_proofs(storage.pool(), PROOFS_READ).await?;
        Some(evidence(&proofs, headers).await)
    };
    let outcome = decide(
        &stored,
        target.as_str(),
        evidence.as_ref().map(|e| e.verdict),
        execute,
        unchecked,
    );
    let applied = matches!(outcome, Outcome::Written | Outcome::WrittenUnchecked);
    if applied {
        crate::store::chain::write_chain_word(storage, target).await?;
    }
    Ok(Report {
        db: db.to_string(),
        refusal: refusal(outcome, target.as_str(), evidence.as_ref()),
        stored,
        target: target.as_str(),
        evidence,
        execute,
        unchecked,
        outcome,
        applied,
    })
}

/// The store at `db`, opened and made available WITHOUT the chain word's
/// refusal; a missing file is refused, never created.
///
/// # Errors
///
/// A missing file, or a file with no wallet's settings.
pub async fn open_unasked(config: &Config, db: &str) -> Result<StorageSqlx> {
    if !std::path::Path::new(db).is_file() {
        bail!("no wallet database at {db}: repair-chain repairs an existing one");
    }
    let storage = StorageSqlx::open_with_options(db, config.wallet.storage_options()).await?;
    storage.make_available().await?;
    Ok(storage)
}

/// `bsv-wallet [--testnet] repair-chain [--execute] [--unchecked] [--json]`.
///
/// # Errors
///
/// [`open_unasked`]'s, `CHAINTRACKS_URL` unset, a store fault, or the
/// refusal of `--execute`.
pub async fn run(config: &Config, execute: bool, unchecked: bool, json: bool) -> Result<()> {
    let db = &config.wallet.db;
    let storage = open_unasked(config, db).await?;
    let headers = config
        .services
        .header_service()?
        .map(|url| ChaintracksServiceClient::from_url(&url));
    let report = repair(
        &storage,
        db,
        config.chain(),
        headers.as_ref(),
        execute,
        unchecked,
    )
    .await?;
    if json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        for line in lines(&report) {
            println!("{line}");
        }
    }
    if report.outcome == Outcome::Refused {
        bail!(report.refusal.unwrap_or_default());
    }
    Ok(())
}

fn short(h: &str) -> &str {
    h.get(..16).unwrap_or(h)
}

/// The report's printed lines.
#[must_use]
pub fn lines(report: &Report) -> Vec<String> {
    let mut out = vec![
        format!("Wallet database: {}", report.db),
        format!("Stored chain word: {}", report.stored),
        format!("Target chain word: {} (the command's chain)", report.target),
    ];
    if let Some(evidence) = &report.evidence {
        evidence_lines(evidence, report.target, &mut out);
    }
    out.extend(closing_lines(report));
    out
}

/// The evidence's lines: the verdict, then one line per proof read.
fn evidence_lines(evidence: &Evidence, target: &str, out: &mut Vec<String>) {
    let reason = evidence
        .reason
        .as_deref()
        .map(|r| format!(" ({r})"))
        .unwrap_or_default();
    out.push(format!(
        "Evidence: {}{reason}: the newest stored proofs against chain {target}'s header service",
        evidence.verdict.word()
    ));
    for p in &evidence.proofs {
        let fault = p
            .fault
            .as_deref()
            .map(|f| format!(" ({f})"))
            .unwrap_or_default();
        out.push(format!(
            "  height {}  txid {}  stored root {}  header root {}  {}{fault}",
            p.height,
            short(&p.txid),
            short(&p.stored_root),
            p.header_root.as_deref().map_or("-", short),
            p.answer
        ));
    }
}

/// What was done, or on a dry run what `--execute` would do.
fn closing_lines(report: &Report) -> Vec<String> {
    let target = report.target;
    let refusal = report.refusal.as_deref().unwrap_or_default();
    let unchecked = report.unchecked
        && report
            .evidence
            .as_ref()
            .is_some_and(|e| e.verdict == Verdict::None);
    let written = |how: &str| {
        vec![
            String::new(),
            "Applied:".to_string(),
            format!("  chain word written: {} -> {target}{how}", report.stored),
        ]
    };
    match report.outcome {
        Outcome::Already => vec![format!(
            "The stored chain word already is {target}: nothing to write."
        )],
        Outcome::WouldWrite => {
            let how = if unchecked { " with no evidence" } else { "" };
            vec![
                String::new(),
                format!("Dry run (no write). Re-run with --execute to write the chain word {target}{how}."),
            ]
        }
        Outcome::WouldWriteOnlyUnchecked | Outcome::WouldRefuse => {
            vec![String::new(), format!("Dry run (no write). {refusal}")]
        }
        Outcome::Written => written(""),
        Outcome::WrittenUnchecked => written(", with no evidence (--unchecked)"),
        Outcome::Refused => vec![String::new(), "Nothing written.".to_string()],
    }
}

#[cfg(test)]
pub(crate) mod tests;
