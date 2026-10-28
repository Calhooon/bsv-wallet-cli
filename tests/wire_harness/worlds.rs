//! The worlds a fixture is served from, each built from its name alone.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use axum::Router;
use bsv_sdk::primitives::PrivateKey;
use bsv_wallet_cli::server::{self, ServerConfig};
use bsv_wallet_toolbox::{Services, StorageSqlx, Wallet, WalletStorageWriter};

use super::fixture::NORMALIZER;
use crate::wire_support::{header_stub, serve_local, services, Req, Resp, CLOSED, J, ROOT_KEY};

// ---------------------------------------------------------------------------
// The worlds.
// ---------------------------------------------------------------------------

/// The bearer of the `auth` world, and the callback token of `callback`.
pub(crate) const WIRE_BEARER: &str = "wire-bearer";
pub(crate) const WIRE_CALLBACK: &str = "wire-callback";

/// The height and root of the seeded proofs (the header stub's tip is
/// [`wire_support::TIP`]).
pub(crate) const MINED_AT: u32 = 900_000;
const ROOT: &str = "5555555555555555555555555555555555555555555555555555555555555555";

/// The seeded transactions (hand-written rows, never broadcast).
pub(crate) const MINED: &str = "1111111111111111111111111111111111111111111111111111111111111111";
pub(crate) const PENDING: &str = "2222222222222222222222222222222222222222222222222222222222222222";
pub(crate) const UNKNOWN: &str = "3333333333333333333333333333333333333333333333333333333333333333";
pub(crate) const FROZEN: &str = "4444444444444444444444444444444444444444444444444444444444444444";
pub(crate) const RECEIVED: &str =
    "6666666666666666666666666666666666666666666666666666666666666666";
pub(crate) const P2PKH: &str = "76a914dbc0a7c84983c5bf199b7b2d41b3acf0408ee5aa88ac";

/// The names of the worlds (see [`World::build`]).
pub(crate) const WORLDS: [&str; 17] = [
    "fresh",
    "certificate-privacy",
    "auth",
    "callback",
    "tracked",
    "received",
    "headers-down",
    "broken",
    "broken-host",
    "busy",
    "busy-read",
    "funded",
    "pin-spend",
    "review-race",
    "named-change",
    "abort-unproven",
    "tracker-fault",
];

/// The `funded` world's coins: (seed of the parent's input, height mined,
/// satoshis, derivation suffix). Each parent is a one-leaf block of its own
/// at its height, so the header stub answers its txid as that height's
/// root. The amounts descend: the toolbox takes the largest offered coin
/// before the others, so the cases spend them in this order.
pub(crate) const FUNDED_COINS: [(u8, u32, u64, &str); 4] = [
    (0xa1, 899_901, 50_000, "QQ=="),
    (0xa2, 899_902, 40_000, "Qg=="),
    (0xa3, 899_903, 30_000, "Qw=="),
    (0xa4, 899_904, 20_000, "RA=="),
];
/// The stranger's key: fixed, so each payment's signature (RFC 6979) and
/// txid are the same on every run.
const STRANGER_KEY: &str = "2e2e2e2e2e2e2e2e2e2e2e2e2e2e2e2e2e2e2e2e2e2e2e2e2e2e2e2e2e2e2e2e";
/// The derivation of the stranger's BRC-29 payment to the wallet (the
/// `wallet payment` protocol's remittance).
pub(crate) const PAYMENT_PREFIX: &str = "eDQwMg==";
pub(crate) const PAYMENT_SUFFIX: &str = "cGF5";
/// A puzzle the caller unlocks in `signAction`'s `spends`: `OP_1
/// OP_EQUAL`, unlocked by `OP_1`. Its coin's mined parent's seed, height
/// and satoshis.
const PUZZLE_LOCK: &str = "5187";
pub(crate) const PUZZLE_UNLOCK: &str = "51";
pub(crate) const PUZZLE: (u8, u32, u64) = (0xe4, 899_908, 5_000);
/// The toolbox's fee for the puzzle's one input (one byte of unlocking
/// script) and one P2PKH output: the template asks for the puzzle less
/// this, so no change output is made.
pub(crate) const FEE_PUZZLE_1_OUT: u64 = 9;
/// The toolbox's fee for one P2PKH input and one output, at its rate: a
/// case asks for exactly the coin less this, so no change output is made
/// (a change output's derivation is drawn at random, and the built bytes
/// would move on every run).
pub(crate) const FEE_1_IN_1_OUT: u64 = 20;
/// The derivation prefix of the funded coins; BRC-29's protocol is the
/// toolbox signer's (`3241645161d8`, counterparty self for a coin with no
/// sender key).
const FUNDED_PREFIX: &str = "d2lyZQ==";
const BRC29_PROTOCOL: &str = "3241645161d8";

