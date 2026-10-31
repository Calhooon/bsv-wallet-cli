//! The worlds' names: a fixture names one, and the replay refuses any other.

/// The names of the worlds (see [`super::worlds::World::build`]).
pub(crate) const WORLDS: [&str; 19] = [
    "fresh",
    "certificate-privacy",
    "auth",
    "callback",
    "tracked",
    "received",
    "headers-down",
    "broken",
    "broken-host",
    "busy",
    "busy-read",
    "funded",
    "pin-spend",
    "review-race",
    "named-change",
    "abort-unproven",
    "tracker-fault",
    "trusted",
    "stranger",
];

/// A world's name is one of [`WORLDS`]; any other is a panic naming it.
pub(crate) fn known_world(name: &str) {
    assert!(
        WORLDS.contains(&name),
        "unknown world `{name}`: the worlds are {}",
        WORLDS.join(", ")
    );
}
