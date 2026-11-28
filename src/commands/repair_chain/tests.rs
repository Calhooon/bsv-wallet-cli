//! `repair-chain`'s branches (#682). The header service of every test is
//! the toolbox's `MockWalletServices` (its `get_header_for_height` answers
//! the headers a test sets, by height: `bsv-wallet-toolbox-rs`
//! 0.14.0-beta.10 `src/services/mock.rs`, `set_header_for_height` and
//! `get_header_for_height`), through the test-only [`HeaderSource`] impl
//! below; the one fault is the production client aimed at a closed
//! loopback port ([`CLOSED`]) with the process sealed ([`seal_network`]),
//! so no test reaches any service (#637).

use super::*;
/// A closed loopback port (`src/test_support/seal.rs`).
const CLOSED: &str = "http://127.0.0.1:9";
use crate::test_support::{seal_network, temp_db};
use bsv_wallet_toolbox::services::mock::MockWalletServices;
use bsv_wallet_toolbox::services::{BlockHeader, WalletServices};
use bsv_wallet_toolbox::storage::sqlx::StorageSqlxOptions;
use serde_json::json;

impl HeaderSource for MockWalletServices {
    async fn header_at(&self, height: u32) -> Result<Vec<u8>> {
        Ok(WalletServices::get_header_for_height(self, height).await?)
    }
}

/// A testnet proof height, and the roots used below.
const H1: u32 = 1_600_000;
const H2: u32 = 1_600_010;
const H3: u32 = 1_600_020;

/// A testnet configuration over `db`, every service a closed loopback
/// port (no test here calls `run`; `open_unasked` reads its options).
fn config_for(db: &str) -> Config {
    seal_network();
    let flags = crate::config::Flags {
        db: db.to_string(),
        testnet: true,
        ..crate::config::Flags::default()
    };
    let vars = [("CHAINTRACKS_URL", CLOSED), ("ARC_URL", CLOSED)]
        .map(|(k, v)| (k.to_string(), v.to_string()));
    Config::from_vars(&flags, vars).expect("the configuration reads")
}

fn root(byte: u8) -> String {
    format!("{byte:02x}").repeat(32)
}

/// The header a mock header service answers at `height`, carrying `root`.
pub(crate) fn header(height: u32, root: &str) -> BlockHeader {
    BlockHeader {
        merkle_root: root.to_string(),
        height,
        ..BlockHeader::default()
    }
}

/// One stored proof (`txid`, `height`, `root`), written as the toolbox's
/// proof funnel writes the columns the evidence reads.
pub(crate) async fn insert_proof(pool: &sqlx::SqlitePool, txid: &str, height: u32, root: &str) {
    sqlx::query(
        "INSERT INTO proven_txs (txid, height, idx, block_hash, merkle_root, merkle_path, raw_tx) \
         VALUES (?, ?, 0, ?, ?, X'00', X'00')",
    )
    .bind(txid)
    .bind(i64::from(height))
    .bind("ee".repeat(32))
    .bind(root)
    .execute(pool)
    .await
    .unwrap();
}

/// The repair on testnet over `storage`, as `run` drives it.
async fn on_testnet<H: HeaderSource>(
    storage: &StorageSqlx,
    db: &str,
    headers: Option<&H>,
    execute: bool,
    unchecked: bool,
) -> Report {
    repair(storage, db, Chain::Test, headers, execute, unchecked)
        .await
        .unwrap()
}

/// A store whose settings row holds `raw` (the legacy `mainnet`, or a
/// word), with one proof per `(height, root)`; opened as `repair-chain`
/// opens it (made available, its word not asked).
async fn store(db: &str, raw: &str, proofs: &[(u32, String)]) -> StorageSqlx {
    let storage = StorageSqlx::open_with_options(db, StorageSqlxOptions::default())
        .await
        .unwrap();
    storage
        .migrate("repair-chain", &("02".to_string() + &"ab".repeat(32)))
        .await
        .unwrap();
    let pool = sqlx::SqlitePool::connect(&format!("sqlite:{db}"))
        .await
        .unwrap();
    sqlx::query("UPDATE settings SET chain = ?")
        .bind(raw)
        .execute(&pool)
        .await
        .unwrap();
    for (n, (height, root)) in proofs.iter().enumerate() {
        insert_proof(&pool, &format!("{:02x}", n + 1).repeat(32), *height, root).await;
    }
    pool.close().await;
    drop(storage);
    let storage = StorageSqlx::open_with_options(db, StorageSqlxOptions::default())
        .await
        .unwrap();
    storage.make_available().await.unwrap();
    storage
}

