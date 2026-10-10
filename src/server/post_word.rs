//! The served doors' word for a transaction whose immediate post drew no
//! accepting word.
//!
//! From the toolbox 0.7.4 (bsv-stack-lean #66) a broadcaster's refusal on
//! the immediate post of `createAction`, `signAction` or
//! `internalizeAction` is a hint, never an error: the caller receives the
//! txid with BRC-100's `sending` (or `accepted` from `internalizeAction`),
//! the refusal goes on the proof request's history (`immediateBroadcastHint`:
//! the outcome, the broadcasters' words, the next re-ask), the request is
//! left `unsent` for the re-ask on the cadence, and the transaction's inputs
//! stay locked until a proof, a competitor's checked proof or this wallet's
//! retire. Nothing is released by the refusal.
//!
//! The doors answer with the tracker's word for that state, `built` (no
//! broadcaster took it), the request's status and the hint the toolbox
//! recorded, as `broadcast` beside the toolbox's own fields. A transaction a
//! broadcaster took carries no `broadcast` field: `sendWithResults` already
//! says `unproven`.

use bsv_wallet_toolbox::StorageSqlx;
use serde::Serialize;

/// The note the toolbox 0.7.4 appends to a request's history when the
/// immediate post drew no accepting word.
pub const IMMEDIATE_HINT: &str = "immediateBroadcastHint";

/// What the inputs of a transaction in this state are, in one word.
pub const INPUTS_LOCKED: &str = "locked";

/// The word for a transaction no broadcaster took on its immediate post.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PostWord {
    /// The tracker's word: `built`, a template no broadcaster took.
    pub word: &'static str,
    /// The proof request's status (`unsent` for the re-ask on the cadence).
    pub request: String,
    /// The last immediate-post hint on the request's history, as the toolbox
    /// wrote it (`outcome`, `words`, `attempts`, `nextReaskMinutes`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hint: Option<serde_json::Value>,
    /// `locked`: until a proof, a competitor's checked proof or this
    /// wallet's retire. A refusal releases nothing.
    pub inputs: &'static str,
}

/// The last note of `history` (the toolbox's `{"notes": [...]}`) whose
/// `what` is `what`.
pub fn last_note(history: &str, what: &str) -> Option<serde_json::Value> {
    let h: serde_json::Value = serde_json::from_str(history).ok()?;
    h.get("notes")?
        .as_array()?
        .iter()
        .rev()
        .find(|n| n.get("what").and_then(|w| w.as_str()) == Some(what))
        .cloned()
}

/// The word for `txid` when its immediate post drew no accepting word and
/// no broadcaster took it since; `None` otherwise (no request, a request
/// past the post, a word a broadcaster gave, a read that failed).
pub async fn post_word(storage: &StorageSqlx, txid: &str) -> Option<PostWord> {
    let row: Option<(String, String)> =
        sqlx::query_as("SELECT status, history FROM proven_tx_reqs WHERE txid = ? LIMIT 1")
            .bind(txid)
            .fetch_optional(storage.pool())
            .await
            .ok()?;
    let (request, history) = row?;
    if !matches!(request.as_str(), "unsent" | "sending" | "unprocessed") {
        return None;
    }
    if crate::retire_guard::broadcaster_took(storage.pool(), txid)
        .await
        .unwrap_or(true)
    {
        return None;
    }
    Some(PostWord {
        word: "built",
        request,
        hint: last_note(&history, IMMEDIATE_HINT),
        inputs: INPUTS_LOCKED,
    })
}

/// The subject txid of an Atomic BEEF (BRC-95: the prefix `0x01010101`,
/// then the txid's 32 bytes in reverse order), without parsing the rest.
pub fn atomic_subject(bytes: &[u8]) -> Option<String> {
    if bytes.len() < 36 || bytes[..4] != [0x01, 0x01, 0x01, 0x01] {
        return None;
    }
    let mut txid = bytes[4..36].to_vec();
    txid.reverse();
    Some(hex::encode(txid))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_last_hint_is_read_from_the_toolbox_history() {
        let h = r#"{"notes":[{"what":"immediateBroadcastHint","outcome":"serviceError"},{"what":"other"},{"what":"immediateBroadcastHint","outcome":"invalidTx","nextReaskMinutes":2}]}"#;
        let n = last_note(h, IMMEDIATE_HINT).unwrap();
        assert_eq!(n["outcome"], "invalidTx");
        assert!(last_note("{}", IMMEDIATE_HINT).is_none());
        assert!(last_note("not json", IMMEDIATE_HINT).is_none());
    }

    #[test]
    fn the_subject_of_an_atomic_beef_is_read_from_its_prefix() {
        let mut b = vec![1, 1, 1, 1];
        let mut id: Vec<u8> = (0u8..32).collect();
        b.extend(&id);
        b.extend([2, 0, 0xbe, 0xef]);
        id.reverse();
        assert_eq!(atomic_subject(&b), Some(hex::encode(id)));
        assert_eq!(atomic_subject(&[2, 0, 0xbe, 0xef]), None);
    }
}
