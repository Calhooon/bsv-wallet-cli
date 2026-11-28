//! A prepared, unsent action at the command line (#673): a wallet holding
//! a `noSend` action (signed, never sent) whose one input, a coin of the
//! wallet, is reserved by it. What `actions` prints for it, in text and
//! under `--json`, what `actions --prepared` lists, what `balance` prints,
//! and what `abort-action` frees and refuses.
//!
//! The store is a file under a temporary directory, seeded in this process:
//! a mined coin written row by row (its proof stored), then the toolbox's
//! own `createAction` with `noSend: true`, on the toolbox's mock services,
//! which answer from memory and post nowhere. The commands then run as a
//! user runs them, the binary on that file with the wallet's `ROOT_KEY`.
//! The binary builds the configured services: `CHAINTRACKS_URL=off` (no
//! header service), the broadcaster's URL and the explorers' bases at a
//! closed local port or off, and every proxy at the same closed port, so
//! any request the run made would fail at once. The listings and the
//! balance read the store; the toolbox's abort reads its records "without
//! a header-service or explorer call" (its CHANGELOG).

use std::process::Command;

use bsv_sdk::primitives::PrivateKey;
use bsv_sdk::script::{templates::P2PKH, LockingScript};
use bsv_sdk::transaction::{MerklePath, Transaction};
use bsv_sdk::wallet::{
    Counterparty, CreateActionArgs, CreateActionOptions, CreateActionOutput, KeyDeriver, Protocol,
    SecurityLevel, WalletInterface,
};
use bsv_wallet_toolbox::services::mock::MockWalletServices;
use bsv_wallet_toolbox::{StorageSqlx, Wallet, WalletStorageWriter};
use serde_json::{json, Value};
use tempfile::TempDir;

/// Each of the wallet's coins.
const COIN: i64 = 100_000;
/// The prepared payment's one output.
const PAYMENT: i64 = 1_000;
/// The prepared payment's description.
const DESCRIPTION: &str = "a prepared payment";
/// The derivation the seeded coins are locked to: the wallet's change key.
const PREFIX: &str = "cHJlcGFyZWQ=";
const SUFFIX: &str = "Y29pbg==";

/// A closed local port: anything sent there fails at once.
const CLOSED: &str = "http://127.0.0.1:9";

/// The run's environment: each variable set, or (`None`) removed so the
/// shell's value never reaches the run.
const ENVIRONMENT: [(&str, Option<&str>); 23] = [
    ("CHAINTRACKS_URL", Some("off")),
    ("ARC_URL", Some(CLOSED)),
    ("WOC_BASE", Some(CLOSED)),
    ("BITAILS_BASE", Some(CLOSED)),
    ("BANANABLOCKS_BASE", Some("")),
    ("ARC_GORILLAPOOL_URL", Some("")),
    ("HTTPS_PROXY", Some(CLOSED)),
    ("HTTP_PROXY", Some(CLOSED)),
    ("ALL_PROXY", Some(CLOSED)),
    ("NO_PROXY", Some("")),
    ("RUST_LOG", Some("warn")),
    ("ARC_MODE", None),
    ("ARCADE", None),
    ("TAAL_API_KEY", None),
    ("MAIN_TAAL_API_KEY", None),
    ("WOC_API_KEY", None),
    ("BITAILS_API_KEY", None),
    ("ALLOWED_ORIGINS", None),
    ("AUTH_TOKEN", None),
    ("TRUST_SETTINGS", None),
    ("SPEND_DEPTH", None),
    ("SPEND_OWN_CHANGE", None),
    ("BREAK_GLASS_EXPLORER_HEADERS", None),
];

/// What an abort is and is not, as every listing and the abort print it.
const ABORT_IS: &str = "An abort frees this wallet's coins and the wallet never sends the \
     action; a prepared transaction whose bytes were handed to someone else can still be \
     posted by them, so abort only a payment that was not handed over";

/// A wallet with a prepared payment: its directory (the store is
/// `wallet.db` there), its root key, the payment's txid and reference, the
/// reserved coin's txid, and what the payment spends from the wallet (its
/// output and its fee: the coin less its change).
struct Prepared {
    dir: TempDir,
    root: PrivateKey,
    txid: String,
    reference: String,
    coin: String,
    spent: i64,
}

