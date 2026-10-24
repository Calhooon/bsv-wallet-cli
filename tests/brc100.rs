//! The BRC-100 oracle (#100): every key the wire fixtures send or serve on
//! the 28 calls is a field of BRC-100 at the pin (`docs/brc100/fields.json`)
//! or a row of the extensions table of `docs/BRC-100-EXTENSIONS.md`, and
//! every row of that document is true of the fixtures (no dead entry).
//!
//! A key is a dotted path: an array is transparent and a key of a BRC
//! `Record` is `*`. A 2xx body is the `response` side, any other body the
//! `refusal` side, judged against the BRC's error structure.

use serde_json::Value;
use std::collections::BTreeSet;
use std::path::PathBuf;

const EXTENSIONS: &str = "| endpoint | side | field | reason | added |";
const GAPS: &str = "| endpoint | side | field | what is served instead | issue |";
const UNRECORDED: &str = "| endpoint | side | field | why no fixture serves it | added |";
const HOSTS: &str = "| route | method | what it answers | added |";
const SIDES: [&str; 3] = ["request", "response", "refusal"];

/// `(call or "*", side, dotted path)`.
type Key = (String, String, String);

/// What the BRC names: its calls, every field as a [`Key`] (the error
/// structure under `*`, `refusal`) and the names of its `Record` fields.
struct Brc {
    calls: BTreeSet<String>,
    named: BTreeSet<Key>,
    records: BTreeSet<String>,
}

/// What the corpus shows: every key on the 28 calls, and each fixture of
/// another path as (file, path, status, whether it has a JSON body).
struct Corpus {
    keys: BTreeSet<Key>,
    others: Vec<(String, String, u64, bool)>,
}

fn at(rel: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(rel)
}

fn json_at(rel: &str) -> Value {
    let text = std::fs::read_to_string(at(rel)).unwrap_or_else(|e| panic!("{rel}: {e}"));
    serde_json::from_str(&text).unwrap_or_else(|e| panic!("{rel}: {e}"))
}

fn key(call: &str, side: &str, path: &str) -> Key {
    (call.to_string(), side.to_string(), path.to_string())
}

/// The key, or the same side and path under `*`.
fn matches(set: &BTreeSet<Key>, call: &str, side: &str, path: &str) -> bool {
    set.contains(&key(call, side, path)) || set.contains(&key("*", side, path))
}

fn load_brc(fields: &Value) -> Brc {
    let mut brc = Brc {
        calls: BTreeSet::new(),
        named: BTreeSet::new(),
        records: BTreeSet::new(),
    };
    for (call, spec) in fields["calls"].as_object().expect("calls") {
        brc.calls.insert(call.clone());
        for side in &SIDES[..2] {
            for field in spec[*side].as_array().expect("side") {
                let name = field["name"].as_str().expect("name");
                brc.named.insert(key(call, side, name));
                if field["record"] == Value::Bool(true) {
                    brc.records
                        .insert(name.rsplit('.').next().unwrap_or(name).into());
                }
            }
        }
    }
    for field in fields["error"]["fields"].as_array().expect("error") {
        brc.named
            .insert(key("*", "refusal", field["name"].as_str().expect("name")));
    }
    brc
}

/// Every dotted path of `value` under `path`; the keys of an object that
/// is a `Record` field's value are `*` and are not paths of their own.
fn paths(value: &Value, path: &str, records: &BTreeSet<String>, out: &mut BTreeSet<String>) {
    match value {
        Value::Object(map) => {
            let is_record = records.contains(path.rsplit('.').next().unwrap_or(path));
            for (name, inner) in map {
                let next = if is_record {
                    format!("{path}.*")
                } else if path.is_empty() {
                    name.clone()
                } else {
                    format!("{path}.{name}")
                };
                if !is_record {
                    out.insert(next.clone());
                }
                paths(inner, &next, records, out);
            }
        }
        Value::Array(items) => items.iter().for_each(|v| paths(v, path, records, out)),
        _ => {}
    }
}

