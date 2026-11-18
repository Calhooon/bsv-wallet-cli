//! The helpers of the served `POST /internalizeAction` tests of the
//! toolbox's 0.14.0-beta.8.1 door (`tests/served_internalize_door.rs` and
//! `tests/served_internalize_restores.rs`, the port of the stable line's
//! 0.18.6): the served router over a fresh wallet with or without a chain
//! tracker, the posts and the row counts, the toolbox's `beef_paying`,
//! the rows read back, the reclassification an earlier release wrote, and
//! the spend run through to its checked unlocking script. Each test target
//! declares `mod chain_tracker` and `mod no_tracker` at its root
//! (`src/test_support/`), which these helpers read.

use crate::chain_tracker::TestChainTracker;
use crate::no_tracker::NoTracker;

use axum::http::StatusCode;
use bsv_sdk::primitives::bsv::sighash::{parse_transaction, TxSighashCache};
use bsv_sdk::primitives::{PrivateKey, PublicKey};
use bsv_sdk::script::{
    LockingScript, ProtocolEra, ScriptFlags, Spend, TxSpendParams, UnlockingScript,
};
use bsv_sdk::transaction::{Beef, MerklePath, Transaction, TransactionInput, TransactionOutput};
use bsv_sdk::wallet::{
    Counterparty, CreateActionArgs, CreateActionOptions, CreateActionOutput, KeyDeriver, Protocol,
    SecurityLevel, SignActionArgs, SignActionOptions, WalletInterface,
};
use serde_json::{json, Value};
use std::sync::Arc;
use tower::ServiceExt;

// The wallet engine, then the host's server over it.
use bsv_wallet_toolbox::services::mock::{
    success_post_beef_result, MockResponse, MockWalletServices,
};
use bsv_wallet_toolbox::{
    Chain, Services, ServicesOptions, StorageSqlx, Wallet, WalletServices, WalletStorageProvider,
    WalletStorageWriter,
};

use bsv_wallet_cli::server::{self, ServerConfig};

/// The served wallet, as `server::make_wallet_state` returns it.
pub(crate) type WalletState = Arc<Wallet<StorageSqlx, Services>>;

pub(crate) const PREFIX: &str = "aW5zZXJ0LXByZWZpeA==";
pub(crate) const SUFFIX: &str = "aW5zZXJ0LXN1ZmZpeA==";
/// The derivation `fund`, `receive`, `sync`, `split` and `gift-claim` write
/// with the anyone key as the sender (`src/brc29.rs`).
pub(crate) const CLI_PREFIX: &str = "SfKxPIJNgdI=";
pub(crate) const CLI_SUFFIX: &str = "NaGLC6fMH50=";
/// A 25-byte P2PKH paying neither party of the tests.
pub(crate) const OTHER_HASH: &str = "dbc0a7c84983c5bf199b7b2d41b3acf0408ee5aa";
pub(crate) const SPEND: &str = "a spend of the restored coin";
/// The height of the subject's parent's proof: after Genesis, and at least
/// 100 below the tracker's tip (bsv-rs #734).
pub(crate) const PARENT_HEIGHT: u32 = ProtocolEra::MAINNET_GENESIS_HEIGHT + 101;

// ---------------------------------------------------------------------------
// The served wallet
// ---------------------------------------------------------------------------

/// The mock broadcaster, accepting every transaction posted.
pub(crate) fn accepting() -> MockWalletServices {
    MockWalletServices::builder()
        .post_beef_response(MockResponse::Success(vec![success_post_beef_result(
            "arc",
            &[],
        )]))
        .build()
}

/// The served router over a fresh wallet of `root` whose storage checks
/// roots against a tracker carrying `proven` as the root at
/// [`PARENT_HEIGHT`], and posts to a mock that accepts every transaction,
/// read back by its call count.
pub(crate) async fn served(
    root: &PrivateKey,
    proven: Option<&str>,
) -> (WalletState, axum::Router, Arc<MockWalletServices>) {
    let posts = Arc::new(accepting());
    let (wallet, router) = served_with(root, proven, posts.clone()).await;
    (wallet, router, posts)
}

