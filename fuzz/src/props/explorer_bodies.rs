//! `fuzz_targets/explorer_bodies.rs`'s property, with the local socket
//! that serves each case.

use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

use super::super::{p2pkh, raw_tx, txid_of};
use bsv_wallet_cli::broadcast_verify::{
    BroadcastVerification, BroadcastVerifier, ChainIndexAnswer, NetworkEvidence, PresenceReport,
};
use bsv_wallet_cli::config::Config;
use bsv_wallet_cli::explorers::Explorers;
use bsv_wallet_toolbox::Chain;

const STATUSES: [u16; 8] = [200, 200, 200, 200, 404, 500, 429, 302];

type Answer = Arc<Mutex<(u16, Vec<u8>)>>;

struct Local {
    base: String,
    answer: Answer,
    runtime: tokio::runtime::Runtime,
}

/// The transaction every case asks about: `examples/seed_corpus.rs`'s
/// control transaction, so a seed can carry bytes that hash to it.
fn txid() -> &'static str {
    static TXID: OnceLock<String> = OnceLock::new();
    TXID.get_or_init(|| {
        let funding = "11".repeat(32);
        txid_of(&raw_tx(&[(&funding, 0)], &[(1000, p2pkh([0xdb; 20]))]))
    })
}

/// One socket for the run, answering each request by its path and closing
/// the connection: `/fz` the current answer, `/arc` a 500, anything else a
/// clean JSON 404.
fn local() -> &'static Local {
    static LOCAL: OnceLock<Local> = OnceLock::new();
    LOCAL.get_or_init(|| {
        let listener = TcpListener::bind("127.0.0.1:0").expect("a local port");
        let base = format!("http://{}", listener.local_addr().expect("an address"));
        let answer: Answer = Arc::new(Mutex::new((404, Vec::new())));
        let serving = answer.clone();
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { continue };
                let mut request = Vec::new();
                let mut buf = [0u8; 1024];
                while !request.windows(4).any(|w| w == b"\r\n\r\n") {
                    match stream.read(&mut buf) {
                        Ok(0) | Err(_) => break,
                        Ok(n) => request.extend_from_slice(&buf[..n]),
                    }
                }
                let path = request
                    .split(|b| *b == b' ')
                    .nth(1)
                    .unwrap_or_default()
                    .to_vec();
                let (status, body) = if path.starts_with(b"/fz") {
                    serving.lock().expect("the answer").clone()
                } else if path.starts_with(b"/arc") {
                    (500, b"{}".to_vec())
                } else {
                    (404, b"{}".to_vec())
                };
                let head = format!(
                    "HTTP/1.1 {status} X\r\ncontent-length: {}\r\ncontent-type: application/json\r\nconnection: close\r\n\r\n",
                    body.len()
                );
                let _ = stream.write_all(head.as_bytes());
                let _ = stream.write_all(&body);
            }
        });
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("a runtime");
        Local {
            base,
            answer,
            runtime,
        }
    })
}

/// `BananaBlocks` paces itself per base, one bucket a process (30 a burst,
/// 0.5 a second): the base turns over a path suffix the socket ignores, so
/// the bucket a call meets is rarely empty. A bounded set of bases keeps the
/// process's table of gates bounded.
fn bananablocks_base(prefix: &str) -> String {
    static TURN: AtomicU64 = AtomicU64::new(0);
    let n = TURN.fetch_add(1, Ordering::Relaxed) % 65_536;
    format!("{prefix}/b{n}")
}

/// Which chain index a case fuzzes.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Index {
    WhatsOnChain,
    Bitails,
    BananaBlocks,
}

/// One verification pass with `fuzzed` at the socket's `/fz` and every
/// other source quiet. `WhatsOnChain` and `Bitails` go through `single_pass`
/// with `BananaBlocks` off (the process's `Explorers` cell is built off by
/// whichever of them runs first, and never at a base). `BananaBlocks` goes
/// through `explicit` with a fresh provider at a turning base and a quiet
/// WhatsOnChain-shaped index as the one voter.
async fn verify(base: &str, fuzzed: Index) -> PresenceReport {
    let arc = format!("{base}/arc");
    let quiet = format!("{base}/quiet");
    let fz = format!("{base}/fz");
    let report = if fuzzed == Index::BananaBlocks {
        BroadcastVerifier::explicit(true, &arc, Some(&quiet))
            .with_bananablocks(&bananablocks_base(&fz))
            .verify_report(txid())
            .await
    } else {
        let at = |index: Index| if index == fuzzed { &fz } else { &quiet };
        let config = Config::from_pairs(&[
            ("ARC_URL", arc.as_str()),
            ("ARC_GORILLAPOOL_URL", ""),
            ("WOC_BASE", at(Index::WhatsOnChain).as_str()),
            ("BITAILS_BASE", at(Index::Bitails).as_str()),
            ("BANANABLOCKS_BASE", ""),
        ])
        .expect("the configuration");
        BroadcastVerifier::single_pass(Chain::Main, &config)
            .verify_report(txid())
            .await
    };
    if std::env::var_os("EXPLORER_BODIES_VERDICT").is_some() {
        let index = match fuzzed {
            Index::WhatsOnChain => "whatsonchain",
            Index::Bitails => "bitails",
            Index::BananaBlocks => "bananablocks",
        };
        let status = local().answer.lock().expect("the answer").0;
        eprintln!(
            "verdict index={index} status={status} answer={:?} by={:?}",
            report.chain_index, report.chain_index_by
        );
    }
    report
}