pub(crate) struct World {
    router: Router,
    /// Held for the world's life: the stubs' tasks, a temp store, the
    /// other connection holding the write lock.
    _keep: Vec<Box<dyn std::any::Any + Send>>,
}

/// A local ARC that takes every post (the mock services'
/// `post_beef_success`, over the wire: the server's state holds the real
/// `Services`): `POST /v1/tx` answers `SEEN_ON_NETWORK` for the posted
/// transaction, the BEEF's subject or the raw transaction's own txid. Every
/// other request is a 404.
async fn arc_stub() -> String {
    use axum::routing::post;
    pub(crate) async fn take(body: axum::body::Bytes) -> axum::Json<serde_json::Value> {
        let bytes = serde_json::from_slice::<serde_json::Value>(&body)
            .ok()
            .and_then(|v| v["rawTx"].as_str().and_then(|h| hex::decode(h).ok()))
            .unwrap_or_else(|| body.to_vec());
        let txid = match bsv_sdk::transaction::Beef::from_binary(&bytes) {
            Ok(beef) => beef
                .txs
                .last()
                .map(bsv_sdk::transaction::BeefTx::txid)
                .unwrap_or_default(),
            Err(_) => bsv_sdk::transaction::Transaction::from_binary(&bytes)
                .as_ref()
                .map(bsv_sdk::transaction::Transaction::id)
                .unwrap_or_default(),
        };
        axum::Json(serde_json::json!({"txid": txid, "txStatus": "SEEN_ON_NETWORK"}))
    }
    let app = Router::new().route("/v1/tx", post(take));
    serve_local(app).await
}

/// The time of every seeded row: half an hour before the world is built,
/// so a held coin's age is two digits on any day (the normalizer keeps a
/// number's digit count).
fn row_time() -> chrono::DateTime<chrono::Utc> {
    chrono::Utc::now() - chrono::Duration::minutes(30)
}

/// A one-input, one-output transaction: the input names `[seed; 32]:0`
/// (never a real coin), the output pays `satoshis` to `lock`.
pub(crate) fn parent_tx(seed: u8, satoshis: u64, lock: &[u8]) -> bsv_sdk::transaction::Transaction {
    let mut tx = bsv_sdk::transaction::Transaction::new();
    tx.add_input(bsv_sdk::transaction::TransactionInput::new(
        hex::encode([seed; 32]),
        0,
    ))
    .unwrap();
    tx.add_output(bsv_sdk::transaction::TransactionOutput::new(
        satoshis,
        bsv_sdk::script::LockingScript::from_binary(lock).unwrap(),
    ))
    .unwrap();
    tx
}

/// The P2PKH lock of the wallet's BRC-29 key for `suffix` under
/// [`FUNDED_PREFIX`], as the toolbox's signer derives it.
pub(crate) fn funded_lock(key: &PrivateKey, suffix: &str) -> Vec<u8> {
    use bsv_sdk::wallet::{Counterparty, KeyDeriver, Protocol, SecurityLevel};
    let deriver = KeyDeriver::new(Some(key.clone()));
    let signing = deriver
        .derive_private_key(
            &Protocol::new(SecurityLevel::Counterparty, BRC29_PROTOCOL),
            &format!("{FUNDED_PREFIX} {suffix}"),
            &Counterparty::Self_,
        )
        .unwrap();
    p2pkh(&signing.public_key().hash160())
}

