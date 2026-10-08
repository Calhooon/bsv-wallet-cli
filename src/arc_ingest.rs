//! Shared ingestion of ARC/Arcade callback payloads.
//!
//! Both proof-delivery push paths converge here:
//! - the daemon's own `POST /arc-callback` route (direct webhook), and
//! - the `bsv-wallet-relay` poller (store-and-forward webhook).
//!
//! The body is Arcade's event (`txid`, `txStatus`, `extraInfo`, `status`,
//! `competingTxs`, `blockHash`, `blockHeight`, `merklePath`; arcade@1ae1208
//! `services/webhook/service.go:317-327`). It is parsed into the toolbox's
//! own `ArcadeStatusEvent` and handed whole to
//! `ArcadeEventsTask::apply_event`, the toolbox's one judgment for the SSE
//! frame and the webhook alike: the CLI adds nothing and drops nothing, so
//! the word decides, never the presence of a path. A MINED or IMMUTABLE path
//! (a `reorg_reanchor` included) goes through the toolbox's proof funnel,
//! checked against our headers; `reorg_unmined` changes nothing and asks for
//! the proof again; a REJECTED with ARC code 466 (or competitors) is a
//! conflict, with 476 it is retryable and not applied; a word Arcade does
//! not define is refused (bsv-stack-lean P0-2b, P0-2c).

use anyhow::{anyhow, Result};
use bsv_wallet_toolbox::monitor::ArcadeEventsTask;
use bsv_wallet_toolbox::services::providers::arcade::{
    arcade_reorg_marker, arcade_verdict, ArcadeReorgMarker, ArcadeStatusEvent, ArcadeVerdict,
};
use bsv_wallet_toolbox::StorageSqlx;
use std::sync::atomic::{AtomicBool, Ordering};

/// The reason carried by [`IngestAction::ProofRejected`]: the toolbox's
/// funnel did not store a MINED path. `apply_event` reports only that the
/// path was not stored (it falls back to the re-ask), so which outcome it was
/// (unparseable, a root our headers do not hold, a block above the processed
/// height, a tracker fault, no chain tracker) is named in the toolbox's warn
/// log, `inline SSE proof not accepted`, not here.
pub const PROOF_NOT_ACCEPTED: &str =
    "proof not stored: the toolbox's funnel refused or deferred the path (its log names why); the proof is asked for again";

/// What ingesting a callback payload did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IngestAction {
    /// A merkle proof was validated and stored; records completed.
    ProofIngested,
    /// The body was MINED or IMMUTABLE with a path and the toolbox did not
    /// store it (see [`PROOF_NOT_ACCEPTED`]); nothing stored.
    ProofRejected(String),
    /// A status-only update was applied to storage.
    StatusApplied,
    /// A status-only update matched no records (unknown txid or already final).
    StatusIgnored,
    /// The toolbox asked for the proof again (`reorg_unmined`, a mined word
    /// with no path, `STUMP_PROCESSING`); any status change it made is in
    /// storage, the word itself moved no proof.
    Reask,
    /// A word Arcade does not define: refused, nothing applied.
    Refused(String),
}

/// Parse one ARC/Arcade callback payload and hand it whole to the toolbox's
/// judgment.
pub async fn ingest_arc_payload(
    storage: &StorageSqlx,
    payload: &serde_json::Value,
) -> Result<IngestAction> {
    let txid = payload
        .get("txid")
        .and_then(|v| v.as_str())
        .ok_or_else(|| anyhow!("payload missing txid"))?;
    if txid.len() != 64 || !txid.chars().all(|c| c.is_ascii_hexdigit()) {
        return Err(anyhow!("invalid txid"));
    }
    let ev: ArcadeStatusEvent = serde_json::from_value(payload.clone())
        .map_err(|e| anyhow!("not an Arcade status body: {}", e))?;

    // The CLI's trigger: the running monitor's proof flag is not reachable
    // from here (toolbox 0.4.0 exposes none), so the re-ask is answered and
    // logged; the monitor's own proof and reorg tasks do the re-check.
    let reask = AtomicBool::new(false);
    let updated = ArcadeEventsTask::<StorageSqlx>::apply_event(storage, &ev, &reask)
        .await
        .map_err(|e| anyhow!("apply_event: {}", e))?;
    let reask = reask.load(Ordering::SeqCst);

    // Name what the toolbox did, by the toolbox's own reading of the body.
    let unmined = arcade_reorg_marker(ev.extra_info.as_deref()) == Some(ArcadeReorgMarker::Unmined);
    let verdict = arcade_verdict(&ev.tx_status);
    let offered_path = ev.merkle_path.as_deref().is_some_and(|p| !p.is_empty());
    let action = if unmined {
        IngestAction::Reask
    } else if verdict == ArcadeVerdict::Invalid {
        IngestAction::Refused(format!(
            "txStatus {:?} is not a word Arcade defines: nothing applied",
            ev.tx_status
        ))
    } else if verdict == ArcadeVerdict::Mined && offered_path {
        // The toolbox returns early, with no re-ask, only when it stored the
        // path; any other outcome falls back to the re-ask.
        if reask {
            IngestAction::ProofRejected(PROOF_NOT_ACCEPTED.into())
        } else {
            IngestAction::ProofIngested
        }
    } else if reask {
        IngestAction::Reask
    } else if updated {
        IngestAction::StatusApplied
    } else {
        IngestAction::StatusIgnored
    };

    match &action {
        IngestAction::Refused(reason) => {
            tracing::warn!(txid = %txid, reason = %reason, "arc-callback: refused")
        }
        IngestAction::ProofRejected(reason) => tracing::warn!(
            txid = %txid,
            status = %ev.tx_status,
            reason = %reason,
            "arc-callback: proof not stored"
        ),
        _ => tracing::info!(
            txid = %txid,
            status = %ev.tx_status,
            extra_info = ?ev.extra_info,
            code = ?ev.status_code,
            action = ?action,
            "arc-callback: Arcade event applied by the toolbox"
        ),
    }
    Ok(action)
}