/// The served router over a fresh wallet of `root` built as
/// `CHAINTRACKS_URL=off` builds it: no tracker on the storage, and
/// services with none (`Services` with no `chaintracks_url` for the
/// wallet; for the storage, whose services the door reads when it has no
/// tracker and which every post goes through, the same mock behind
/// [`NoTracker`], so the posts are counted).
pub(crate) async fn served_untracked(
    root: &PrivateKey,
) -> (WalletState, axum::Router, Arc<NoTracker>) {
    let posts = Arc::new(NoTracker(accepting()));
    let (wallet, router) = served_with(root, None, posts.clone()).await;
    (wallet, router, posts)
}

pub(crate) async fn served_with(
    root: &PrivateKey,
    proven: Option<&str>,
    posts: Arc<dyn WalletServices>,
) -> (WalletState, axum::Router) {
    let storage = StorageSqlx::in_memory().await.expect("in-memory storage");
    storage
        .migrate("served-internalize-door", &root.public_key().to_hex())
        .await
        .expect("migrate");
    storage.make_available().await.expect("make available");
    bsv_wallet_toolbox::MonitorStorage::set_max_acceptable_proof_height(&storage, u32::MAX)
        .await
        .expect("open the proof gate");
    if let Some(txid) = proven {
        let mut tracker = TestChainTracker::new(900_000);
        tracker.add_root(PARENT_HEIGHT, txid.to_string());
        storage.set_chain_tracker(Arc::new(tracker)).await;
    }
    let services =
        Services::with_options(Chain::Main, ServicesOptions::mainnet()).expect("services");
    assert!(services.chaintracks.is_none(), "no header service");
    let wallet = Wallet::new(Some(root.clone()), storage, services)
        .await
        .expect("wallet");
    let state = server::make_wallet_state(wallet);
    WalletStorageProvider::set_services(state.storage(), posts);
    let config = ServerConfig::admitting(["test.local"]).expect("an originator");
    let router = server::make_router(state.clone(), config);
    (state, router)
}

/// Posts `body` to `route` of the served router: the status and the JSON
/// answer.
pub(crate) async fn post_to(app: &axum::Router, route: &str, body: Value) -> (StatusCode, Value) {
    let request = axum::http::Request::builder()
        .method("POST")
        .uri(route)
        .header("Content-Type", "application/json")
        .header("Origin", "http://test.local")
        .body(axum::body::Body::from(body.to_string()))
        .unwrap();
    let response = app.clone().oneshot(request).await.unwrap();
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    (status, serde_json::from_slice(&bytes).unwrap())
}

/// `POST /internalizeAction` of `tx` with `outputs`.
pub(crate) async fn internalize(
    app: &axum::Router,
    tx: &[u8],
    outputs: Vec<Value>,
) -> (StatusCode, Value) {
    post_to(
        app,
        "/internalizeAction",
        json!({ "tx": tx, "outputs": outputs, "description": "an output to check" }),
    )
    .await
}

/// A wallet payment of output `vout` naming `prefix`, `suffix` and `sender`
/// as given (any string, the empty one included).
pub(crate) fn payment_of(vout: u32, prefix: &str, suffix: &str, sender: &str) -> Value {
    json!({
        "outputIndex": vout,
        "protocol": "wallet payment",
        "paymentRemittance": {
            "derivationPrefix": prefix,
            "derivationSuffix": suffix,
            "senderIdentityKey": sender
        }
    })
}

pub(crate) fn insertion(vout: u32, basket: &str) -> Value {
    json!({ "outputIndex": vout, "protocol": "basket insertion",
        "insertionRemittance": { "basket": basket } })
}

pub(crate) fn assert_refused(status: StatusCode, body: &Value, code: &str, message: &str) {
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert_eq!(body, &json!({ "code": code, "message": message }));
}

/// The rows of every table an internalize writes.
pub(crate) async fn stored(wallet: &WalletState) -> (i64, i64, i64, i64, i64) {
    sqlx::query_as(
        "SELECT (SELECT COUNT(*) FROM transactions), (SELECT COUNT(*) FROM outputs), \
         (SELECT COUNT(*) FROM proven_txs), (SELECT COUNT(*) FROM proven_tx_reqs), \
         (SELECT COUNT(*) FROM output_baskets)",
    )
    .fetch_one(wallet.storage().pool())
    .await
    .unwrap()
}

pub(crate) async fn balance(wallet: &WalletState) -> u64 {
    wallet.balance().await.expect("the balance").total
}