/// The `funded` world's coins ([`FUNDED_COINS`]): each a mined coin the
/// toolbox can carry in a BEEF (the parent's bytes stored, its one-leaf
/// proof at its height) and sign (the BRC-29 derivation on its row). The
/// roots the header stub must answer, by height.
async fn seed_funded(
    storage: &StorageSqlx,
    identity: &str,
    key: &PrivateKey,
) -> HashMap<u32, String> {
    let rows = Rows::open(storage, identity).await;
    // The basket wants no more coins than it has: the toolbox then makes
    // no change output for an action that spends a coin exactly (with the
    // default of six wanted, it adds change outputs, each with a random
    // derivation, and the built bytes move on every run).
    sqlx::query("UPDATE output_baskets SET number_of_desired_utxos = 0 WHERE basket_id = ?")
        .bind(rows.basket)
        .execute(&rows.pool)
        .await
        .unwrap();
    let mut roots = HashMap::new();
    for (seed, height, satoshis, suffix) in FUNDED_COINS {
        let lock = funded_lock(key, suffix);
        let parent = parent_tx(seed, satoshis, &lock);
        let txid = parent.id();
        roots.insert(height, txid.clone());
        let one_leaf = bsv_sdk::transaction::MerklePath::from_coinbase_txid(&txid, height);
        let proven = rows
            .proven(
                &txid,
                height,
                &txid,
                one_leaf.to_binary(),
                parent.to_binary(),
            )
            .await;
        rows.coin(Coin {
            status: "completed",
            txid: &txid,
            made: false,
            proven: Some(proven),
            satoshis,
            description: "a funded coin",
            raw: parent.to_binary(),
            lock,
            derivation: Some(suffix),
        })
        .await;
    }
    for payment in Payment::ALL {
        let (parent, _) = stranger_payment(payment, key).await;
        roots.insert(payment.parent_at().1, parent.id());
    }
    roots.insert(PUZZLE.1, puzzle_parent().0.id());
    roots
}

/// The review fixture needs a stored caller coin, so the signed action's
/// spender is visible to the next createAction (toolbox #245/#264).
async fn seed_puzzle(storage: &StorageSqlx, identity: &str) {
    let rows = Rows::open(storage, identity).await;
    let (puzzle, _) = puzzle_parent();
    let txid = puzzle.id();
    let proven = rows
        .proven(
            &txid,
            PUZZLE.1,
            &txid,
            bsv_sdk::transaction::MerklePath::from_coinbase_txid(&txid, PUZZLE.1).to_binary(),
            puzzle.to_binary(),
        )
        .await;
    rows.coin(Coin {
        status: "completed",
        txid: &txid,
        made: false,
        proven: Some(proven),
        satoshis: PUZZLE.2,
        description: "a caller puzzle",
        raw: puzzle.to_binary(),
        lock: hex::decode(PUZZLE_LOCK).unwrap(),
        derivation: None,
    })
    .await;
    sqlx::query("UPDATE outputs SET change = 0, type = 'custom' WHERE txid = ?")
        .bind(txid)
        .execute(&rows.pool)
        .await
        .unwrap();
}

/// A seeded world's way into the store: the wallet's user, its default
/// basket, and the store's writer.
struct Rows {
    pool: sqlx::SqlitePool,
    user: i64,
    basket: i64,
}

/// A coin of the wallet's, written as the toolbox would have stored it.
struct Coin<'a> {
    status: &'static str,
    txid: &'a str,
    made: bool,
    /// Its creator's proof row, if it is mined.
    proven: Option<i64>,
    satoshis: u64,
    description: &'a str,
    /// Its creator's bytes, as stored.
    pub(crate) raw: Vec<u8>,
    lock: Vec<u8>,
    /// The suffix of its BRC-29 derivation under [`FUNDED_PREFIX`], for a
    /// coin the wallet signs for.
    derivation: Option<&'a str>,
}

