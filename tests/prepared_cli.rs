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
use bsv_wallet_toolbox::{StorageSqlx, Wallet, WalletStorageWriter};
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
    let coin = seed_coin(&storage, &root, 0).await;
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
            Some("app.example"),
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
/// The `n`th coin's funding transaction differs by its lock time. Its txid.
async fn seed_coin(storage: &StorageSqlx, root: &PrivateKey, n: u32) -> String {
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
        n,
    );
    let now = Utc::now();
    let row: i64 = sqlx::query_scalar(
        "INSERT INTO transactions (user_id,status,reference,is_outgoing,satoshis,version,lock_time,description,txid,raw_tx,created_at,updated_at) \
         VALUES (?,'completed',?,0,?,1,?,'a mined coin',?,?,?,?) RETURNING transaction_id")
        .bind(user.user_id).bind(format!("funding-{n}")).bind(COIN as i64).bind(n).bind(funding.id()).bind(funding.to_binary()).bind(now).bind(now)
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
    .to_binary()
    .unwrap();
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

/// A second prepared payment on the wallet `p`: a second coin of `COIN`
/// sats, then a `noSend` payment too large for the first payment's change,
/// so it holds the second coin alone. Its txid, its reference and the
/// coin's txid.
async fn another(p: &Prepared) -> (String, String, String) {
    let storage = StorageSqlx::open(p.dir.path().join("wallet.db").to_str().unwrap())
        .await
        .unwrap();
    storage.make_available().await.unwrap();
    let coin = seed_coin(&storage, &p.root, 1).await;
    let wallet = Wallet::new(
        Some(p.root.clone()),
        storage,
        MockWalletServices::builder().build(),
    )
    .await
    .unwrap();
    let made = wallet
        .create_action(
            CreateActionArgs {
                description: "a second prepared payment".into(),
                input_beef: None,
                inputs: None,
                outputs: Some(vec![CreateActionOutput {
                    locking_script: LockingScript::from_hex("51").unwrap().to_binary(),
                    satoshis: COIN - 500,
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
            Some("app.example"),
        )
        .await
        .unwrap();
    let txid = hex::encode(made.txid.unwrap());
    let reference: String = sqlx::query_scalar("SELECT reference FROM transactions WHERE txid = ?")
        .bind(&txid)
        .fetch_one(wallet.storage().pool())
        .await
        .unwrap();
    (txid, reference, coin)
}

/// The command's (exit code, stdout, stderr).
fn ran(p: &Prepared, args: &[&str]) -> (Option<i32>, String, String) {
    let out = run(p.dir.path(), &p.root, args);
    (out.status.code(), text(&out.stdout), text(&out.stderr))
}

/// What an abort is and is not, as every listing and the abort print it.
const ABORT_IS: &str = "An abort frees this wallet's coins and the wallet never sends the \
     action; a prepared transaction whose bytes were handed to someone else can still be \
     posted by them, so abort only a payment that was not handed over";

/// `actions` in text (#673): the prepared payment's line is today's, and
/// under it a line marks it prepared and not sent, with the sats its input
/// holds, its age, its whole txid and its reference; a last line totals
/// the hold and names the two commands. At the base the listing was the
/// count line and one line per action alone.
#[test]
fn actions_in_text() {
    on_runtime(|| async {
        let p = prepared().await;
        let out = said(&p, &["actions"]);
        eprintln!("#673 actions:\n{out}");
        let lines: Vec<&str> = out.lines().collect();
        let at = lines
            .iter()
            .position(|l| l.contains(DESCRIPTION))
            .expect("the prepared payment is listed");
        assert_eq!(
            lines[at],
            format!(
                "  OUT | {} | nosend | -{} sats | {DESCRIPTION}",
                &p.txid[..8],
                spent(&p)
            )
        );
        let mark = lines[at + 1];
        assert!(
            mark.starts_with(&format!(
                "      prepared, not sent: holds {COIN} sats in 1 input, "
            )),
            "{mark}"
        );
        assert!(
            mark.ends_with(&format!(
                " old | txid {} | reference {}",
                p.txid, p.reference
            )),
            "{mark}"
        );
        assert_eq!(
            *lines.last().unwrap(),
            format!(
                "1 prepared, unsent action holds {COIN} sats: `bsv-wallet actions --prepared` \
                 lists them, `bsv-wallet abort-action <txid>` frees one. {ABORT_IS}"
            )
        );
        assert_eq!(lines.len(), 5, "{out}");
        // A mined coin's line has no mark.
        let mined = lines
            .iter()
            .position(|l| l.contains("a mined coin"))
            .unwrap();
        assert!(!lines[mined + 1].starts_with("      prepared"), "{out}");
    });
}

/// `actions --json` (#673): today's keys stay as they were, each of the
/// same JSON type, on every action and at the top; a prepared action
/// carries one ADDED key, `prepared` (its reference, when it was made, its
/// age, the inputs it holds), and the listing one ADDED key,
/// `preparedTotal`. At the base the prepared action had today's eight
/// keys and nothing carried its reference or the hold.
#[test]
fn actions_in_json() {
    on_runtime(|| async {
        let p = prepared().await;
        let out: Value = serde_json::from_str(&said(&p, &["--json", "actions"])).unwrap();
        eprintln!("#673 actions --json: {out}");
        assert_eq!(keys(&out), ["actions", "preparedTotal", "totalActions"]);
        assert!(out["totalActions"].is_u64());
        assert_eq!(
            out["preparedTotal"],
            serde_json::json!({"actions": 1, "heldSatoshis": COIN})
        );
        // Today's keys and their types, on every action.
        let today = [
            ("description", Value::is_string as fn(&Value) -> bool),
            ("isOutgoing", Value::is_boolean),
            ("labels", Value::is_array),
            ("lockTime", Value::is_u64),
            ("satoshis", Value::is_i64),
            ("status", Value::is_string),
            ("txid", Value::is_string),
            ("version", Value::is_u64),
        ];
        for action in out["actions"].as_array().unwrap() {
            for (key, is) in today {
                assert!(is(&action[key]), "{key} in {action}");
            }
        }
        let mined = out["actions"]
            .as_array()
            .unwrap()
            .iter()
            .find(|a| a["txid"] == p.coin.as_str())
            .unwrap();
        assert_eq!(
            keys(mined),
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
        let action = out["actions"]
            .as_array()
            .unwrap()
            .iter()
            .find(|a| a["txid"] == p.txid.as_str())
            .expect("the prepared payment is listed");
        assert_eq!(action["status"], "nosend");
        assert_eq!(action["satoshis"], -(spent(&p) as i64));
        assert_eq!(
            keys(action),
            [
                "description",
                "isOutgoing",
                "labels",
                "lockTime",
                "prepared",
                "satoshis",
                "status",
                "txid",
                "version"
            ]
        );
        let added = &action["prepared"];
        assert_eq!(
            keys(added),
            [
                "ageSeconds",
                "createdAt",
                "heldInputs",
                "heldSatoshis",
                "reference"
            ]
        );
        assert_eq!(added["reference"], p.reference.as_str());
        assert_eq!(added["heldInputs"], 1);
        assert_eq!(added["heldSatoshis"], COIN);
        let now = Utc::now().timestamp();
        let made = added["createdAt"].as_i64().unwrap();
        assert!((now - 600..=now).contains(&made), "{added}");
        assert!((0..600).contains(&added["ageSeconds"].as_i64().unwrap()));
    });
}

/// `actions --prepared` (#673), text and JSON: the prepared actions alone,
/// oldest first, and the total; none at all reads so. `--label` and
/// `--prepared` together are refused by the parser.
#[test]
fn actions_prepared_lists_them_alone() {
    on_runtime(|| async {
        let p = prepared().await;
        let (second, second_ref, _) = another(&p).await;
        let out = said(&p, &["actions", "--prepared"]);
        eprintln!("#673 actions --prepared:\n{out}");
        let lines: Vec<&str> = out.lines().collect();
        assert_eq!(lines.len(), 3, "{out}");
        assert!(
            lines[0].starts_with(&format!("  {} | reference {} | ", p.txid, p.reference)),
            "{out}"
        );
        assert!(
            lines[0].ends_with(&format!(
                " old | holds {COIN} sats in 1 input | -{} sats | {DESCRIPTION}",
                spent(&p)
            )),
            "{out}"
        );
        assert!(
            lines[1].starts_with(&format!("  {second} | reference {second_ref} | ")),
            "{out}"
        );
        assert!(
            lines[2].starts_with(&format!(
                "2 prepared, unsent actions hold {} sats: ",
                2 * COIN
            )),
            "{out}"
        );
        assert!(!out.contains("a mined coin"), "{out}");

        let json: Value =
            serde_json::from_str(&said(&p, &["--json", "actions", "--prepared"])).unwrap();
        eprintln!("#673 actions --prepared --json: {json}");
        assert_eq!(keys(&json), ["heldSatoshis", "prepared"]);
        assert_eq!(json["heldSatoshis"], 2 * COIN);
        let rows = json["prepared"].as_array().unwrap();
        assert_eq!(rows.len(), 2);
        assert_eq!(
            keys(&rows[0]),
            [
                "ageSeconds",
                "createdAt",
                "description",
                "heldInputs",
                "heldSatoshis",
                "reference",
                "satoshis",
                "txid"
            ]
        );
        assert_eq!(
            (
                &rows[0]["txid"],
                &rows[0]["reference"],
                &rows[0]["heldSatoshis"]
            ),
            (
                &Value::from(p.txid.as_str()),
                &Value::from(p.reference.as_str()),
                &Value::from(COIN)
            )
        );
        assert_eq!(rows[1]["txid"], second.as_str());

        let (code, _, err) = ran(&p, &["actions", "--prepared", "--label", "x"]);
        assert_eq!(code, Some(2), "{err}");
    });
}

/// THE TEST THAT DECIDES (#673), `balance`: the prepared payment's change
/// is counted and its reserved coin is not (unchanged by #673: a row).
#[test]
fn balance_counts_the_change_and_not_the_held_coin() {
    on_runtime(|| async {
        let p = prepared().await;
        let out = said(&p, &["balance"]);
        eprintln!("#673 balance: {out}");
        let sats: u64 = out.split_whitespace().next().unwrap().parse().unwrap();
        // The change: the coin less the payment and its fee.
        assert_eq!(sats, COIN - spent(&p), "{out}");
    });
}

/// `abort-action <txid>` (#673): the commands `--help` lists now hold
/// one that aborts, `abort-action`; it aborts the named payment through
/// this host's `abortAction`: the payment reads `failed`, its coin is
/// spendable again with no spender, the other prepared payment is
/// untouched, the listing shows none but that one, and `balance` counts
/// the freed coin. At the base no command aborted it.
#[test]
fn abort_action_by_txid_frees_the_coin_of_that_one_alone() {
    on_runtime(|| async {
        let p = prepared().await;
        let names = commands(&p);
        eprintln!("#673 commands: {names:?}");
        let aborts: Vec<&String> = names.iter().filter(|n| n.contains("abort")).collect();
        assert_eq!(aborts, ["abort-action"], "{names:?}");
        let (second, _, second_coin) = another(&p).await;

        let out = said(&p, &["abort-action", &p.txid]);
        eprintln!("#673 abort-action: {out}");
        assert_eq!(
            out.trim_end(),
            format!(
                "aborted {} (reference {}): 1 input holding {COIN} sats, spendable again; the \
                 action is failed and this wallet will not send it. A transaction whose bytes \
                 were handed to someone else can still be posted by them, and then spends \
                 these coins",
                p.txid, p.reference
            )
        );
        let (status, coin) = store_of(&p).await;
        assert_eq!((status.as_str(), coin), ("failed", (true, None)));

        let listed = said(&p, &["actions", "--prepared"]);
        assert_eq!(listed.lines().count(), 2, "{listed}");
        assert!(listed.starts_with(&format!("  {second} | ")), "{listed}");
        let storage = StorageSqlx::open(p.dir.path().join("wallet.db").to_str().unwrap())
            .await
            .unwrap();
        let held: (String, bool, Option<String>) = sqlx::query_as(
            "SELECT t.status, o.spendable, s.txid FROM outputs o \
             JOIN transactions t ON t.txid = ? \
             LEFT JOIN transactions s ON s.transaction_id = o.spent_by \
             WHERE o.txid = ? AND o.vout = 0",
        )
        .bind(&second)
        .bind(&second_coin)
        .fetch_one(storage.pool())
        .await
        .unwrap();
        assert_eq!(held, ("nosend".to_string(), false, Some(second.clone())));

        // The freed coin is counted again, beside the second payment's
        // change (the balance's own rule, unchanged).
        let balance = said(&p, &["balance"]);
        let sats: u64 = balance.split_whitespace().next().unwrap().parse().unwrap();
        assert!(sats >= COIN, "{balance}");
    });
}

/// `abort-action <reference> --json` (#673): the one action its
/// reference names, aborted; the answer names it, what it was and what it
/// freed. A second abort of it is the wallet's own refusal, exit 1.
#[test]
fn abort_action_by_reference_under_json() {
    on_runtime(|| async {
        let p = prepared().await;
        let out: Value =
            serde_json::from_str(&said(&p, &["--json", "abort-action", &p.reference])).unwrap();
        eprintln!("#673 abort-action --json: {out}");
        assert_eq!(
            out,
            serde_json::json!({
                "aborted": true,
                "txid": p.txid,
                "reference": p.reference,
                "was": "nosend",
                "freedInputs": 1,
                "freedSatoshis": COIN,
            })
        );
        assert_eq!(store_of(&p).await.1, (true, None));

        let (code, out, err) = ran(&p, &["--json", "abort-action", &p.reference]);
        eprintln!("#673 abort-action again: {out}{err}");
        assert_eq!(code, Some(1));
        let refused: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(keys(&refused), ["code", "message"]);
        assert!(
            refused["message"]
                .as_str()
                .unwrap()
                .contains("cannot be aborted"),
            "{refused}"
        );
    });
}

/// `abort-action` of a payment a broadcaster took (#673): refused by the
/// served door's own guard, its code and words, and nothing moves: the
/// payment stays `nosend`, its coin reserved. In text the refusal is on
/// stderr, exit 1; under `--json` the door's body on stdout, exit 1.
#[test]
fn abort_action_refuses_a_payment_a_broadcaster_took() {
    on_runtime(|| async {
        let p = prepared().await;
        let storage = StorageSqlx::open(p.dir.path().join("wallet.db").to_str().unwrap())
            .await
            .unwrap();
        sqlx::query(
            "INSERT INTO broadcast_seen (txid, provider, status) VALUES (?, 'arcade', 'accepted')",
        )
        .bind(&p.txid)
        .execute(storage.writer())
        .await
        .unwrap();
        drop(storage);
        let guard = format!(
            "Invalid operation: a broadcaster took transaction {}; it is not aborted. It stays, \
             re-asked, until a proof, a node verdict or a competitor's checked proof decides it",
            p.txid
        );

        let (code, out, err) = ran(&p, &["abort-action", &p.txid]);
        eprintln!("#673 abort-action of a taken payment: {out}{err}");
        assert_eq!(code, Some(1));
        assert!(out.is_empty(), "{out}");
        assert!(
            err.contains(&format!("abort-action {}: {guard}", p.txid)),
            "{err}"
        );

        let (code, out, _) = ran(&p, &["--json", "abort-action", &p.reference]);
        assert_eq!(code, Some(1));
        let refused: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(
            refused,
            serde_json::json!({"code": "CONFLICT", "message": guard})
        );
        let (status, coin) = store_of(&p).await;
        assert_eq!(status, "nosend");
        assert_eq!(coin, (false, Some(p.txid.clone())));
    });
}

/// `abort-action` names one action: two names are refused by the parser
/// and nothing moves; a name that is no action of this wallet is the
/// wallet's own refusal, exit 1, and nothing moves.
#[test]
fn abort_action_names_one_action() {
    on_runtime(|| async {
        let p = prepared().await;
        let (second, _, _) = another(&p).await;
        let (code, _, err) = ran(&p, &["abort-action", &p.txid, &second]);
        assert_eq!(code, Some(2), "{err}");
        let (code, _, err) = ran(&p, &["abort-action", &"ab".repeat(32)]);
        eprintln!("#673 abort-action of no action: {err}");
        assert_eq!(code, Some(1), "{err}");
        let listed = said(&p, &["actions", "--prepared"]);
        assert_eq!(listed.lines().count(), 3, "{listed}");
        assert_eq!(store_of(&p).await.0, "nosend");
    });
}

/// The help of `abort-action` says what an abort is and is not.
#[test]
fn abort_actions_help_says_what_an_abort_is_not() {
    on_runtime(|| async {
        let p = prepared().await;
        let help = said(&p, &["abort-action", "--help"]).replace('\n', " ");
        let help = help.split_whitespace().collect::<Vec<_>>().join(" ");
        for said in [
            "freeing the coins its inputs hold",
            "A transaction a broadcaster took is never aborted",
            "a prepared transaction whose bytes were handed to someone else can still be \
             posted by them, so abort only a payment that was not handed over",
        ] {
            assert!(help.contains(said), "{help}");
        }
    });
}

/// The store, unchanged by any listing: the prepared payment reads
/// `nosend`, its coin reserved, after `actions`, `--json actions` and
/// `actions --prepared`.
#[test]
fn no_listing_moves_the_store() {
    on_runtime(|| async {
        let p = prepared().await;
        for args in [
            &["actions"][..],
            &["--json", "actions"],
            &["actions", "--prepared"],
            &["--json", "actions", "--prepared"],
        ] {
            said(&p, args);
        }
        let (status, coin) = store_of(&p).await;
        assert_eq!(status, "nosend");
        assert_eq!(coin, (false, Some(p.txid.clone())));
    });
}
