//! The world the witnesses run in: a wallet on real SQLite with a phantom
//! spend of a mined coin, the mock status sources, and the marker capture.

use std::sync::Arc;

use bsv_sdk::primitives::PrivateKey;
use bsv_sdk::script::{templates::P2PKH, LockingScript, UnlockingScript};
use bsv_sdk::transaction::{
    MerklePath, MerklePathLeaf, Transaction, TransactionInput, TransactionOutput,
};
use bsv_sdk::wallet::{
    Counterparty, CreateActionArgs, CreateActionOptions, CreateActionOutput, KeyDeriver, Protocol,
    SecurityLevel, WalletInterface,
};
use bsv_wallet_toolbox::services::mock::{
    success_post_beef_result, MockResponse, MockWalletServices,
};
use bsv_wallet_toolbox::{
    BroadcastStatus, GetStatusForTxidsResult, StorageSqlx, TxStatusDetail, Wallet,
    WalletStorageWriter,
};
use chrono::Utc;

pub(crate) struct Fixture {
    pub(crate) wallet: Wallet<StorageSqlx, MockWalletServices>,
    pub(crate) root: PrivateKey,
    pub(crate) phantom: String,
    pub(crate) coin: i64,
    pub(crate) phantom_row: i64,
    pub(crate) change: i64,
    pub(crate) funding: String,
}

/// The header service's view of the funding block: the header at 870,800
/// carries the funding proof's root, so the spend gate's read of the
/// respend's inputs (0.11.0, #29) finds the coin mined.
pub(crate) fn headed(services: &MockWalletServices, funding: &str) {
    services.set_header_for_height(bsv_wallet_toolbox::services::BlockHeader {
        version: 1,
        previous_hash: "00".repeat(32),
        merkle_root: funding.to_string(),
        time: 1_700_000_000,
        bits: 0x1d00_ffff,
        nonce: 0,
        hash: String::new(),
        height: 870_800,
    });
}

pub(crate) fn status_source(
    txid: &str,
    status: &str,
    broadcaster: BroadcastStatus,
) -> MockWalletServices {
    status_answer(
        GetStatusForTxidsResult {
            name: "mock".into(),
            status: "success".into(),
            error: None,
            results: vec![TxStatusDetail {
                txid: txid.into(),
                status: status.into(),
                ..Default::default()
            }],
        },
        broadcaster,
    )
}

/// The status sources answering `result` for the phantom, Arcade's own
/// word `broadcaster`.
pub(crate) fn status_answer(
    result: GetStatusForTxidsResult,
    broadcaster: BroadcastStatus,
) -> MockWalletServices {
    MockWalletServices::builder()
        .post_beef_response(MockResponse::Success(vec![success_post_beef_result(
            "mock",
            &[],
        )]))
        .get_status_for_txids_response(MockResponse::Success(result))
        .broadcaster_statuses(vec![("mock".into(), broadcaster)])
        .build()
}