impl Rows {
    async fn open(storage: &StorageSqlx, identity: &str) -> Rows {
        let user = storage
            .find_or_insert_user(identity)
            .await
            .unwrap()
            .0
            .user_id;
        let basket = storage.find_or_create_default_basket(user).await;
        Rows {
            pool: storage.writer().clone(),
            user,
            basket: basket.unwrap().basket_id,
        }
    }

    /// A proof row: `txid` mined at `height` in a block whose root is
    /// `root`. Its id.
    async fn proven(
        &self,
        txid: &str,
        height: u32,
        root: &str,
        path: Vec<u8>,
        raw: Vec<u8>,
    ) -> i64 {
        let now = row_time();
        sqlx::query(
            "INSERT INTO proven_txs (txid, height, idx, block_hash, merkle_root, merkle_path, raw_tx, created_at, updated_at) \
             VALUES (?, ?, 0, ?, ?, ?, ?, ?, ?)",
        )
        .bind(txid)
        .bind(i64::from(height))
        .bind(format!("{height:064x}"))
        .bind(root)
        .bind(path)
        .bind(raw)
        .bind(now)
        .bind(now)
        .execute(&self.pool)
        .await
        .unwrap()
        .last_insert_rowid()
    }

    /// A completed transaction and its one output, the coin.
    async fn coin(&self, coin: Coin<'_>) {
        let now = row_time();
        let satoshis = i64::try_from(coin.satoshis).unwrap();
        let tx_row = sqlx::query(
            "INSERT INTO transactions (user_id, proven_tx_id, status, reference, is_outgoing, satoshis, version, lock_time, description, txid, raw_tx, created_at, updated_at) \
             VALUES (?, ?, ?, ?, ?, ?, 1, 0, ?, ?, ?, ?, ?)",
        )
        .bind(self.user)
        .bind(coin.proven)
        .bind(coin.status)
        .bind(format!("ref-{}", &coin.txid[..8]))
        .bind(coin.made)
        .bind(satoshis)
        .bind(coin.description)
        .bind(coin.txid)
        .bind(coin.raw)
        .bind(now)
        .bind(now)
        .execute(&self.pool)
        .await
        .unwrap()
        .last_insert_rowid();
        sqlx::query(
            "INSERT INTO outputs (user_id, transaction_id, basket_id, vout, satoshis, locking_script, txid, type, spendable, change, provided_by, purpose, output_description, derivation_prefix, derivation_suffix, created_at, updated_at) \
             VALUES (?, ?, ?, 0, ?, ?, ?, 'P2PKH', 1, 1, 'storage', 'change', 'c', ?, ?, ?, ?)",
        )
        .bind(self.user)
        .bind(tx_row)
        .bind(self.basket)
        .bind(satoshis)
        .bind(coin.lock)
        .bind(coin.txid)
        .bind(coin.derivation.map(|_| FUNDED_PREFIX))
        .bind(coin.derivation)
        .bind(now)
        .bind(now)
        .execute(&self.pool)
        .await
        .unwrap();
    }
}

/// The stranger's payments `internalizeAction` is offered in `funded`.
/// Each is a mined parent paying the stranger's P2PKH and a child spending
/// it.
#[derive(Clone, Copy)]
pub(crate) enum Payment {
    /// A valid spend paying 9,000 to [`P2PKH`] (a basket insertion).
    Insertion,
    /// The same, its input's unlocking script `OP_0`: a spend whose script
    /// fails (`OP_EQUALVERIFY`).
    BadUnlock,
    /// A valid spend paying 9,000 to the wallet's BRC-29 key for the
    /// stranger, [`PAYMENT_PREFIX`] and [`PAYMENT_SUFFIX`].
    Wallet,
}