impl Prepared {
    /// The store, opened on its file.
    async fn store(&self) -> StorageSqlx {
        let path = self.dir.path().join("wallet.db");
        let storage = StorageSqlx::open(&path.to_string_lossy()).await.unwrap();
        storage.make_available().await.unwrap();
        storage
    }
}

/// The wallet of one coin with one prepared, unsent payment holding it.
async fn prepared() -> Prepared {
    let dir = TempDir::new().unwrap();
    let root = PrivateKey::random();
    let storage = StorageSqlx::open(&dir.path().join("wallet.db").to_string_lossy())
        .await
        .unwrap();
    storage
        .migrate("prepared-cli", &root.public_key().to_hex())
        .await
        .unwrap();
    storage.make_available().await.unwrap();
    let coin = seed_coin(&storage, &root, 0).await;
    let (txid, reference, change) = pay(storage, &root, DESCRIPTION, PAYMENT).await;
    Prepared {
        dir,
        root,
        txid,
        reference,
        coin,
        spent: COIN - change,
    }
}

/// A second prepared payment on the wallet `p`: a second coin, then a
/// payment too large for the earlier payment's change, so it holds the
/// second coin alone. Its txid, its reference and the coin's txid.
async fn another(p: &Prepared) -> (String, String, String) {
    let storage = p.store().await;
    let coin = seed_coin(&storage, &p.root, 1).await;
    let (txid, reference, _) = pay(storage, &p.root, "a second prepared payment", COIN - 500).await;
    (txid, reference, coin)
}

/// The toolbox's own `createAction` of one `sats` output, `noSend`, on the
/// mock services: its txid, its reference and its change's satoshis.
async fn pay(
    storage: StorageSqlx,
    root: &PrivateKey,
    description: &str,
    sats: i64,
) -> (String, String, i64) {
    let wallet = Wallet::new(
        Some(root.clone()),
        storage,
        MockWalletServices::builder().build(),
    )
    .await
    .unwrap();
    let output = CreateActionOutput {
        locking_script: LockingScript::from_hex("51").unwrap().to_binary(),
        satoshis: u64::try_from(sats).unwrap(),
        output_description: "the payment".into(),
        basket: None,
        custom_instructions: None,
        tags: None,
    };
    let options = CreateActionOptions {
        no_send: Some(true),
        accept_delayed_broadcast: Some(false),
        ..Default::default()
    };
    let args = CreateActionArgs {
        description: description.into(),
        outputs: Some(vec![output]),
        options: Some(options),
        input_beef: None,
        inputs: None,
        lock_time: None,
        version: None,
        labels: None,
    };
    let made = wallet
        .create_action(args, Some("app.example"))
        .await
        .unwrap();
    let txid = hex::encode(made.txid.unwrap());
    let (reference, change): (String, i64) = sqlx::query_as(
        "SELECT t.reference, (SELECT COALESCE(SUM(satoshis), 0) FROM outputs \
         WHERE transaction_id = t.transaction_id AND change = 1) \
         FROM transactions t WHERE t.txid = ?",
    )
    .bind(&txid)
    .fetch_one(wallet.storage().pool())
    .await
    .unwrap();
    (txid, reference, change)
}

/// The `n`th coin's locking script (the wallet's change key at
/// (`PREFIX`, `SUFFIX`)) and its transaction's bytes: version 1, one input
/// of no parent with an empty-push script, one output of `COIN` sats, lock
/// time `n`.
fn coin_bytes(root: &PrivateKey, n: u32) -> (Vec<u8>, Vec<u8>) {
    let protocol = Protocol::new(SecurityLevel::Counterparty, "3241645161d8");
    let owner = KeyDeriver::new(Some(root.clone()))
        .derive_private_key(
            &protocol,
            &format!("{PREFIX} {SUFFIX}"),
            &Counterparty::Self_,
        )
        .unwrap()
        .public_key();
    let script = P2PKH::lock_from_address(&owner.to_address())
        .unwrap()
        .to_binary();
    let mut raw = 1u32.to_le_bytes().to_vec();
    raw.extend([1u8; 1]);
    raw.extend([0u8; 32]);
    raw.extend(u32::MAX.to_le_bytes());
    raw.extend([1u8, 0u8]);
    raw.extend(u32::MAX.to_le_bytes());
    raw.push(1);
    raw.extend(COIN.to_le_bytes());
    raw.push(u8::try_from(script.len()).unwrap());
    raw.extend(&script);
    raw.extend(n.to_le_bytes());
    (script, raw)
}

