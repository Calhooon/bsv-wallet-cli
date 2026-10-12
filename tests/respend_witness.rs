//! L2-67: real SQLite, throwaway keys and the #66 mock chain/status sources.
//! No live wallet, external HTTP, or real coin.
use std::sync::Arc;

use bsv_sdk::primitives::PrivateKey;
use bsv_sdk::script::{templates::P2PKH, LockingScript, UnlockingScript};
use bsv_sdk::transaction::{
    Beef, MerklePath, MerklePathLeaf, MockChainTracker, Transaction, TransactionInput,
    TransactionOutput,
};
use bsv_sdk::wallet::{
    Counterparty, CreateActionArgs, CreateActionOptions, CreateActionOutput, KeyDeriver, Protocol,
    SecurityLevel, WalletInterface,
};
use bsv_wallet_cli::respend::{build_and_post, settle_proven};
use bsv_wallet_toolbox::services::mock::{
    success_post_beef_result, MockResponse, MockWalletServices,
};
use bsv_wallet_toolbox::{
    BroadcastStatus, GetMerklePathResult, GetRawTxResult, GetStatusForTxidsResult, MonitorStorage,
    StorageSqlx, TxStatusDetail, Wallet, WalletServices, WalletStorageProvider,
    WalletStorageWriter,
};
use chrono::Utc;

struct Fixture {
    wallet: Wallet<StorageSqlx, MockWalletServices>,
    root: PrivateKey,
    phantom: String,
    coin: i64,
    phantom_row: i64,
    change: i64,
}

fn status_source(txid: &str, status: &str, broadcaster: BroadcastStatus) -> MockWalletServices {
    MockWalletServices::builder()
        .post_beef_response(MockResponse::Success(vec![success_post_beef_result(
            "mock",
            &[],
        )]))
        .get_status_for_txids_response(MockResponse::Success(GetStatusForTxidsResult {
            name: "mock".into(),
            status: "success".into(),
            error: None,
            results: vec![TxStatusDetail {
                txid: txid.into(),
                status: status.into(),
                ..Default::default()
            }],
        }))
        .broadcaster_statuses(vec![("mock".into(), broadcaster)])
        .build()
}

async fn fixture() -> Fixture {
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
        .bind(user.user_id).bind(funding.id()).bind(funding.to_binary()).bind(now).bind(now).fetch_one(storage.pool()).await.unwrap();
    let coin: i64 = sqlx::query_scalar(
        "INSERT INTO outputs (user_id,transaction_id,basket_id,vout,satoshis,locking_script,txid,type,spendable,change,derivation_prefix,derivation_suffix,provided_by,purpose,output_description,created_at,updated_at) VALUES (?,?,?,0,100000,?,?,'P2PKH',1,1,?,?,'storage','change','fixture',?,?) RETURNING output_id")
        .bind(user.user_id).bind(funding_row).bind(basket).bind(lock.to_binary()).bind(funding.id()).bind(prefix).bind(suffix).bind(now).bind(now).fetch_one(storage.pool()).await.unwrap();
    let funding_proof = MerklePath {
        block_height: 800,
        path: vec![vec![MerklePathLeaf {
            offset: 0,
            hash: Some(funding.id()),
            txid: true,
            duplicate: false,
        }]],
    }
    .to_binary();
    let proven_id: i64 = sqlx::query_scalar("INSERT INTO proven_txs (txid,height,idx,block_hash,merkle_root,merkle_path,raw_tx,created_at,updated_at) VALUES (?,800,0,?,?,?,?,?,?) RETURNING proven_tx_id")
        .bind(funding.id()).bind("bb".repeat(32)).bind(funding.id()).bind(funding_proof).bind(funding.to_binary()).bind(now).bind(now).fetch_one(storage.pool()).await.unwrap();
    sqlx::query("UPDATE transactions SET proven_tx_id=? WHERE transaction_id=?")
        .bind(proven_id)
        .bind(funding_row)
        .execute(storage.pool())
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
            "bsv-wallet-cli",
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
    }
}

