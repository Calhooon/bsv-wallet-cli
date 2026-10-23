//! Each explorer answer `src/explorers.rs` reads, on arbitrary bytes: the
//! spender rungs (WhatsOnChain's `/tx/{txid}/{vout}/spent`, BananaBlocks'
//! `/txo/{txid}/{vout}` with and without the asked script), WhatsOnChain's
//! `/tx/{txid}/beef` and BananaBlocks' `/tx/{txid}/hex`. The parsers sit
//! inside the calls, so the target serves the fuzzed status and body from
//! a local socket and makes the calls (`Explorers::at`). None may panic;
//! a named spender is 64 lowercase hex; a 404 from WhatsOnChain's spend
//! route is "knows no spend", and any non-200 from the others is a fault.
//!
//! Input: byte 0 picks the call, byte 1 the status, the rest is the body.
#![no_main]

use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

use bsv_wallet_cli::explorers::{Explorers, Rung};
use libfuzzer_sys::fuzz_target;

const SRC: &str = "aa00000000000000000000000000000000000000000000000000000000000001";
const STATUSES: [u16; 8] = [200, 200, 200, 200, 404, 500, 429, 302];

type Answer = Arc<Mutex<(u16, Vec<u8>)>>;

struct Local {
    base: String,
    answer: Answer,
    runtime: tokio::runtime::Runtime,
}

/// One socket for the run, answering every request with the current answer
/// and closing the connection.
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
                let (status, body) = serving.lock().expect("the answer").clone();
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

/// BananaBlocks paces itself per base, one bucket a process (30 a burst,
/// 0.5 a second): the base turns over a path prefix the socket ignores, so
/// the bucket a call meets is rarely empty. A bounded set of bases keeps the
/// process's table of gates bounded.
fn bananablocks_base(base: &str) -> String {
    static TURN: AtomicU64 = AtomicU64::new(0);
    let n = TURN.fetch_add(1, Ordering::Relaxed) % 65_536;
    format!("{base}/b{n}")
}

fn is_txid_lower(s: &str) -> bool {
    s.len() == 64 && s.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

fuzz_target!(|data: &[u8]| {
    if data.len() < 2 {
        return;
    }
    let local = local();
    let status = STATUSES[data[1] as usize % STATUSES.len()];
    *local.answer.lock().expect("the answer") = (status, data[2..].to_vec());

    let explorers = Explorers::at(
        Some(&local.base),
        Some(&bananablocks_base(&local.base)),
        None,
    );
    let script = [
        0x76u8, 0xa9, 0x14, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 0x88, 0xac,
    ];
    local.runtime.block_on(async {
        match data[0] % 5 {
            0 => {
                let answer = explorers.spender(Rung::WhatsOnChain, SRC, 0, None).await;
                match &answer {
                    Ok(Some(spender)) => {
                        assert_eq!(status, 200);
                        assert!(is_txid_lower(spender), "{spender:?}");
                    }
                    Ok(None) => assert_eq!(status, 404, "no spend from a {status}"),
                    Err(_) => {}
                }
            }
            1 | 2 => {
                let asked = (data[0] % 5 == 2).then_some(&script[..]);
                let answer = explorers.spender(Rung::BananaBlocks, SRC, 0, asked).await;
                if let Ok(word) = &answer {
                    assert_eq!(status, 200, "an answer from a {status}");
                    if let Some(spender) = word {
                        assert!(is_txid_lower(spender), "{spender:?}");
                    }
                }
            }
            3 => {
                if explorers.woc_beef(SRC).await.is_ok() {
                    assert_eq!(status, 200);
                }
            }
            _ => {
                if explorers.bananablocks_raw_tx(SRC).await.is_ok() {
                    panic!("bytes delivered for a txid no bytes hash to");
                }
            }
        }
    });
});