/// The proof of the coin `txid` (bytes `raw`), at height 870,800 in a
/// block of one transaction, stored: its row's id.
async fn store_proof(storage: &StorageSqlx, txid: &str, raw: &[u8]) -> i64 {
    let proof = MerklePath::from_coinbase_txid(txid, 870_800)
        .to_binary()
        .unwrap();
    let now = chrono::Utc::now();
    sqlx::query_scalar(
        "INSERT INTO proven_txs (created_at, updated_at, txid, height, idx, merkle_path, raw_tx, \
         block_hash, merkle_root) VALUES (?1, ?1, ?2, 870800, 0, ?3, ?4, ?5, ?2) \
         RETURNING proven_tx_id",
    )
    .bind(now)
    .bind(txid)
    .bind(&proof)
    .bind(raw)
    .bind("cc".repeat(32))
    .fetch_one(storage.writer())
    .await
    .unwrap()
}

/// The `n`th mined coin of `COIN` sats, locked to the wallet's change key
/// at (`PREFIX`, `SUFFIX`): its transaction (one input of no parent, its
/// lock time `n`), its output, its proof at height 870,800 (a block of one
/// transaction). Its txid.
async fn seed_coin(storage: &StorageSqlx, root: &PrivateKey, n: u32) -> String {
    let identity = root.public_key().to_hex();
    let user_id = storage
        .find_or_insert_user(&identity)
        .await
        .unwrap()
        .0
        .user_id;
    let basket_id = storage
        .find_or_create_default_basket(user_id)
        .await
        .unwrap()
        .basket_id;
    let (script, raw) = coin_bytes(root, n);
    let txid = Transaction::from_binary(&raw).unwrap().id();
    let now = chrono::Utc::now();
    let pool = storage.writer();
    let proven = store_proof(storage, &txid, &raw).await;
    let row: i64 = sqlx::query_scalar(
        "INSERT INTO transactions (created_at, updated_at, user_id, proven_tx_id, status, \
         reference, is_outgoing, satoshis, description, version, lock_time, txid, raw_tx) \
         VALUES (?1, ?1, ?2, ?3, 'completed', ?4, 0, ?5, 'a mined coin', 1, ?6, ?7, ?8) \
         RETURNING transaction_id",
    )
    .bind(now)
    .bind(user_id)
    .bind(proven)
    .bind(format!("coin-{n}"))
    .bind(COIN)
    .bind(n)
    .bind(&txid)
    .bind(&raw)
    .fetch_one(pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO outputs (created_at, updated_at, user_id, transaction_id, basket_id, \
         txid, vout, satoshis, locking_script, type, purpose, provided_by, spendable, change, \
         derivation_prefix, derivation_suffix, output_description) \
         VALUES (?1, ?1, ?2, ?3, ?4, ?5, 0, ?6, ?7, 'P2PKH', 'change', 'storage', 1, 1, ?8, ?9, \
         'a mined coin')",
    )
    .bind(now)
    .bind(user_id)
    .bind(row)
    .bind(basket_id)
    .bind(&txid)
    .bind(COIN)
    .bind(&script)
    .bind(PREFIX)
    .bind(SUFFIX)
    .execute(pool)
    .await
    .unwrap();
    txid
}

fn on_runtime<F: std::future::Future<Output = ()>>(f: impl FnOnce() -> F) {
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(f());
}

