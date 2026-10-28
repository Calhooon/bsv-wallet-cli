//! The wire oracle (#69): every served call and every refusal body, as a
//! recorded request and the exact response, under `tests/wire/`.
//!
//! `cargo test --test wire` replays every fixture against the router,
//! in-process (`tower::ServiceExt::oneshot`, no socket for the wallet), and
//! fails on any byte of difference after normalization, printing the diff.
//! `WIRE_TEXT_DIFF=report` reports only a refusal's message-text differences
//! for the toolbox's dependents gate; the default comparison stays strict.
//! `WIRE_RECORD=1 cargo test --test wire` re-records every fixture from the
//! cases below and the manifest `tests/wire/MANIFEST.json` (the recorded
//! set), then fails with the word to replay; nothing else re-records. See
//! `docs/WIRE.md`.
//!
//! The field names the shapes must keep are BRC-100's at the line's pin,
//! bsv-blockchain/BRCs `5119a9b` (2026-10-19). Ordinary basket lists remain
//! numeric; bsv-rs #105 supplies the module-specific JSON shapes at beta.2.
//! The internalize instruction bound is already recorded by fixture 123.
//!
//! Each fixture names its world (the wallet and the server config it is
//! served from). A world is built once per run from its name alone, and the
//! fixtures of one world run in their file order, so a fixture may read the
//! state an earlier one of its world left (a certificate acquired, then
//! listed). Nothing reaches the network: the header service is a local
//! stub, the broadcaster a closed local port (in `funded`, a local stub
//! that takes every post), and every other request goes through a proxy on
//! a closed local port.

mod wire_harness;
mod wire_support;

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::{Path, PathBuf};

use wire_harness::cases::record_all;
use wire_harness::fixture::{answered_differently, j, normalize, normalize_text, Fixture};
use wire_harness::judge::{app_codes_in_source, judge_coverage, APP_CODES, UNPRODUCED};
use wire_harness::manifest::{judge_manifest, recorded_fixtures, Manifest, MANIFEST};
use wire_harness::recorder::{get, Recorder};
use wire_harness::text_report::{
    message_difference, print_text_diffs, recording_difference, text_diff_line, write_text_diff,
};
use wire_harness::worlds::{known_world, resolve, World};
use wire_support::{on_runtime, read_fixture, Resp, J};

// ---------------------------------------------------------------------------
// Record or replay.
// ---------------------------------------------------------------------------

fn wire_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/wire")
}

/// Whether `WIRE_RECORD` asks for a recording: `1` records, unset
/// replays, and any other value is a panic naming it (a `true` or a `0`
/// would otherwise replay in silence).
fn record_requested() -> bool {
    record_word(std::env::var("WIRE_RECORD").ok().as_deref())
}

fn record_word(value: Option<&str>) -> bool {
    match value {
        None => false,
        Some("1") => true,
        Some(other) => panic!(
            "WIRE_RECORD is `{other}`: `1` records, unset replays, and nothing else is a word"
        ),
    }
}

/// Only the dependents gate opts in. A recording never accepts this word.
fn text_diff_requested(recording: bool) -> bool {
    text_diff_word(std::env::var("WIRE_TEXT_DIFF").ok().as_deref(), recording)
}

fn text_diff_word(value: Option<&str>, recording: bool) -> bool {
    match value {
        None => false,
        Some("report") if !recording => true,
        Some("report") => panic!("WIRE_TEXT_DIFF=report is refused with WIRE_RECORD=1: a recording is never lenient"),
        Some(other) => panic!(
            "WIRE_TEXT_DIFF is `{other}`: `report` reports message-text differences, unset is strict, and nothing else is a word"
        ),
    }
}

#[derive(Debug)]
struct Replay {
    summary: String,
    text_diffs: Vec<String>,
}

