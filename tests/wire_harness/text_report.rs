//! The dependents gate's text report: a refusal whose only difference is
//! its top-level JSON `message` is one `TEXT-DIFF: ` line, not a failure
//! (`WIRE_TEXT_DIFF=report` in `tests/wire.rs`; see `docs/WIRE.md`).

use std::collections::BTreeSet;

use super::fixture::Fixture;
use crate::wire_support::{Resp, J};

/// Keep the ordered body, status and contract headers. The internalize
/// note in MANIFEST names the offset and script reason: that entire detail
/// after `spend refused at offset ` must also stay equal.
pub(crate) fn message_difference<'a>(want: &'a Resp, got: &'a Resp) -> Option<(&'a str, &'a str)> {
    if !(400..600).contains(&want.status) || want.text.is_some() || got.text.is_some() {
        return None;
    }
    let expected = want.json.as_ref()?;
    let actual = got.json.as_ref()?;
    if expected.get("code")?.as_str()? != actual.get("code")?.as_str()? {
        return None;
    }
    let expected = expected.get("message")?.as_str()?;
    let actual = actual.get("message")?.as_str()?;
    let detail = |s: &'a str| {
        s.split_once("spend refused at offset ")
            .map(|(_, tail)| tail)
    };
    if expected == actual || detail(expected) != detail(actual) {
        return None;
    }
    let mut same = want.clone();
    same.json
        .as_mut()?
        .set_pointer("/message", J::Str(actual.to_string()));
    (same == *got).then_some((expected, actual))
}

pub(crate) fn text_diff_line(file: &str, expected: &str, actual: &str) -> String {
    format!(
        "TEXT-DIFF: {file} {} {}",
        serde_json::to_string(expected).unwrap(),
        serde_json::to_string(actual).unwrap()
    )
}

/// Recording comparison keeps even the file's whitespace and envelope.
/// Only the JSON-quoted message may be replaced in the committed bytes.
pub(crate) fn recording_difference(file: &str, want: &[u8], got: &[u8]) -> Option<String> {
    let expected: Fixture = serde_json::from_slice(want).ok()?;
    let actual: Fixture = serde_json::from_slice(got).ok()?;
    let (expected, actual) = message_difference(&expected.response, &actual.response)?;
    let replaced = std::str::from_utf8(want).ok()?.replacen(
        &serde_json::to_string(expected).unwrap(),
        &serde_json::to_string(actual).unwrap(),
        1,
    );
    (replaced.as_bytes() == got).then(|| text_diff_line(file, expected, actual))
}

/// The test runner captures the print macros of a passing test, not a
/// direct write: the marker goes straight to stderr, which the gate reads.
/// The replay and fresh recording report the same row only once.
pub(crate) fn print_text_diffs(lines: &[String]) {
    static PRINTED: std::sync::Mutex<BTreeSet<String>> = std::sync::Mutex::new(BTreeSet::new());
    let mut printed = PRINTED.lock().unwrap();
    for line in lines {
        if printed.insert(line.clone()) {
            write_text_diff(&mut std::io::stderr().lock(), line).unwrap();
        }
    }
}

/// A newline on each side: the marker never shares a line with the test
/// runner's own `test wire ... ` output, so `^TEXT-DIFF: ` finds it.
pub(crate) fn write_text_diff(out: &mut impl std::io::Write, line: &str) -> std::io::Result<()> {
    out.write_all(format!("\n{line}\n").as_bytes())?;
    out.flush()
}

/// The marker's exact bytes: a newline before and after the line, so it
/// starts a line whatever the test runner wrote before it.
#[test]
fn a_text_difference_is_written_on_its_own_line() {
    const FILE: &str = "066-arc-callback.badJson.json";
    let line = text_diff_line(FILE, "was \"x\"", "now");
    let mut out = Vec::new();
    write_text_diff(&mut out, &line).unwrap();
    assert_eq!(
        out,
        format!("\nTEXT-DIFF: {FILE} \"was \\\"x\\\"\" \"now\"\n").into_bytes()
    );
    let mut shared = b"test wire ... ".to_vec();
    write_text_diff(&mut shared, &line).unwrap();
    let shared = String::from_utf8(shared).unwrap();
    assert_eq!(
        shared
            .lines()
            .filter(|l| l.starts_with("TEXT-DIFF: "))
            .count(),
        1,
        "{shared}"
    );
}