/// The binary run on the wallet `p` with `args`: (exit code, stdout,
/// stderr).
fn cli(p: &Prepared, args: &[&str]) -> (Option<i32>, String, String) {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_bsv-wallet"));
    cmd.current_dir(p.dir.path());
    for (name, value) in ENVIRONMENT {
        match value {
            Some(value) => cmd.env(name, value),
            None => cmd.env_remove(name),
        };
    }
    let out = cmd
        .env("ROOT_KEY", p.root.to_hex())
        .args(["--db", "wallet.db"])
        .args(args)
        .output()
        .unwrap();
    let words = |b: &[u8]| String::from_utf8_lossy(b).into_owned();
    (out.status.code(), words(&out.stdout), words(&out.stderr))
}

/// The command's stdout, asserting it exits 0.
fn said(p: &Prepared, args: &[&str]) -> String {
    let (code, out, err) = cli(p, args);
    assert_eq!(code, Some(0), "{args:?}: {out}{err}");
    out
}

/// The store's word for the payment `txid`, and the coin `coin`'s
/// (spendable, its spender's txid).
async fn held(p: &Prepared, txid: &str, coin: &str) -> (String, bool, Option<String>) {
    sqlx::query_as(
        "SELECT (SELECT status FROM transactions WHERE txid = ?1), o.spendable, s.txid \
         FROM outputs o LEFT JOIN transactions s ON s.transaction_id = o.spent_by \
         WHERE o.txid = ?2 AND o.vout = 0",
    )
    .bind(txid)
    .bind(coin)
    .fetch_one(p.store().await.pool())
    .await
    .unwrap()
}

/// The payment `p` still prepared, its coin reserved by it.
async fn still_prepared(p: &Prepared) {
    let reserved = ("nosend".to_string(), false, Some(p.txid.clone()));
    assert_eq!(held(p, &p.txid, &p.coin).await, reserved);
}

/// The names of the commands `--help` lists.
fn commands(p: &Prepared) -> Vec<String> {
    said(p, &["--help"])
        .lines()
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

/// The keys of the door's action object today (the `listActions` shape
/// of the `MetaNet` Client), sorted.
const TODAY: [&str; 8] = [
    "description",
    "isOutgoing",
    "labels",
    "lockTime",
    "satoshis",
    "status",
    "txid",
    "version",
];

/// The action of `txid` in a `--json` listing.
fn action<'a>(listing: &'a Value, txid: &str) -> &'a Value {
    listing["actions"]
        .as_array()
        .unwrap()
        .iter()
        .find(|a| a["txid"] == txid)
        .unwrap()
}

/// A prepared payment's wallet and its `--json actions` listing.
async fn listed_in_json() -> (Prepared, Value) {
    let p = prepared().await;
    let listing = said(&p, &["--json", "actions"]);
    (p, serde_json::from_str(&listing).unwrap())
}

/// THE TEST THAT DECIDES (#673), the store: the prepared payment reads
/// `nosend`, the wallet's only coin reserved by it.
#[test]
fn the_prepared_payment_holds_the_wallets_coin() {
    on_runtime(|| async {
        still_prepared(&prepared().await).await;
    });
}

/// THE TEST THAT DECIDES (#673), `actions` in text. At the base the
/// payment was one line like any other (`OUT | <eight digits> | nosend |
/// -<payment and fee> sats | <description>`), with no whole txid, no
/// reference, no age, nothing of the sats its input holds, and no total.
/// Now that line stands, a line under it marks it prepared and not sent
/// with the sats its input holds, its age, its whole txid and its
/// reference, and a last line totals the hold and names the two commands.
#[test]
fn actions_in_text() {
    on_runtime(|| async {
        let p = prepared().await;
        let out = said(&p, &["actions"]);
        eprintln!("#673 actions:\n{out}");
        let lines: Vec<&str> = out.lines().collect();
        let at = lines.iter().position(|l| l.contains(DESCRIPTION)).unwrap();
        let today = format!(
            "  OUT | {} | nosend | -{} sats | {DESCRIPTION}",
            &p.txid[..8],
            p.spent
        );
        assert_eq!(lines[at], today);
        let mark = lines[at + 1];
        let holds = format!("      prepared, not sent: holds {COIN} sats in 1 input, ");
        assert!(mark.starts_with(&holds), "{mark}");
        let named = format!(" old | txid {} | reference {}", p.txid, p.reference);
        assert!(mark.ends_with(&named), "{mark}");
        let total = format!(
            "1 prepared, unsent action holds {COIN} sats: `bsv-wallet actions --prepared` \
             lists them, `bsv-wallet abort-action <txid>` frees one. {ABORT_IS}"
        );
        assert_eq!(*lines.last().unwrap(), total);
        assert_eq!(lines.len(), 5, "{out}");
        let mined = lines
            .iter()
            .position(|l| l.contains("a mined coin"))
            .unwrap();
        assert!(!lines[mined + 1].starts_with("      prepared"), "{out}");
    });
}