/// Record every case into `dir` (the old fixtures and manifest removed
/// before). A recording never passes: the message says to replay it.
async fn record(dir: &Path) -> String {
    std::fs::create_dir_all(dir).unwrap();
    for entry in std::fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        let name = path.file_name().unwrap().to_string_lossy().into_owned();
        if path.extension().is_some_and(|x| x == "json") || name == MANIFEST {
            std::fs::remove_file(path).unwrap();
        }
    }
    bsv_wallet_cli::server::audit::reset();
    let mut recorder = Recorder {
        dir: dir.to_path_buf(),
        worlds: HashMap::new(),
        seq: 0,
        written: Vec::new(),
        served: HashMap::new(),
    };
    record_all(&mut recorder).await;
    recorder.write_manifest();
    format!(
        "recorded {} fixtures and {MANIFEST} under {}. A recording is not a replay: \
         run `cargo test --locked --test wire` again, without WIRE_RECORD, to replay what \
         was recorded (and `git diff tests/wire` to see what moved)",
        recorder.written.len(),
        dir.display()
    )
}

/// Replay every fixture the manifest names: the failures, or the counts.
async fn replay(dir: &Path, report_text: bool) -> Result<Replay, String> {
    // The permission audit is the process's: each pass starts it empty.
    bsv_wallet_cli::server::audit::reset();
    let fixtures = recorded_fixtures(dir)?;
    let mut worlds: HashMap<String, World> = HashMap::new();
    let mut served: HashMap<String, J> = HashMap::new();
    let mut failures = Vec::new();
    let mut text_diffs = Vec::new();
    for (file, fixture) in &fixtures {
        if !worlds.contains_key(&fixture.world) {
            worlds.insert(fixture.world.clone(), World::build(&fixture.world).await);
        }
        let request = resolve(file, &fixture.request, &served);
        let (got, body) = worlds[&fixture.world].serve_raw(&request).await;
        if let Some(body) = body {
            served.insert(file.clone(), body);
        }
        if got != fixture.response {
            if report_text {
                if let Some((expected, actual)) = message_difference(&fixture.response, &got) {
                    text_diffs.push(text_diff_line(file, expected, actual));
                    continue;
                }
            }
            failures.push(answered_differently(file, fixture, &got));
        }
    }
    let missing = judge_coverage(&fixtures);
    let summary = format!(
        "{} fixtures replayed, {} differ, {} coverage gaps, {} text differences reported",
        fixtures.len(),
        failures.len(),
        missing.len(),
        text_diffs.len()
    );
    if failures.is_empty() && missing.is_empty() {
        Ok(Replay {
            summary,
            text_diffs,
        })
    } else {
        Err(format!(
            "{summary}\n{}\n{}\n{}\n(a change on purpose: WIRE_RECORD=1 cargo test --test wire, then a \
             replay, in its own commit; see docs/WIRE.md)",
            text_diffs.join("\n"),
            failures.join("\n"),
            missing.join("\n")
        ))
    }
}

/// The oracle: `WIRE_RECORD=1` records and fails with the word to replay;
/// otherwise every fixture is replayed.
#[test]
fn wire() {
    let dir = wire_dir();
    let recording = record_requested();
    let report_text = text_diff_requested(recording);
    if recording {
        let message = on_runtime(move || async move { record(&dir).await });
        panic!("{message}");
    }
    match on_runtime(move || async move { replay(&dir, report_text).await }) {
        Ok(replayed) => {
            print_text_diffs(&replayed.text_diffs);
            println!("{}", replayed.summary);
        }
        Err(message) => panic!("\n{message}"),
    }
}

// ---------------------------------------------------------------------------
// The planted cases: the judge is judged.
// ---------------------------------------------------------------------------

