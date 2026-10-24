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
use std::ops::AsyncFnMut;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use bsv_sdk::primitives::{to_base64, PrivateKey};
use bsv_sdk::script::LockingScript;
use bsv_sdk::transaction::{MerklePath, Transaction, TransactionInput, TransactionOutput};
use bsv_sdk::wallet::{Counterparty, KeyDeriver, Protocol, SecurityLevel};
use bsv_wallet_toolbox::services::mock::MockWalletServices;
use bsv_wallet_toolbox::{BlockHeader, StorageSqlx, Wallet, WalletStorageWriter};
use chrono::{DateTime, Utc};
use criterion::measurement::WallTime;
use criterion::{BenchmarkGroup, BenchmarkId, SamplingMode};
use sqlx::SqliteConnection;
use tokio::runtime::Runtime;

pub type BenchWallet = Wallet<StorageSqlx, MockWalletServices>;

/// The wallet sizes of the scoreboard (#106).
pub const SIZES: [usize; 2] = [1_000, 10_000];

/// Criterion's samples per bench id. Flat sampling gives every sample the
/// same call count, at least one, so the measurement is at least this many
/// calls and a p99 rests on at least 100 whatever the machine's speed (a
/// measure time too short for 100 calls is stretched by criterion, which
/// says so). `tools/bench-compare` fails a result of fewer (`MIN_CALLS`).
pub const SAMPLES: usize = 100;

/// BRC-29's protocol id.
pub const BRC29: &str = "3241645161d8";

/// The derivation prefix of every fixture coin and payment.
pub const PREFIX: &str = "SfKxPIJNgdI=";

/// A payment's P2PKH (the gate tests' `LOCK`).
pub const LOCK: &str = "76a914dbc0a7c84983c5bf199b7b2d41b3acf0408ee5aa88ac";

/// The height of the first mined coin; coin `i` mined is at `FIRST + i`.
pub const FIRST_HEIGHT: u32 = 800_000;
/// The tip: above every coin's block, so each is offered at depth 1.
pub const TIP_HEIGHT: u32 = 900_000;

/// A built wallet and the directory its database lives in, with a closed
/// copy of the database (`template.db`) that [`Fixture::fresh`] copies for a
/// bench whose call changes the wallet.
pub struct Fixture {
    pub wallet: BenchWallet,
    pub root: PrivateKey,
    pub coins: usize,
    pub mined: usize,
    /// Each mined coin's block: its height and merkle root.
    blocks: Vec<(u32, String)>,
    template: PathBuf,
    _dir: tempfile::TempDir,
}

/// A wallet on its own copy of a fixture's database, as built: `coins`
/// coins at every call however the call before it changed its copy.
pub struct Fresh {
    pub wallet: BenchWallet,
    _dir: tempfile::TempDir,
}

/// The mock header service of a fixture: the tip, and each mined coin's
/// block.
fn services_for(blocks: &[(u32, String)]) -> MockWalletServices {
    let services = MockWalletServices::builder().height(TIP_HEIGHT).build();
    services.set_tip_header(header_with_hash(TIP_HEIGHT, &"0c".repeat(32)));
    for (height, root) in blocks {
        services.set_header_for_height(header(*height, root));
    }
    services
}

/// The wallet's spendable coins, counted in its database.
pub async fn coin_count(wallet: &BenchWallet) -> usize {
    let (n,): (i64,) = sqlx::query_as("SELECT COUNT(*) FROM outputs WHERE spendable = 1")
        .fetch_one(wallet.storage().pool())
        .await
        .unwrap();
    usize::try_from(n).unwrap()
}

/// The storage on the database at `path`, made available; a new database
/// is migrated for `identity` first.
async fn storage_at(path: &Path, identity: Option<&str>) -> StorageSqlx {
    let storage = StorageSqlx::open(path.to_str().unwrap()).await.unwrap();
    if let Some(identity) = identity {
        storage.migrate("bench", identity).await.unwrap();
    }
    storage.make_available().await.unwrap();
    storage
}

/// The wallet of `root` on `storage`, the mock header service knowing
/// `blocks`.
async fn wallet_on(
    root: &PrivateKey,
    storage: StorageSqlx,
    blocks: &[(u32, String)],
) -> BenchWallet {
    Wallet::new(Some(root.clone()), storage, services_for(blocks))
        .await
        .unwrap()
}