pub(crate) async fn fixture() -> Fixture {
    let root = PrivateKey::random();
    let identity = root.public_key().to_hex();
    let storage = StorageSqlx::in_memory().await.unwrap();
    storage.migrate("respend-witness", &identity).await.unwrap();
    storage.make_available().await.unwrap();
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
            satoshis: Some(100_000),
            locking_script: lock.clone(),
            change: false,
        }],
        0,
    );
    let now = Utc::now();
    let funding_row: i64 = sqlx::query_scalar(
        "INSERT INTO transactions (user_id,status,reference,is_outgoing,satoshis,version,lock_time,description,txid,raw_tx,created_at,updated_at) VALUES (?,'completed','funding',0,100000,1,0,'fixture',?,?,?,?) RETURNING transaction_id")
        .bind(user.user_id).bind(funding.id()).bind(funding.to_binary()).bind(now).bind(now).fetch_one(storage.writer()).await.unwrap();
    let coin: i64 = sqlx::query_scalar(
        "INSERT INTO outputs (user_id,transaction_id,basket_id,vout,satoshis,locking_script,txid,type,spendable,change,derivation_prefix,derivation_suffix,provided_by,purpose,output_description,created_at,updated_at) VALUES (?,?,?,0,100000,?,?,'P2PKH',1,1,?,?,'storage','change','fixture',?,?) RETURNING output_id")
        .bind(user.user_id).bind(funding_row).bind(basket).bind(lock.to_binary()).bind(funding.id()).bind(prefix).bind(suffix).bind(now).bind(now).fetch_one(storage.writer()).await.unwrap();
    let funding_proof = MerklePath {
        block_height: 870_800,
        path: vec![vec![MerklePathLeaf {
            offset: 0,
            hash: Some(funding.id()),
            txid: true,
            duplicate: false,
        }]],
    }
    .to_binary();
    let proven_id: i64 = sqlx::query_scalar("INSERT INTO proven_txs (txid,height,idx,block_hash,merkle_root,merkle_path,raw_tx,created_at,updated_at) VALUES (?,870800,0,?,?,?,?,?,?) RETURNING proven_tx_id")
        .bind(funding.id()).bind("bb".repeat(32)).bind(funding.id()).bind(funding_proof).bind(funding.to_binary()).bind(now).bind(now).fetch_one(storage.writer()).await.unwrap();
    sqlx::query("UPDATE transactions SET proven_tx_id=? WHERE transaction_id=?")
        .bind(proven_id)
        .bind(funding_row)
        .execute(storage.writer())
        .await
        .unwrap();
    let services = status_source("", "unknown", BroadcastStatus::Unknown);
    let wallet = Wallet::new(Some(root.clone()), storage, services)
        .await
        .unwrap();
    let result = wallet
        .create_action(
            CreateActionArgs {
                description: "accepted phantom never mined by mock chain".into(),
                input_beef: None,
                inputs: None,
                outputs: Some(vec![CreateActionOutput {
                    locking_script: LockingScript::from_hex("51").unwrap().to_binary(),
                    satoshis: 1000,
                    output_description: "payment".into(),
                    basket: None,
                    custom_instructions: None,
                    tags: None,
                }]),
                lock_time: None,
                version: None,
                labels: None,
                options: Some(CreateActionOptions {
                    accept_delayed_broadcast: Some(false),
                    ..Default::default()
                }),
            },
            Some("bsv-wallet-cli"),
        )
        .await
        .unwrap();
    let phantom = hex::encode(result.txid.unwrap());
    let phantom_row = sqlx::query_scalar("SELECT transaction_id FROM transactions WHERE txid=?")
        .bind(&phantom)
        .fetch_one(wallet.storage().pool())
        .await
        .unwrap();
    let change = sqlx::query_scalar(
        "SELECT output_id FROM outputs WHERE transaction_id=? AND change=1 LIMIT 1",
    )
    .bind(phantom_row)
    .fetch_one(wallet.storage().pool())
    .await
    .unwrap();
    Fixture {
        wallet,
        root,
        phantom,
        coin,
        phantom_row,
        change,
        funding: funding.id(),
    }
}

pub(crate) async fn words(f: &Fixture) -> (String, String) {
    sqlx::query_as("SELECT t.status,r.status FROM transactions t JOIN proven_tx_reqs r ON r.txid=t.txid WHERE t.txid=?")
        .bind(&f.phantom).fetch_one(f.wallet.storage().pool()).await.unwrap()
}
pub(crate) async fn coin_state(f: &Fixture, coin: i64) -> (bool, Option<i64>) {
    sqlx::query_as("SELECT spendable,spent_by FROM outputs WHERE output_id=?")
        .bind(coin)
        .fetch_one(f.wallet.storage().pool())
        .await
        .unwrap()
}
pub(crate) fn proof(txid: &str) -> Vec<u8> {
    MerklePath {
        block_height: 870_900,
        path: vec![vec![MerklePathLeaf {
            offset: 0,
            hash: Some(txid.into()),
            txid: true,
            duplicate: false,
        }]],
    }
    .to_binary()
}

/// Every marked line: (marker, txid).
pub(crate) type MarkedLines = Arc<std::sync::Mutex<Vec<(String, Option<String>)>>>;

/// The marker of every line logged while the guard lives on this thread.
pub(crate) fn capture_markers() -> (MarkedLines, tracing::subscriber::DefaultGuard) {
    use tracing_subscriber::layer::SubscriberExt;
    struct Capture(MarkedLines);
    #[derive(Default)]
    struct Line(String, Option<String>);
    impl tracing::field::Visit for Line {
        fn record_str(&mut self, field: &tracing::field::Field, value: &str) {
            self.record_debug(field, &value);
        }
        fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
            let value = format!("{value:?}").trim_matches('"').to_string();
            match field.name() {
                "marker" => self.0 = value,
                "txid" => self.1 = Some(value),
                _ => {}
            }
        }
    }
    impl<S: tracing::Subscriber> tracing_subscriber::Layer<S> for Capture {
        fn on_event(
            &self,
            event: &tracing::Event<'_>,
            _: tracing_subscriber::layer::Context<'_, S>,
        ) {
            let mut line = Line::default();
            event.record(&mut line);
            if !line.0.is_empty() {
                self.0.lock().unwrap().push((line.0, line.1));
            }
        }
    }
    let lines = Arc::new(std::sync::Mutex::new(Vec::new()));
    let subscriber = tracing_subscriber::registry().with(Capture(lines.clone()));
    (lines, tracing::subscriber::set_default(subscriber))
}
