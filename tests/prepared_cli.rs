//! A prepared, unsent action at the command line (#673): a wallet holding
//! one `noSend` action (signed, never sent) whose one input, the wallet's
//! only coin, is reserved by it. What `actions` prints for it, in text and
//! under `--json`, what `balance` prints, and which commands exist.
//!
//! The store is a file under a temporary directory, seeded in this process:
//! one mined coin written as the respend witness writes its own (its proof
//! stored), then the toolbox's own `createAction` with `noSend: true`, on
//! the toolbox's mock services, which answer from memory and post nowhere.
//! The commands then run as a user runs them, the binary on that file with
//! the wallet's `ROOT_KEY`. The binary builds the configured services:
//! `CHAINTRACKS_URL=off` (no header service), the broadcaster's URL and the
//! explorers' bases at a closed local port or off, and every proxy at the
//! same closed port, so any request the run made would fail at once. No
//! command run here asks a service: the listing and the balance read the
//! store, and the abort path makes no service call.

use std::path::Path;
use std::process::{Command, Output};

use bsv_sdk::primitives::PrivateKey;
use bsv_sdk::script::{templates::P2PKH, LockingScript, UnlockingScript};
use bsv_sdk::transaction::{
    MerklePath, MerklePathLeaf, Transaction, TransactionInput, TransactionOutput,
};
use bsv_sdk::wallet::{
    Counterparty, CreateActionArgs, CreateActionOptions, CreateActionOutput, KeyDeriver, Protocol,
    SecurityLevel, WalletInterface,
};
use bsv_wallet_toolbox::services::mock::MockWalletServices;
use bsv_wallet_toolbox::storage::StorageProcessActionArgs;
use bsv_wallet_toolbox::{AuthId, StorageSqlx, Wallet, WalletStorageWriter};
use chrono::Utc;
use serde_json::Value;
use tempfile::TempDir;

/// The wallet's one coin.
const COIN: u64 = 100_000;
/// The prepared payment's one output.
const PAYMENT: u64 = 1_000;
/// The prepared payment's description.
const DESCRIPTION: &str = "a prepared payment";

/// A closed local port: anything sent there fails at once.
const CLOSED: &str = "http://127.0.0.1:9";

/// The variables a run must not take from the shell.
const UNSET: [&str; 12] = [
    "ARC_MODE",
    "ARCADE",
    "TAAL_API_KEY",
    "MAIN_TAAL_API_KEY",
    "WOC_API_KEY",
    "BITAILS_API_KEY",
    "ALLOWED_ORIGINS",
    "AUTH_TOKEN",
    "TRUST_SETTINGS",
    "SPEND_DEPTH",
    "SPEND_OWN_CHANGE",
    "BREAK_GLASS_EXPLORER_HEADERS",
];

/// The variables a run is given (see the module's text).
const SET: [(&str, &str); 11] = [
    ("CHAINTRACKS_URL", "off"),
    ("ARC_URL", CLOSED),
    ("WOC_BASE", CLOSED),
    ("BITAILS_BASE", CLOSED),
    ("BANANABLOCKS_BASE", ""),
    ("ARC_GORILLAPOOL_URL", ""),
    ("HTTPS_PROXY", CLOSED),
    ("HTTP_PROXY", CLOSED),
    ("ALL_PROXY", CLOSED),
    ("NO_PROXY", ""),
    ("RUST_LOG", "warn"),
];

/// The wallet a case runs on: its directory (the store is `wallet.db`
/// there), its root key, the prepared action's txid and reference, and the
/// reserved coin's txid.
struct Prepared {
    dir: TempDir,
    root: PrivateKey,
    txid: String,
    reference: String,
    coin: String,
    /// The payment and its fee: the coin less the action's change.
    spent: u64,
}

