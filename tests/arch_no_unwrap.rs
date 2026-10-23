//! Rule A4 (#73): the shipped code does not `unwrap()` or `expect(..)`; a
//! fault is an error the caller sees. Test code (`#[cfg(test)]` items,
//! `tests.rs`, `*_tests.rs`, `src/test_support.rs`) may. Every file that
//! holds one today is listed with its count as a ceiling.

mod arch_support;

use arch_support::{count, hits, judge};

/// `(file, today's count, why)`. Only shrinks.
const ALLOWED: &[(&str, usize, &str)] = &[
    ("src/atomic_beef.rs", 1, "the one leaf of a length-one list"),
    ("src/bin/bsv-wallet-relay.rs", 1, "a literal address parse"),
    ("src/commands/drain.rs", 1, "the txid of a broadcast result"),
    ("src/commands/gift_send.rs", 1, "midnight of a parsed date"),
    ("src/commands/send.rs", 1, "the txid of a broadcast result"),
    ("src/commands/split.rs", 1, "the txid of a broadcast result"),
    (
        "src/gift/covenant.rs",
        1,
        "the last byte of a non-empty push",
    ),
    (
        "src/respend.rs",
        1,
        "the history object checked one line above",
    ),
    ("src/server/mod.rs", 1, "the ctrl-c listener"),
];

#[test]
fn the_shipped_code_does_not_unwrap() {
    let found = hits(|l| count(l.bare, ".unwrap()") + count(l.bare, ".expect("));
    judge("A4: unwrap/expect outside test code", &found, ALLOWED);
}

/// The scanner the six rules share blanks what it should and nothing more.
#[test]
fn the_scan_reads_only_the_shipped_code() {
    let text = r##"fn a() { x.unwrap(); } // y.unwrap()
/* z.unwrap() */ let s = "w.unwrap()"; let r = r#"v.unwrap()"#; let c = '{';
#[cfg(test)]
mod tests { fn t() { if true { q.unwrap(); } } }
#[cfg(test)]
let _ = p.unwrap();
fn b() { k.expect("a {brace"); }
"##;
    let (code, bare) = arch_support::views(text);
    assert_eq!(count(&bare, ".unwrap()"), 1, "{bare}");
    assert_eq!(count(&bare, ".expect("), 1, "{bare}");
    assert_eq!(code.lines().count(), text.lines().count());
    assert!(code.contains("\"w.unwrap()\"") && !code.contains("y.unwrap"));
    assert!(bare.contains("fn b()"), "{bare}");

    let mut paths = Vec::new();
    for f in [
        "src/lib.rs",
        "src/main.rs",
        "src/chain_feed.rs",
        "src/spend_gate.rs",
        "src/server/handlers.rs",
    ] {
        paths.push(arch_support::root().join(f));
    }
    let tests: Vec<String> = arch_support::test_files(&paths)
        .iter()
        .map(|p| {
            p.strip_prefix(arch_support::root())
                .unwrap()
                .display()
                .to_string()
        })
        .collect();
    for f in [
        "src/test_support.rs",
        "src/tracker_words_witnesses.rs",
        "src/chain_feed_tests.rs",
        "src/spend_gate/tests.rs",
        "src/server/handlers/tests.rs",
    ] {
        assert!(
            tests.iter().any(|t| t == f),
            "{f} not seen as test code: {tests:?}"
        );
    }
}
