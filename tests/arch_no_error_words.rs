//! Rule A8 (#81): in the served layer (`src/server/`) no error is
//! classified by its words. A refusal's status and code come from its
//! `Kind` (`src/server/error.rs`), reached from a typed error by its
//! variant; a toolbox refusal the `WalletInterface` boundary erases to text
//! is read in `src/toolbox_words.rs`, outside this rule's scope and counted
//! by A5. Counted: a string test (`starts_with(`, `strip_prefix(`,
//! `ends_with(`, `strip_suffix(`, `contains(`, `find(`, `rfind(`,
//! `matches(`, `split_once(`) whose receiver is an error's text: a call of
//! `to_string()` or `message()`, or a binding the served layer gives an
//! error or its words (`e`, `err`, `error`, `msg`, `message`, `clean`,
//! `text`, `detail`). The allowlist is empty and stays so.

mod arch_support;

use arch_support::Rule;

/// The string tests.
const TESTS: &[&str] = &[
    "starts_with",
    "strip_prefix",
    "ends_with",
    "strip_suffix",
    "contains",
    "find",
    "rfind",
    "matches",
    "split_once",
];

/// A receiver that is an error's text: these calls...
const TEXT_CALLS: &[&str] = &["to_string", "message"];

/// ...and these bindings.
const TEXT_NAMES: &[&str] = &[
    "e", "err", "error", "msg", "message", "clean", "text", "detail",
];

/// The rule's scope.
const SCOPE: &str = "src/server/";

/// `(file, today's count, why)`. Empty at landing (#81); it stays so.
/// Written open, as `tools/scoreboard` reads an allowlist.
#[rustfmt::skip]
const ALLOWED: &[(&str, usize, &str)] = &[
];

fn is_ident(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

/// The identifier `before` ends with.
fn trailing_ident(before: &str) -> &str {
    let start = before
        .char_indices()
        .rev()
        .take_while(|(_, c)| is_ident(*c))
        .last()
        .map_or(before.len(), |(i, _)| i);
    &before[start..]
}

/// Whether the expression `before` ends with is an error's text.
fn is_error_text(before: &str) -> bool {
    let before = before.trim_end();
    match before.strip_suffix("()") {
        Some(call) => {
            let name = trailing_ident(call);
            TEXT_CALLS.contains(&name) && call[..call.len() - name.len()].ends_with('.')
        }
        None => TEXT_NAMES.contains(&trailing_ident(before)),
    }
}

/// The string tests on an error's text in one line of code (string
/// literals blanked).
fn error_text_tests(bare: &str) -> usize {
    TESTS
        .iter()
        .map(|name| {
            bare.match_indices(&format!(".{name}("))
                .filter(|(at, _)| is_error_text(&bare[..*at]))
                .count()
        })
        .sum()
}

const RULE: Rule = Rule {
    name: "A8: an error classified by its words in the served layer",
    homes: &[],
    allowed: ALLOWED,
    sites: |l| {
        if l.source.file.starts_with(SCOPE) {
            error_text_tests(l.bare)
        } else {
            0
        }
    },
};

#[test]
fn no_error_is_classified_by_its_words_in_the_served_layer() {
    RULE.check();
}

/// The counter sees the shapes the old prefix table had (the planted lines
/// are `AppError::classify`'s before #81) and leaves the served layer's
/// other string tests alone.
#[test]
fn the_counter_sees_an_error_read_by_its_words() {
    for line in [
        r#"let clean = msg.strip_prefix("wallet error: ").unwrap_or(msg);"#,
        r#"if clean.starts_with("Insufficient funds:") {"#,
        r#"if err.message().contains("the network has transaction") {"#,
        r#"let rest = &message[message.find("Unspendable coin: ")? + 18..];"#,
        r#"e.to_string().contains("insufficient funds")"#,
        r#"if error.ends_with("x") {"#,
    ] {
        let (_, bare) = arch_support::views(line);
        assert_eq!(error_text_tests(&bare), 1, "{line}");
    }
    for line in [
        r#".and_then(|v| v.strip_prefix("Bearer "));"#,
        r"let is_lead = (0xD800..=0xDBFF).contains(&cu);",
        r#".is_some_and(|ct| ct.to_ascii_lowercase().contains("json"));"#,
        r"let cap = if BEEF_DOORS.contains(&parts.uri.path()) {",
        r".and_then(|rs| rs.iter().find(|r| r.txid == txid));",
        r#"let s = "msg.starts_with(x)";"#,
        r#"// clean.starts_with("Insufficient funds:")"#,
        r#"fn message() -> String { mymessage.contains("x") }"#,
    ] {
        let (_, bare) = arch_support::views(line);
        assert_eq!(error_text_tests(&bare), 0, "{line}");
    }
}