/// THE TEST THAT DECIDES (#673), `actions --json`, today's keys. At the
/// base the listing's keys were `actions` and `totalActions`, and the
/// payment's object had today's eight keys, status `nosend`, nothing
/// carrying its reference or the hold. Today's keys stay, each of the
/// same JSON type, on every action: a caller that parses them keeps
/// working; the mined coin's object is today's exactly.
#[test]
fn actions_in_json_keeps_todays_keys() {
    on_runtime(|| async {
        let (p, out) = listed_in_json().await;
        eprintln!("#673 actions --json: {out}");
        assert_eq!(keys(&out), ["actions", "preparedTotal", "totalActions"]);
        assert!(out["totalActions"].is_u64());
        let types: [fn(&Value) -> bool; 8] = [
            Value::is_string,
            Value::is_boolean,
            Value::is_array,
            Value::is_u64,
            Value::is_i64,
            Value::is_string,
            Value::is_string,
            Value::is_u64,
        ];
        for listed in out["actions"].as_array().unwrap() {
            for (key, is) in TODAY.iter().zip(types) {
                assert!(is(&listed[key]), "{key} in {listed}");
            }
        }
        assert_eq!(keys(action(&out, &p.coin)), TODAY);
        let payment = action(&out, &p.txid);
        assert_eq!(payment["status"], "nosend");
        assert_eq!(payment["satoshis"], -p.spent);
    });
}

/// `actions --json`, the ADDED keys (#673): the payment's object carries
/// `prepared` (its reference, when it was made, its age, the inputs it
/// holds) beside today's keys, and the listing `preparedTotal`.
#[test]
fn actions_in_json_adds_the_prepared_keys() {
    on_runtime(|| async {
        let (p, out) = listed_in_json().await;
        let total = json!({"actions": 1, "heldSatoshis": COIN});
        assert_eq!(out["preparedTotal"], total);
        let payment = action(&out, &p.txid);
        let mut want: Vec<&str> = TODAY.to_vec();
        want.push("prepared");
        want.sort_unstable();
        assert_eq!(keys(payment), want);
        let added = &payment["prepared"];
        let named = [
            "ageSeconds",
            "createdAt",
            "heldInputs",
            "heldSatoshis",
            "reference",
        ];
        assert_eq!(keys(added), named);
        assert_eq!(added["reference"], p.reference.as_str());
        assert_eq!(
            (&added["heldInputs"], &added["heldSatoshis"]),
            (&json!(1), &json!(COIN))
        );
        let now = chrono::Utc::now().timestamp();
        let made = added["createdAt"].as_i64().unwrap();
        assert!((now - 600..=now).contains(&made), "{added}");
        assert!((0..600).contains(&added["ageSeconds"].as_i64().unwrap()));
    });
}

/// `actions --prepared` (#673): the prepared payments alone, from the
/// oldest, and the total; the mined coin is not listed. `--label` and
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
        let head = format!("  {} | reference {} | ", p.txid, p.reference);
        assert!(lines[0].starts_with(&head), "{out}");
        let tail = format!(
            " old | holds {COIN} sats in 1 input | -{} sats | {DESCRIPTION}",
            p.spent
        );
        assert!(lines[0].ends_with(&tail), "{out}");
        let head = format!("  {second} | reference {second_ref} | ");
        assert!(lines[1].starts_with(&head), "{out}");
        let total = format!("2 prepared, unsent actions hold {} sats: ", 2 * COIN);
        assert!(lines[2].starts_with(&total), "{out}");
        assert!(!out.contains("a mined coin"), "{out}");
        let (code, _, err) = cli(&p, &["actions", "--prepared", "--label", "x"]);
        assert_eq!(code, Some(2), "{err}");
    });
}