impl Payment {
    const ALL: [Payment; 3] = [Payment::Insertion, Payment::BadUnlock, Payment::Wallet];

    /// The seed of the parent's input and the height it is mined at.
    fn parent_at(self) -> (u8, u32) {
        match self {
            Payment::Insertion => (0xe1, 899_905),
            Payment::BadUnlock => (0xe2, 899_906),
            Payment::Wallet => (0xe3, 899_907),
        }
    }

    /// The lock the payment's one output pays.
    fn pays_to(self, wallet: &PrivateKey) -> Vec<u8> {
        use bsv_sdk::wallet::{Counterparty, KeyDeriver, Protocol, SecurityLevel};
        match self {
            Payment::Insertion | Payment::BadUnlock => hex::decode(P2PKH).unwrap(),
            // As the payer derives it: the stranger's key, the wallet the
            // counterparty.
            Payment::Wallet => p2pkh(
                &KeyDeriver::new(Some(stranger_key()))
                    .derive_public_key(
                        &Protocol::new(SecurityLevel::Counterparty, BRC29_PROTOCOL),
                        &format!("{PAYMENT_PREFIX} {PAYMENT_SUFFIX}"),
                        &Counterparty::Other(wallet.public_key()),
                        false,
                    )
                    .unwrap()
                    .hash160(),
            ),
        }
    }
}

pub(crate) fn stranger_key() -> PrivateKey {
    PrivateKey::from_hex(STRANGER_KEY).unwrap()
}

/// The P2PKH lock of `hash`.
fn p2pkh(hash: &[u8]) -> Vec<u8> {
    [&[0x76, 0xa9, 0x14][..], hash, &[0x88, 0xac][..]].concat()
}

/// A [`Payment`]'s mined parent and child `AtomicBEEF`, checked before serving.
pub(crate) async fn stranger_payment(
    payment: Payment,
    wallet: &PrivateKey,
) -> (bsv_sdk::transaction::Transaction, Vec<u8>) {
    use bsv_sdk::script::templates::P2PKH as Template;
    use bsv_sdk::script::SignOutputs;
    let stranger = stranger_key();
    let (seed, at) = payment.parent_at();
    let mut parent = parent_tx(seed, 10_000, &p2pkh(&stranger.public_key().hash160()));
    parent.merkle_path = Some(bsv_sdk::transaction::MerklePath::from_coinbase_txid(
        &parent.id(),
        at,
    ));
    let mut child = bsv_sdk::transaction::Transaction::new();
    let mut input =
        bsv_sdk::transaction::TransactionInput::with_source_transaction(parent.clone(), 0);
    match payment {
        Payment::BadUnlock => {
            input.unlocking_script =
                Some(bsv_sdk::script::UnlockingScript::from_hex("00").unwrap());
        }
        Payment::Insertion | Payment::Wallet => input
            .set_unlocking_script_template(Template::unlock(&stranger, SignOutputs::All, false)),
    }
    child.add_input(input).unwrap();
    child
        .add_output(bsv_sdk::transaction::TransactionOutput::new(
            9_000,
            bsv_sdk::script::LockingScript::from_binary(&payment.pays_to(wallet)).unwrap(),
        ))
        .unwrap();
    child.sign().await.unwrap();
    // Parity-mode replay checks the planted good/bad payment specimens.
    // A node-word check needs spend-era flags plus coin-era UTXO bits per input;
    // see docs/ARCHITECTURE.md for the combined-word rule.
    let verified = child
        .verify(
            &bsv_sdk::transaction::AlwaysValidChainTracker::new(at),
            None,
            None,
        )
        .await
        .unwrap_or(false);
    assert_eq!(
        verified,
        !matches!(payment, Payment::BadUnlock),
        "the stranger's spend verifies exactly when it is meant to"
    );
    let mut beef = bsv_sdk::transaction::Beef::new();
    beef.merge_bump(parent.merkle_path.clone().unwrap());
    beef.merge_transaction(parent.clone());
    beef.merge_transaction(child.clone());
    let atomic = beef.to_binary_atomic(&child.id()).unwrap();
    (parent, atomic)
}

