//! The wire oracle (#69): every served call and every refusal body, as a
//! recorded request and the exact response, under `tests/wire/`.
//!
//! `cargo test --test wire` replays every fixture against the router,
//! in-process (`tower::ServiceExt::oneshot`, no socket for the wallet), and
//! fails on any byte of difference after normalization, printing the diff.
//! `WIRE_RECORD=1 cargo test --test wire` re-records every fixture from the
//! cases below and the manifest `tests/wire/MANIFEST.json` (the recorded
//! set), then fails with the word to replay; nothing else re-records. See
//! `docs/WIRE.md`.
//!
//! The field names the shapes must keep are BRC-100's at the line's pin,
//! bsv-blockchain/BRCs `ed1b015` (2026-10-05, the stack's
//! docs/PROVENANCE.md). Upstream master `5119a9b` (2026-10-19) differs for
//! `listOutputs` (`totalOutputs` may be a string, `spendable` may be false,
//! for BRC-112 and BRC-207 module views) and for `internalizeAction` (an
//! insertion's `customInstructions` bounded at 1000 bytes). The latter is
//! enforced since 0.18.2 and recorded by fixture 123; the list shape stays
//! at the line's pin.
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
use wire_harness::fixture::{j, normalize, normalize_text, Fixture};
use wire_harness::judge::{app_codes_in_source, judge_coverage, APP_CODES, UNPRODUCED};
use wire_harness::manifest::{judge_manifest, Manifest, MANIFEST};
use wire_harness::recorder::{get, Recorder};
use wire_harness::worlds::{known_world, resolve, World, WORLDS};
use wire_support::{diff, on_runtime, read_fixture, Resp, J};

// ---------------------------------------------------------------------------
// Record or replay.
// ---------------------------------------------------------------------------

fn wire_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/wire")
}

/// The fixtures the manifest names, in name order; a fixture of an unknown
/// world is a panic naming it.
fn read_fixtures(dir: &Path) -> Result<Vec<(String, Fixture)>, Vec<String>> {
    let mut files = judge_manifest(dir)?;
    files.sort();
    Ok(files
        .into_iter()
        .map(|name| {
            let fixture: Fixture = read_fixture(&dir.join(&name));
            // A world the run does not know fails before any is built.
            assert!(
                WORLDS.contains(&fixture.world.as_str()),
                "{name}: unknown world `{}`: the worlds are {}",
                fixture.world,
                WORLDS.join(", ")
            );
            (name, fixture)
        })
        .collect())
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
async fn replay(dir: &Path) -> Result<String, String> {
    // The permission audit is the process's: each pass starts it empty.
    bsv_wallet_cli::server::audit::reset();
    let fixtures = read_fixtures(dir).map_err(|faults| {
        format!(
            "the fixtures under {} are not the recorded set:\n{}",
            dir.display(),
            faults.join("\n")
        )
    })?;
    if fixtures.is_empty() {
        return Err(format!("no fixture under {}", dir.display()));
    }
    let mut worlds: HashMap<String, World> = HashMap::new();
    let mut served: HashMap<String, J> = HashMap::new();
    let mut failures = Vec::new();
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
            let want = serde_json::to_string_pretty(&fixture.response).unwrap();
            let got = serde_json::to_string_pretty(&got).unwrap();
            failures.push(format!(
                "{file}: {} {} answered differently (- fixture, + served)\n{}",
                fixture.request.method,
                fixture.request.path,
                diff(&want, &got)
            ));
        }
    }
    let missing = judge_coverage(&fixtures);
    let summary = format!(
        "{} fixtures replayed, {} differ, {} coverage gaps",
        fixtures.len(),
        failures.len(),
        missing.len()
    );
    if failures.is_empty() && missing.is_empty() {
        Ok(summary)
    } else {
        Err(format!(
            "{summary}\n{}\n{}\n(a change on purpose: WIRE_RECORD=1 cargo test --test wire, then a \
             replay, in its own commit; see docs/WIRE.md)",
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
    if record_requested() {
        let message = on_runtime(move || async move { record(&dir).await });
        panic!("{message}");
    }
    match on_runtime(move || async move { replay(&dir).await }) {
        Ok(summary) => println!("{summary}"),
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
    let replayed = on_runtime(move || async move { replay(&dir).await });
    assert!(replayed.is_ok(), "the second run: {replayed:?}");
    for name in listed.iter().map(String::as_str).chain([MANIFEST]) {
        let recorded = std::fs::read(tmp.path().join(name)).unwrap();
        let committed = std::fs::read(wire_dir().join(name)).unwrap_or_default();
        assert!(
            recorded == committed,
            "{name}: a fresh recording differs from tests/wire (WIRE_RECORD=1, then replay)"
        );
    }
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
    let refused = on_runtime(move || async move { replay(&at).await }).unwrap_err();
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
        })
        .unwrap(),
    )
    .unwrap();
    let dir = tmp.path().to_path_buf();
    let outcome =
        std::panic::catch_unwind(|| on_runtime(move || async move { replay(&dir).await }));
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