fn load_corpus(brc: &Brc) -> Corpus {
    let mut corpus = Corpus {
        keys: BTreeSet::new(),
        others: Vec::new(),
    };
    for file in json_at("tests/wire/MANIFEST.json")["fixtures"]
        .as_array()
        .expect("fixtures")
    {
        let file = file.as_str().expect("file name");
        let fixture = json_at(&format!("tests/wire/{file}"));
        let (request, response) = (&fixture["request"], &fixture["response"]);
        let route = request["path"].as_str().expect("path");
        let status = response["status"].as_u64().expect("status");
        let call = route.trim_start_matches('/');
        if !brc.calls.contains(call) {
            let has_json = response.get("json").is_some();
            corpus
                .others
                .push((file.into(), route.into(), status, has_json));
            continue;
        }
        let answer = if status < 300 { "response" } else { "refusal" };
        for (side, body) in [("request", &request["json"]), (answer, &response["json"])] {
            let mut found = BTreeSet::new();
            paths(body, "", &brc.records, &mut found);
            corpus.keys.extend(found.iter().map(|p| key(call, side, p)));
        }
    }
    corpus
}

/// The rows of the table under `header`, each with its line number.
fn rows<'a>(doc: &'a str, header: &str) -> Vec<(usize, Vec<&'a str>)> {
    let lines: Vec<&str> = doc.lines().collect();
    let Some(start) = lines.iter().position(|l| l.trim_end() == header) else {
        return Vec::new();
    };
    let cells = |l: &'a str| {
        l.trim()
            .trim_matches('|')
            .split('|')
            .map(str::trim)
            .collect()
    };
    (start + 2..lines.len())
        .take_while(|&i| lines[i].starts_with('|'))
        .map(|i| (i + 1, cells(lines[i])))
        .collect()
}

fn code(cell: &str) -> Option<&str> {
    cell.strip_prefix('`')?.strip_suffix('`')
}

/// A key table's rows as keys; a row the format does not allow is a failure.
fn key_rows(doc: &str, header: &str, brc: &Brc, failures: &mut Vec<String>) -> Vec<(usize, Key)> {
    let mut keys = Vec::new();
    for (line, cells) in rows(doc, header) {
        let parsed = match (
            cells.len(),
            code(cells[0]),
            cells.get(2).and_then(|c| code(c)),
        ) {
            (5, Some(call), Some(path))
                if (call == "*" || brc.calls.contains(call)) && SIDES.contains(&cells[1]) =>
            {
                Some(key(call, cells[1], path))
            }
            _ => None,
        };
        match parsed {
            Some(k) => keys.push((line, k)),
            None => failures.push(format!("line {line}: not a row of `{header}`")),
        }
    }
    keys
}

/// Whether the corpus serves the row's key (under `*`, on any call).
fn served(corpus: &Corpus, (call, side, path): &Key) -> bool {
    corpus
        .keys
        .iter()
        .any(|(c, s, p)| (call == "*" || c == call) && s == side && p == path)
}

fn check_keys(brc: &Brc, corpus: &Corpus, doc: &str) -> Vec<String> {
    let mut failures = Vec::new();
    let extensions = key_rows(doc, EXTENSIONS, brc, &mut failures);
    let listed: BTreeSet<Key> = extensions.iter().map(|(_, k)| k.clone()).collect();
    for (call, side, path) in &corpus.keys {
        if !matches(&brc.named, call, side, path) && !matches(&listed, call, side, path) {
            failures.push(format!(
                "{call} {side} `{path}`: neither BRC-100 at the pin nor a listed extension"
            ));
        }
    }
    let named = |(c, s, p): &Key| matches(&brc.named, c, s, p);
    for (line, k) in &extensions {
        if named(k) {
            failures.push(format!(
                "line {line}: `{}` is a field of BRC-100, not an extension",
                k.2
            ));
        } else if !served(corpus, k) {
            failures.push(format!(
                "line {line}: `{}` is listed and no fixture serves it",
                k.2
            ));
        }
    }
    for (line, k) in key_rows(doc, GAPS, brc, &mut failures) {
        if !named(&k) {
            failures.push(format!(
                "line {line}: gap `{}` is not a field of BRC-100",
                k.2
            ));
        } else if k.1 != "request" && served(corpus, &k) {
            failures.push(format!("line {line}: gap `{}` is served", k.2));
        }
    }
    for (line, k) in key_rows(doc, UNRECORDED, brc, &mut failures) {
        if named(&k) || served(corpus, &k) {
            failures.push(format!(
                "line {line}: `{}` is served or named; move the row",
                k.2
            ));
        }
    }
    failures
}

