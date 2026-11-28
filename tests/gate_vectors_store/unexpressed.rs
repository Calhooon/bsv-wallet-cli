//! The rows of the wallet-gate vectors the CLI cannot hold
//! (`tests/gate_vectors.rs`), by class, each with why: exact, a row that
//! leaves its class, or a listed key no row has, fails the replay, and so
//! does a class whose count moves. 62 rows at this pin.

use super::gate_vectors_store::{b, n, s};
use serde_json::Value;

/// The classes: the name the replay prints, why, and the rows' keys.
pub(crate) const UNEXPRESSED: &[(&str, &str, &[&str])] = &[
    (
        "abandoned",
        ABANDONED,
        &[
            "WalletGate.contendedSteps | step 5 | T1.0 | allow_received false",
            "WalletGate.contendedSteps | step 5 | T1.0 | allow_received true",
            "Wallet.Check.heldGivenUp | step 2 | B.0 | allow_received false",
            "Wallet.Check.heldGivenUp | step 2 | B.0 | allow_received true",
            "Wallet.Default.Check.abortedParentChild | step 4 | T.0 | allow_received false",
            "Wallet.Default.Check.abortedParentChild | step 4 | T.0 | allow_received true",
        ],
    ),
    (
        "mined above the tip",
        ABOVE_TIP,
        &[
            "Wallet.Check.depth_floor | step 1 | F.0 | allow_received false",
            "Wallet.Check.depth_floor | step 1 | F.0 | allow_received true",
        ],
    ),
    (
        "handed",
        HANDED,
        &[
            "Wallet.Forms (cancel at handed) | step 6 | T.0 | allow_received false",
            "Wallet.Forms (cancel at handed) | step 6 | T.0 | allow_received true",
            "Wallet.Forms (cancel at handed) | step 7 | T.0 | allow_received false",
            "Wallet.Forms (cancel at handed) | step 7 | T.0 | allow_received true",
            "Wallet.Forms (cancel at handed) | step 8 | T.0 | allow_received false",
            "Wallet.Forms (cancel at handed) | step 8 | T.0 | allow_received true",
            "Wallet.Forms (same seller at handed) | step 6 | T.0 | allow_received false",
            "Wallet.Forms (same seller at handed) | step 6 | T.0 | allow_received true",
            "Wallet.Forms (same seller at handed) | step 7 | T.0 | allow_received false",
            "Wallet.Forms (same seller at handed) | step 7 | T.0 | allow_received true",
            "Wallet.Handed (our own change) | step 3 | T.0 | allow_received false",
            "Wallet.Handed (our own change) | step 3 | T.0 | allow_received true",
            "Wallet.Handed (a received coin) | step 2 | B.0 | allow_received false",
            "Wallet.Handed (a received coin) | step 2 | B.0 | allow_received true",
        ],
    ),
    (
        "held door",
        HELD_DOOR,
        &[
            "Wallet.Check.heldSeen (the word, mined) | step 1 | B.0 | allow_received false",
            "Wallet.Check.heldSeen (the word, mined) | step 1 | B.0 | allow_received true",
            "Wallet.Check.heldSeen (the word, mined) | step 2 | B.0 | allow_received false",
            "Wallet.Check.heldSeen (the word, mined) | step 2 | B.0 | allow_received true",
            "Wallet.Check.heldSeen (the word, mined) | step 3 | B.0 | allow_received false",
            "Wallet.Check.heldSeen (the word, mined) | step 3 | B.0 | allow_received true",
            "Wallet.Check.heldSeen (the word, mined) | step 4 | B.0 | allow_received false",
            "Wallet.Check.heldSeen (the word, mined) | step 4 | B.0 | allow_received true",
            "Wallet.Check.heldProofThenReorg | step 1 | B.0 | allow_received false",
            "Wallet.Check.heldProofThenReorg | step 1 | B.0 | allow_received true",
            "Wallet.Check.heldProofThenReorg | step 2 | B.0 | allow_received false",
            "Wallet.Check.heldProofThenReorg | step 2 | B.0 | allow_received true",
            "Wallet.Check.heldProofThenReorg | step 3 | B.0 | allow_received false",
            "Wallet.Check.heldProofThenReorg | step 3 | B.0 | allow_received true",
            "Wallet.Check.heldProofThenReorg | step 4 | B.0 | allow_received false",
            "Wallet.Check.heldProofThenReorg | step 4 | B.0 | allow_received true",
            "Wallet.Check.heldGivenUp | step 1 | B.0 | allow_received false",
            "Wallet.Check.heldGivenUp | step 1 | B.0 | allow_received true",
        ],
    ),
    (
        "over an unproven ancestor",
        OVER_UNPROVEN,
        &[
            "Wallet.Default.Check.cutOverUnproven | step 1 | B.0 | allow_received false",
            "Wallet.Default.Check.cutOverUnproven | step 1 | B.0 | allow_received true",
            "Wallet.Default.Check.cutOverUnproven | step 2 | B.0 | allow_received false",
            "Wallet.Default.Check.cutOverUnproven | step 2 | B.0 | allow_received true",
            "Wallet.Default.Check.cutOverUnproven | step 3 | B.0 | allow_received false",
            "Wallet.Default.Check.cutOverUnproven | step 3 | B.0 | allow_received true",
        ],
    ),
    (
        "noSend creator",
        NO_SEND,
        &[
            "Wallet.Default.Check.noSendT (named by a noSend action) | step 2 | T.0 | allow_received false",
            "Wallet.Default.Check.noSendT (named by a noSend action) | step 2 | T.0 | allow_received true",
            "Wallet.Default.Check.noSendT (named by a noSend action) | step 3 | T.0 | allow_received false",
            "Wallet.Default.Check.noSendT (named by a noSend action) | step 3 | T.0 | allow_received true",
            "Wallet.Default.Check.noSendT (named by a noSend action) | step 4 | T.0 | allow_received false",
            "Wallet.Default.Check.noSendT (named by a noSend action) | step 4 | T.0 | allow_received true",
            "Wallet.Default.Check.noSendT (named by a noSend action) | step 4 | U.0 | allow_received false",
            "Wallet.Default.Check.noSendT (named by a noSend action) | step 4 | U.0 | allow_received true",
        ],
    ),
    (
        "released lock",
        RELEASED_LOCK,
        &[
            "Wallet.Forms (dead child: the offer) | step 6 | G.0 | allow_received false",
            "Wallet.Forms (dead child: the offer) | step 6 | G.0 | allow_received true",
            "Wallet.Forms (dead child: the plain release) | step 6 | G.0 | allow_received false",
            "Wallet.Forms (dead child: the plain release) | step 6 | G.0 | allow_received true",
        ],
    ),
    (
        "door-born contest",
        CONTEST,
        &[
            "Wallet.Contested (passed whole, the spender dead) | step 2 | F.0 | allow_received false",
            "Wallet.Contested (passed whole, the spender dead) | step 2 | F.0 | allow_received true",
            "Wallet.Contested (passed whole, the spender proven) | step 2 | F.0 | allow_received false",
            "Wallet.Contested (passed whole, the spender proven) | step 2 | F.0 | allow_received true",
        ],
    ),
];