/// `actions --prepared --json` (#673): the rows, from the oldest, and the
/// satoshis they hold.
#[test]
fn actions_prepared_in_json() {
    on_runtime(|| async {
        let p = prepared().await;
        let (second, _, _) = another(&p).await;
        let out = said(&p, &["--json", "actions", "--prepared"]);
        let listed: Value = serde_json::from_str(&out).unwrap();
        eprintln!("#673 actions --prepared --json: {listed}");
        assert_eq!(keys(&listed), ["heldSatoshis", "prepared"]);
        assert_eq!(listed["heldSatoshis"], 2 * COIN);
        let rows = listed["prepared"].as_array().unwrap();
        assert_eq!(rows.len(), 2);
        let named = [
            "ageSeconds",
            "createdAt",
            "description",
            "heldInputs",
            "heldSatoshis",
            "reference",
            "satoshis",
            "txid",
        ];
        assert_eq!(keys(&rows[0]), named);
        assert_eq!(rows[0]["txid"], p.txid.as_str());
        assert_eq!(rows[0]["reference"], p.reference.as_str());
        assert_eq!(rows[0]["heldSatoshis"], COIN);
        assert_eq!(rows[0]["satoshis"], -p.spent);
        assert_eq!(rows[1]["txid"], second.as_str());
    });
}

/// THE TEST THAT DECIDES (#673), `balance`: the payment's change is
/// counted and its reserved coin is not (at the base, and unchanged here:
/// a row).
#[test]
fn balance_counts_the_change_and_not_the_held_coin() {
    on_runtime(|| async {
        let p = prepared().await;
        let out = said(&p, &["balance"]);
        eprintln!("#673 balance: {out}");
        let sats: i64 = out.split_whitespace().next().unwrap().parse().unwrap();
        assert_eq!(sats, COIN - p.spent, "{out}");
    });
}

/// THE TEST THAT DECIDES (#673), the commands. At the base none of the
/// commands `--help` listed aborted an action. Now one does,
/// `abort-action`, and its help says what an abort is and is not.
#[test]
fn one_command_aborts_and_says_what_an_abort_is_not() {
    on_runtime(|| async {
        let p = prepared().await;
        let names = commands(&p);
        eprintln!("#673 commands: {names:?}");
        let aborts: Vec<&String> = names.iter().filter(|n| n.contains("abort")).collect();
        assert_eq!(aborts, ["abort-action"], "{names:?}");
        let help = said(&p, &["abort-action", "--help"]);
        let help = help.split_whitespace().collect::<Vec<_>>().join(" ");
        for words in [
            "freeing the coins its inputs hold",
            "A transaction a broadcaster took is never aborted",
            "a prepared transaction whose bytes were handed to someone else can still be \
             posted by them, so abort only a payment that was not handed over",
        ] {
            assert!(help.contains(words), "{help}");
        }
        still_prepared(&p).await;
    });
}

/// `abort-action <txid>` (#673) aborts the named payment through this
/// host's `abortAction`: it reads `failed`, its coin is spendable with no
/// spender, the other prepared payment is untouched, and `balance` counts
/// the freed coin.
#[test]
fn abort_action_by_txid_frees_the_coin_of_that_one_alone() {
    on_runtime(|| async {
        let p = prepared().await;
        let (second, _, second_coin) = another(&p).await;
        let out = said(&p, &["abort-action", &p.txid]);
        eprintln!("#673 abort-action: {out}");
        let answer = format!(
            "aborted {} (reference {}): 1 input holding {COIN} sats, spendable again; the \
             action is failed and this wallet will not send it. A transaction whose bytes \
             were handed to someone else can still be posted by them, and then spends \
             these coins",
            p.txid, p.reference
        );
        assert_eq!(out.trim_end(), answer);
        let freed = ("failed".to_string(), true, None);
        assert_eq!(held(&p, &p.txid, &p.coin).await, freed);
        let other = ("nosend".to_string(), false, Some(second.clone()));
        assert_eq!(held(&p, &second, &second_coin).await, other);
        let listed = said(&p, &["actions", "--prepared"]);
        assert_eq!(listed.lines().count(), 2, "{listed}");
        assert!(listed.starts_with(&format!("  {second} | ")), "{listed}");
        let balance = said(&p, &["balance"]);
        let sats: i64 = balance.split_whitespace().next().unwrap().parse().unwrap();
        assert!(sats >= COIN, "{balance}");
    });
}