fn route_matches(pattern: &str, route: &str) -> bool {
    let (p, r): (Vec<_>, Vec<_>) = (pattern.split('/').collect(), route.split('/').collect());
    p.len() == r.len() && p.iter().zip(&r).all(|(a, b)| a == b || a.starts_with('{'))
}

fn check_hosts(corpus: &Corpus, doc: &str) -> Vec<String> {
    let mut failures = Vec::new();
    let mut hosts = Vec::new();
    for (line, cells) in rows(doc, HOSTS) {
        match (cells.len(), code(cells[0])) {
            (4, Some(route)) => hosts.push((line, route)),
            _ => failures.push(format!("line {line}: not a row of `{HOSTS}`")),
        }
    }
    for (file, route, status, has_json) in &corpus.others {
        let unknown_route = *status == 404 && !has_json;
        if !unknown_route && !hosts.iter().any(|(_, h)| route_matches(h, route)) {
            failures.push(format!(
                "{file}: {route} is neither a BRC-100 call nor a host route"
            ));
        }
    }
    for (line, host) in hosts {
        if !corpus
            .others
            .iter()
            .any(|(_, r, _, _)| route_matches(host, r))
        {
            failures.push(format!("line {line}: host route {host} has no fixture"));
        }
    }
    failures
}

fn judge(brc: &Brc, corpus: &Corpus, doc: &str) -> Vec<String> {
    let mut failures = check_keys(brc, corpus, doc);
    failures.extend(check_hosts(corpus, doc));
    failures
}

fn setup() -> (Brc, Corpus, String) {
    let brc = load_brc(&json_at("docs/brc100/fields.json"));
    let corpus = load_corpus(&brc);
    let doc = std::fs::read_to_string(at("docs/BRC-100-EXTENSIONS.md")).expect("extensions");
    (brc, corpus, doc)
}

#[test]
fn every_served_key_is_brc100_or_a_listed_extension_and_every_row_is_true() {
    let (brc, corpus, doc) = setup();
    assert_eq!(brc.calls.len(), 28, "fields.json names the 28 calls");
    for header in [EXTENSIONS, GAPS, UNRECORDED, HOSTS] {
        assert!(!rows(&doc, header).is_empty(), "no table under {header}");
    }
    let failures = judge(&brc, &corpus, &doc);
    assert!(failures.is_empty(), "\n{}", failures.join("\n"));
}

#[test]
fn an_unlisted_key_and_a_dead_row_fail_by_name() {
    let (brc, mut corpus, doc) = setup();
    corpus
        .keys
        .insert(key("createAction", "request", "planted"));
    let row = "| `getHeight` | response | `deadEntry` | planted | none |";
    let doc = doc.replacen(
        &format!("{EXTENSIONS}\n|---|---|---|---|---|\n"),
        &format!("{EXTENSIONS}\n|---|---|---|---|---|\n{row}\n"),
        1,
    );
    let failures = judge(&brc, &corpus, &doc).join("\n");
    assert!(
        failures.contains("createAction request `planted`: neither"),
        "{failures}"
    );
    assert!(
        failures.contains("`deadEntry` is listed and no fixture serves it"),
        "{failures}"
    );
}

#[test]
fn a_record_field_keys_are_the_callers_not_the_brcs() {
    let records = BTreeSet::from(["spends".to_string()]);
    let body = serde_json::json!({"spends": {"0": {"unlockingScript": "51"}}, "reference": "r"});
    let mut found = BTreeSet::new();
    paths(&body, "", &records, &mut found);
    let want = ["reference", "spends", "spends.*.unlockingScript"];
    assert_eq!(found, want.iter().map(|s| (*s).to_string()).collect());
}