/// The puzzle's mined parent ([`PUZZLE`]) and the BEEF carrying it with its
/// proof (a template's `inputBEEF`).
pub(crate) fn puzzle_parent() -> (bsv_sdk::transaction::Transaction, Vec<u8>) {
    let (seed, at, satoshis) = PUZZLE;
    let parent = parent_tx(seed, satoshis, &hex::decode(PUZZLE_LOCK).unwrap());
    let mut beef = bsv_sdk::transaction::Beef::new();
    beef.merge_bump(bsv_sdk::transaction::MerklePath::from_coinbase_txid(
        &parent.id(),
        at,
    ));
    beef.merge_transaction(parent.clone());
    (parent, beef.to_binary())
}

/// The world's rows, written into the store as the tracker and the toolbox
/// would leave them (no transaction here was ever built or broadcast).
pub(crate) async fn seed(storage: &StorageSqlx, identity: &str, frozen: bool) {
    let rows = Rows::open(storage, identity).await;
    // The tracker's table, made by a read of it.
    bsv_wallet_cli::tracker_host::stored_word(storage, UNKNOWN)
        .await
        .unwrap();
    let mined = Some(i64::from(MINED_AT));
    for (txid, word, height, reask) in [
        (MINED, "mined", mined, None),
        (PENDING, "seen", None, Some("age")),
        (RECEIVED, "mined", mined, None),
    ] {
        sqlx::query("INSERT INTO tracker_states (txid, word, height, reask) VALUES (?, ?, ?, ?)")
            .bind(txid)
            .bind(word)
            .bind(height)
            .bind(reask)
            .execute(&rows.pool)
            .await
            .unwrap();
    }
    // The tracker's mined word's proof (its root only is read), and the
    // received coin's: a one-leaf block at MINED_AT, so its root is its
    // txid and the header stub answers that root. Neither stores its
    // transaction's bytes.
    let stub = vec![1u8, 0, 0, 0];
    rows.proven(MINED, MINED_AT, ROOT, vec![0u8], stub.clone())
        .await;
    let one_leaf = bsv_sdk::transaction::MerklePath::from_coinbase_txid(RECEIVED, MINED_AT);
    let received = rows
        .proven(
            RECEIVED,
            MINED_AT,
            RECEIVED,
            one_leaf.to_binary(),
            stub.clone(),
        )
        .await;
    // A received coin, its creator mined at MINED_AT; or one of ours the
    // chain feed froze, its creator unproven.
    let (txid, proven) = if frozen {
        (FROZEN, None)
    } else {
        (RECEIVED, Some(received))
    };
    rows.coin(Coin {
        status: "completed",
        txid,
        made: false,
        proven,
        satoshis: 50_000,
        description: "a received coin",
        raw: stub,
        lock: hex::decode(P2PKH).unwrap(),
        // Reach the ancestry check through #295's managed-coin predicate.
        derivation: Some("Zml4dHVyZQ=="),
    })
    .await;
}

