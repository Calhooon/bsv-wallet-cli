//! The benches' shared fixtures (#71): a file-backed SQLite wallet of N
//! coins, mined and pending mixed, and the toolbox's mock services standing
//! in for the header service. Nothing reaches the network.
//!
//! The fixtures are the spend gate's own (`src/spend_gate/tests.rs`:
//! `storage_for`, `coin`, `mined_coin`, `headers_with`), restated here
//! because a bench sees the library's public surface only.
//!
//! Every bench times each call on its own and reports p50 and p99 over the
//! calls of criterion's measurement samples (its warm-up calls are dropped),
//! written as one JSON file per bench id for `tools/bench-compare`.

#![allow(dead_code)]

use std::cell::RefCell;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use bsv_sdk::primitives::{to_base64, PrivateKey};
use bsv_sdk::script::LockingScript;
use bsv_sdk::transaction::{MerklePath, Transaction, TransactionInput, TransactionOutput};
use bsv_sdk::wallet::{Counterparty, KeyDeriver, Protocol, SecurityLevel};
use bsv_wallet_toolbox::services::mock::MockWalletServices;
use bsv_wallet_toolbox::{BlockHeader, StorageSqlx, Wallet, WalletStorageWriter};
use chrono::Utc;
use criterion::{BenchmarkGroup, SamplingMode};

pub type BenchWallet = Wallet<StorageSqlx, MockWalletServices>;

/// The wallet sizes of the scoreboard (#106).
pub const SIZES: [usize; 2] = [1_000, 10_000];

/// The derivation prefix of every fixture coin.
pub const PREFIX: &str = "SfKxPIJNgdI=";

/// A payment's P2PKH (the gate tests' `LOCK`).
pub const LOCK: &str = "76a914dbc0a7c84983c5bf199b7b2d41b3acf0408ee5aa88ac";

/// The height of the first mined coin; coin `i` mined is at `FIRST + i`.
pub const FIRST_HEIGHT: u32 = 800_000;
/// The tip: above every coin's block, so each is offered at depth 1.
pub const TIP_HEIGHT: u32 = 900_000;

/// A built wallet and the directory its database lives in.
pub struct Fixture {
    pub wallet: BenchWallet,
    pub root: PrivateKey,
    pub mined: usize,
    _dir: tempfile::TempDir,
}

/// The kind of coin `i` of a wallet: seven in ten mined (received, with a
/// stored proof), two in ten our own change not yet mined, one in ten a
/// received payment not yet mined (held by the gate without the word).
#[derive(Clone, Copy, PartialEq)]
enum Kind {
    Mined,
    OwnPending,
    ReceivedPending,
}

fn kind(i: usize) -> Kind {
    match i % 10 {
        0..=6 => Kind::Mined,
        7 | 8 => Kind::OwnPending,
        _ => Kind::ReceivedPending,
    }
}

/// A coin's value, spread over 1,000 to 49,999 sats.
fn satoshis(i: usize) -> u64 {
    1_000 + (i as u64 * 7_919) % 49_000
}

/// The block header at `height` whose merkle root is `root` (the tracker
/// host tests' `header`).
pub fn header(height: u32, root: &str) -> BlockHeader {
    BlockHeader {
        version: 1,
        previous_hash: "00".repeat(32),
        merkle_root: root.to_string(),
        time: 1_700_000_000,
        bits: 0x1d00_ffff,
        nonce: 0,
        hash: String::new(),
        height,
    }
}

/// [`header`] with its hash filled in (the tracker host tests'
/// `header_with_hash`).
pub fn header_with_hash(height: u32, root: &str) -> BlockHeader {
    let mut h = header(height, root);
    let mut hash = bsv_sdk::primitives::hash::sha256d(&h.to_binary()).to_vec();
    hash.reverse();
    h.hash = hex::encode(hash);
    h
}

