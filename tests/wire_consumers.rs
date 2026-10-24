//! The consumers' recorded calls (#110): one exchange per call shape a
//! consumer of this binary sends over the wire, under
//! `tests/wire_consumers/<consumer>/`.
//!
//! `cargo test --test wire_consumers` replays every fixture against the
//! router, in-process (`tower::ServiceExt::oneshot`), and fails on any byte
//! of difference after normalization, naming the consumer whose call
//! changed. `WIRE_RECORD=1 cargo test --test wire_consumers` re-records
//! every fixture from the cases below; nothing else re-records. The
//! consumers, what they call and where each shape comes from are in
//! `docs/DEPENDENTS.md`.
//!
//! The fixture format is the wire oracle's (#69, `docs/WIRE.md` on
//! `lane/oracle-fixtures`): `world`, `request`, `response`, with three
//! fields of its own: `consumer` (the dependent, also the directory),
//! `call` (the consumer's name for the call) and `source` (the file in the
//! consumer that sends this shape). Each consumer's fixtures are served
//! from worlds of their own, in file order, so no consumer's state reaches
//! another's. Nothing leaves the machine: the header service is a local
//! stub, the broadcaster a closed local port, every other request goes
//! through a proxy on a closed local port.

mod wire_consumers_harness;
mod wire_support;

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::{Path, PathBuf};

use wire_consumers_harness::cases::CASES;
use wire_consumers_harness::fixture::Fixture;
use wire_consumers_harness::worlds::World;
use wire_support::{diff, on_runtime, read_fixture};

// ---------------------------------------------------------------------------
// The consumers and the fixtures each one must have, by file name. The list
// is the corpus's manifest: a fixture deleted or renamed, or a file no case
// names, fails the replay naming it (a case added or renamed in the
// recorder is added or renamed here in the same commit). Every fixture
// must name its consumer (`docs/DEPENDENTS.md` has a row for each).
// ---------------------------------------------------------------------------

const CONSUMERS: [(&str, &[&str]); 7] = [
    // LOW's fleet: the daemon's HTTP on four hosts, its funder's spends.
    (
        "low-fleet",
        &[
            "01-isAuthenticated.json",
            "02-getVersion.json",
            "03-getPublicKey.identity.json",
            "04-getPublicKey.brc29Self.json",
            "05-createSignature.hashToDirectlySign.json",
            "06-createHmac.handSecret.json",
            "07-listActions.hopLedger.json",
            "08-createAction.stake.json",
            "09-createAction.allowPending.json",
            "10-createAction.scoped.json",
            "11-internalizeAction.potPayout.json",
            "12-getPublicKey.harnessBearer.json",
        ],
    ),
    // The stack's witness spend wallet (bsv-stack-lean).
    ("stack-witness", &["01-txWord.json"]),
    // The x402 skill's wallet client (lib/metanet.py, lib/auth_request.py,
    // scripts/brc31_helpers.py).
    (
        "x402-skill",
        &[
            "01-isAuthenticated.json",
            "02-getPublicKey.identity.json",
            "03-getPublicKey.payment.json",
            "04-createSignature.auth.json",
            "05-createAction.payment.json",
            "06-createAction.executeAction.json",
            "07-internalizeAction.refund.json",
            "08-verifySignature.response.json",
        ],
    ),
    // The MCP companion (mcp/src/main.rs) on behalf of its clients.
    (
        "mcp",
        &[
            "01-is_authenticated.json",
            "02-get_height.json",
            "03-get_public_key.identity.json",
            "04-create_signature.json",
            "05-wallet_balance.json",
            "06-list_actions.json",
            "07-create_action.allowPending.json",
            "08-create_action.json",
            "09-relinquish_output.json",
            "10-chain_feed_status.json",
            "11-tx_word.json",
        ],
    ),
    // The apps' x402 payers (~/bsv/agents, ~/bsv/zanaadu, ~/bsv/teragunv2).
    (
        "app-payers",
        &[
            "01-agents.createAction.noSend.json",
            "02-agents.abortAction.json",
            "03-zanaadu.getPublicKey.identity.json",
            "04-zanaadu.createAction.fund.json",
            "05-teragunv2.getPublicKey.payment.json",
            "06-teragunv2.createAction.pack.json",
        ],
    ),
    // dolphinmilk: the 0.1.23 crate's server in-process, called over HTTP.
    (
        "dolphinmilk",
        &[
            "01-getVersion.json",
            "02-listOutputs.balance.json",
            "03-createAction.x402.json",
        ],
    ),
    // The e2e harness's client (tests/e2e/lib/wallet-client.js).
    (
        "e2e-harness",
        &[
            "01-getPublicKey.brc29.json",
            "02-createSignature.json",
            "03-listOutputs.balance.json",
            "04-createAction.transferTo.json",
            "05-createAction.scoped.json",
            "06-relinquishOutput.json",
            "07-isAuthenticated.bearer.json",
        ],
    ),
];

/// Writes every case's fixture, each consumer's served from its own
/// worlds in table order.
async fn record(dir: &Path) -> usize {
    let mut written = 0;
    for (consumer, cases) in CASES {
        let mut worlds: HashMap<&str, World> = HashMap::new();
        let dir = dir.join(consumer);
        std::fs::create_dir_all(&dir).unwrap();
        for (seq, case) in cases.iter().enumerate() {
            if !worlds.contains_key(case.world) {
                worlds.insert(case.world, World::build(case.world).await);
            }
            let request = case.request();
            let response = worlds[case.world].serve(&request).await;
            let fixture = Fixture {
                consumer: consumer.to_string(),
                call: case.call.to_string(),
                source: case.source.to_string(),
                world: case.world.to_string(),
                request,
                response,
            };
            let mut text = serde_json::to_string_pretty(&fixture).unwrap();
            text.push('\n');
            std::fs::write(dir.join(format!("{:02}-{}.json", seq + 1, case.call)), text).unwrap();
            written += 1;
        }
    }
    written
}