/// A recording run is not a pass: it writes the set and the manifest, and
/// ends with the word to replay. The second run replays it clean, and the
/// recording is the committed corpus byte for byte (two recordings in a row
/// are identical, and the cases have not drifted from the fixtures).
#[test]
fn a_recording_is_not_a_replay() {
    let report_text = text_diff_requested(record_requested());
    let tmp = tempfile::TempDir::new().unwrap();
    let dir = tmp.path().to_path_buf();
    let message = on_runtime(move || async move { record(&dir).await });
    assert!(message.contains("A recording is not a replay"), "{message}");
    let listed = judge_manifest(tmp.path()).unwrap();
    assert!(listed.len() > 79, "{} fixtures recorded", listed.len());
    assert_eq!(listed, {
        let mut sorted = listed.clone();
        sorted.sort();
        sorted
    });
    let dir = tmp.path().to_path_buf();
    let replayed = on_runtime(move || async move { replay(&dir, report_text).await });
    assert!(replayed.is_ok(), "the second run: {replayed:?}");
    let mut text_diffs = Vec::new();
    for name in listed.iter().map(String::as_str).chain([MANIFEST]) {
        let recorded = std::fs::read(tmp.path().join(name)).unwrap();
        let committed = std::fs::read(wire_dir().join(name)).unwrap_or_default();
        if recorded != committed && report_text && name != MANIFEST {
            if let Some(line) = recording_difference(name, &committed, &recorded) {
                text_diffs.push(line);
                continue;
            }
        }
        assert!(
            recorded == committed,
            "{name}: a fresh recording differs from tests/wire (WIRE_RECORD=1, then replay)"
        );
    }
    print_text_diffs(&text_diffs);
    println!(
        "fresh recording: {} text differences reported",
        text_diffs.len()
    );
}

/// `WIRE_RECORD=1` records, unset replays, and any other value is refused
/// by name.
#[test]
fn only_one_word_records() {
    assert!(!record_word(None));
    assert!(record_word(Some("1")));
    for other in ["", "0", "true", "yes"] {
        let refused = std::panic::catch_unwind(|| record_word(Some(other)));
        let payload = refused.expect_err("a word that is not `1` must be refused");
        let message = payload
            .downcast_ref::<String>()
            .cloned()
            .unwrap_or_default();
        assert!(
            message.contains(&format!("WIRE_RECORD is `{other}`")),
            "{message}"
        );
    }
}

/// The report word is explicit, including when an empty value was set.
#[test]
fn only_one_word_reports_text_differences() {
    assert!(!text_diff_word(None, false));
    assert!(text_diff_word(Some("report"), false));
    for other in ["", "1", "true", "REPORT"] {
        let payload = std::panic::catch_unwind(|| text_diff_word(Some(other), false))
            .expect_err("only `report` may opt in");
        let message = payload.downcast_ref::<String>().unwrap();
        assert!(message.contains(&format!("WIRE_TEXT_DIFF is `{other}`")));
    }
}

/// Refuse the two knobs together before a recording can change any file.
#[test]
fn a_recording_never_reports_text_differences() {
    assert!(!text_diff_word(None, true));
    let payload = std::panic::catch_unwind(|| text_diff_word(Some("report"), true))
        .expect_err("a recording is never lenient");
    let message = payload.downcast_ref::<&str>().unwrap();
    assert!(message.contains("WIRE_TEXT_DIFF=report"));
    assert!(message.contains("WIRE_RECORD=1"));
}

const TEXT_FIXTURE: &str = "066-arc-callback.badJson.json";

/// Each planted response lives in a copy; the committed corpus is untouched.
fn plant_response(change: impl FnOnce(&mut Resp)) -> (tempfile::TempDir, Resp) {
    let tmp = copy_corpus();
    let original = plant_into(tmp.path(), TEXT_FIXTURE, change);
    (tmp, original)
}

fn copy_corpus() -> tempfile::TempDir {
    let tmp = tempfile::TempDir::new().unwrap();
    for entry in std::fs::read_dir(wire_dir()).unwrap() {
        let entry = entry.unwrap();
        std::fs::copy(entry.path(), tmp.path().join(entry.file_name())).unwrap();
    }
    tmp
}

fn plant_into(dir: &Path, file: &str, change: impl FnOnce(&mut Resp)) -> Resp {
    let path = dir.join(file);
    let mut fixture: Fixture = read_fixture(&path);
    let original = fixture.response.clone();
    change(&mut fixture.response);
    std::fs::write(path, serde_json::to_vec_pretty(&fixture).unwrap()).unwrap();
    original
}

fn plant_message(r: &mut Resp, message: &str) {
    r.json
        .as_mut()
        .unwrap()
        .set_pointer("/message", J::Str(message.to_string()));
}

fn replay_copy(dir: &Path, report_text: bool) -> Result<Replay, String> {
    let dir = dir.to_path_buf();
    on_runtime(move || async move { replay(&dir, report_text).await })
}