async fn words(f: &Fixture) -> (String, String) {
    sqlx::query_as("SELECT t.status,r.status FROM transactions t JOIN proven_tx_reqs r ON r.txid=t.txid WHERE t.txid=?")
        .bind(&f.phantom).fetch_one(f.wallet.storage().pool()).await.unwrap()
}
async fn coin_state(f: &Fixture, coin: i64) -> (bool, Option<i64>) {
    sqlx::query_as("SELECT spendable,spent_by FROM outputs WHERE output_id=?")
        .bind(coin)
        .fetch_one(f.wallet.storage().pool())
        .await
        .unwrap()
}
fn proof(txid: &str) -> Vec<u8> {
    MerklePath {
        block_height: 900,
        path: vec![vec![MerklePathLeaf {
            offset: 0,
            hash: Some(txid.into()),
            txid: true,
            duplicate: false,
        }]],
    }
    .to_binary()
}

#[tokio::test]
async fn accepted_phantom_exits_only_when_its_deliberate_competitor_proves() {
    let f = fixture().await;
    let s = f.wallet.storage();
    let before = words(&f).await;
    assert_eq!(before, ("unproven".into(), "unmined".into()));
    // This shared memory marker records an old index sighting. It is not
    // a broadcaster endpoint; current batch status below is unknown.
    s.record_broadcast_status(
        &f.phantom,
        bsv_wallet_toolbox::BROADCAST_PROVIDER_NETWORK,
        bsv_wallet_toolbox::BROADCAST_STATUS_SEEN,
    )
    .await
    .unwrap();
    let absent = status_source(&f.phantom, "unknown", BroadcastStatus::Unknown);
    let replacement = build_and_post(s, &absent, &f.root, &f.phantom, &["mock"])
        .await
        .expect("respend must build and post the host's competitor");
    assert_eq!(absent.call_count("post_beef"), 1);
    assert_eq!(
        words(&f).await,
        before,
        "no host act or hint writes the phantom's word"
    );
    assert_eq!(coin_state(&f, f.coin).await, (false, Some(f.phantom_row)));
    assert!(
        coin_state(&f, f.change).await.0,
        "today's age policy stays unchanged"
    );
    let raw: Vec<u8> = sqlx::query_scalar("SELECT raw_tx FROM transactions WHERE txid=?")
        .bind(&replacement)
        .fetch_one(s.pool())
        .await
        .unwrap();
    let t = Transaction::from_binary(&raw).unwrap();
    let mut input_beef = Beef::from_binary(
        &sqlx::query_scalar::<_, Vec<u8>>("SELECT input_beef FROM proven_tx_reqs WHERE txid=?")
            .bind(&replacement)
            .fetch_one(s.pool())
            .await
            .unwrap(),
    )
    .unwrap();
    input_beef.merge_raw_tx(raw.clone(), None);
    let verified = Transaction::from_beef(&input_beef.to_binary(), Some(&replacement)).unwrap();
    assert!(
        verified
            .verify(absent.get_chain_tracker().await.unwrap(), None)
            .await
            .unwrap(),
        "replacement signatures must execute with the proven source"
    );
    let original_raw: Vec<u8> = sqlx::query_scalar("SELECT raw_tx FROM transactions WHERE txid=?")
        .bind(&f.phantom)
        .fetch_one(s.pool())
        .await
        .unwrap();
    let original = Transaction::from_binary(&original_raw).unwrap();
    assert_eq!(t.inputs.len(), original.inputs.len());
    for (a, b) in t.inputs.iter().zip(&original.inputs) {
        assert_eq!(a.get_source_txid().unwrap(), b.get_source_txid().unwrap());
        assert_eq!(a.source_output_index, b.source_output_index);
    }
    assert_eq!(t.outputs.len(), 1);
    assert!(t.outputs[0].satoshis.unwrap() < 100_000);
    let proof_services = Arc::new(
        MockWalletServices::builder()
            .get_raw_tx_response(MockResponse::Success(GetRawTxResult {
                name: "mock".into(),
                txid: replacement.clone(),
                raw_tx: Some(raw),
                error: None,
                could_not_look: false,
            }))
            .get_merkle_path_response(MockResponse::Success(GetMerklePathResult {
                name: Some("mock".into()),
                merkle_path: Some(hex::encode(proof(&replacement))),
                header: None,
                error: None,
                notes: vec![],
            }))
            .is_utxo_response(MockResponse::Success(true))
            .build(),
    );
    WalletStorageProvider::set_services(s, proof_services.clone() as Arc<dyn WalletServices>);
    s.set_max_acceptable_proof_height(1000).await.unwrap();
    // Wrong active root: the same bytes and proof are no evidence yet.
    s.set_chain_tracker(Arc::new(MockChainTracker::new(1000)))
        .await;
    s.synchronize_transaction_statuses().await.unwrap();
    settle_proven(s, proof_services.as_ref()).await.unwrap();
    assert_eq!(words(&f).await, before);
    assert_eq!(coin_state(&f, f.coin).await, (false, Some(f.phantom_row)));
    let mut tracker = MockChainTracker::new(1000);
    tracker.add_root(900, replacement.clone());
    s.set_chain_tracker(Arc::new(tracker)).await;
    s.synchronize_transaction_statuses().await.unwrap();
    // The competitor path is the sole writer of this word.
    assert_eq!(words(&f).await, ("failed".into(), "doubleSpend".into()));
    // A stale positive unspent hint from the rollback helper cannot let a
    // subsequent CLI spend select these deliberately reserved inputs.
    bsv_wallet_cli::spend_guard::run(s.pool()).await.unwrap();
    assert_eq!(coin_state(&f, f.coin).await, (false, Some(f.phantom_row)));
    // Deliver the replacement's own proof through the existing ingest funnel.
    s.ingest_merkle_proof(
        &replacement,
        &proof(&replacement),
        900,
        &"aa".repeat(32),
        None,
    )
    .await
    .unwrap();
    bsv_wallet_cli::broadcast_reconcile::run_sweep(s, proof_services.as_ref(), true)
        .await
        .unwrap();
    let replacement_row =
        sqlx::query_scalar("SELECT transaction_id FROM transactions WHERE txid=?")
            .bind(&replacement)
            .fetch_one(s.pool())
            .await
            .unwrap();
    assert_eq!(coin_state(&f, f.coin).await, (false, Some(replacement_row)));
    assert!(
        !coin_state(&f, f.change).await.0,
        "phantom change cannot be selected"
    );
    let replacement_change: i64 =
        sqlx::query_scalar("SELECT output_id FROM outputs WHERE transaction_id=? AND change=1")
            .bind(replacement_row)
            .fetch_one(s.pool())
            .await
            .unwrap();
    assert!(coin_state(&f, replacement_change).await.0);
    // Exercise the toolbox's real selector and signer on the recovered change.
    let spend = f
        .wallet
        .create_action(
            CreateActionArgs {
                description: "spend the proven replacement change".into(),
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
            "bsv-wallet-cli",
        )
        .await
        .unwrap();
    let spent = Transaction::from_binary(&spend.tx.unwrap()).unwrap();
    assert!(spent
        .inputs
        .iter()
        .any(|i| i.get_source_txid().unwrap() == replacement));
    assert!(spent
        .inputs
        .iter()
        .all(|i| i.get_source_txid().unwrap() != f.phantom));
}

#[tokio::test]
async fn respend_refuses_while_a_status_source_holds_the_phantom() {
    let f = fixture().await;
    let held = status_source(&f.phantom, "known", BroadcastStatus::Seen);
    let before = words(&f).await;
    let err = build_and_post(f.wallet.storage(), &held, &f.root, &f.phantom, &["mock"])
        .await
        .unwrap_err();
    assert!(err.to_string().contains("holds"), "{err}");
    assert_eq!(held.call_count("post_beef"), 0);
    assert_eq!(words(&f).await, before);
    assert_eq!(coin_state(&f, f.coin).await, (false, Some(f.phantom_row)));
}

#[tokio::test]
async fn respend_refuses_unavailable_status_sources_without_posting() {
    let f = fixture().await;
    let unavailable = MockWalletServices::new();
    let before = words(&f).await;
    let err = build_and_post(
        f.wallet.storage(),
        &unavailable,
        &f.root,
        &f.phantom,
        &["mock"],
    )
    .await
    .unwrap_err();
    assert!(err.to_string().contains("unavailable"), "{err}");
    assert_eq!(unavailable.call_count("post_beef"), 0);
    assert_eq!(words(&f).await, before);
    assert_eq!(coin_state(&f, f.coin).await, (false, Some(f.phantom_row)));
}

#[tokio::test]
async fn respend_refuses_an_input_outside_the_wallet() {
    let f = fixture().await;
    sqlx::query("DELETE FROM outputs WHERE output_id=?")
        .bind(f.coin)
        .execute(f.wallet.storage().pool())
        .await
        .unwrap();
    let absent = status_source(&f.phantom, "unknown", BroadcastStatus::Unknown);
    let before = words(&f).await;
    let err = build_and_post(f.wallet.storage(), &absent, &f.root, &f.phantom, &["mock"])
        .await
        .unwrap_err();
    assert!(err.to_string().contains("not the wallet's"), "{err}");
    assert_eq!(absent.call_count("post_beef"), 0);
    assert_eq!(words(&f).await, before);
}

#[tokio::test]
async fn respend_refuses_a_phantom_with_competitor_proof_already_recorded() {
    let f = fixture().await;
    // Initial state for this control, not a host status mutation under test.
    sqlx::query("UPDATE proven_tx_reqs SET status='doubleSpend' WHERE txid=?")
        .bind(&f.phantom)
        .execute(f.wallet.storage().pool())
        .await
        .unwrap();
    let absent = status_source(&f.phantom, "unknown", BroadcastStatus::Unknown);
    let err = build_and_post(f.wallet.storage(), &absent, &f.root, &f.phantom, &["mock"])
        .await
        .unwrap_err();
    assert!(err.to_string().contains("already has proof"), "{err}");
    assert_eq!(absent.call_count("post_beef"), 0);
    assert_eq!(coin_state(&f, f.coin).await, (false, Some(f.phantom_row)));
}

#[tokio::test]
async fn a_refused_replacement_is_a_retryable_hint_and_cannot_be_built_twice() {
    let f = fixture().await;
    let s = f.wallet.storage();
    let absent = status_source(&f.phantom, "unknown", BroadcastStatus::Unknown);
    // The post is a service error: no accepting word. Its request must still
    // be on the toolbox's retry path, with the phantom untouched.
    let refusing = MockWalletServices::builder()
        .post_beef_response(MockResponse::Error(
            bsv_wallet_toolbox::services::mock::MockErrorKind::NetworkError,
            "mock unavailable".into(),
        ))
        .get_status_for_txids_response(MockResponse::Success(GetStatusForTxidsResult {
            name: "mock".into(),
            status: "success".into(),
            error: None,
            results: vec![TxStatusDetail {
                txid: f.phantom.clone(),
                status: "unknown".into(),
                ..Default::default()
            }],
        }))
        .broadcaster_statuses(vec![("mock".into(), BroadcastStatus::Unknown)])
        .build();
    let before = words(&f).await;
    let replacement = build_and_post(s, &refusing, &f.root, &f.phantom, &["mock"])
        .await
        .unwrap();
    assert_eq!(words(&f).await, before);
    assert_eq!(coin_state(&f, f.coin).await, (false, Some(f.phantom_row)));
    let request: String = sqlx::query_scalar("SELECT status FROM proven_tx_reqs WHERE txid=?")
        .bind(&replacement)
        .fetch_one(s.pool())
        .await
        .unwrap();
    assert_eq!(request, "unsent");
    let err = build_and_post(s, &absent, &f.root, &f.phantom, &["mock"])
        .await
        .unwrap_err();
    assert!(err.to_string().contains("already tracked"), "{err}");
    assert_eq!(absent.call_count("post_beef"), 0);
    let accepting = Arc::new(status_source(
        &replacement,
        "unknown",
        BroadcastStatus::Unknown,
    ));
    WalletStorageProvider::set_services(s, accepting.clone() as Arc<dyn WalletServices>);
    sqlx::query("UPDATE proven_tx_reqs SET updated_at='2026-01-01T00:00:00+00:00',created_at='2026-01-01T00:00:00+00:00' WHERE txid=?").bind(&replacement).execute(s.pool()).await.unwrap();
    s.send_waiting_transactions(std::time::Duration::ZERO)
        .await
        .unwrap();
    assert!(
        accepting.call_count("post_beef") > 0,
        "cadence retries the replacement"
    );
    assert_eq!(words(&f).await, before);
}

#[tokio::test]
async fn respend_refuses_if_one_configured_source_cannot_answer() {
    let f = fixture().await;
    let absent = status_source(&f.phantom, "unknown", BroadcastStatus::Unknown);
    let before = words(&f).await;
    let err = build_and_post(
        f.wallet.storage(),
        &absent,
        &f.root,
        &f.phantom,
        &["mock", "unavailable-failover"],
    )
    .await
    .unwrap_err();
    assert!(err.to_string().contains("unavailable"), "{err}");
    assert_eq!(absent.call_count("post_beef"), 0);
    assert_eq!(words(&f).await, before);
}

#[tokio::test]
async fn served_pass_recovers_a_durable_build_interrupted_before_processing() {
    let f = fixture().await;
    let s = f.wallet.storage();
    let absent = status_source(&f.phantom, "unknown", BroadcastStatus::Unknown);
    let before = words(&f).await;
    // Inject a crash boundary between the committed host build and toolbox
    // processing; no post has happened, but the exact signed act is durable.
    sqlx::query("CREATE TRIGGER interrupt_respend BEFORE INSERT ON proven_tx_reqs BEGIN SELECT RAISE(FAIL,'mock interruption before processing'); END")
        .execute(s.pool()).await.unwrap();
    let err = build_and_post(s, &absent, &f.root, &f.phantom, &["mock"])
        .await
        .unwrap_err();
    assert!(err.to_string().contains("mock interruption"), "{err}");
    assert_eq!(absent.call_count("post_beef"), 0);
    assert_eq!(words(&f).await, before);
    assert_eq!(coin_state(&f, f.coin).await, (false, Some(f.phantom_row)));
    let replacement: String = sqlx::query_scalar("SELECT t.txid FROM respend_links l JOIN transactions t ON t.transaction_id=l.replacement_id WHERE l.phantom_id=?")
        .bind(f.phantom_row).fetch_one(s.pool()).await.unwrap();
    sqlx::query("DROP TRIGGER interrupt_respend")
        .execute(s.pool())
        .await
        .unwrap();
    bsv_wallet_cli::broadcast_reconcile::run_sweep(s, &absent, true)
        .await
        .unwrap();
    let request: String = sqlx::query_scalar("SELECT status FROM proven_tx_reqs WHERE txid=?")
        .bind(&replacement)
        .fetch_one(s.pool())
        .await
        .unwrap();
    assert_eq!(request, "unsent");
    assert_eq!(words(&f).await, before);
    assert_eq!(coin_state(&f, f.coin).await, (false, Some(f.phantom_row)));
    let err = build_and_post(s, &absent, &f.root, &f.phantom, &["mock"])
        .await
        .unwrap_err();
    assert!(err.to_string().contains("already tracked"), "{err}");
}