/// The wallet of one coin with one prepared, unsent payment holding it.
async fn prepared() -> Prepared {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("wallet.db");
    let root = PrivateKey::random();
    let identity = root.public_key().to_hex();
    let storage = StorageSqlx::open(path.to_str().unwrap()).await.unwrap();
    storage.migrate("prepared-cli", &identity).await.unwrap();
    storage.make_available().await.unwrap();
    let coin = seed_coin(&storage, &root).await;
    let wallet = Wallet::new(
        Some(root.clone()),
        storage,
        MockWalletServices::builder().build(),
    )
    .await
    .unwrap();
    let made = wallet
        .create_action(
            CreateActionArgs {
                description: DESCRIPTION.into(),
                input_beef: None,
                inputs: None,
                outputs: Some(vec![CreateActionOutput {
                    locking_script: LockingScript::from_hex("51").unwrap().to_binary(),
                    satoshis: PAYMENT,
                    output_description: "the payment".into(),
                    basket: None,
                    custom_instructions: None,
                    tags: None,
                }]),
                lock_time: None,
                version: None,
                labels: None,
                options: Some(CreateActionOptions {
                    no_send: Some(true),
                    accept_delayed_broadcast: Some(false),
                    ..Default::default()
                }),
            },
            "app.example",
        )
        .await
        .unwrap();
    let txid = hex::encode(made.txid.unwrap());
    let reference: String = sqlx::query_scalar("SELECT reference FROM transactions WHERE txid = ?")
        .bind(&txid)
        .fetch_one(wallet.storage().pool())
        .await
        .unwrap();
    let change: i64 = sqlx::query_scalar(
        "SELECT COALESCE(SUM(o.satoshis), 0) FROM outputs o \
         JOIN transactions t ON t.transaction_id = o.transaction_id \
         WHERE t.txid = ? AND o.change = 1",
    )
    .bind(&txid)
    .fetch_one(wallet.storage().pool())
    .await
    .unwrap();
    drop(wallet);
    Prepared {
        dir,
        root,
        txid,
        reference,
        coin,
        spent: COIN - change as u64,
    }
}

/// One mined coin of `COIN` sats to the wallet's change derivation, its
/// proof stored (the respend witness's fixture, `tests/respend_witness/`).
/// Its txid.
async fn seed_coin(storage: &StorageSqlx, root: &PrivateKey) -> String {
    let identity = root.public_key().to_hex();
    let (user, _) = storage.find_or_insert_user(&identity).await.unwrap();
    let basket = storage
        .find_or_create_default_basket(user.user_id)
        .await
        .unwrap()
        .basket_id;
    let prefix = "dGVzdC1wcmVmaXg=";
    let suffix = "dGVzdC1zdWZmaXg=";
    let key = KeyDeriver::new(Some(root.clone()))
        .derive_private_key(
            &Protocol::new(SecurityLevel::Counterparty, "3241645161d8"),
            &format!("{prefix} {suffix}"),
            &Counterparty::Self_,
        )
        .unwrap();
    let lock = P2PKH::lock_from_address(&key.public_key().to_address()).unwrap();
    let funding = Transaction::with_params(
        1,
        vec![TransactionInput {
            source_transaction: None,
            source_txid: Some("00".repeat(32)),
            source_output_index: u32::MAX,
            unlocking_script: Some(UnlockingScript::from_hex("00").unwrap()),
            unlocking_script_template: None,
            sequence: u32::MAX,
        }],
        vec![TransactionOutput {
            satoshis: Some(COIN),
            locking_script: lock.clone(),
            change: false,
        }],
        0,
    );
    let now = Utc::now();
    let row: i64 = sqlx::query_scalar(
        "INSERT INTO transactions (user_id,status,reference,is_outgoing,satoshis,version,lock_time,description,txid,raw_tx,created_at,updated_at) \
         VALUES (?,'completed','funding',0,?,1,0,'a mined coin',?,?,?,?) RETURNING transaction_id")
        .bind(user.user_id).bind(COIN as i64).bind(funding.id()).bind(funding.to_binary()).bind(now).bind(now)
        .fetch_one(storage.writer()).await.unwrap();
    sqlx::query(
        "INSERT INTO outputs (user_id,transaction_id,basket_id,vout,satoshis,locking_script,txid,type,spendable,change,derivation_prefix,derivation_suffix,provided_by,purpose,output_description,created_at,updated_at) \
         VALUES (?,?,?,0,?,?,?,'P2PKH',1,1,?,?,'storage','change','a mined coin',?,?)")
        .bind(user.user_id).bind(row).bind(basket).bind(COIN as i64).bind(lock.to_binary()).bind(funding.id()).bind(prefix).bind(suffix).bind(now).bind(now)
        .execute(storage.writer()).await.unwrap();
    let proof = MerklePath {
        block_height: 870_800,
        path: vec![vec![MerklePathLeaf {
            offset: 0,
            hash: Some(funding.id()),
            txid: true,
            duplicate: false,
        }]],
    }
    .to_binary();
    let proven: i64 = sqlx::query_scalar(
        "INSERT INTO proven_txs (txid,height,idx,block_hash,merkle_root,merkle_path,raw_tx,created_at,updated_at) \
         VALUES (?,870800,0,?,?,?,?,?,?) RETURNING proven_tx_id")
        .bind(funding.id()).bind("bb".repeat(32)).bind(funding.id()).bind(proof).bind(funding.to_binary()).bind(now).bind(now)
        .fetch_one(storage.writer()).await.unwrap();
    sqlx::query("UPDATE transactions SET proven_tx_id = ? WHERE transaction_id = ?")
        .bind(proven)
        .bind(row)
        .execute(storage.writer())
        .await
        .unwrap();
    funding.id()
}

