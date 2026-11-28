//! The offer rule per coin kind, creator word, setting and action word
//! (#659): the table of the stack's one function (`offerOne`,
//! `lean/Wallet/OneFunction.lean`) under the cut's default and of
//! `offerableD` under `Setting.hold`, as [`hold_reason`] answers it. The
//! gate vectors replay the same rule over the stack's rows
//! (`tests/gate_vectors.rs`); this table names every word, `handed`
//! included, which no store of this host reads yet.

use super::*;

const TXID: &str = "abababababababababababababababababababababababababababababababab";
const OTHER: &str = "cdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcd";
const NO_WORD: &str = "depth 1: 0 blocks; received, no proof; offered under allowPending: received";
const NOT_NAMED: &str =
    "depth 1: 0 blocks; received, no proof; not named under allowPending: received";

/// A reading of `word` with no proof and no fault.
fn reading(word: &'static str) -> CreatorReading {
    CreatorReading {
        word,
        depth: 0,
        suspect: false,
        door: false,
        fault: None,
        headers_fault: false,
    }
}

fn gate(received: ReceivedSetting, own_change: bool, word: AllowPending) -> GateOptions {
    GateOptions {
        received,
        ..opts(1, own_change).with_allow_pending(word)
    }
}

fn held(word: &'static str, origin: Origin, gate: &GateOptions) -> Option<String> {
    hold_reason(&reading(word), origin, (TXID, 0), gate)
}

/// The action's words: none, plain, scoped naming the coin, scoped naming
/// another.
fn words() -> [AllowPending; 4] {
    [
        AllowPending::Off,
        AllowPending::Received,
        AllowPending::ReceivedOnly(vec![(TXID.to_string(), 0)]),
        AllowPending::ReceivedOnly(vec![(OTHER.to_string(), 0)]),
    ]
}

const SETTINGS: [ReceivedSetting; 2] = [ReceivedSetting::Taken, ReceivedSetting::Hold];
const TAKEN: [&str; 3] = ["announced", "seen", "stale"];
const NOT_TAKEN: [&str; 4] = ["unknown", "built", "handed", "abandoned"];

/// The table holds every word the door reads, and no other.
#[test]
fn the_door_reads_ten_words_and_the_tracker_nine_of_them() {
    let labels: Vec<&str> = DOOR_WORDS.iter().map(|(label, _)| *label).collect();
    assert_eq!(labels.len(), 10);
    for label in crate::tracker_host::WORD_LABELS {
        assert!(DoorWord::of(label).is_some(), "{label}");
    }
    let not_the_trackers: Vec<&str> = labels
        .iter()
        .copied()
        .filter(|l| !crate::tracker_host::WORD_LABELS.contains(l))
        .collect();
    assert_eq!(not_the_trackers, ["handed"]);
    for (label, word) in DOOR_WORDS {
        assert_eq!(DoorWord::of(label), Some(word));
    }
    assert_eq!(DoorWord::of("Built"), None);
}

/// A label outside the table is held as a fault under every setting, rule
/// and word, for either origin: never offered.
#[test]
fn a_word_the_host_does_not_read_is_never_offered() {
    for received in SETTINGS {
        for word in words() {
            for origin in [Origin::Made, Origin::Received] {
                let gate = gate(received, true, word.clone());
                assert_eq!(
                    held("minted", origin, &gate).as_deref(),
                    Some("the creator's word minted is not one this host reads"),
                    "{origin:?}, {gate:?}"
                );
            }
        }
    }
}

/// Our own change: offered from `built` on under `SPEND_OWN_CHANGE`, under
/// both settings and whatever the action says; never at `unknown` or
/// `abandoned`; held at every word with the rule off.
#[test]
fn our_own_change_is_offered_from_built_on_under_every_setting() {
    for received in SETTINGS {
        for word in words() {
            let on = gate(received, true, word.clone());
            let off = gate(received, false, word);
            for live in ["built", "handed", "announced", "seen", "stale"] {
                assert_eq!(held(live, Origin::Made, &on), None, "{live}, {on:?}");
            }
            for unsigned in ["unknown", "abandoned"] {
                assert_eq!(
                    held(unsigned, Origin::Made, &on),
                    Some(format!("{unsigned}; own change not signed: never offered")),
                    "{unsigned}, {on:?}"
                );
            }
            for any in [
                "unknown",
                "built",
                "handed",
                "announced",
                "seen",
                "abandoned",
            ] {
                assert_eq!(
                    held(any, Origin::Made, &off).as_deref(),
                    Some("depth 1: 0 blocks; own change, SPEND_OWN_CHANGE=0"),
                    "{any}"
                );
            }
            assert_eq!(
                held("stale", Origin::Made, &off).as_deref(),
                Some("stale: its block left the chain; own change, SPEND_OWN_CHANGE=0")
            );
        }
    }
}

/// The default: a received coin a broadcaster took is offered with no word
/// and under the plain word; a scoped word offers it only when it names it.
#[test]
fn the_default_offers_a_received_coin_a_broadcaster_took() {
    let [none, plain, named, other] = words().map(|w| gate(ReceivedSetting::Taken, true, w));
    for taken in TAKEN {
        for offering in [&none, &plain, &named] {
            assert_eq!(
                held(taken, Origin::Received, offering),
                None,
                "{taken}, {offering:?}"
            );
        }
    }
    for taken in ["announced", "seen"] {
        assert_eq!(
            held(taken, Origin::Received, &other).as_deref(),
            Some(NOT_NAMED)
        );
    }
    assert_eq!(
        held("stale", Origin::Received, &other).as_deref(),
        Some(
            "stale: its block left the chain; received, confirming again; not named under \
             allowPending: received"
        )
    );
}