/// The body as the readers take it: `reqwest`'s `text()`, which decodes a
/// body with no charset through `encoding_rs`'s UTF-8 `decode`: a byte-order
/// mark is sniffed and dropped (a UTF-16 one decodes the rest as UTF-16), an
/// invalid sequence replaced. `from_utf8_lossy` keeps a BOM, so it is not
/// the readers' text.
fn text(body: &[u8]) -> String {
    encoding_rs::UTF_8.decode(body).0.into_owned()
}

fn json(body: &[u8]) -> Option<serde_json::Value> {
    serde_json::from_str(&text(body)).ok()
}

/// A voting index's answer against the status it was served.
fn check_voting_index(report: &PresenceReport, status: u16, name: &str, mined: bool) {
    match report.chain_index {
        ChainIndexAnswer::Present(evidence) => {
            assert_eq!(status, 200, "present from a {status}");
            assert_eq!(report.chain_index_by, Some(name));
            assert_eq!(evidence == NetworkEvidence::Mined, mined, "{evidence:?}");
        }
        ChainIndexAnswer::Absent => assert_eq!(status, 404, "absent from a {status}"),
        ChainIndexAnswer::Unknown => assert!(
            status != 200 && status != 404,
            "could not look from a {status}"
        ),
    }
    if report.verification == BroadcastVerification::Rejected {
        assert_eq!(status, 404, "rejected from a {status}");
    }
}

/// `BananaBlocks`' index against the status and body it was served, beside
/// a voting index's clean 404.
fn check_bananablocks_index(report: &PresenceReport, status: u16, body: &[u8]) {
    let about = json(body)
        .and_then(|v| v.get("txid").and_then(|t| t.as_str()).map(str::to_string))
        .is_some_and(|t| t.eq_ignore_ascii_case(txid()));
    match report.chain_index {
        ChainIndexAnswer::Present(evidence) => {
            assert_eq!(status, 200, "mined from a {status}");
            assert!(about, "mined from a body about another transaction");
            assert_eq!(evidence, NetworkEvidence::Mined);
            assert_eq!(report.chain_index_by, Some("bananablocks"));
        }
        ChainIndexAnswer::Absent => {
            if report.chain_index_by.is_some() {
                assert_eq!(status, 200, "known from a {status}");
                assert!(about, "known from a body about another transaction");
            }
        }
        ChainIndexAnswer::Unknown => {
            panic!("BananaBlocks' answer stood against the voting indexes' 404s")
        }
    }
}

pub fn check(data: &[u8]) {
    if data.len() < 2 {
        return;
    }
    let local = local();
    let status = STATUSES[data[1] as usize % STATUSES.len()];
    let body = &data[2..];
    *local.answer.lock().expect("the answer") = (status, body.to_vec());
    let base = local.base.as_str();
    let fz = format!("{base}/fz");

    local.runtime.block_on(async {
        match data[0] % 5 {
            0 => {
                let explorers = Explorers::at(Some(&fz), None, None);
                if let Ok(bytes) = explorers.woc_beef(txid()).await {
                    assert_eq!(status, 200, "a BEEF from a {status}");
                    assert_eq!(Ok(bytes), hex::decode(text(body).trim()));
                }
            }
            1 => {
                let explorers = Explorers::at(None, Some(&bananablocks_base(&fz)), None);
                if let Ok(bytes) = explorers.bananablocks_raw_tx(txid()).await {
                    assert_eq!(status, 200, "bytes from a {status}");
                    assert_eq!(
                        txid_of(&bytes),
                        txid(),
                        "bytes that do not hash to the txid"
                    );
                }
            }
            2 => {
                let report = verify(base, Index::WhatsOnChain).await;
                let mined = json(body)
                    .and_then(|v| v.get("confirmations").and_then(serde_json::Value::as_i64))
                    .is_some_and(|c| c >= 1);
                check_voting_index(&report, status, "whatsonchain", mined);
            }
            3 => {
                let report = verify(base, Index::Bitails).await;
                let mined = json(body)
                    .and_then(|v| v.get("blockHeight").map(|h| h.as_u64().is_some()))
                    .unwrap_or(false);
                check_voting_index(&report, status, "bitails", mined);
            }
            _ => {
                let report = verify(base, Index::BananaBlocks).await;
                check_bananablocks_index(&report, status, body);
            }
        }
    });
}
