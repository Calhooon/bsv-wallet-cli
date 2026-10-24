//! Rule A4 (#73): the shipped code does not `unwrap()` or `expect(..)`; a
//! fault is an error the caller sees. Test code (`#[cfg(test)]` items,
//! `tests.rs`, `*_tests.rs`, `src/test_support.rs`) may. Every file that
//! holds one today is listed with its count as a ceiling. A call is counted
//! by its name at an identifier boundary: `x.unwrap(`, `Option::unwrap(x)`
//! and `Result::expect(r, ..)` alike.

mod arch_support;

use arch_support::{calls, calls_any, count, Rule};

/// `(file, today's count, why)`. Only shrinks.
const ALLOWED: &[(&str, usize, &str)] = &[
    ("src/atomic_beef.rs", 1, "the one leaf of a length-one list"),
    ("src/bin/bsv-wallet-relay.rs", 1, "a literal address parse"),
    ("src/commands/gift_send.rs", 1, "midnight of a parsed date"),
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
];

const RULE: Rule = Rule {
    name: "A4: unwrap/expect outside test code",
    homes: &[],
    allowed: ALLOWED,
    sites: |l| calls_any(l, &["unwrap", "expect"]),
};

#[test]
fn the_shipped_code_does_not_unwrap() {
    RULE.check();
}

/// The scanner the six rules share blanks comments, string contents and
/// test items, and nothing more.
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
}

#[test]
fn a_test_member_ends_at_its_own_end() {
    // An attribute on a field, a variant or a match arm ends with that
    // member, never past the brace that closes its holder: the arm after a
    // test arm is still read, and an item's own commas are passed.
    let text = r"struct S { #[cfg(test)] a: u8, b: u8 }
enum E { A, #[cfg(test)] B(u8, u8), C }
enum F { A, #[cfg(test)] B }
fn m(x: E) -> u8 { match x {
    #[cfg(test)]
    E::B(n, _) => n.unwrap(),
    _ => y.unwrap(),
} }
fn n(x: E) -> u8 { match x { E::A => 1, #[cfg(test)] _ => z.unwrap() } }
#[cfg(test)]
fn t<A, B>(a: A, b: B) { w.unwrap() }
#[cfg(test)]
let f = |v| { v.unwrap() };
fn o() { u.unwrap() }
";
    let (code, bare) = arch_support::views(text);
    assert_eq!(calls(&bare, "unwrap("), 2, "{bare}");
    for kept in [
        "b: u8 }",
        "C }",
        "enum F { A,",
        "_ => y.unwrap(),",
        "fn o() { u.unwrap() }",
    ] {
        assert!(bare.contains(kept), "{kept} lost: {bare}");
    }
    assert_eq!(code.lines().count(), text.lines().count());
}

#[test]
fn a_call_is_its_name_at_a_boundary() {
    // A call is its name at an identifier boundary, not a definition.
    let line = "x.unwrap(); Option::unwrap(y); unwrap(z); my_unwrap(w); fn unwrap(a: A) {}";
    assert_eq!(calls(line, "unwrap("), 3, "{line}");
    let line = "use tokio::task::{spawn as go, spawn_blocking}; use std::thread::spawn;";
    assert_eq!(arch_support::words(line, "spawn"), 2, "{line}");
}

#[test]
fn test_files_are_declared_in_code() {
    // A test module declared in a comment or a string is not a test file;
    // one declared in code is, at its `#[path]` when it has one.
    let text = "/*\n#[cfg(test)]\nmod spend_gate;\n*/\nlet s = \"#[cfg(test)] mod spend_gate;\";\n\
#[cfg(test)]\n#[path = \"spend_gate/tests.rs\"]\nmod gate_tests;\n\
#[cfg(test)]\npub(crate) mod test_support;\n#[cfg(test)]\nmod tests {\n    mod inner;\n}\n";
    let (code, bare) = arch_support::lexed(text);
    let root = arch_support::root();
    let declared: Vec<String> =
        arch_support::declared_test_files(&root.join("src/lib.rs"), &code, &bare)
            .iter()
            .map(|p| p.strip_prefix(&root).unwrap().display().to_string())
            .collect();
    assert_eq!(declared, ["src/spend_gate/tests.rs", "src/test_support.rs"]);

    let mut paths = Vec::new();
    for f in [
        "src/lib.rs",
        "src/main.rs",
        "src/chain_feed.rs",
        "src/spend_gate.rs",
        "src/server/handlers.rs",
    ] {
        paths.push(root.join(f));
    }
    let tests: Vec<String> = arch_support::test_files(&paths)
        .iter()
        .map(|p| p.strip_prefix(&root).unwrap().display().to_string())
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