impl World {
    /// Construct a named world from WORLDS. The ordinary worlds are empty,
    /// authenticated, tracked, received, faulty or funded. The named-change
    /// and abort-unproven worlds isolate their guards. Pin-spend holds a
    /// stored caller puzzle, and review-race takes that coin during a header
    /// read so the toolbox must recheck it under its write transaction.
    pub(crate) async fn build(name: &str) -> World {
        known_world(name);
        let mut keep: Vec<Box<dyn std::any::Any + Send>> = Vec::new();
        let key = PrivateKey::from_hex(ROOT_KEY).unwrap();
        let storage = World::store(name, &mut keep).await;
        let roots = World::fill(name, &storage, &key).await;
        let services = World::services(name, roots, &storage).await;
        if name == "tracked" {
            bsv_wallet_cli::tracker_host::deliver_chain_event(
                &storage,
                &services,
                &bsv_wallet_cli::tracker_host::SystemClock,
                &bsv_wallet_cli::tracker_host::TickOptions::default(),
                1,
                bsv_tracker::ChainEvent::Frozen {
                    outpoint: bsv_tracker::Outpoint {
                        txid: FROZEN.to_string(),
                        vout: 0,
                    },
                },
            )
            .await
            .unwrap();
        }
        let wallet = Wallet::new(Some(key), storage, services).await.unwrap();
        if name == "tracker-fault" {
            super::tracker_fault::Unavailable::install(wallet.storage()).await;
        }
        // The wallet is opened (its opening writes made), then the lock.
        if name == "busy" || name == "busy-read" {
            let lock = hold_write_lock(&storage_path(&keep), name == "busy-read").await;
            keep.push(Box::new(lock));
        }
        let mut config = ServerConfig {
            settings: crate::wire_support::quiet_config(),
            ..ServerConfig::default()
        };
        match name {
            "auth" => config.auth_token = Some(WIRE_BEARER.to_string()),
            "callback" | "busy-read" => config.callback_token = Some(WIRE_CALLBACK.to_string()),
            _ => {}
        }
        let router = server::make_router(server::make_wallet_state(wallet), config);
        World {
            router,
            _keep: keep,
        }
    }

    /// The world's store, migrated: a file under a temp directory `keep`
    /// holds for `busy` and `busy-read` (another connection must open it),
    /// `busy-read`'s on the dotfile locking, else in memory.
    async fn store(name: &str, keep: &mut Vec<Box<dyn std::any::Any + Send>>) -> StorageSqlx {
        let storage = if name == "busy" || name == "busy-read" {
            let tmp = tempfile::TempDir::new().unwrap();
            let path = tmp.path().join("wire.db");
            let storage = if name == "busy-read" {
                StorageSqlx::new(&format!("sqlite:{}?vfs={DOTFILE}", path.display()))
                    .await
                    .unwrap()
            } else {
                StorageSqlx::open(path.to_str().unwrap()).await.unwrap()
            };
            keep.push(Box::new(tmp));
            storage
        } else {
            StorageSqlx::in_memory().await.unwrap()
        };
        let identity = PrivateKey::from_hex(ROOT_KEY)
            .unwrap()
            .public_key()
            .to_hex();
        storage.migrate("wire", &identity).await.unwrap();
        storage.make_available().await.unwrap();
        storage
    }

    /// The world's rows (seeded or spoiled): the roots the header stub must
    /// answer, by height.
    pub(crate) async fn fill(
        name: &str,
        storage: &StorageSqlx,
        key: &PrivateKey,
    ) -> HashMap<u32, String> {
        let identity = key.public_key().to_hex();
        match name {
            "tracked" => seed(storage, &identity, true).await,
            "received" | "headers-down" => seed(storage, &identity, false).await,
            "funded" => return seed_funded(storage, &identity, key).await,
            "pin-spend" | "review-race" => {
                let roots = seed_funded(storage, &identity, key).await;
                seed_puzzle(storage, &identity).await;
                return roots;
            }
            "abort-unproven" | "named-change" => {
                Rows::open(storage, &identity)
                    .await
                    .coin(Coin {
                        status: if name == "abort-unproven" {
                            "unproven"
                        } else {
                            "completed"
                        },
                        txid: PENDING,
                        made: true,
                        proven: None,
                        satoshis: 50_000,
                        description: "our change",
                        raw: vec![1, 0, 0, 0],
                        lock: hex::decode(P2PKH).unwrap(),
                        derivation: None,
                    })
                    .await;
            }
            "broken" => {
                sqlx::query("DROP TABLE outputs")
                    .execute(storage.writer())
                    .await
                    .unwrap();
            }
            "broken-host" => {
                // The tracker's tables, made by a read, then spoiled: a feed
                // status that is not JSON, and a states table that keeps the
                // columns the host's migration checks for and none it reads.
                bsv_wallet_cli::tracker_host::stored_word(storage, UNKNOWN)
                    .await
                    .unwrap();
                for sql in [
                    "INSERT INTO tracker_meta (key, value) VALUES ('feed_status', 'not json')",
                    "DROP TABLE tracker_states",
                    "CREATE TABLE tracker_states (txid TEXT PRIMARY KEY, chain TEXT NOT NULL DEFAULT '[]', verdict TEXT, follow_up TEXT)",
                ] {
                    sqlx::query(sql).execute(storage.writer()).await.unwrap();
                }
            }
            _ => {}
        }
        HashMap::new()
    }

