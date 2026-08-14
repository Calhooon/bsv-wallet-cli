//! PERMISSION AUDIT — record what a REAL wallet would have prompted the user for.
//!
//! This daemon grants everything silently, which is what makes it usable
//! headlessly — and also what makes it blind. A BRC-100 wallet with a human
//! behind it prompts on two axes:
//!
//!   • PROTOCOL (BRC-43): security level 1 asks once per protocol, level 2 once
//!     per protocol AND counterparty. Level 0 never asks. An app can pre-grant
//!     these in its `manifest.json` (BRC-73 `groupPermissions.protocolPermissions`)
//!     — but only for a level-2 counterparty it can name in advance.
//!   • SPENDING: every action that moves satoshis, unless the app declared a
//!     `spendingAuthorization` budget.
//!
//! An app can therefore be correct, fast, fully tested — and still interrupt a
//! player a dozen times a hand, with nothing in any test suite noticing. Worse,
//! a dismissed prompt fails SILENTLY at the call site, so a missing manifest
//! entry looks like a mysteriously absent marker rather than a permission bug
//! (bsv-low #386 was found exactly that way).
//!
//! So: record the raw facts here and let the CALLER judge them. This module
//! deliberately does NOT know what a manifest is or which protocols are
//! pre-granted — that belongs with the app being tested, which owns its own
//! manifest. Here we only answer "what was asked for, in what order, at what
//! level, against which counterparty, for how many satoshis".
//!
//! Process-global on purpose: this daemon serves exactly one wallet
//! (`bsv-wallet serve --db … --port N`), and threading a sink through every
//! handler signature would be a large diff for no extra fidelity.

use serde::{Deserialize, Serialize};
use std::sync::{Mutex, OnceLock};

/// One permissioned request, as the wallet received it.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuditEntry {
    /// BRC-100 method: `createSignature`, `createAction`, `getPublicKey`, …
    pub method: String,
    /// BRC-43 security level, when the call carries a protocol ID.
    #[serde(rename = "protocolLevel", skip_serializing_if = "Option::is_none")]
    pub protocol_level: Option<u8>,
    #[serde(rename = "protocolName", skip_serializing_if = "Option::is_none")]
    pub protocol_name: Option<String>,
    /// `self` / `anyone` / a 66-hex public key — what level 2 scopes on.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub counterparty: Option<String>,
    #[serde(rename = "keyID", skip_serializing_if = "Option::is_none")]
    pub key_id: Option<String>,
    /// Satoshis the caller asked to move (sum of requested outputs). `None` for
    /// non-spending calls; `Some(0)` for a fee-only action such as an
    /// OP_RETURN marker, which still costs a miner fee and still prompts.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub satoshis: Option<u64>,
    /// The requesting app (Origin / Originator header).
    pub originator: String,
    /// Caller-supplied description, which is the text a wallet shows a human.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Monotonic sequence — ordering matters when reading a hand back.
    pub seq: u64,
}

fn log() -> &'static Mutex<Vec<AuditEntry>> {
    static LOG: OnceLock<Mutex<Vec<AuditEntry>>> = OnceLock::new();
    LOG.get_or_init(|| Mutex::new(Vec::new()))
}

/// Cap the ring so a long-lived daemon cannot grow without bound. A hand costs
/// on the order of ten entries; this holds thousands of them.
const MAX_ENTRIES: usize = 5000;

/// Record one permissioned request. Never panics and never fails a request —
/// an audit that can break the wallet it observes is worse than no audit.
pub fn record(mut entry: AuditEntry) {
    if let Ok(mut guard) = log().lock() {
        entry.seq = guard.len() as u64;
        if guard.len() >= MAX_ENTRIES {
            guard.remove(0);
        }
        guard.push(entry);
    }
}

/// Everything recorded since start or the last `reset`.
pub fn snapshot() -> Vec<AuditEntry> {
    log().lock().map(|g| g.clone()).unwrap_or_default()
}

/// Clear the log — a test calls this immediately before the flow it measures.
pub fn reset() {
    if let Ok(mut guard) = log().lock() {
        guard.clear();
    }
}

/// Build an entry for a protocol-bearing call.
pub fn protocol_entry(
    method: &str,
    level: u8,
    name: &str,
    counterparty: Option<String>,
    key_id: Option<String>,
    originator: &str,
) -> AuditEntry {
    AuditEntry {
        method: method.to_string(),
        protocol_level: Some(level),
        protocol_name: Some(name.to_string()),
        counterparty,
        key_id,
        satoshis: None,
        originator: originator.to_string(),
        description: None,
        seq: 0,
    }
}

/// Build an entry for a spending call.
pub fn spend_entry(method: &str, satoshis: u64, description: Option<String>, originator: &str) -> AuditEntry {
    AuditEntry {
        method: method.to_string(),
        protocol_level: None,
        protocol_name: None,
        counterparty: None,
        key_id: None,
        satoshis: Some(satoshis),
        originator: originator.to_string(),
        description,
        seq: 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The log is process-global by design, so these cells would otherwise
    /// race each other under vitest-style parallelism and fail for a reason
    /// that has nothing to do with the code. Serialize them explicitly rather
    /// than tuning the order and hoping.
    fn serial() -> std::sync::MutexGuard<'static, ()> {
        static GUARD: OnceLock<Mutex<()>> = OnceLock::new();
        GUARD
            .get_or_init(|| Mutex::new(()))
            .lock()
            .unwrap_or_else(|e| e.into_inner())
    }

    #[test]
    fn records_in_order_and_resets() {
        let _s = serial();
        reset();
        record(protocol_entry("createSignature", 2, "low settle", Some("02ab".into()), None, "low.game"));
        record(spend_entry("createAction", 20_000, Some("LOW pot JOIN funding hop".into()), "low.game"));
        let s = snapshot();
        assert_eq!(s.len(), 2);
        assert_eq!(s[0].seq, 0);
        assert_eq!(s[0].protocol_level, Some(2));
        assert_eq!(s[1].seq, 1);
        assert_eq!(s[1].satoshis, Some(20_000));
        reset();
        assert!(snapshot().is_empty());
    }

    #[test]
    fn the_ring_is_bounded() {
        let _s = serial();
        reset();
        for _ in 0..(MAX_ENTRIES + 10) {
            record(spend_entry("createAction", 1, None, "low.game"));
        }
        assert_eq!(snapshot().len(), MAX_ENTRIES);
        reset();
    }
}