fn assert_response_difference(refused: &str, detail: &str) {
    assert!(refused.contains(&format!(
        "{TEXT_FIXTURE}: POST /arc-callback answered differently"
    )));
    assert!(refused.contains(detail));
    assert!(!refused.contains(&format!("TEXT-DIFF: {TEXT_FIXTURE} ")));
}

/// A real replay refuses a changed message by default and reports exactly
/// one JSON-quoted line for that fixture under the gate's word.
#[test]
fn a_message_only_difference_fails_strict_and_reports_under_the_knob() {
    let planted = "planted \"words\"\nsecond line";
    let (tmp, original) = plant_response(|r| {
        r.json
            .as_mut()
            .unwrap()
            .set_pointer("/message", J::Str(planted.to_string()));
    });
    let strict = replay_copy(tmp.path(), false).unwrap_err();
    assert!(strict.contains(&format!(
        "{TEXT_FIXTURE}: POST /arc-callback answered differently"
    )));
    assert!(!strict.contains("TEXT-DIFF:"));
    let report = replay_copy(tmp.path(), true).unwrap();
    let expected = format!(
        "TEXT-DIFF: {TEXT_FIXTURE} \"planted \\\"words\\\"\\nsecond line\" \"EOF while parsing a value at line 1 column 8\""
    );
    let lines: Vec<_> = report
        .text_diffs
        .iter()
        .filter(|line| line.starts_with(&format!("TEXT-DIFF: {TEXT_FIXTURE} ")))
        .collect();
    assert_eq!(lines, vec![&expected]);
    assert!(report.summary.contains(&format!(
        "{} text differences reported",
        report.text_diffs.len()
    )));
    assert_eq!(
        original.json.unwrap().get("message").unwrap().as_str(),
        Some("EOF while parsing a value at line 1 column 8")
    );
}

/// Two fixtures' changed messages are two markers, in fixture order, and
/// the summary counts both. A row the corpus already reports (the gate's
/// toolbox tip) is counted beside them, never in place of them.
#[test]
fn two_message_only_differences_report_two_markers() {
    const SECOND: &str = "083-createAction.inputNotOffered.json";
    let tmp = copy_corpus();
    let first = plant_into(tmp.path(), TEXT_FIXTURE, |r| {
        plant_message(r, "planted one");
    });
    let second = plant_into(tmp.path(), SECOND, |r| plant_message(r, "planted two"));
    let served = |r: Resp| {
        r.json
            .unwrap()
            .get("message")
            .unwrap()
            .as_str()
            .unwrap()
            .to_string()
    };
    let expected = vec![
        text_diff_line(TEXT_FIXTURE, "planted one", &served(first)),
        text_diff_line(SECOND, "planted two", &served(second)),
    ];
    let planted = |line: &str| {
        [TEXT_FIXTURE, SECOND]
            .iter()
            .any(|file| line.starts_with(&format!("TEXT-DIFF: {file} ")))
    };
    let strict = replay_copy(tmp.path(), false).unwrap_err();
    assert!(strict.contains(&format!(
        "{TEXT_FIXTURE}: POST /arc-callback answered differently"
    )));
    assert!(strict.contains(&format!(
        "{SECOND}: POST /createAction answered differently"
    )));
    let report = replay_copy(tmp.path(), true).unwrap();
    let (lines, others): (Vec<_>, Vec<_>) = report
        .text_diffs
        .iter()
        .cloned()
        .partition(|line| planted(line));
    assert_eq!(lines, expected);
    assert!(
        report.summary.ends_with(&format!(
            " 0 differ, 0 coverage gaps, {} text differences reported",
            2 + others.len()
        )),
        "{}",
        report.summary
    );
    let mut out = Vec::new();
    for line in &report.text_diffs {
        write_text_diff(&mut out, line).unwrap();
    }
    let out = String::from_utf8(out).unwrap();
    let markers: Vec<_> = out
        .lines()
        .filter(|l| l.starts_with("TEXT-DIFF: ") && planted(l))
        .collect();
    assert_eq!(markers, expected);
}