/// `abort-action <reference> --json` (#673): the answer names the action,
/// what it was and what it freed. A second abort of it is the wallet's own
/// refusal, the door's body, exit 1.
#[test]
fn abort_action_by_reference_under_json() {
    on_runtime(|| async {
        let p = prepared().await;
        let out = said(&p, &["--json", "abort-action", &p.reference]);
        let answer: Value = serde_json::from_str(&out).unwrap();
        eprintln!("#673 abort-action --json: {answer}");
        let want = json!({
            "aborted": true,
            "txid": p.txid,
            "reference": p.reference,
            "was": "nosend",
            "freedInputs": 1,
            "freedSatoshis": COIN,
        });
        assert_eq!(answer, want);
        assert!(held(&p, &p.txid, &p.coin).await.1);
        let (code, out, err) = cli(&p, &["--json", "abort-action", &p.reference]);
        eprintln!("#673 abort-action again: {out}{err}");
        assert_eq!(code, Some(1));
        let refused: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(keys(&refused), ["code", "message"]);
        let message = refused["message"].as_str().unwrap();
        assert!(message.contains("cannot be aborted"), "{refused}");
    });
}

/// `abort-action` of a payment a broadcaster took (#673): refused by the
/// served door's own guard, its code and words, and nothing moves. In text
/// the refusal is on stderr, exit 1; under `--json` the door's body on
/// stdout, exit 1.
#[test]
fn abort_action_refuses_a_payment_a_broadcaster_took() {
    on_runtime(|| async {
        let p = prepared().await;
        let taken =
            "INSERT INTO broadcast_seen (txid, provider, status) VALUES (?, 'arcade', 'accepted')";
        let storage = p.store().await;
        let wrote = sqlx::query(taken)
            .bind(&p.txid)
            .execute(storage.writer())
            .await;
        assert!(wrote.is_ok(), "{wrote:?}");
        drop(storage);
        let guard = format!(
            "Invalid operation: a broadcaster took transaction {}; it is not aborted. It stays, \
             re-asked, until a proof, a node verdict or a competitor's checked proof decides it",
            p.txid
        );
        let (code, out, err) = cli(&p, &["abort-action", &p.txid]);
        eprintln!("#673 abort-action of a taken payment: {out}{err}");
        assert_eq!((code, out.as_str()), (Some(1), ""));
        assert!(
            err.contains(&format!("abort-action {}: {guard}", p.txid)),
            "{err}"
        );
        let (code, out, _) = cli(&p, &["--json", "abort-action", &p.reference]);
        assert_eq!(code, Some(1));
        let refused: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(refused, json!({"code": "CONFLICT", "message": guard}));
        still_prepared(&p).await;
    });
}

/// `abort-action` names one action: two names are refused by the parser,
/// a name that is no action of this wallet is the wallet's own refusal,
/// exit 1, and neither moves anything.
#[test]
fn abort_action_names_one_action() {
    on_runtime(|| async {
        let p = prepared().await;
        let (second, _, _) = another(&p).await;
        let (code, _, err) = cli(&p, &["abort-action", &p.txid, &second]);
        assert_eq!(code, Some(2), "{err}");
        let (code, _, err) = cli(&p, &["abort-action", &"ab".repeat(32)]);
        eprintln!("#673 abort-action of no action: {err}");
        assert_eq!(code, Some(1), "{err}");
        let listed = said(&p, &["actions", "--prepared"]);
        assert_eq!(listed.lines().count(), 3, "{listed}");
        still_prepared(&p).await;
    });
}

/// No listing moves the store: after `actions`, `--json actions` and
/// `actions --prepared` in both forms the payment is still prepared.
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
        still_prepared(&p).await;
    });
}
