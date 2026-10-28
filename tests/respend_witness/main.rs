//! L2-67: real SQLite, throwaway keys and the #66 mock chain/status sources.
//! No live wallet, external HTTP, or real coin.

mod proof_presence;
mod world;

#[path = "../../src/test_support/chain_tracker.rs"]
mod chain_tracker;

use std::sync::Arc;

use bsv_sdk::primitives::PrivateKey;
use bsv_sdk::script::LockingScript;
use bsv_sdk::transaction::{Beef, Transaction};
use bsv_sdk::wallet::{CreateActionArgs, CreateActionOptions, CreateActionOutput, WalletInterface};
use bsv_wallet_cli::respend::settle_proven;
use bsv_wallet_cli::spend_gate::GateOptions;
use bsv_wallet_toolbox::services::mock::{MockResponse, MockWalletServices};
use bsv_wallet_toolbox::{
    BroadcastStatus, GetMerklePathResult, GetRawTxResult, GetStatusForTxidsResult, MonitorStorage,
    StorageSqlx, TxStatusDetail, WalletServices, WalletStorageProvider, WalletStorageWriter,
};
use chain_tracker::TestChainTracker;

use world::{
    capture_markers, coin_state, fixture, headed, proof, status_answer, status_source, words,
};

/// The sweep's absence threshold: `BROADCAST_ABSENCE_MINUTES`' default.
const ABSENCE: i64 = bsv_wallet_cli::broadcast_reconcile::DEFAULT_ABSENCE_MINUTES;

/// The respend under the gate's default settings.
async fn build_and_post(
    storage: &StorageSqlx,
    services: &dyn WalletServices,
    root: &PrivateKey,
    phantom: &str,
    sources: &[&str],
) -> anyhow::Result<String> {
    let gate = GateOptions::default();
    bsv_wallet_cli::respend::build_and_post(storage, services, root, phantom, sources, &gate).await
}

/// Add an unproven ancestor between the next phantom and the proven coin.
async fn descendant_phantom(f: &world::Fixture) -> String {
    let result = f
        .wallet
        .create_action(
            CreateActionArgs {
                description: "phantom with unproven ancestry".into(),
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
                options: Some(CreateActionOptions {
                    accept_delayed_broadcast: Some(false),
                    ..Default::default()
                }),
                lock_time: None,
                version: None,
                labels: None,
            },
            Some("bsv-wallet-cli"),
        )
        .await
        .unwrap();
    hex::encode(result.txid.unwrap())
}

async fn store_child_first_beef(storage: &StorageSqlx, txid: &str) {
    let bytes: Vec<u8> = sqlx::query_scalar("SELECT input_beef FROM proven_tx_reqs WHERE txid=?")
        .bind(txid)
        .fetch_one(storage.pool())
        .await
        .unwrap();
    let mut beef = Beef::from_binary(&bytes).unwrap();
    assert!(
        beef.txs.len() >= 2,
        "the input must have an unproven ancestor"
    );
    beef.sort_txs();
    // Like the 0.8.3 writer, the writer sorts only when needs_sort is set.
    // Reordering the public vector after sorting keeps that flag clear.
    beef.txs.reverse();
    let child_first = beef.to_binary();
    // Bsv-rs #240 accepts valid BEEF regardless of transaction order.
    assert!(Beef::from_binary(&child_first)
        .unwrap()
        .verify_structure()
        .is_ok());
    for table in ["transactions", "proven_tx_reqs"] {
        sqlx::query(&format!("UPDATE {table} SET input_beef=? WHERE txid=?"))
            .bind(&child_first)
            .bind(txid)
            .execute(storage.writer())
            .await
            .unwrap();
    }
}