/// Coin `i`'s BRC-29 derivation (our own, counterparty self, as the
/// toolbox's change): its suffix and its P2PKH, so the toolbox can sign it.
fn derived(deriver: &KeyDeriver, i: usize) -> (String, Vec<u8>) {
    let suffix = to_base64(&(i as u64).to_le_bytes());
    let key = deriver
        .derive_public_key(
            &Protocol::new(SecurityLevel::Counterparty, "3241645161d8"),
            &format!("{PREFIX} {suffix}"),
            &Counterparty::Self_,
            true,
        )
        .unwrap();
    let mut script = vec![0x76, 0xa9, 0x14];
    script.extend_from_slice(&key.hash160());
    script.extend_from_slice(&[0x88, 0xac]);
    (suffix, script)
}

/// A one-output transaction to `lock` spending a made-up outpoint
/// numbered `n`.
fn parent(n: u64, satoshis: u64, lock: &[u8]) -> Transaction {
    let lock = LockingScript::from_binary(lock).unwrap();
    let mut seed = [0u8; 32];
    seed[..8].copy_from_slice(&n.to_le_bytes());
    seed[31] = 0xbe;
    let mut tx = Transaction::new();
    tx.add_input(TransactionInput::new(hex::encode(seed), 0))
        .unwrap();
    tx.add_output(TransactionOutput::new(satoshis, lock))
        .unwrap();
    tx
}

/// A file-backed wallet of `coins` coins (see [`Kind`]); the mock header
/// service knows each mined coin's block and the tip.
pub async fn wallet_of(coins: usize) -> Fixture {
    let root = PrivateKey::random();
    let identity = root.public_key().to_hex();
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("wallet.db");
    let storage = StorageSqlx::open(path.to_str().unwrap()).await.unwrap();
    storage.migrate("bench", &identity).await.unwrap();
    storage.make_available().await.unwrap();
    let (user, _) = storage.find_or_insert_user(&identity).await.unwrap();
    let basket = storage
        .find_or_create_default_basket(user.user_id)
        .await
        .unwrap()
        .basket_id;

    let services = MockWalletServices::builder().height(TIP_HEIGHT).build();
    services.set_tip_header(header_with_hash(TIP_HEIGHT, &"0c".repeat(32)));
    let when = Utc::now() - chrono::Duration::minutes(9);
    let mut db = storage.write_tx().await.unwrap();
    let mut mined = 0;
    let deriver = KeyDeriver::new(Some(root.clone()));
    for i in 0..coins {
        let kind = kind(i);
        let sats = satoshis(i);
        let (suffix, lock) = derived(&deriver, i);
        let tx = parent(i as u64, sats, &lock);
        let txid = tx.id();
        let raw = tx.to_binary();
        let status = if kind == Kind::Mined {
            "completed"
        } else {
            "unproven"
        };
        let tx_row = sqlx::query(
            "INSERT INTO transactions (user_id, status, reference, is_outgoing, satoshis, version, lock_time, description, txid, raw_tx, created_at, updated_at) \
             VALUES (?, ?, ?, ?, 0, 1, 0, 'bench', ?, ?, ?, ?)",
        )
        .bind(user.user_id)
        .bind(status)
        .bind(format!("bench-{i}"))
        .bind((kind == Kind::OwnPending) as i64)
        .bind(&txid)
        .bind(&raw)
        .bind(when)
        .bind(when)
        .execute(&mut *db)
        .await
        .unwrap()
        .last_insert_rowid();
        if kind == Kind::Mined {
            let height = FIRST_HEIGHT + i as u32;
            sqlx::query(
                "INSERT INTO proven_txs (txid, height, idx, block_hash, merkle_root, merkle_path, raw_tx, created_at, updated_at) \
                 VALUES (?, ?, 0, ?, ?, ?, ?, ?, ?)",
            )
            .bind(&txid)
            .bind(height as i64)
            .bind("bb".repeat(32))
            .bind(&txid)
            .bind(MerklePath::from_coinbase_txid(&txid, height).to_binary())
            .bind(&raw)
            .bind(when)
            .bind(when)
            .execute(&mut *db)
            .await
            .unwrap();
            services.set_header_for_height(header(height, &txid));
            mined += 1;
        }
        sqlx::query(
            "INSERT INTO outputs (user_id, transaction_id, basket_id, vout, satoshis, locking_script, txid, type, spendable, change, provided_by, purpose, output_description, derivation_prefix, derivation_suffix, created_at, updated_at) \
             VALUES (?, ?, ?, 0, ?, ?, ?, 'P2PKH', 1, 1, 'storage', 'change', 'c', ?, ?, ?, ?)",
        )
        .bind(user.user_id)
        .bind(tx_row)
        .bind(basket)
        .bind(sats as i64)
        .bind(&lock)
        .bind(&txid)
        .bind(PREFIX)
        .bind(&suffix)
        .bind(when)
        .bind(when)
        .execute(&mut *db)
        .await
        .unwrap();
    }
    db.commit().await.unwrap();

    let wallet = Wallet::new(Some(root.clone()), storage, services)
        .await
        .unwrap();
    Fixture {
        wallet,
        root,
        mined,
        _dir: dir,
    }
}