// ---------------------------------------------------------------------------
// The transactions
// ---------------------------------------------------------------------------

/// The P2PKH of `key`: `OP_DUP OP_HASH160 <20 bytes> OP_EQUALVERIFY OP_CHECKSIG`.
pub(crate) fn p2pkh(key: &PublicKey) -> Vec<u8> {
    [&[0x76, 0xa9, 0x14][..], &key.hash160(), &[0x88, 0xac]].concat()
}

pub(crate) fn other_lock() -> Vec<u8> {
    hex::decode(format!("76a914{OTHER_HASH}88ac")).unwrap()
}

/// The key `root` derives for a remittance from `sender`.
pub(crate) fn derived(
    root: &PrivateKey,
    sender: &PublicKey,
    prefix: &str,
    suffix: &str,
) -> PublicKey {
    KeyDeriver::new(Some(root.clone()))
        .derive_public_key(
            &Protocol::new(SecurityLevel::Counterparty, "3241645161d8"),
            &format!("{prefix} {suffix}"),
            &Counterparty::Other(sender.clone()),
            true,
        )
        .expect("the derived key")
}

pub(crate) fn input(txid: &str, vout: u32) -> TransactionInput {
    let mut input = TransactionInput::new(txid.to_string(), vout);
    input.unlocking_script = Some(UnlockingScript::from_hex("00").unwrap());
    input
}

/// The toolbox's `beef_paying`: one unproven output of 4500 satoshis locked
/// by `lock`, spending five outputs of one parent proven at
/// [`PARENT_HEIGHT`], each locked by a script `00` spends. Its `AtomicBEEF`,
/// its txid and the parent's.
pub(crate) fn beef_paying(lock: &[u8]) -> (Vec<u8>, String, String) {
    let (parent, child) = paying(lock, 5);
    let parent_txid = parent.id();
    let txid = child.id();
    let mut beef = Beef::new();
    let bump = beef.merge_bump(MerklePath::from_coinbase_txid(&parent_txid, PARENT_HEIGHT));
    beef.merge_raw_tx(parent.to_binary(), Some(bump)).unwrap();
    beef.merge_raw_tx(child.to_binary(), None).unwrap();
    (beef.to_binary_atomic(&txid).unwrap(), txid, parent_txid)
}

/// `beef_paying`'s parent of `inputs` outputs of 1,000 satoshis, each
/// locked by a script `00` spends, and its child spending every one to one
/// output of 4500 locked by `lock`.
pub(crate) fn paying(lock: &[u8], inputs: u32) -> (Transaction, Transaction) {
    let anyone = LockingScript::from_hex(&format!("14{}757551", "00".repeat(20))).unwrap();
    let mut parent = Transaction::new();
    parent.inputs_mut().push(input(&"11".repeat(32), 0));
    for _ in 0..inputs {
        parent.outputs_mut().push(TransactionOutput {
            satoshis: Some(1_000),
            locking_script: anyone.clone(),
            change: false,
        });
    }
    let parent_txid = parent.id();
    let mut child = Transaction::new();
    child.set_version(1);
    for vout in 0..inputs {
        child.inputs_mut().push(input(&parent_txid, vout));
    }
    child.outputs_mut().push(TransactionOutput {
        satoshis: Some(4_500),
        locking_script: LockingScript::from_binary(lock).unwrap(),
        change: false,
    });
    (parent, child)
}

// ---------------------------------------------------------------------------
// The rows
// ---------------------------------------------------------------------------

/// The stored row of `txid.vout`: basket, type, change, derivation prefix,
/// suffix and sender.
pub(crate) type Row = (
    Option<String>,
    String,
    i64,
    Option<String>,
    Option<String>,
    Option<String>,
);

pub(crate) async fn row(wallet: &WalletState, txid: &str, vout: u32) -> Row {
    sqlx::query_as(
        "SELECT b.name, o.type, o.change, o.derivation_prefix, o.derivation_suffix, \
         o.sender_identity_key FROM outputs o LEFT JOIN output_baskets b \
         ON b.basket_id = o.basket_id WHERE o.txid = ? AND o.vout = ?",
    )
    .bind(txid)
    .bind(vout)
    .fetch_one(wallet.storage().writer())
    .await
    .unwrap()
}