const ABANDONED: &str = "the creator's word is abandoned: the CLI's tracker row stores no \
     abandon (HostAction::Abandon is never kept or replayed by tracker_host), so no store \
     reads `abandoned`; an action the host gives up is the toolbox's `failed` status, which \
     the allocation never reads as a candidate";

const ABOVE_TIP: &str = "the creator is mined at a height above the headers' tip: the CLI \
     checks the stored proof against the headers on every read and the tracker refuses a proof \
     above the tip, so no store reads `mined` there (the reading is `unknown`)";

const HANDED: &str = "the creator's word is handed: bsv-tracker at the pin has no `handed` \
     word, so `read_creators` cannot read it; the gate's rule already names it \
     (spend_gate::DOOR_WORDS)";

const HELD_DOOR: &str = "the creator was taken in through the held door (`creator_held_door`): \
     no store of this host produces `Action.held`";

const OVER_UNPROVEN: &str = "the creator is a received body over an unproven ancestor \
     (`creator_over_unproven`): the store keeps no mark of it";

const NO_SEND: &str = "the creator is a `noSend` action of ours (`creator_no_send`): the \
     replay's store plants `completed` or `unproven` creators only, and the gate has no \
     conjunct for a `noSend` action's unsent change named by a `noSend` action";

const RELEASED_LOCK: &str = "the coin is locked to an action of a released lineage, which the \
     one function's view reads free (`offerable_one` true where `offerable_cut` is false): the \
     store plants a lock to a live action, never the holder's dead lineage, and the gate \
     releases no lock";

const CONTEST: &str = "the coin is contested by a spender that passed a door of this wallet \
     (`coin_contested`): the store keeps no spender that is not an action of ours";

/// Why the CLI cannot hold this row, or `None`: the class, read from the
/// row's own columns, in the order the classes are named.
pub(crate) fn unexpressed(row: &Value) -> Option<&'static str> {
    let word = s(row, "creator_word");
    if word == Some("abandoned") {
        return Some(ABANDONED);
    }
    if word == Some("mined") && n(row, "creator_proof_height") > n(row, "headers_tip") {
        return Some(ABOVE_TIP);
    }
    if word == Some("handed") {
        return Some(HANDED);
    }
    if b(row, "creator_held_door") == Some(true) {
        return Some(HELD_DOOR);
    }
    if b(row, "creator_over_unproven") == Some(true) {
        return Some(OVER_UNPROVEN);
    }
    if b(row, "creator_no_send") == Some(true) {
        return Some(NO_SEND);
    }
    if s(row, "coin_lock") == Some("locked")
        && b(row, "offerable_one") == Some(true)
        && b(row, "offerable_cut") == Some(false)
    {
        return Some(RELEASED_LOCK);
    }
    if s(row, "coin_contested").is_some() {
        return Some(CONTEST);
    }
    None
}