/// The settings row's word as stored, bytes and all.
async fn raw_word(db: &str) -> String {
    let pool = sqlx::SqlitePool::connect(&format!("sqlite:{db}"))
        .await
        .unwrap();
    let word: String = sqlx::query_scalar("SELECT chain FROM settings")
        .fetch_one(&pool)
        .await
        .unwrap();
    pool.close().await;
    word
}

/// The three testnet proofs of the legacy store.
fn three() -> Vec<(u32, String)> {
    vec![(H1, root(0x11)), (H2, root(0x22)), (H3, root(0x33))]
}

/// A mock header service answering `roots` at their heights.
fn mock(roots: &[(u32, String)]) -> MockWalletServices {
    let headers = MockWalletServices::new();
    for (h, r) in roots {
        headers.set_header_for_height(header(*h, r));
    }
    headers
}

/// One run: a fresh store holding `raw` and `proofs`, repaired on testnet
/// with the flags against `headers`.
struct Run {
    _dir: tempfile::TempDir,
    db: String,
    storage: StorageSqlx,
    report: Report,
}

impl Run {
    async fn of<H: HeaderSource>(
        raw: &str,
        proofs: &[(u32, String)],
        headers: Option<&H>,
        flags: (bool, bool),
    ) -> Self {
        let (dir, db) = temp_db();
        let storage = store(&db, raw, proofs).await;
        let report = on_testnet(&storage, &db, headers, flags.0, flags.1).await;
        Self {
            _dir: dir,
            db,
            storage,
            report,
        }
    }

    fn evidence(&self) -> &Evidence {
        self.report.evidence.as_ref().unwrap()
    }

    async fn word(&self) -> String {
        raw_word(&self.db).await
    }
}

/// The dry run (the default) writes nothing and says what `--execute`
/// would write; the legacy `mainnet` reads `main`.
#[tokio::test]
async fn the_dry_run_writes_nothing_and_says_what_execute_would_write() {
    let headers = mock(&three());
    let run = Run::of("mainnet", &three(), Some(&headers), (false, false)).await;
    assert_eq!(
        (run.report.stored.as_str(), run.report.target),
        ("main", "test")
    );
    assert_eq!(run.report.outcome, Outcome::WouldWrite);
    assert!(!run.report.applied);
    assert_eq!(run.evidence().verdict, Verdict::Agrees);
    assert_eq!(headers.call_count("get_header_for_height"), 3);
    assert_eq!(run.word().await, "mainnet", "nothing written");
    let printed = lines(&run.report);
    assert_eq!(
        printed.last().unwrap(),
        "Dry run (no write). Re-run with --execute to write the chain word test."
    );
    assert!(
        printed.iter().any(|l| l.starts_with("Evidence: agrees")),
        "{printed:?}"
    );
}

/// `agrees` with `--execute`: the word written once, `test`; the store
/// then passes the chain word's check on testnet and no longer on mainnet.
#[tokio::test]
async fn agreeing_evidence_with_execute_writes_the_word() {
    let run = Run::of("mainnet", &three(), Some(&mock(&three())), (true, false)).await;
    assert_eq!(run.report.outcome, Outcome::Written);
    assert!(run.report.applied);
    assert_eq!(run.report.refusal, None);
    assert_eq!(run.word().await, "test");
    assert!(lines(&run.report).contains(&"  chain word written: main -> test".to_string()));
    let again = open_unasked(&config_for(&run.db), &run.db).await.unwrap();
    assert_eq!(
        crate::store::require_chain(&again, &run.db, Chain::Test),
        Ok(())
    );
    assert!(crate::store::require_chain(&again, &run.db, Chain::Main).is_err());
}