/// A changed refusal class remains a failure even in the gate's mode.
#[test]
fn a_code_difference_fails_under_the_knob() {
    let (tmp, _) = plant_response(|r| {
        r.json
            .as_mut()
            .unwrap()
            .set_pointer("/code", J::Str("OTHER".to_string()));
    });
    let refused = replay_copy(tmp.path(), true).unwrap_err();
    assert_response_difference(&refused, "OTHER");
}

/// A message change never masks another changed field or its line diff.
#[test]
fn a_message_and_another_field_difference_fail_under_the_knob() {
    let (tmp, _) = plant_response(|r| {
        r.json
            .as_mut()
            .unwrap()
            .set_pointer("/message", J::Str("changed".to_string()));
        r.headers.insert("retry-after".to_string(), "9".to_string());
    });
    let refused = replay_copy(tmp.path(), true).unwrap_err();
    assert_response_difference(&refused, "retry-after");
}

/// Refusal text is the only exception: status, headers, JSON types, nested
/// values and key order, text bodies and successful bodies stay strict.
#[test]
fn text_reporting_keeps_every_other_response_detail() {
    let want: Fixture = read_fixture(&wire_dir().join("083-createAction.inputNotOffered.json"));
    let mut text = want.response.clone();
    text.json
        .as_mut()
        .unwrap()
        .set_pointer("/message", J::Str("changed".to_string()));
    assert!(message_difference(&want.response, &text).is_some());
    let mut variants = Vec::new();
    let mut status = text.clone();
    status.status = 409;
    variants.push(status);
    let mut headers = text.clone();
    headers.headers.clear();
    variants.push(headers);
    for (pointer, value) in [
        ("/code", J::Null),
        ("/message", J::Null),
        ("/held/0/reason", J::Str("changed".to_string())),
    ] {
        let mut changed = text.clone();
        changed.json.as_mut().unwrap().set_pointer(pointer, value);
        variants.push(changed);
    }
    let mut reordered = text.clone();
    let J::Obj(fields) = reordered.json.as_mut().unwrap() else {
        unreachable!()
    };
    fields.reverse();
    variants.push(reordered);
    let mut raw = text.clone();
    raw.text = Some("changed".to_string());
    variants.push(raw);
    for changed in variants {
        assert!(message_difference(&want.response, &changed).is_none());
    }
    let mut success = want.response.clone();
    success.status = 200;
    text.status = 200;
    assert!(message_difference(&success, &text).is_none());
    assert_script_fault_detail_stays_strict();
}

/// The internalize refusal's detail after `spend refused at offset ` (the
/// offset and the script reason) stays strict inside a moved message.
fn assert_script_fault_detail_stays_strict() {
    let mut detail: Fixture =
        read_fixture(&wire_dir().join("103-internalizeAction.badUnlock.json"));
    let expected = detail.response.clone();
    let message = expected
        .json
        .as_ref()
        .unwrap()
        .get("message")
        .unwrap()
        .as_str()
        .unwrap();
    for changed in [
        message.replace("offset 176", "offset 177"),
        message.replace("OP_EQUALVERIFY", "OP_EQUAL"),
    ] {
        detail
            .response
            .json
            .as_mut()
            .unwrap()
            .set_pointer("/message", J::Str(changed));
        assert!(message_difference(&expected, &detail.response).is_none());
    }
}

/// Fresh recording leniency cannot hide a changed request, envelope,
/// whitespace, or manifest. Only the quoted response message may move.
#[test]
fn a_recording_reports_only_the_message_bytes() {
    let path = wire_dir().join(TEXT_FIXTURE);
    let want = std::fs::read_to_string(path).unwrap();
    let got = want.replace("EOF while parsing a value at line 1 column 8", "changed");
    assert_eq!(recording_difference(TEXT_FIXTURE, want.as_bytes(), got.as_bytes()), Some(format!("TEXT-DIFF: {TEXT_FIXTURE} \"EOF while parsing a value at line 1 column 8\" \"changed\"")));
    for changed in [
        format!("{got}\n"),
        got.replace("POST", "PUT"),
        got.replace("BAD_JSON", "OTHER"),
    ] {
        assert!(recording_difference(TEXT_FIXTURE, want.as_bytes(), changed.as_bytes()).is_none());
    }
    assert!(recording_difference(MANIFEST, b"{}", b"{\n}").is_none());
}