fn on_runtime<F: std::future::Future<Output = ()>>(f: impl FnOnce() -> F) {
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(f());
}

/// The binary run on the wallet in `dir` with `args`.
fn run(dir: &Path, root: &PrivateKey, args: &[&str]) -> Output {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_bsv-wallet"));
    cmd.current_dir(dir)
        .envs(SET)
        .env("ROOT_KEY", root.to_hex())
        .args(["--db", "wallet.db"])
        .args(args);
    for name in UNSET {
        cmd.env_remove(name);
    }
    cmd.output().expect("run the command")
}

fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

/// The command's stdout, asserting it exits 0.
fn said(p: &Prepared, args: &[&str]) -> String {
    let out = run(p.dir.path(), &p.root, args);
    assert!(
        out.status.success(),
        "{args:?}: {}{}",
        text(&out.stdout),
        text(&out.stderr)
    );
    text(&out.stdout)
}

/// The store's word for `txid`, and the reserved coin's (spendable,
/// spender's txid).
async fn store_of(p: &Prepared) -> (String, (bool, Option<String>)) {
    let storage = StorageSqlx::open(p.dir.path().join("wallet.db").to_str().unwrap())
        .await
        .unwrap();
    let status: String = sqlx::query_scalar("SELECT status FROM transactions WHERE txid = ?")
        .bind(&p.txid)
        .fetch_one(storage.pool())
        .await
        .unwrap();
    let coin: (bool, Option<String>) = sqlx::query_as(
        "SELECT o.spendable, s.txid FROM outputs o \
         LEFT JOIN transactions s ON s.transaction_id = o.spent_by \
         WHERE o.txid = ? AND o.vout = 0",
    )
    .bind(&p.coin)
    .fetch_one(storage.pool())
    .await
    .unwrap();
    (status, coin)
}

/// The satoshis the prepared payment spends from the wallet: its payment
/// and its fee (the coin less its change), read from the store.
fn spent(p: &Prepared) -> u64 {
    p.spent
}

/// The names of the commands `--help` lists.
fn commands(p: &Prepared) -> Vec<String> {
    let help = said(p, &["--help"]);
    help.lines()
        .skip_while(|l| !l.starts_with("Commands:"))
        .skip(1)
        .take_while(|l| l.starts_with("  "))
        .filter_map(|l| l.split_whitespace().next().map(str::to_string))
        .collect()
}

/// The keys of a JSON object, sorted.
fn keys(v: &Value) -> Vec<String> {
    let mut k: Vec<String> = v.as_object().unwrap().keys().cloned().collect();
    k.sort();
    k
}

/// THE TEST THAT DECIDES (#673), the store: the prepared payment reads
/// `nosend`, the wallet's only coin reserved by it.
#[test]
fn the_prepared_payment_holds_the_wallets_coin() {
    on_runtime(|| async {
        let p = prepared().await;
        let (status, coin) = store_of(&p).await;
        assert_eq!(status, "nosend");
        assert_eq!(coin, (false, Some(p.txid.clone())));
    });
}

/// THE TEST THAT DECIDES (#673), `actions` in text: at the base the
/// prepared payment is one line like any other action, its status the
/// wire's word `nosend`, its txid's first eight digits, the action's net
/// satoshis; neither its whole txid, its reference, its age nor the
/// satoshis its input holds is printed, and no line totals a hold.
#[test]
fn actions_in_text() {
    on_runtime(|| async {
        let p = prepared().await;
        let out = said(&p, &["actions"]);
        eprintln!("#673 actions:\n{out}");
        let line = out
            .lines()
            .find(|l| l.contains(DESCRIPTION))
            .expect("the prepared payment is listed");
        assert!(line.starts_with("  OUT | "), "{line}");
        assert!(line.contains(&format!(" | {} | nosend | ", &p.txid[..8])));
        assert_eq!(
            line,
            format!(
                "  OUT | {} | nosend | -{} sats | {DESCRIPTION}",
                &p.txid[..8],
                spent(&p)
            )
        );
        assert!(!out.contains(&p.txid), "{out}");
        assert!(!out.contains(&p.reference), "{out}");
        // The coin's satoshis on the mined coin's own line alone.
        assert_eq!(out.matches(&COIN.to_string()).count(), 1, "{out}");
        assert_eq!(out.lines().count(), 3, "{out}");
    });
}