#[tokio::test]
async fn respend_accepts_child_first_stored_input_beef() {
    let f = fixture().await;
    let phantom = descendant_phantom(&f).await;
    let storage = f.wallet.storage();
    store_child_first_beef(storage, &phantom).await;
    let before: (String, String) = sqlx::query_as("SELECT t.status,r.status FROM transactions t JOIN proven_tx_reqs r ON r.txid=t.txid WHERE t.txid=?")
        .bind(&phantom).fetch_one(storage.pool()).await.unwrap();
    let reserved = coin_state(&f, f.change).await;
    let absent = status_source(&phantom, "unknown", BroadcastStatus::Unknown);
    headed(&absent, &f.funding);
    let replacement = build_and_post(storage, &absent, &f.root, &phantom, &["mock"])
        .await
        .expect("stored child-first ancestry remains usable at the beta.2 pin");
    assert_eq!(absent.call_count("post_beef"), 1);
    let after: (String, String) = sqlx::query_as("SELECT t.status,r.status FROM transactions t JOIN proven_tx_reqs r ON r.txid=t.txid WHERE t.txid=?")
        .bind(&phantom).fetch_one(storage.pool()).await.unwrap();
    assert_eq!(after, before);
    assert_eq!(coin_state(&f, f.change).await, reserved);
    let bytes: Vec<u8> = sqlx::query_scalar("SELECT input_beef FROM proven_tx_reqs WHERE txid=?")
        .bind(&replacement)
        .fetch_one(storage.pool())
        .await
        .unwrap();
    assert!(Beef::from_binary(&bytes)
        .unwrap()
        .verify_structure()
        .is_ok());
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
    headed(&absent, &f.funding);
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
    // Parity-mode replay of signatures and ancestry at synthetic funding height 800.
    // A node-word check needs spend-era flags plus coin-era UTXO bits per input;
    // see docs/ARCHITECTURE.md for the combined-word rule.
    assert!(
        verified
            .verify(absent.get_chain_tracker().await.unwrap(), None, None)
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
    s.set_chain_tracker(Arc::new(TestChainTracker::new(1000)))
        .await;
    s.synchronize_transaction_statuses().await.unwrap();
    settle_proven(s, proof_services.as_ref()).await.unwrap();
    assert_eq!(words(&f).await, before);
    assert_eq!(coin_state(&f, f.coin).await, (false, Some(f.phantom_row)));
    let mut tracker = TestChainTracker::new(1000);
    tracker.add_root(900, replacement.clone());
    s.set_chain_tracker(Arc::new(tracker)).await;
    s.synchronize_transaction_statuses().await.unwrap();
    // The competitor path is the sole writer of this word.
    assert_eq!(words(&f).await, ("failed".into(), "doubleSpend".into()));
    // A stale positive unspent hint from the rollback helper cannot let a
    // subsequent CLI spend select these deliberately reserved inputs.
    bsv_wallet_cli::spend_guard::run(s).await.unwrap();
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
    bsv_wallet_cli::broadcast_reconcile::run_sweep(s, proof_services.as_ref(), true, ABSENCE)
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
            Some("bsv-wallet-cli"),
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

/// The respend under toolbox 0.12.0 (0.11.1: Arcade's `known` for its
/// SEEN words only; two chain indexes' `unknown` stand even against a
/// broadcaster's `known`, so the status ladder answers `unknown` for a
/// phantom Arcade still calls SEEN). The ladder's `unknown` is not
/// enough: while Arcade's own word is a SEEN word, or one of its holding
/// words (`RECEIVED`, `ACCEPTED_BY_NETWORK`: `known` no longer in the
/// ladder, `Accepted` in Arcade's own word), the respend refuses before
/// anything is built or posted, and nothing is released; once Arcade
/// stops (a 404, `Unknown`) it permits, and still releases nothing.
#[tokio::test]
async fn a_phantom_permits_only_once_arcade_stops_seen() {
    let f = fixture().await;
    let s = f.wallet.storage();
    let before = words(&f).await;
    for word in [
        "SEEN_MULTIPLE_NODES",
        "SEEN_ON_NETWORK",
        "RECEIVED",
        "ACCEPTED_BY_NETWORK",
    ] {
        let held = status_source(
            &f.phantom,
            "unknown",
            BroadcastStatus::from_arcade_status(word),
        );
        headed(&held, &f.funding);
        let err = build_and_post(s, &held, &f.root, &f.phantom, &["mock"])
            .await
            .unwrap_err();
        assert!(err.to_string().contains("holds"), "{word}: {err}");
        assert_eq!(held.call_count("post_beef"), 0, "{word}");
        assert_eq!(words(&f).await, before, "{word}");
        assert_eq!(
            coin_state(&f, f.coin).await,
            (false, Some(f.phantom_row)),
            "{word}"
        );
    }
    let stopped = status_source(&f.phantom, "unknown", BroadcastStatus::Unknown);
    headed(&stopped, &f.funding);
    build_and_post(s, &stopped, &f.root, &f.phantom, &["mock"])
        .await
        .expect("Arcade stopped: the respend permits");
    assert_eq!(stopped.call_count("post_beef"), 1);
    assert_eq!(words(&f).await, before);
    assert_eq!(coin_state(&f, f.coin).await, (false, Some(f.phantom_row)));
}

/// The respend under toolbox 0.12.0: a single gap, one chain index that
/// could not look while the other answered `unknown`, is no absence. The
/// ladder leaves the phantom out and says "could not look" (`status:
/// "error"` when it was the only txid asked; `success` with the error when
/// others were), and the respend refuses on either shape: nothing built,
/// nothing posted, nothing released.
#[tokio::test]
async fn a_single_gap_at_the_status_ladder_refuses_and_releases_nothing() {
    let f = fixture().await;
    let s = f.wallet.storage();
    let before = words(&f).await;
    for status in ["error", "success"] {
        let gap = status_answer(
            GetStatusForTxidsResult {
                name: "Services".into(),
                status: status.into(),
                error: Some(format!(
                    "could not look: no chain index answered {}",
                    f.phantom
                )),
                results: vec![],
            },
            BroadcastStatus::Unknown,
        );
        headed(&gap, &f.funding);
        let err = build_and_post(s, &gap, &f.root, &f.phantom, &["mock"])
            .await
            .unwrap_err();
        assert!(err.to_string().contains("unavailable"), "{status}: {err}");
        assert_eq!(gap.call_count("post_beef"), 0, "{status}");
        assert_eq!(words(&f).await, before, "{status}");
        assert_eq!(
            coin_state(&f, f.coin).await,
            (false, Some(f.phantom_row)),
            "{status}"
        );
    }
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

/// #166: a source the wallet does not hold is refused at the gate, the
/// word a named input's is, before a creator is read; nothing posted or
/// written.
#[tokio::test]
async fn respend_refuses_an_input_outside_the_wallet() {
    let f = fixture().await;
    sqlx::query("DELETE FROM outputs WHERE output_id=?")
        .bind(f.coin)
        .execute(f.wallet.storage().writer())
        .await
        .unwrap();
    let absent = status_source(&f.phantom, "unknown", BroadcastStatus::Unknown);
    headed(&absent, &f.funding);
    let before = words(&f).await;
    let err = build_and_post(f.wallet.storage(), &absent, &f.root, &f.phantom, &["mock"])
        .await
        .unwrap_err();
    assert_eq!(
        err.to_string(),
        format!(
            "respend refused: Invalid argument: inputs names {}.0, which is not a coin this \
             wallet could spend: {}",
            f.funding,
            bsv_wallet_cli::spend_gate::NOT_HELD
        )
    );
    assert_eq!(absent.call_count("post_beef"), 0);
    assert_eq!(words(&f).await, before);
}

#[tokio::test]
async fn respend_refuses_a_phantom_with_competitor_proof_already_recorded() {
    let f = fixture().await;
    // Initial state for this control, not a host status mutation under test.
    sqlx::query("UPDATE proven_tx_reqs SET status='doubleSpend' WHERE txid=?")
        .bind(&f.phantom)
        .execute(f.wallet.storage().writer())
        .await
        .unwrap();
    let absent = status_source(&f.phantom, "unknown", BroadcastStatus::Unknown);
    headed(&absent, &f.funding);
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
    headed(&absent, &f.funding);
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
    headed(&refusing, &f.funding);
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
    sqlx::query("UPDATE proven_tx_reqs SET updated_at='2026-01-01T00:00:00+00:00',created_at='2026-01-01T00:00:00+00:00' WHERE txid=?").bind(&replacement).execute(s.writer()).await.unwrap();
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
    headed(&absent, &f.funding);
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
    headed(&absent, &f.funding);
    let before = words(&f).await;
    // Inject a crash boundary between the committed host build and toolbox
    // processing; no post has happened, but the exact signed act is durable.
    sqlx::query("CREATE TRIGGER interrupt_respend BEFORE INSERT ON proven_tx_reqs BEGIN SELECT RAISE(FAIL,'mock interruption before processing'); END")
        .execute(s.writer()).await.unwrap();
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
        .execute(s.writer())
        .await
        .unwrap();
    bsv_wallet_cli::broadcast_reconcile::run_sweep(s, &absent, true, ABSENCE)
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

/// 0.11.0 (#29): the respend's exact inputs pass the spend gate; an input
/// the feed froze refuses it before anything is built or posted.
#[tokio::test]
async fn respend_refuses_an_input_the_gate_holds() {
    let f = fixture().await;
    let s = f.wallet.storage();
    let absent = status_source(&f.phantom, "unknown", BroadcastStatus::Unknown);
    headed(&absent, &f.funding);
    bsv_wallet_cli::tracker_host::deliver_chain_event(
        s,
        &absent,
        &bsv_wallet_cli::tracker_host::SystemClock,
        &bsv_wallet_cli::tracker_host::TickOptions::default(),
        1,
        bsv_tracker::ChainEvent::Frozen {
            outpoint: bsv_tracker::Outpoint {
                txid: f.funding.clone(),
                vout: 0,
            },
        },
    )
    .await
    .unwrap();
    assert_held_input(&f, &absent, "frozen: never offered").await;
}

/// A held exact input refuses the replacement without a post or a changed lock.
async fn assert_held_input(f: &world::Fixture, services: &MockWalletServices, reason: &str) {
    let before = words(f).await;
    let error = build_and_post(f.wallet.storage(), services, &f.root, &f.phantom, &["mock"])
        .await
        .unwrap_err();
    assert!(error.to_string().contains(reason), "{error}");
    assert_eq!(services.call_count("post_beef"), 0);
    assert_eq!(words(f).await, before);
    assert_eq!(coin_state(f, f.coin).await, (false, Some(f.phantom_row)));
}

/// The lock reserved to the phantom does not hide the source's failed
/// status, even though its funding proof still checks. No replacement is posted.
#[tokio::test]
async fn respend_refuses_an_input_whose_transaction_failed() {
    let f = fixture().await;
    let storage = f.wallet.storage();
    storage
        .mark_internalized_tx_failed(&f.funding)
        .await
        .unwrap();
    let absent = status_source(&f.phantom, "unknown", BroadcastStatus::Unknown);
    headed(&absent, &f.funding);
    assert_held_input(&f, &absent, "its transaction is failed: never offered").await;
}

/// F3 of the #44 CLI review: once the replacement is posted, the status
/// write and the spend guard after it fail (a fault injected on each), and
/// the call still answers the replacement's txid, each fault logged with
/// its marker and the txid. The request's word is read tolerantly: the req
/// the failed status write left, `unknown` for none, and `unknown` with a
/// marker when the read itself fails.
#[tokio::test]
async fn a_store_fault_after_the_replacement_is_posted_is_logged_and_answers_its_txid() {
    let (lines, _guard) = capture_markers();
    let f = fixture().await;
    let s = f.wallet.storage();
    // The status write advances the replacement's req to 'unmined'; the
    // guard's first write re-reserves the phantom's coin. Both refused.
    for fault in [
        "CREATE TRIGGER fault_status BEFORE UPDATE OF status ON proven_tx_reqs \
         WHEN NEW.status = 'unmined' AND OLD.status <> 'unmined' \
         BEGIN SELECT RAISE(ABORT, 'injected fault after the post'); END"
            .to_string(),
        format!(
            "CREATE TRIGGER fault_guard BEFORE UPDATE ON outputs WHEN OLD.output_id = {} \
             BEGIN SELECT RAISE(ABORT, 'injected fault after the post'); END",
            f.coin
        ),
    ] {
        sqlx::query(&fault).execute(s.writer()).await.unwrap();
    }
    let absent = status_source(&f.phantom, "unknown", BroadcastStatus::Unknown);
    headed(&absent, &f.funding);
    let replacement = build_and_post(s, &absent, &f.root, &f.phantom, &["mock"])
        .await
        .expect("a fault after the post is never the respend's error");
    assert_eq!(absent.call_count("post_beef"), 1);
    let marked = lines.lock().unwrap().clone();
    println!("{marked:?}");
    for marker in [
        "post_broadcast_status_write_failed",
        "post_broadcast_guard_failed",
    ] {
        assert!(
            marked
                .iter()
                .any(|(m, t)| m == marker && t.as_deref() == Some(replacement.as_str())),
            "{marker} with the replacement's txid: {marked:?}"
        );
    }
    let left = bsv_wallet_cli::respend::request_word(s, &replacement).await;
    assert_ne!(left, "unmined", "the failed status write left the req");
    assert_eq!(
        bsv_wallet_cli::respend::request_word(s, &"ee".repeat(32)).await,
        "unknown"
    );
    s.pool().close().await;
    assert_eq!(
        bsv_wallet_cli::respend::request_word(s, &replacement).await,
        "unknown"
    );
    assert!(lines
        .lock()
        .unwrap()
        .iter()
        .any(|(m, t)| m == "post_broadcast_request_read_failed"
            && t.as_deref() == Some(replacement.as_str())));
}
