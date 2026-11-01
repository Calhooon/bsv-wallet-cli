//! `fuzz_targets/allow_pending.rs`'s property.

use bsv_wallet_cli::spend_gate::{parse_outpoint, AllowPending};

fn check_word(word: &AllowPending) {
    if let AllowPending::ReceivedOnly(coins) = word {
        assert!(!coins.is_empty(), "an empty scope");
        assert!(
            coins.windows(2).all(|w| w[0] < w[1]),
            "a scope unsorted or repeated"
        );
        for (txid, vout) in coins {
            assert!(word.reaches(txid, *vout));
            assert!(
                txid.len() == 64 && txid.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f')),
                "a named txid that is not 64 lowercase hex: {txid:?}"
            );
        }
    }
    if !word.is_on() {
        assert!(word.named().is_empty());
    }
}

pub fn check(data: &[u8]) {
    if let Ok(value) = serde_json::from_slice::<serde_json::Value>(data) {
        if let Ok(word) = AllowPending::from_json(Some(&value)) {
            check_word(&word);
        }
        // As the body field it is: `{"allowPending": <value>}`.
        if let Some(field) = value.get("allowPending") {
            if let Ok(word) = AllowPending::from_json(Some(field)) {
                check_word(&word);
            }
        }
    }
    if let Ok(text) = std::str::from_utf8(data) {
        if let Ok((txid, vout)) = parse_outpoint(text) {
            assert_eq!(parse_outpoint(&format!("{txid}:{vout}")), Ok((txid, vout)));
        }
        let coins: Vec<String> = text.lines().map(str::to_string).collect();
        for word in [None, Some("received"), Some(text)] {
            if let Ok(scope) = AllowPending::from_cli(word, &coins) {
                check_word(&scope);
            }
        }
    }
}