/// A fixture missing from disk, a file not in the manifest, and a file
/// that is not a `.json` fixture are each named; the set that matches
/// passes.
#[test]
fn the_manifest_judges_the_set_on_disk() {
    let tmp = tempfile::TempDir::new().unwrap();
    let dir = tmp.path();
    let write = |name: &str, text: &str| std::fs::write(dir.join(name), text).unwrap();
    let manifest = |names: &[&str]| {
        write(
            MANIFEST,
            &serde_json::to_string(&Manifest {
                fixtures: names.iter().map(ToString::to_string).collect(),
                notes: Default::default(),
            })
            .unwrap(),
        );
    };
    assert!(
        judge_manifest(dir).unwrap_err()[0].contains(MANIFEST),
        "no manifest is a fault"
    );
    write("001-a.json", "{}");
    write("002-b.json", "{}");
    manifest(&["001-a.json", "002-b.json"]);
    assert_eq!(
        judge_manifest(dir).unwrap(),
        vec!["001-a.json", "002-b.json"]
    );

    std::fs::remove_file(dir.join("002-b.json")).unwrap();
    write("003-c.json", "{}");
    write("notes.txt", "a stray file");
    let faults = judge_manifest(dir).unwrap_err();
    assert_eq!(
        faults,
        vec![
            format!("002-b.json: listed in {MANIFEST}, not on disk"),
            format!("003-c.json: on disk, not listed in {MANIFEST}"),
            "notes.txt: on disk, not a .json fixture".to_string(),
        ]
    );

    // The replay refuses the set before any world is built, naming each.
    let at = dir.to_path_buf();
    let refused = on_runtime(move || async move { replay(&at, false).await }).unwrap_err();
    for name in ["002-b.json", "003-c.json", "notes.txt"] {
        assert!(refused.contains(name), "{refused}");
    }
}

/// A fixture naming a world the run does not know is a panic naming it.
#[test]
#[should_panic(expected = "unknown world `nowhere`")]
fn an_unknown_world_is_named() {
    known_world("nowhere");
}

/// The replay refuses a corpus naming an unknown world before any world
/// is built.
#[test]
fn a_fixture_of_an_unknown_world_fails_the_replay() {
    let tmp = tempfile::TempDir::new().unwrap();
    let fixture = Fixture {
        world: "nowhere".to_string(),
        request: get("/isAuthenticated"),
        response: Resp {
            status: 200,
            headers: BTreeMap::new(),
            json: None,
            text: None,
        },
    };
    std::fs::write(
        tmp.path().join("001-nowhere.json"),
        serde_json::to_string(&fixture).unwrap(),
    )
    .unwrap();
    std::fs::write(
        tmp.path().join(MANIFEST),
        serde_json::to_string(&Manifest {
            fixtures: vec!["001-nowhere.json".to_string()],
            notes: Default::default(),
        })
        .unwrap(),
    )
    .unwrap();
    let dir = tmp.path().to_path_buf();
    let outcome =
        std::panic::catch_unwind(|| on_runtime(move || async move { replay(&dir, false).await }));
    let message = match outcome {
        Err(payload) => payload
            .downcast_ref::<String>()
            .cloned()
            .or_else(|| payload.downcast_ref::<&str>().map(ToString::to_string))
            .unwrap_or_default(),
        Ok(result) => panic!("the replay did not panic: {result:?}"),
    };
    assert!(
        message.contains("001-nowhere.json: unknown world `nowhere`"),
        "{message}"
    );
}