/// `none` (no proof): `--execute` alone is refused naming `--unchecked`;
/// with `--unchecked` the word is written and the report says with no
/// evidence. The dry run says so too.
#[tokio::test]
async fn no_evidence_writes_only_with_unchecked_and_says_so() {
    let headers = MockWalletServices::new();
    let refused = "repair-chain refused: no evidence (the store holds no proof); \
                   --execute --unchecked writes the word test with no evidence";
    for (flags, outcome, word, said) in [
        (
            (false, false),
            Outcome::WouldWriteOnlyUnchecked,
            "mainnet",
            refused,
        ),
        (
            (false, true),
            Outcome::WouldWrite,
            "mainnet",
            "write the chain word test with no evidence.",
        ),
        (
            (true, false),
            Outcome::Refused,
            "mainnet",
            "Nothing written.",
        ),
        (
            (true, true),
            Outcome::WrittenUnchecked,
            "test",
            "main -> test, with no evidence (--unchecked)",
        ),
    ] {
        let run = Run::of("mainnet", &[], Some(&headers), flags).await;
        assert_eq!(run.evidence().verdict, Verdict::None);
        assert_eq!(
            run.evidence().reason.as_deref(),
            Some("the store holds no proof")
        );
        assert_eq!(run.report.outcome, outcome, "{flags:?}");
        assert_eq!(run.word().await, word);
        assert!(lines(&run.report).join("\n").ends_with(said), "{flags:?}");
        if outcome == Outcome::Refused {
            // `run` prints the lines, then exits with the refusal.
            assert_eq!(run.report.refusal.as_deref(), Some(refused));
        }
    }
    assert_eq!(headers.total_calls(), 0, "no proof, nothing asked");
}

/// `none` (no header service configured, `CHAINTRACKS_URL=off`).
#[tokio::test]
async fn no_header_service_is_no_evidence() {
    let run = Run::of::<MockWalletServices>("mainnet", &three(), None, (true, false)).await;
    assert_eq!(run.evidence().verdict, Verdict::None);
    assert_eq!(
        run.evidence().reason.as_deref(),
        Some("no header service is configured (CHAINTRACKS_URL=off)")
    );
    assert_eq!(run.report.outcome, Outcome::Refused);
    assert_eq!(run.word().await, "mainnet");
}

/// `disagrees`: refused under every flag, naming the heights that
/// disagreed; nothing written.
#[tokio::test]
async fn disagreeing_evidence_is_refused_under_every_flag() {
    // Mainnet's header service at two of the heights: other roots.
    let headers = mock(&[(H1, root(0x99)), (H2, root(0x22)), (H3, root(0x98))]);
    for flags in [(false, false), (false, true), (true, false), (true, true)] {
        let run = Run::of("mainnet", &three(), Some(&headers), flags).await;
        assert_eq!(run.evidence().verdict, Verdict::Disagrees);
        assert_eq!(run.evidence().disagreeing_heights(), vec![H3, H1]);
        let refused = if flags.0 {
            Outcome::Refused
        } else {
            Outcome::WouldRefuse
        };
        assert_eq!(run.report.outcome, refused);
        assert!(!run.report.applied);
        assert_eq!(
            run.report.refusal.as_deref(),
            Some(
                "repair-chain refused: the stored proofs at height 1600020, 1600000 do not carry \
                 the root of chain test's header at that height; this store is not shown to be \
                 chain test's, and no flag writes the word against its evidence"
            )
        );
        assert_eq!(run.word().await, "mainnet", "{flags:?}");
    }
}

/// The word already right: said, nothing written, nothing asked.
#[tokio::test]
async fn a_word_already_right_writes_and_asks_nothing() {
    let headers = MockWalletServices::new();
    let run = Run::of("test", &three(), Some(&headers), (true, true)).await;
    assert_eq!(run.report.outcome, Outcome::Already);
    assert_eq!(run.report.evidence, None);
    assert!(!run.report.applied);
    assert_eq!(headers.total_calls(), 0);
    assert_eq!(
        lines(&run.report).last().unwrap(),
        "The stored chain word already is test: nothing to write."
    );
}

/// Agrees at every height but [`H2`], where it faults.
struct Half(MockWalletServices);

