//! `sanitize_lone_surrogates` (moved from `mod.rs` unchanged).

use super::sanitize_lone_surrogates;

#[test]
fn lone_lead_and_lone_trail_become_fffd_and_pairs_survive() {
    let lead = br#"{"keyID":"ab \ud83d cd"}"#;
    assert_eq!(
        &*sanitize_lone_surrogates(lead),
        br#"{"keyID":"ab \ufffd cd"}"#
    );
    let trail = br#"{"keyID":"\udc00"}"#;
    assert_eq!(&*sanitize_lone_surrogates(trail), br#"{"keyID":"\ufffd"}"#);
    let pair = br#"{"keyID":"\ud83d\ude00"}"#;
    assert!(matches!(
        sanitize_lone_surrogates(pair),
        std::borrow::Cow::Borrowed(_)
    ));
    let plain = br#"{"keyID":"plain \\u0041"}"#;
    assert!(matches!(
        sanitize_lone_surrogates(plain),
        std::borrow::Cow::Borrowed(_)
    ));
    let fixed = sanitize_lone_surrogates(lead);
    let v: serde_json::Value = serde_json::from_slice(&fixed).unwrap();
    assert_eq!(v["keyID"], "ab \u{fffd} cd");
}