    /// The toolbox's services, pointed at the world's stubs.
    async fn services(name: &str, roots: HashMap<u32, String>, storage: &StorageSqlx) -> Services {
        let stub = header_stub(name == "headers-down", roots, RECEIVED).await;
        let stub = if name == "review-race" {
            super::review_race::headers(stub, storage.writer().clone()).await
        } else {
            stub
        };
        let arc = if matches!(name, "funded" | "pin-spend") {
            arc_stub().await
        } else {
            CLOSED.to_string()
        };
        services(stub, arc)
    }

    /// The normalized response, and the JSON body as served.
    pub(crate) async fn serve_raw(&self, req: &Req) -> (Resp, Option<J>) {
        NORMALIZER.serve(&self.router, req).await
    }
}

/// A world's name is one of [`WORLDS`]; any other is a panic naming it.
pub(crate) fn known_world(name: &str) {
    assert!(
        WORLDS.contains(&name),
        "unknown world `{name}`: the worlds are {}",
        WORLDS.join(", ")
    );
}

/// The request with each bound value read from the body `served` holds for
/// its file (a panic names a file not served yet or a pointer it lacks).
pub(crate) fn resolve(file: &str, req: &Req, served: &HashMap<String, J>) -> Req {
    let mut req = req.clone();
    for (at, from) in &req.bind {
        let body = served.get(&from.file).unwrap_or_else(|| {
            panic!(
                "{file}: binds {at} to {}, which was not served before it",
                from.file
            )
        });
        let value = body.pointer(&from.pointer).unwrap_or_else(|| {
            panic!(
                "{file}: binds {at} to {}{}, which its body does not carry",
                from.file, from.pointer
            )
        });
        req.json
            .as_mut()
            .unwrap_or_else(|| panic!("{file}: binds {at} on a request with no JSON body"))
            .set_pointer(at, value.clone());
    }
    req
}

/// SQLite's dotfile locking: one lock (a `.lock` beside the file) for
/// readers and writers alike, and no shared memory, so the store keeps its
/// rollback journal and a held lock keeps every reader out.
const DOTFILE: &str = "unix-dotfile";

/// Another connection to the store at `path`, holding its write lock
/// (never closed: the lock is held for as long as it is kept); on the
/// `dotfile` locking, its one lock.
async fn hold_write_lock(path: &Path, dotfile: bool) -> sqlx::SqliteConnection {
    use sqlx::{ConnectOptions, Connection};
    let mut options = sqlx::sqlite::SqliteConnectOptions::new().filename(path);
    if dotfile {
        options = options.vfs(DOTFILE);
    }
    let mut other = options.connect().await.unwrap();
    sqlx::query(if dotfile {
        "BEGIN EXCLUSIVE"
    } else {
        "BEGIN IMMEDIATE"
    })
    .execute(&mut other)
    .await
    .unwrap();
    let _ = other.ping().await;
    other
}

fn storage_path(keep: &[Box<dyn std::any::Any + Send>]) -> PathBuf {
    keep.iter()
        .find_map(|k| k.downcast_ref::<tempfile::TempDir>())
        .unwrap()
        .path()
        .join("wire.db")
}