impl Fixture {
    /// A wallet on a new copy of the built database and a new mock header
    /// service: the setup of one call of a bench that writes, done before
    /// its clock starts.
    pub async fn fresh(&self) -> Fresh {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("wallet.db");
        std::fs::copy(&self.template, &path).unwrap();
        let storage = storage_at(&path, None).await;
        let wallet = wallet_on(&self.root, storage, &self.blocks).await;
        Fresh { wallet, _dir: dir }
    }

    /// The built database still holds its `coins` coins: no call wrote to
    /// it.
    pub async fn assert_as_built(&self) {
        assert_eq!(coin_count(&self.fresh().await.wallet).await, self.coins);
    }
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
        height,
        merkle_root: root.to_string(),
        previous_hash: "00".repeat(32),
        version: 1,
        time: 1_700_000_000,
        bits: 0x1d00_ffff,
        ..BlockHeader::default()
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

/// The P2PKH of the BRC-29 key `PREFIX suffix` of `deriver`'s root toward
/// `counterparty`, so the toolbox can sign for it.
pub fn brc29_lock(deriver: &KeyDeriver, suffix: &str, counterparty: &Counterparty) -> Vec<u8> {
    let key = deriver
        .derive_public_key(
            &Protocol::new(SecurityLevel::Counterparty, BRC29),
            &format!("{PREFIX} {suffix}"),
            counterparty,
            true,
        )
        .unwrap();
    let mut script = vec![0x76, 0xa9, 0x14];
    script.extend_from_slice(&key.hash160());
    script.extend_from_slice(&[0x88, 0xac]);
    script
}

/// A one-output transaction of `satoshis` to `lock` spending a made-up
/// outpoint: `n` and `tag` (the outpoint's last byte) make it unique.
pub fn parent(n: u64, tag: u8, satoshis: u64, lock: &[u8]) -> Transaction {
    let lock = LockingScript::from_binary(lock).unwrap();
    let mut seed = [0u8; 32];
    seed[..8].copy_from_slice(&n.to_le_bytes());
    seed[31] = tag;
    let mut tx = Transaction::new();
    tx.add_input(TransactionInput::new(hex::encode(seed), 0))
        .unwrap();
    tx.add_output(TransactionOutput::new(satoshis, lock))
        .unwrap();
    tx
}

/// Whose coins a fixture's rows are: the user, the default basket, and the
/// rows' timestamp.
struct Owner {
    user_id: i64,
    basket_id: i64,
    when: DateTime<Utc>,
}

/// The stored proof of the mined `txid` at `height`.
async fn insert_proof(
    db: &mut SqliteConnection,
    txid: &str,
    height: u32,
    raw: &[u8],
    when: DateTime<Utc>,
) {
    sqlx::query(
        "INSERT INTO proven_txs (txid, height, idx, block_hash, merkle_root, merkle_path, raw_tx, created_at, updated_at) \
         VALUES (?, ?, 0, ?, ?, ?, ?, ?, ?)",
    )
    .bind(txid)
    .bind(i64::from(height))
    .bind("bb".repeat(32))
    .bind(txid)
    .bind(MerklePath::from_coinbase_txid(txid, height).to_binary())
    .bind(raw)
    .bind(when)
    .bind(when)
    .execute(db)
    .await
    .unwrap();
}

/// Coin `i` (see [`Kind`]): its transaction, its proof when mined, and its
/// output, our own change derived from `deriver`'s root. Returns a mined
/// coin's block.
async fn insert_coin(
    db: &mut SqliteConnection,
    owner: &Owner,
    deriver: &KeyDeriver,
    i: usize,
) -> Option<(u32, String)> {
    let kind = kind(i);
    let sats = satoshis(i);
    let suffix = to_base64(&(i as u64).to_le_bytes());
    let lock = brc29_lock(deriver, &suffix, &Counterparty::Self_);
    let tx = parent(i as u64, 0xbe, sats, &lock);
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
    .bind(owner.user_id)
    .bind(status)
    .bind(format!("bench-{i}"))
    .bind(i64::from(kind == Kind::OwnPending))
    .bind(&txid)
    .bind(&raw)
    .bind(owner.when)
    .bind(owner.when)
    .execute(&mut *db)
    .await
    .unwrap()
    .last_insert_rowid();
    let block = if kind == Kind::Mined {
        let height = FIRST_HEIGHT + u32::try_from(i).unwrap();
        insert_proof(db, &txid, height, &raw, owner.when).await;
        Some((height, txid.clone()))
    } else {
        None
    };
    sqlx::query(
        "INSERT INTO outputs (user_id, transaction_id, basket_id, vout, satoshis, locking_script, txid, type, spendable, change, provided_by, purpose, output_description, derivation_prefix, derivation_suffix, created_at, updated_at) \
         VALUES (?, ?, ?, 0, ?, ?, ?, 'P2PKH', 1, 1, 'storage', 'change', 'c', ?, ?, ?, ?)",
    )
    .bind(owner.user_id)
    .bind(tx_row)
    .bind(owner.basket_id)
    .bind(i64::try_from(sats).unwrap())
    .bind(&lock)
    .bind(&txid)
    .bind(PREFIX)
    .bind(&suffix)
    .bind(owner.when)
    .bind(owner.when)
    .execute(db)
    .await
    .unwrap();
    block
}

/// A file-backed wallet of `coins` coins (see [`Kind`]); the mock header
/// service knows each mined coin's block and the tip.
pub async fn wallet_of(coins: usize) -> Fixture {
    let root = PrivateKey::random();
    let identity = root.public_key().to_hex();
    let dir = tempfile::tempdir().unwrap();
    let storage = storage_at(&dir.path().join("wallet.db"), Some(&identity)).await;
    let user_id = storage
        .find_or_insert_user(&identity)
        .await
        .unwrap()
        .0
        .user_id;
    let owner = Owner {
        user_id,
        basket_id: storage
            .find_or_create_default_basket(user_id)
            .await
            .unwrap()
            .basket_id,
        when: Utc::now() - chrono::Duration::minutes(9),
    };

    let mut db = storage.write_tx().await.unwrap();
    let mut blocks = Vec::new();
    let deriver = KeyDeriver::new(Some(root.clone()));
    for i in 0..coins {
        blocks.extend(insert_coin(&mut db, &owner, &deriver, i).await);
    }
    db.commit().await.unwrap();

    // A whole copy of the database in one file, for `Fixture::fresh`.
    let template = dir.path().join("template.db");
    sqlx::query("VACUUM INTO ?")
        .bind(template.to_str().unwrap())
        .execute(storage.writer())
        .await
        .unwrap();

    let wallet = wallet_on(&root, storage, &blocks).await;
    Fixture {
        wallet,
        root,
        coins,
        mined: blocks.len(),
        blocks,
        template,
        _dir: dir,
    }
}

/// A current-thread runtime, the shape of one request's task.
pub fn runtime() -> Runtime {
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
fn percentile(sorted: &[Duration], p: usize) -> Duration {
    let rank = (p * sorted.len()).div_ceil(100);
    sorted[rank.clamp(1, sorted.len()) - 1]
}

/// Where the per-bench JSON goes: `BENCH_OUT`, else the target's bench
/// scratch directory.
fn out_dir() -> PathBuf {
    std::env::var_os("BENCH_OUT").map_or_else(
        || PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("bench-percentiles"),
        PathBuf::from,
    )
}

/// Report `id`'s p50 and p99 over the measurement's calls: printed, and
/// written to `<out>/<id with / as __>.json`.
pub fn report(id: &str, calls: &Calls) {
    let measured = calls.measured(SAMPLES);
    if measured.is_empty() {
        // `cargo bench -- <filter>` skipped this id.
        return;
    }
    let us = |d: Duration| d.as_secs_f64() * 1e6;
    let (p50, p99) = (percentile(&measured, 50), percentile(&measured, 99));
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
/// the last [`SAMPLES`] routines are the measurement), [`SAMPLES`] samples
/// over `measure`.
pub fn configure<M: criterion::measurement::Measurement>(
    group: &mut BenchmarkGroup<'_, M>,
    warm_up: Duration,
    measure: Duration,
) {
    group
        .sampling_mode(SamplingMode::Flat)
        .sample_size(SAMPLES)
        .warm_up_time(warm_up)
        .measurement_time(measure);
}

/// Bench `bench/name/coins` of `group`: criterion's every routine runs
/// `call` its count of times, each call returning its own timed duration
/// (its setup untimed), then `routine_end` once; then p50 and p99 are
/// reported (see [`report`]).
pub fn measure_calls(
    group: &mut BenchmarkGroup<'_, WallTime>,
    rt: &Runtime,
    (bench, name, coins): (&str, &str, usize),
    mut call: impl AsyncFnMut(&Calls) -> Duration,
    mut routine_end: impl AsyncFnMut(),
) {
    let calls = Calls::new();
    group.bench_function(BenchmarkId::new(name, coins), |b| {
        b.iter_custom(|iters| {
            calls.routine();
            rt.block_on(async {
                let mut total = Duration::ZERO;
                for _ in 0..iters {
                    total += call(&calls).await;
                }
                routine_end().await;
                total
            })
        });
    });
    report(&format!("{bench}/{name}/{coins}"), &calls);
}