/// THE TEST THAT DECIDES (#673), `actions --json`: at the base the
/// prepared payment is an action object of today's keys, its status
/// `nosend`; nothing carries its reference or the satoshis its input
/// holds. Today's keys, pinned: a caller that parses them keeps working.
#[test]
fn actions_in_json() {
    on_runtime(|| async {
        let p = prepared().await;
        let out: Value = serde_json::from_str(&said(&p, &["--json", "actions"])).unwrap();
        eprintln!("#673 actions --json: {out}");
        assert_eq!(keys(&out), ["actions", "totalActions"]);
        let action = out["actions"]
            .as_array()
            .unwrap()
            .iter()
            .find(|a| a["txid"] == p.txid.as_str())
            .expect("the prepared payment is listed");
        assert_eq!(action["status"], "nosend");
        assert_eq!(
            keys(action),
            [
                "description",
                "isOutgoing",
                "labels",
                "lockTime",
                "satoshis",
                "status",
                "txid",
                "version"
            ]
        );
        assert_eq!(action["satoshis"], -(spent(&p) as i64));
        let shown = out.to_string();
        assert!(!shown.contains(&p.reference), "{shown}");
        assert!(!action.to_string().contains(&COIN.to_string()), "{action}");
    });
}

/// The store's own send of the prepared payment `p`: the wallet library's
/// `process_action` naming its txid, a delayed broadcast. One write to the
/// store, no service asked: the payment then reads `sending` and waits for
/// a monitor, which no test here runs. (No command or served call of this
/// line sends a prepared payment, #147.)
async fn send(p: &Prepared) {
    let storage = StorageSqlx::open(p.dir.path().join("wallet.db").to_str().unwrap())
        .await
        .unwrap();
    storage.make_available().await.unwrap();
    let identity = p.root.public_key().to_hex();
    let user = storage.find_or_insert_user(&identity).await.unwrap().0;
    let auth = AuthId::with_user_id(identity, user.user_id);
    let args = StorageProcessActionArgs {
        is_new_tx: false,
        is_send_with: false,
        is_no_send: false,
        is_delayed: true,
        reference: None,
        txid: Some(p.txid.clone()),
        raw_tx: None,
        send_with: Vec::new(),
    };
    let sent = storage.process_action(&auth, args).await.unwrap();
    let word = &sent.send_with_results.unwrap()[0];
    assert_eq!((&word.txid, word.status.as_str()), (&p.txid, "sending"));
}

/// The sats `balance` prints for the wallet `p`.
fn balance(p: &Prepared) -> u64 {
    let out = said(p, &["balance"]);
    eprintln!("#673 balance: {out}");
    out.split_whitespace().next().unwrap().parse().unwrap()
}

/// THE TEST THAT DECIDES (#673), `balance`, on this line's wallet library,
/// which writes a `noSend` action's change not spendable until its
/// transaction is sent. At the base, while the payment is prepared, the
/// balance counts neither the coin it holds nor its change: 0 for this
/// wallet of one coin, and nothing says a coin is held. Once it is sent
/// (the store's own send) its change is counted.
#[test]
fn balance_counts_neither_the_held_coin_nor_the_unsent_change() {
    on_runtime(|| async {
        let p = prepared().await;
        assert_eq!(balance(&p), 0);
        send(&p).await;
        assert_eq!(balance(&p), COIN - spent(&p));
    });
}

/// THE TEST THAT DECIDES (#673), the commands: at the base none of the
/// commands `--help` lists aborts an action, and running every listing the
/// payment appears in leaves it `nosend`, its coin reserved.
#[test]
fn no_command_aborts_it() {
    on_runtime(|| async {
        let p = prepared().await;
        let names = commands(&p);
        eprintln!("#673 commands: {names:?}");
        assert!(names.contains(&"actions".to_string()), "{names:?}");
        assert!(!names.iter().any(|n| n.contains("abort")), "{names:?}");
        let (status, coin) = store_of(&p).await;
        assert_eq!(status, "nosend");
        assert_eq!(coin, (false, Some(p.txid.clone())));
    });
}