// ---------------------------------------------------------------------------
// Record or replay.
// ---------------------------------------------------------------------------

fn consumers_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/wire_consumers")
}

/// Every file under a consumer's directory, sorted.
fn list_files(dir: &Path) -> Vec<PathBuf> {
    let mut files: Vec<PathBuf> = Vec::new();
    for entry in std::fs::read_dir(dir).unwrap_or_else(|e| panic!("{}: {e}", dir.display())) {
        let sub = entry.unwrap().path();
        if !sub.is_dir() {
            continue;
        }
        for file in std::fs::read_dir(&sub).unwrap() {
            files.push(file.unwrap().path());
        }
    }
    files.sort();
    files
}

/// Every fixture, by consumer directory then file name.
fn read_fixtures(dir: &Path) -> Vec<(String, Fixture)> {
    list_files(dir)
        .into_iter()
        .filter(|p| p.extension().is_some_and(|x| x == "json"))
        .map(|p| {
            let fixture: Fixture = read_fixture(&p);
            let name = p.strip_prefix(dir).unwrap().to_string_lossy().into_owned();
            (name, fixture)
        })
        .collect()
}

/// What the corpus must keep: every file of [`CONSUMERS`]' list on disk,
/// nothing on disk the list does not name, every fixture a known
/// consumer's, in its own directory.
fn judge(dir: &Path, fixtures: &[(String, Fixture)]) -> Vec<String> {
    let mut gaps = Vec::new();
    let on_disk: BTreeSet<String> = list_files(dir)
        .iter()
        .map(|p| p.strip_prefix(dir).unwrap().to_string_lossy().into_owned())
        .collect();
    let listed: BTreeSet<String> = CONSUMERS
        .iter()
        .flat_map(|(consumer, files)| files.iter().map(move |f| format!("{consumer}/{f}")))
        .collect();
    for file in listed.difference(&on_disk) {
        gaps.push(format!("{file}: listed, missing (deleted or renamed?)"));
    }
    for file in on_disk.difference(&listed) {
        gaps.push(format!("{file}: on disk, not in the list (CONSUMERS)"));
    }
    for (file, f) in fixtures {
        if !CONSUMERS.iter().any(|(c, _)| *c == f.consumer) {
            gaps.push(format!("{file}: unknown consumer {}", f.consumer));
        }
        if !file.starts_with(&format!("{}/", f.consumer)) {
            gaps.push(format!("{file}: not under {}/", f.consumer));
        }
    }
    gaps
}

/// The consumers' replay (or, `WIRE_RECORD=1`, their recording first), on
/// the run's own runtime.
#[test]
fn wire_consumers() {
    on_runtime(record_or_replay);
}

async fn record_or_replay() {
    let dir = consumers_dir();
    if std::env::var("WIRE_RECORD").is_ok_and(|v| v == "1") {
        if dir.exists() {
            std::fs::remove_dir_all(&dir).unwrap();
        }
        std::fs::create_dir_all(&dir).unwrap();
        println!("recorded {} consumer fixtures", record(&dir).await);
    }
    let fixtures = read_fixtures(&dir);
    assert!(!fixtures.is_empty(), "no fixture under {}", dir.display());
    let failures = replay(&fixtures).await;
    let gaps = judge(&dir, &fixtures);
    println!(
        "{} consumer fixtures replayed ({}), {} differ, {} gaps",
        fixtures.len(),
        count_by_consumer(&fixtures),
        failures.len(),
        gaps.len()
    );
    assert!(
        failures.is_empty() && gaps.is_empty(),
        "\n{}\n{}\n(a change on purpose: WIRE_RECORD=1 cargo test --test wire_consumers, in its own commit, and tell the consumer's captain; see docs/DEPENDENTS.md)",
        failures.join("\n"),
        gaps.join("\n")
    );
}

/// "consumer n" for each consumer, in name order.
fn count_by_consumer(fixtures: &[(String, Fixture)]) -> String {
    let mut count: BTreeMap<&str, usize> = BTreeMap::new();
    for (_, f) in fixtures {
        *count.entry(f.consumer.as_str()).or_default() += 1;
    }
    count
        .iter()
        .map(|(c, n)| format!("{c} {n}"))
        .collect::<Vec<_>>()
        .join(", ")
}

/// Serves every fixture's request from its consumer's worlds; one message
/// per answer that differs from the fixture's.
async fn replay(fixtures: &[(String, Fixture)]) -> Vec<String> {
    let mut worlds: HashMap<(String, String), World> = HashMap::new();
    let mut failures = Vec::new();
    for (file, f) in fixtures {
        let key = (f.consumer.clone(), f.world.clone());
        if !worlds.contains_key(&key) {
            worlds.insert(key.clone(), World::build(&f.world).await);
        }
        let got = worlds[&key].serve(&f.request).await;
        if got != f.response {
            let want = serde_json::to_string_pretty(&f.response).unwrap();
            let served = serde_json::to_string_pretty(&got).unwrap();
            failures.push(format!(
                "consumer {} would feel this: {file} ({}, sent by {}): {} {} answered differently (- fixture, + served)\n{}",
                f.consumer,
                f.call,
                f.source,
                f.request.method,
                f.request.path,
                diff(&want, &served)
            ));
        }
    }
    failures
}