/// A current-thread runtime, the shape of one request's task.
pub fn runtime() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
}

/// Per-call timings of one bench id. Criterion calls the routine once per
/// sample (flat sampling: the same call count each) after its warm-up
/// calls; the last `sample_size` routines' calls are the measurement.
pub struct Calls {
    routines: RefCell<Vec<Vec<Duration>>>,
}

impl Calls {
    pub fn new() -> Self {
        Self {
            routines: RefCell::new(Vec::new()),
        }
    }

    /// Start one routine of criterion's.
    pub fn routine(&self) {
        self.routines.borrow_mut().push(Vec::new());
    }

    pub fn record(&self, d: Duration) {
        self.routines
            .borrow_mut()
            .last_mut()
            .expect("routine started")
            .push(d);
    }

    /// The measurement's calls, sorted: the last `samples` routines.
    fn measured(&self, samples: usize) -> Vec<Duration> {
        let routines = self.routines.borrow();
        let from = routines.len().saturating_sub(samples);
        let mut all: Vec<Duration> = routines[from..].iter().flatten().copied().collect();
        all.sort();
        all
    }
}

/// Time `op` once.
pub async fn timed<T, F: std::future::Future<Output = T>>(calls: &Calls, op: F) -> (T, Duration) {
    let start = Instant::now();
    let out = op.await;
    let d = start.elapsed();
    calls.record(d);
    (out, d)
}

/// The nearest-rank percentile `p` (0 to 100) of sorted timings.
fn percentile(sorted: &[Duration], p: f64) -> Duration {
    let rank = ((p / 100.0) * sorted.len() as f64).ceil() as usize;
    sorted[rank.clamp(1, sorted.len()) - 1]
}

/// Where the per-bench JSON goes: `BENCH_OUT`, else the target's bench
/// scratch directory.
fn out_dir() -> PathBuf {
    std::env::var_os("BENCH_OUT")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("bench-percentiles"))
}

/// Report `id`'s p50 and p99 over the measurement's calls: printed, and
/// written to `<out>/<id with / as __>.json`.
pub fn report(id: &str, calls: &Calls, samples: usize) {
    let measured = calls.measured(samples);
    if measured.is_empty() {
        // `cargo bench -- <filter>` skipped this id.
        return;
    }
    let us = |d: Duration| d.as_secs_f64() * 1e6;
    let (p50, p99) = (percentile(&measured, 50.0), percentile(&measured, 99.0));
    println!(
        "percentiles {id}: p50 {:.1} us, p99 {:.1} us over {} calls",
        us(p50),
        us(p99),
        measured.len()
    );
    let dir = out_dir();
    std::fs::create_dir_all(&dir).unwrap();
    let body = serde_json::json!({
        "id": id,
        "p50_us": (us(p50) * 10.0).round() / 10.0,
        "p99_us": (us(p99) * 10.0).round() / 10.0,
        "calls": measured.len(),
    });
    std::fs::write(
        dir.join(format!("{}.json", id.replace('/', "__"))),
        serde_json::to_string_pretty(&body).unwrap(),
    )
    .unwrap();
}

/// A group's settings: flat sampling (every sample the same call count, so
/// the last `samples` routines are the measurement), `samples` samples over
/// `measure`.
pub fn configure<M: criterion::measurement::Measurement>(
    group: &mut BenchmarkGroup<'_, M>,
    samples: usize,
    warm_up: Duration,
    measure: Duration,
) {
    group
        .sampling_mode(SamplingMode::Flat)
        .sample_size(samples)
        .warm_up_time(warm_up)
        .measurement_time(measure);
}