/// The hold: the same coin is offered only under the action's word.
#[test]
fn the_hold_offers_a_taken_received_coin_only_under_the_word() {
    let [none, plain, named, other] = words().map(|w| gate(ReceivedSetting::Hold, true, w));
    for taken in TAKEN {
        for offering in [&plain, &named] {
            assert_eq!(held(taken, Origin::Received, offering), None, "{taken}");
        }
    }
    for taken in ["announced", "seen"] {
        assert_eq!(
            held(taken, Origin::Received, &none).as_deref(),
            Some(NO_WORD)
        );
        assert_eq!(
            held(taken, Origin::Received, &other).as_deref(),
            Some(NOT_NAMED)
        );
    }
    assert_eq!(
        held("stale", Origin::Received, &none).as_deref(),
        Some(
            "stale: its block left the chain; received, confirming again; offered under \
             allowPending: received"
        )
    );
}

/// A received coin no broadcaster took is offered under no setting and no
/// word: under the default, and under the hold with the word, it is held
/// until a broadcaster takes it or it is mined; under the hold without the
/// word, and under a scope that does not name it, for want of the word.
#[test]
fn a_received_coin_no_broadcaster_took_is_never_offered() {
    for received in SETTINGS {
        let [none, plain, named, other] = words().map(|w| gate(received, true, w));
        for word in NOT_TAKEN {
            let not_taken = format!(
                "{word}; received, not taken by a broadcaster: offered once a broadcaster \
                 takes it or once mined"
            );
            for with_the_word in [&plain, &named] {
                assert_eq!(
                    held(word, Origin::Received, with_the_word),
                    Some(not_taken.clone()),
                    "{word}, {received:?}"
                );
            }
            assert_eq!(
                held(word, Origin::Received, &none),
                Some(match received {
                    ReceivedSetting::Taken => not_taken.clone(),
                    ReceivedSetting::Hold => NO_WORD.to_string(),
                }),
                "{word}, {received:?}"
            );
            assert_eq!(
                held(word, Origin::Received, &other).as_deref(),
                Some(NOT_NAMED),
                "{word}, {received:?}"
            );
        }
    }
}

/// The chain's words are the setting's to no one: `mined` below the depth
/// and the dead words read as before under both settings.
#[test]
fn the_chain_words_read_alike_under_both_settings() {
    for received in SETTINGS {
        for word in words() {
            let gate = gate(received, true, word);
            for origin in [Origin::Made, Origin::Received] {
                assert_eq!(
                    held("mined", origin, &gate).as_deref(),
                    Some("depth 1: 0 blocks")
                );
                for dead in ["rejected", "conflicted"] {
                    assert_eq!(
                        held(dead, origin, &gate),
                        Some(format!("{dead}: never offered"))
                    );
                }
            }
        }
    }
}

/// The allocation under the default: of two received coins a broadcaster
/// took, both are offered with no word; a scoped word naming one offers it
/// alone and holds the other, not named. A received coin with no word of
/// its own (`unknown`) is held beside them.
#[tokio::test]
async fn the_allocation_offers_taken_coins_and_the_scope_still_narrows() {
    let (storage, user, basket) = storage_for(&("02".to_string() + &"ab".repeat(32))).await;
    let (first, second, unheard) = ("a1".repeat(32), "a2".repeat(32), "a3".repeat(32));
    let ids = (user, basket);
    let first_id = coin(&storage, ids, &first, false, None, 5_000, 4).await;
    let second_id = coin(&storage, ids, &second, false, None, 4_000, 4).await;
    coin_unknown(&storage, ids, &unheard, false, None, 3_000, 4).await;
    let services = headers_with(&first, 900, 900);
    let default = GateOptions {
        received: ReceivedSetting::Taken,
        ..opts(1, true)
    };

    let a = assess(UserCoins::new(&storage, user), &services, &default, None)
        .await
        .unwrap();
    assert_eq!(a.offered, vec![(first_id, 5_000), (second_id, 4_000)]);
    assert_eq!(
        reason_of(&a, &unheard),
        Some(format!("unknown; {RECEIVED_NOT_TAKEN}"))
    );

    let scoped = default.with_allow_pending(AllowPending::ReceivedOnly(vec![(first.clone(), 0)]));
    let a = assess(UserCoins::new(&storage, user), &services, &scoped, None)
        .await
        .unwrap();
    assert_eq!(a.offered, vec![(first_id, 5_000)]);
    assert_eq!(reason_of(&a, &second).as_deref(), Some(NOT_NAMED));
    assert_eq!(reason_of(&a, &unheard).as_deref(), Some(NOT_NAMED));
}

/// `GateOptions::default` and the configuration's default are the cut's:
/// `taken`; `SPEND_RECEIVED=hold` is read into the gate's options.
#[test]
fn the_setting_reaches_the_gate_from_the_configuration() {
    use crate::config::Config;
    assert_eq!(GateOptions::default().received, ReceivedSetting::Taken);
    assert_eq!(
        GateOptions::from_config(&Config::default()).received,
        ReceivedSetting::Taken
    );
    let hold = Config::from_pairs(&[("SPEND_RECEIVED", " Hold ")]).unwrap();
    assert_eq!(
        GateOptions::from_config(&hold).received,
        ReceivedSetting::Hold
    );
}
