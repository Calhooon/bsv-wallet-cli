//! #540 (the keeper's refinement of 2026-11-03, beside W15): the version
//! door answers BRC-100's `[vendor]-[major].[minor].[patch]` within 7 to 30
//! bytes (`wallet/0100.md:2671` at `5119a9b`) and nothing of the build.

use super::super::WALLET_VERSION;

/// Whatever the package's version reads (a `-beta.6` pre-release
/// included), the constant is the package's name and its version's core,
/// and carries no pre-release identifier, no build metadata, no commit sha
/// and no toolchain.
#[test]
fn the_version_door_is_brc100s_shape_and_names_nothing_of_the_build() {
    let core = env!("CARGO_PKG_VERSION")
        .split(['-', '+'])
        .next()
        .unwrap_or_default();
    assert_eq!(WALLET_VERSION, format!("{}-{core}", env!("CARGO_PKG_NAME")));
    assert!((7..=30).contains(&WALLET_VERSION.len()), "{WALLET_VERSION}");
    let numbers: Vec<u64> = core.split('.').map(|n| n.parse().unwrap()).collect();
    assert_eq!(
        numbers.len(),
        3,
        "{WALLET_VERSION}: [major].[minor].[patch]"
    );
    let vendor = &WALLET_VERSION[..WALLET_VERSION.len() - core.len() - 1];
    assert!(
        vendor.bytes().all(|b| b.is_ascii_lowercase() || b == b'-'),
        "{vendor}"
    );
    let pre = env!("CARGO_PKG_VERSION_PRE");
    assert!(
        pre.is_empty() || !WALLET_VERSION.contains(pre),
        "{pre} leaked"
    );
    for word in [
        "beta", "alpha", "rc", "+", "rustc", "nightly", "stable", "toolbox",
    ] {
        assert!(!core.contains(word), "{WALLET_VERSION}: names {word}");
    }
    // A sha is a run of seven or more hex digits holding a letter.
    let sha = WALLET_VERSION
        .split(|c: char| !c.is_ascii_hexdigit())
        .any(|run| run.len() >= 7 && run.bytes().any(|b| b.is_ascii_alphabetic()));
    assert!(!sha, "{WALLET_VERSION}: a sha");
}