/// The normalizer keeps a volatile value's shape: a byte array's length, a
/// string's kind and length, a number's digit count; a key that is not
/// volatile is kept whole.
#[test]
fn the_normalizer_keeps_the_shape() {
    use serde_json::json;
    let body = j(json!({
        "ciphertext": [1, 2, 3, 255],
        "encryptedLinkage": "00ff",
        "revelationTime": "2026-10-23T00:00:00.000Z",
        "ageMinutes": 31,
        "reference": "abc+/=",
        "held": [{"ageMinutes": 1234, "reason": "5 min old"}],
        "txid": "ab".repeat(32),
        "nested": {"ciphertext": null},
        "keyringForVerifier": {"name": "AQID", "role": "AAAAAA=="},
    }));
    let want = j(json!({
        "ciphertext": "<bytes:4>",
        "encryptedLinkage": "<hex:4>",
        "revelationTime": "<rfc3339>",
        "ageMinutes": "<uint:2>",
        "reference": "<string:6>",
        "held": [{"ageMinutes": "<uint:4>", "reason": "<minutes> min old"}],
        "txid": "ab".repeat(32),
        "nested": {"ciphertext": null},
        "keyringForVerifier": {"name": "<string:4>", "role": "<string:8>"},
    }));
    assert_eq!(normalize(body).compact(), want.compact());
    assert_eq!(
        normalize_text("tcp connect error: Connection refused (os error 61) at 127.0.0.1:4242"),
        "tcp connect error: Connection refused (os error <errno>) at 127.0.0.1:<port>"
    );
}

/// The judged `AppError` codes and the unproduced ones are exactly the
/// codes `src/server/error.rs` writes, at their statuses: a code added
/// there with no fixture, or one gone from it, fails here by name.
#[test]
fn every_app_error_code_is_judged() {
    let in_source = app_codes_in_source();
    let listed: BTreeSet<(u16, String)> = APP_CODES
        .iter()
        .chain(UNPRODUCED.iter())
        .map(|(s, c)| (*s, c.to_string()))
        .collect();
    assert_eq!(
        APP_CODES.len() + UNPRODUCED.len(),
        listed.len(),
        "a code is listed twice"
    );
    let unlisted: Vec<_> = in_source.difference(&listed).collect();
    let gone: Vec<_> = listed.difference(&in_source).collect();
    assert!(
        unlisted.is_empty() && gone.is_empty(),
        "src/server/error.rs writes {unlisted:?}, which neither APP_CODES (with a fixture) nor \
         UNPRODUCED (with its reason in docs/WIRE.md) lists; and lists {gone:?}, which it no \
         longer writes"
    );
}

/// #140: the verifier refuses the script sentence, stores nothing, then
/// takes the valid payment as pending. The host code moves again at beta.2.
#[test]
fn internalize_verifies_the_script_before_storing_a_payment() {
    on_runtime(|| async {
        use wire_harness::recorder::post;
        let world = World::build("funded").await;
        let listing = post(
            "/listActions",
            serde_json::json!({"labels": [], "limit": 100}),
        );
        let as_value = |r: wire_support::Resp| serde_json::to_value(r.json.unwrap()).unwrap();
        let before = as_value(world.serve_raw(&listing).await.0);
        let bad: Fixture = read_fixture(&wire_dir().join("103-internalizeAction.badUnlock.json"));
        let refused = world.serve_raw(&bad.request).await.0;
        assert_eq!(refused.status, 400);
        let body = as_value(refused);
        assert_eq!(body["code"], "VALIDATION_ERROR");
        assert!(body["message"]
            .as_str()
            .unwrap()
            .contains("OP_EQUALVERIFY requires the top two stack items to be equal."));
        assert_ne!(body["accepted"], true);
        assert_eq!(
            as_value(world.serve_raw(&listing).await.0),
            before,
            "nothing stored"
        );
        assert_eq!(
            as_value(
                world
                    .serve_raw(&post("/listOutputs", serde_json::json!({"basket": "wire"})))
                    .await
                    .0
            )["totalOutputs"],
            0
        );
        let good: Fixture =
            read_fixture(&wire_dir().join("104-internalizeAction.walletPayment.json"));
        let accepted = world.serve_raw(&good.request).await.0;
        assert_eq!(accepted.status, 200);
        assert_eq!(as_value(accepted)["accepted"], true);
        let after = as_value(world.serve_raw(&listing).await.0);
        assert_eq!(
            after["totalActions"].as_u64().unwrap(),
            before["totalActions"].as_u64().unwrap() + 1
        );
        assert!(after["actions"]
            .as_array()
            .unwrap()
            .iter()
            .any(|a| a["status"] == "unproven"));
    });
}