impl HeaderSource for Half {
    async fn header_at(&self, height: u32) -> Result<Vec<u8>> {
        if height == H2 {
            bail!("mock: the header service is down");
        }
        self.0.header_at(height).await
    }
}

/// A header service that faults is "could not look", never an answer:
/// the production client at a closed loopback port gives `none`, and one
/// fault beside agreeing proofs is `none` too.
#[tokio::test]
async fn a_faulting_header_service_is_never_an_answer() {
    seal_network();
    let closed = ChaintracksServiceClient::from_url(CLOSED);
    let run = Run::of("mainnet", &three(), Some(&closed), (true, false)).await;
    assert_eq!(run.evidence().verdict, Verdict::None);
    assert!(run
        .evidence()
        .proofs
        .iter()
        .all(|p| p.answer == "could not look" && p.fault.is_some() && p.header_root.is_none()));
    assert_eq!(run.report.outcome, Outcome::Refused);
    assert_eq!(run.word().await, "mainnet");

    let half = Half(mock(&three()));
    let report = on_testnet(&run.storage, &run.db, Some(&half), true, false).await;
    let answers: Vec<&str> = report
        .evidence
        .unwrap()
        .proofs
        .iter()
        .map(|p| p.answer)
        .collect();
    assert_eq!(answers, ["agrees", "could not look", "agrees"]);
    assert_eq!(report.outcome, Outcome::Refused);
    assert_eq!(run.word().await, "mainnet");
}

/// `--json`'s fields: the same facts.
#[tokio::test]
async fn the_json_report_carries_the_same_facts() {
    let run = Run::of("mainnet", &three(), Some(&mock(&three())), (false, false)).await;
    let proofs = [(3u8, H3), (2, H2), (1, H1)].map(|(n, height)| {
        let r = root(0x11 * n);
        let txid = format!("{n:02x}").repeat(32);
        json!({"txid": txid, "height": height, "stored_root": r, "header_root": r, "answer": "agrees", "fault": null})
    });
    assert_eq!(
        serde_json::to_value(&run.report).unwrap(),
        json!({
            "db": run.db,
            "stored": "main",
            "target": "test",
            "evidence": {"verdict": "agrees", "reason": null, "proofs": proofs},
            "execute": false,
            "unchecked": false,
            "outcome": "would_write",
            "applied": false,
            "refusal": null,
        })
    );
}

/// Only the newest three proofs are read.
#[tokio::test]
async fn the_newest_three_proofs_are_read() {
    let proofs: Vec<(u32, String)> = (0..5u8).map(|i| (H1 + u32::from(i), root(i + 1))).collect();
    let run = Run::of("mainnet", &proofs, Some(&mock(&proofs)), (false, false)).await;
    let heights: Vec<u32> = run.evidence().proofs.iter().map(|p| p.height).collect();
    assert_eq!(heights, vec![H1 + 4, H1 + 3, H1 + 2]);
}

/// A missing file is refused by name, never created.
#[tokio::test]
async fn a_missing_file_is_refused_and_not_created() {
    let (_dir, db) = temp_db();
    let Err(e) = open_unasked(&config_for(&db), &db).await else {
        panic!("a missing file opened");
    };
    assert_eq!(
        e.to_string(),
        format!("no wallet database at {db}: repair-chain repairs an existing one")
    );
    assert!(!std::path::Path::new(&db).exists());
}

/// The decision table, from the words, the verdict and the flags.
#[test]
fn the_decision_table() {
    use Outcome::*;
    use Verdict::{Agrees, Disagrees};
    let none = Some(Verdict::None);
    for (verdict, execute, unchecked, outcome) in [
        (Some(Agrees), false, false, WouldWrite),
        (Some(Agrees), true, false, Written),
        (Some(Agrees), true, true, Written),
        (none, false, false, WouldWriteOnlyUnchecked),
        (none, true, false, Refused),
        (none, true, true, WrittenUnchecked),
        (Some(Disagrees), false, true, WouldRefuse),
        (Some(Disagrees), true, true, Refused),
    ] {
        assert_eq!(decide("main", "test", verdict, execute, unchecked), outcome);
    }
    assert_eq!(decide("test", "test", None, true, true), Already);
}