/// The row `txid.vout` as a basket insertion into "tokens" left it before
/// the toolbox's rules (the toolbox test's `reclassify`): `custom`, not
/// change, its custom instructions, derivation and sender NULL.
pub(crate) async fn reclassify(wallet: &WalletState, txid: &str, vout: u32) {
    let user_id: i64 =
        sqlx::query_scalar("SELECT user_id FROM outputs WHERE txid = ? AND vout = ?")
            .bind(txid)
            .bind(vout)
            .fetch_one(wallet.storage().writer())
            .await
            .unwrap();
    let tokens = sqlx::query(
        "INSERT INTO output_baskets (user_id, name, number_of_desired_utxos, \
         minimum_desired_utxo_value) VALUES (?, 'tokens', 6, 10000)",
    )
    .bind(user_id)
    .execute(wallet.storage().writer())
    .await
    .unwrap()
    .last_insert_rowid();
    sqlx::query(
        "UPDATE outputs SET basket_id = ?, type = 'custom', change = 0, \
         custom_instructions = NULL, derivation_prefix = NULL, derivation_suffix = NULL, \
         sender_identity_key = NULL WHERE txid = ? AND vout = ?",
    )
    .bind(tokens)
    .bind(txid)
    .bind(vout)
    .execute(wallet.storage().writer())
    .await
    .unwrap();
}

pub(crate) async fn spent(wallet: &WalletState, txid: &str) -> (bool, Option<i64>) {
    sqlx::query_as("SELECT spendable, spent_by FROM outputs WHERE txid = ? AND vout = 0")
        .bind(txid)
        .fetch_one(wallet.storage().writer())
        .await
        .unwrap()
}

pub(crate) async fn relinquish(app: &axum::Router, txid: &str, vout: u32) {
    let (status, body) = post_to(
        app,
        "/relinquishOutput",
        json!({ "basket": "default", "output": format!("{txid}.{vout}") }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["relinquished"], true, "{body}");
}

// ---------------------------------------------------------------------------
// The spend
// ---------------------------------------------------------------------------

/// A `createAction` of 1000 satoshis to another key with the
/// `underReceivedRule` label, signed and not sent (the toolbox test's
/// `spend_args`): the signed transaction, or the refusal's text.
pub(crate) async fn spend(wallet: &WalletState, labelled: bool) -> Result<Transaction, String> {
    let made = wallet
        .create_action(
            CreateActionArgs {
                description: SPEND.to_string(),
                input_beef: None,
                inputs: None,
                outputs: Some(vec![CreateActionOutput {
                    locking_script: other_lock(),
                    satoshis: 1_000,
                    output_description: "a payment".to_string(),
                    basket: None,
                    custom_instructions: None,
                    tags: None,
                }]),
                lock_time: None,
                version: None,
                labels: labelled.then(|| vec!["underReceivedRule".to_string()]),
                options: Some(CreateActionOptions {
                    no_send: Some(true),
                    sign_and_process: Some(true),
                    ..Default::default()
                }),
            },
            Some("test.local"),
        )
        .await
        .map_err(|e| e.to_string())?;
    let bytes = made.tx.expect("the signed transaction");
    Ok(Transaction::from_atomic_beef(&bytes)
        .or_else(|_| Transaction::from_binary(&bytes))
        .expect("the signed transaction"))
}

/// A payment of `satoshis` locked by `lock`, built by `wallet`'s own
/// `createAction` (`signAndProcess` false, `noSend`, the `underReceivedRule`
/// label so its received unproven coin is selected, the outputs in order)
/// and signed by its `signAction` (`noSend`): the stable line's
/// create-and-sign shape (main's `tests/served_internalize_scripts.rs`,
/// `noSend` on both as there). The signed `AtomicBEEF` and its txid.
pub(crate) async fn create_and_sign(
    wallet: &WalletState,
    lock: &[u8],
    satoshis: u64,
) -> (Vec<u8>, String) {
    let created = wallet
        .create_action(
            CreateActionArgs {
                description: "a payment to another wallet".to_string(),
                input_beef: None,
                inputs: None,
                outputs: Some(vec![CreateActionOutput {
                    locking_script: lock.to_vec(),
                    satoshis,
                    output_description: "the payment".to_string(),
                    basket: None,
                    custom_instructions: None,
                    tags: None,
                }]),
                lock_time: None,
                version: None,
                labels: Some(vec!["underReceivedRule".to_string()]),
                options: Some(CreateActionOptions {
                    sign_and_process: Some(false),
                    no_send: Some(true),
                    randomize_outputs: Some(false),
                    ..Default::default()
                }),
            },
            Some("test.local"),
        )
        .await
        .expect("the template");
    let signable = created
        .signable_transaction
        .expect("a signable transaction");
    let signed = wallet
        .sign_action(
            SignActionArgs {
                spends: std::collections::HashMap::new(),
                // The reference as `spend_gate::reference_string` writes it.
                reference: bsv_sdk::primitives::to_base64(&signable.reference),
                options: Some(SignActionOptions {
                    no_send: Some(true),
                    ..Default::default()
                }),
            },
            Some("test.local"),
        )
        .await
        .expect("signed");
    (
        signed.tx.expect("the signed AtomicBEEF"),
        hex::encode(signed.txid.expect("the txid")),
    )
}

/// The input of `tx` spending `txid.vout`, its unlocking script run against
/// the coin's stored P2PKH under the node's post-Chronicle word (the
/// toolbox test's `spends_the_coin`).
pub(crate) async fn spends_the_coin(wallet: &WalletState, tx: &Transaction, txid: &str, vout: u32) {
    let (script, satoshis): (Vec<u8>, i64) =
        sqlx::query_as("SELECT locking_script, satoshis FROM outputs WHERE txid = ? AND vout = ?")
            .bind(txid)
            .bind(vout)
            .fetch_one(wallet.storage().writer())
            .await
            .unwrap();
    assert_eq!(script.len(), 25, "a P2PKH lock");
    assert_eq!(tx.inputs().len(), 1, "the coin alone funds the spend");
    let spent_input = &tx.inputs()[0];
    assert_eq!(
        (
            spent_input.get_source_txid().unwrap(),
            spent_input.source_output_index
        ),
        (txid.to_string(), vout),
        "the spend selects the coin"
    );
    let raw = parse_transaction(&tx.to_binary()).unwrap();
    let cache = Arc::new(TxSighashCache::new(
        raw.version,
        raw.inputs,
        raw.outputs,
        raw.locktime,
    ));
    let mut check = Spend::with_transaction(TxSpendParams {
        transaction: cache,
        input_index: 0,
        source_satoshis: u64::try_from(satoshis).unwrap(),
        locking_script: LockingScript::from_binary(&script).unwrap(),
        unlocking_script: spent_input
            .unlocking_script
            .clone()
            .expect("an unlocking script"),
        memory_limit: None,
        work_limit: None,
    })
    .unwrap();
    check.set_flags(ScriptFlags::standard(ProtocolEra::PostChronicle));
    assert_eq!(check.validate().map_err(|e| e.message), Ok(true));
}

/// A served wallet funded by a received payment of 4500 satoshis, which
/// then makes an action of 1000 satoshis to another key, not sent: its
/// change (3,000-odd satoshis) is the wallet's own, the funding spent. The
/// wallet, the router, the action's `AtomicBEEF`, its txid and the change's
/// index; the action moved to `unproven`, as its broadcast would leave it.
pub(crate) async fn own_change(
    root: &PrivateKey,
) -> (
    WalletState,
    axum::Router,
    Arc<MockWalletServices>,
    Vec<u8>,
    String,
    u32,
) {
    let funding = Paid::to(root);
    let (wallet, app, posts) = funding.served().await;
    admitted(&app, &funding.tx, funding.payment()).await;
    assert_eq!(balance(&wallet).await, 4_500);
    let made = spend(&wallet, true).await.expect("the action");
    let txid = made.id();
    write(
        &wallet,
        "UPDATE transactions SET status = 'unproven' WHERE txid = ?",
        &txid,
    )
    .await;
    let change: i64 = sqlx::query_scalar("SELECT vout FROM outputs WHERE txid = ? AND change = 1")
        .bind(&txid)
        .fetch_one(wallet.storage().writer())
        .await
        .expect("the change output");
    let mut beef = Beef::from_binary(&funding.tx).expect("the funding's BEEF");
    beef.merge_raw_tx(made.to_binary(), None).unwrap();
    let atomic = beef.to_binary_atomic(&txid).unwrap();
    assert_ne!(funding.txid, txid);
    (
        wallet,
        app,
        posts,
        atomic,
        txid,
        u32::try_from(change).unwrap(),
    )
}

/// A payment to a fresh wallet of `root` from `sender` under `prefix` and
/// `suffix`, locked by the key the wallet derives for it: its `AtomicBEEF`
/// (the toolbox's `beef_paying`), its txid and its parent's.
pub(crate) struct Paid {
    pub(crate) root: PrivateKey,
    pub(crate) sender: PublicKey,
    pub(crate) prefix: &'static str,
    pub(crate) suffix: &'static str,
    pub(crate) lock: Vec<u8>,
    pub(crate) tx: Vec<u8>,
    pub(crate) txid: String,
    pub(crate) parent: String,
}

impl Paid {
    /// A random wallet paid by a random sender under [`PREFIX`] and [`SUFFIX`].
    pub(crate) fn new() -> Self {
        Self::to(&PrivateKey::random())
    }

    /// The wallet of `root` paid by a random sender.
    pub(crate) fn to(root: &PrivateKey) -> Self {
        Self::from(root, PrivateKey::random().public_key(), PREFIX, SUFFIX)
    }

    pub(crate) fn from(
        root: &PrivateKey,
        sender: PublicKey,
        prefix: &'static str,
        suffix: &'static str,
    ) -> Self {
        let lock = p2pkh(&derived(root, &sender, prefix, suffix));
        let (tx, txid, parent) = beef_paying(&lock);
        Self {
            root: root.clone(),
            sender,
            prefix,
            suffix,
            lock,
            tx,
            txid,
            parent,
        }
    }

    /// The served wallet of `root`, its tracker carrying the parent's root.
    pub(crate) async fn served(&self) -> (WalletState, axum::Router, Arc<MockWalletServices>) {
        served(&self.root, Some(&self.parent)).await
    }

    /// [`Paid::served`], the payment admitted as its sender makes it.
    pub(crate) async fn stored(&self) -> (WalletState, axum::Router, Arc<MockWalletServices>) {
        let (wallet, app, posts) = self.served().await;
        admitted(&app, &self.tx, self.payment()).await;
        (wallet, app, posts)
    }

    /// [`Paid::stored`], then the row reclassified as an earlier release's
    /// basket insertion left it: out of the balance.
    pub(crate) async fn reclassified(
        &self,
    ) -> (WalletState, axum::Router, Arc<MockWalletServices>) {
        let (wallet, app, posts) = self.stored().await;
        reclassify(&wallet, &self.txid, 0).await;
        assert_eq!(balance(&wallet).await, 0);
        (wallet, app, posts)
    }

    /// The wallet payment of output 0 naming `sender` as given.
    pub(crate) fn payment_naming(&self, sender: &str) -> Vec<Value> {
        vec![payment_of(0, self.prefix, self.suffix, sender)]
    }

    /// The wallet payment of output 0 as its sender makes it.
    pub(crate) fn payment(&self) -> Vec<Value> {
        self.payment_naming(&self.sender.to_hex())
    }
}

/// `POST /internalizeAction` of `tx` with `outputs`, answered 200.
pub(crate) async fn admitted(app: &axum::Router, tx: &[u8], outputs: Vec<Value>) {
    let (status, body) = internalize(app, tx, outputs).await;
    assert_eq!(status, StatusCode::OK, "{body}");
}

/// `POST /internalizeAction` of `tx` with `outputs`, refused 400
/// `VALIDATION_ERROR` with `message`.
pub(crate) async fn refused(app: &axum::Router, tx: &[u8], outputs: Vec<Value>, message: &str) {
    let (status, body) = internalize(app, tx, outputs).await;
    assert_refused(status, &body, "VALIDATION_ERROR", message);
}

/// The empty sender's refusal on this line.
pub(crate) const EMPTY_SENDER: &str =
    "Validation error: The senderIdentityKey parameter must be hexadecimal string.";

/// Runs `sql` with `txid` bound, on the wallet's writer.
pub(crate) async fn write(wallet: &WalletState, sql: &str, txid: &str) {
    sqlx::query(sql)
        .bind(txid)
        .execute(wallet.storage().writer())
        .await
        .unwrap();
}
