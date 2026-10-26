//! #174: served witnesses for the ten misses of nightly 40290596569.

use super::super::*;
use crate::test_support::coins::{mined_coin_with_lock, storage_for};
use crate::test_support::{mock_services, response_answer, serve_router};
use axum::{
    body::Body,
    http::{Request, StatusCode},
    response::IntoResponse,
    routing::post,
    Extension, Router,
};
use bsv_sdk::{primitives::PrivateKey, transaction::Beef};
use serde_json::Value;
use std::sync::atomic::{AtomicUsize, Ordering};
use tower::ServiceExt;

#[derive(Default)]
struct Calls {
    posts: AtomicUsize,
    probes: AtomicUsize,
}

#[derive(Clone, Copy)]
enum Presence {
    Seen,
    Absent,
    Unknown,
}

struct World {
    wallet: WalletState,
    router: Router,
    funding: String,
    calls: Arc<Calls>,
    mock: tokio::task::JoinHandle<()>,
}

impl Drop for World {
    fn drop(&mut self) {
        self.mock.abort();
    }
}

impl World {
    async fn new(presence: Presence, keep: bool) -> Self {
        let key = PrivateKey::from_hex(&"01".repeat(32)).unwrap();
        let (storage, user, basket) = storage_for(&key.public_key().to_hex()).await;
        let (funding, _) =
            mined_coin_with_lock(&storage, (user, basket), 0xd3, 50_000, 800, "5187").await;
        // The shared planter makes wallet-managed change; this puzzle is a
        // caller input, whose script is supplied by createAction/signAction.
        sqlx::query("UPDATE outputs SET change = 0 WHERE txid = ?")
            .bind(&funding)
            .execute(storage.writer())
            .await
            .unwrap();
        let calls = Arc::new(Calls::default());
        let app = mock_router(
            storage.clone(),
            funding.clone(),
            calls.clone(),
            presence,
            keep,
        );
        let (url, mock) = serve_router(app).await;
        let services = mock_services(&url);
        let wallet = Arc::new(Wallet::new(Some(key), storage, services).await.unwrap());
        let verifier =
            crate::broadcast_verify::BroadcastVerifier::explicit(false, &url, Some(&url));
        let router = Router::new()
            .route("/createAction", post(create_action))
            .route("/signAction", post(sign_action))
            .route("/getPublicKey", post(get_public_key))
            .layer(Extension(Arc::new(tokio::sync::Mutex::new(()))))
            .layer(Extension(verifier))
            .layer(Extension(crate::spend_gate::GateOptions::default()))
            .with_state(wallet.clone());
        Self {
            wallet,
            router,
            funding,
            calls,
            mock,
        }
    }

    fn request(&self, options: &Value) -> Value {
        json!({"description": "handler mutant witness",
            "inputs": [{"outpoint": format!("{}.0", self.funding), "unlockingScript": "51"}],
            "outputs": [{"lockingScript": "51", "satoshis": 1000, "outputDescription": "output one"},
                        {"lockingScript": "52", "satoshis": 2000, "outputDescription": "output two"}],
            "options": options})
    }

    async fn serve(&self, path: &str, body: Value) -> (StatusCode, Value) {
        let request = Request::post(path)
            .header("content-type", "application/json")
            .header("origin", "http://handler.test")
            .body(Body::from(body.to_string()))
            .unwrap();
        let (status, _, answer) =
            response_answer(self.router.clone().oneshot(request).await.unwrap()).await;
        (status, answer)
    }

    async fn create(&self, options: Value) -> (StatusCode, Value) {
        self.serve("/createAction", self.request(&options)).await
    }

    async fn signed(&self, options: Value) -> (StatusCode, Value) {
        let (status, unsigned) = self.create(json!({"signAndProcess": false})).await;
        assert_eq!(status, StatusCode::OK, "{unsigned}");
        self.serve(
            "/signAction",
            json!({
                "reference": unsigned["signableTransaction"]["reference"],
                "spends": {"0": {"unlockingScript": "51"}}, "options": options
            }),
        )
        .await
    }

    async fn seen(&self, txid: &str) -> Vec<String> {
        sqlx::query_scalar("SELECT status FROM broadcast_seen WHERE txid = ?")
            .bind(txid)
            .fetch_all(self.wallet.storage().pool())
            .await
            .unwrap()
    }
}

/// The post is refused, but the independent presence probe can see the tx.
/// `keep` models an acceptance arriving while an absence probe is in flight.
fn mock_router(
    storage: StorageSqlx,
    funding: String,
    calls: Arc<Calls>,
    presence: Presence,
    keep: bool,
) -> Router {
    Router::new().fallback(move |method: axum::http::Method, uri: axum::http::Uri| {
        let (storage, funding, calls) = (storage.clone(), funding.clone(), calls.clone());
        async move {
            if method == axum::http::Method::POST {
                calls.posts.fetch_add(1, Ordering::SeqCst);
                return (
                    StatusCode::SERVICE_UNAVAILABLE,
                    Json(json!({"status": 503, "detail": "unavailable"})),
                )
                    .into_response();
            }
            if uri.path().starts_with("/v1/tx/") || uri.path().starts_with("/tx/hash/") {
                calls.probes.fetch_add(1, Ordering::SeqCst);
                if keep {
                    let txid = uri.path().rsplit('/').next().unwrap();
                    storage
                        .record_broadcast_status(txid, "witness", "accepted")
                        .await
                        .unwrap();
                }
                return match presence {
                    Presence::Absent => (
                        StatusCode::NOT_FOUND,
                        Json(json!({"status": 404, "detail": "not found"})),
                    )
                        .into_response(),
                    Presence::Seen => Json(json!({"txStatus": "SEEN_ON_NETWORK"})).into_response(),
                    Presence::Unknown => StatusCode::SERVICE_UNAVAILABLE.into_response(),
                };
            }
            let height = if uri.query() == Some("height=800") {
                800
            } else {
                900
            };
            Json(json!({"status": "success", "value": {
                "height": height, "version": 1, "previousHash": "00".repeat(32),
                "merkleRoot": if height == 800 { funding } else { "0c".repeat(32) },
                "time": 1_700_000_000, "bits": 486_604_799_u32, "nonce": 0,
                "hash": format!("{height:064x}")
            }}))
            .into_response()
        }
    })
}

async fn broadcast_case(options: Value, expected: bool) {
    let w = World::new(Presence::Seen, false).await;
    let (status, answer) = w.create(options).await;
    assert_eq!(status, StatusCode::OK, "{answer}");
    assert_eq!(w.calls.posts.load(Ordering::SeqCst) > 0, expected);
    assert_eq!(w.calls.probes.load(Ordering::SeqCst) > 0, expected);
    if let Some(txid) = answer["txid"].as_str() {
        assert_eq!(
            w.seen(txid).await.iter().any(|s| s == "seen"),
            expected,
            "{answer}"
        );
    } else {
        assert!(answer["signableTransaction"].is_object(), "{answer}");
    }
}

#[tokio::test]
async fn broadcast_now_no_send_operand() {
    for no_send in [false, true] {
        broadcast_case(
            json!({"noSend": no_send, "acceptDelayedBroadcast": false}),
            !no_send,
        )
        .await;
    }
}

#[tokio::test]
async fn broadcast_now_accept_delayed_operand() {
    for delayed in [false, true] {
        broadcast_case(
            json!({"noSend": false, "acceptDelayedBroadcast": delayed}),
            !delayed,
        )
        .await;
    }
}

#[tokio::test]
async fn broadcast_now_requires_sign_and_process() {
    broadcast_case(json!({"signAndProcess": false}), false).await;
    broadcast_case(json!({"signAndProcess": true}), true).await;
}

async fn retired_broadcast() {
    let w = World::new(Presence::Absent, false).await;
    let (status, answer) = w.create(json!({})).await;
    assert_eq!(status, StatusCode::BAD_GATEWAY, "{answer}");
    assert_eq!(answer["code"], "BROADCAST_REJECTED");
    let failed: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM transactions WHERE status = 'failed'")
            .fetch_one(w.wallet.storage().pool())
            .await
            .unwrap();
    assert_eq!(failed, 1);
    assert!(w.calls.probes.load(Ordering::SeqCst) >= 2);
}

#[tokio::test]
async fn rejection_outcome_eq_to_ne_mutant() {
    retired_broadcast().await;
}

#[tokio::test]
async fn rejection_failed_status_eq_to_ne_mutant() {
    retired_broadcast().await;
}

#[tokio::test]
async fn rejection_and_to_or_mutant_keeps_a_concurrent_acceptance() {
    let w = World::new(Presence::Absent, true).await;
    let (status, answer) = w.create(json!({})).await;
    assert_eq!(status, StatusCode::OK, "{answer}");
    let txid = answer["txid"].as_str().unwrap();
    assert!(w.seen(txid).await.iter().any(|s| s == "accepted"));
    assert_ne!(
        super::super::super::broadcast_follow_up::tx_status(w.wallet.storage(), txid)
            .await
            .as_deref(),
        Some("failed")
    );
    assert!(w.calls.probes.load(Ordering::SeqCst) >= 2);
}

async fn sending_guard(sign: bool, no_send: bool) {
    let w = World::new(Presence::Unknown, false).await;
    if no_send {
        // Model another actor queuing the transaction before the answer:
        // the result has no sending entry, even though post_word has a row.
        sqlx::query(
            "CREATE TRIGGER queue_no_send AFTER INSERT ON proven_tx_reqs \
            WHEN NEW.status = 'nosend' BEGIN UPDATE proven_tx_reqs SET status = 'unsent' \
            WHERE proven_tx_req_id = NEW.proven_tx_req_id; END",
        )
        .execute(w.wallet.storage().writer())
        .await
        .unwrap();
    }
    let options = json!({"noSend": no_send});
    let (status, answer) = if sign {
        w.signed(options).await
    } else {
        w.create(options).await
    };
    assert_eq!(status, StatusCode::OK, "{answer}");
    let results = answer["sendWithResults"].as_array().unwrap();
    assert_eq!(
        results.iter().any(|r| r["status"] == "sending"),
        !no_send,
        "{answer}"
    );
    assert_eq!(answer.get("broadcast").is_some(), !no_send, "{answer}");
    if no_send {
        let txid = answer["txid"].as_str().unwrap();
        assert!(
            super::super::super::post_word::post_word(w.wallet.storage(), txid)
                .await
                .is_some(),
            "the queued row must expose a guard forced to true"
        );
    } else {
        assert_eq!(answer["broadcast"]["word"], "built");
    }
}

#[tokio::test]
async fn create_sending_guard_true_and_false_mutants() {
    sending_guard(false, false).await;
    sending_guard(false, true).await;
}

#[tokio::test]
async fn sign_sending_guard_true_and_false_mutants() {
    sending_guard(true, false).await;
    sending_guard(true, true).await;
}

fn mapped_options(request: &Value) -> CreateActionOptions {
    let mapped = serde_json::from_value(request.clone()).unwrap();
    create_action_args(mapped).unwrap().0.options.unwrap()
}

#[tokio::test]
async fn delete_accept_delayed_broadcast_option_mutant() {
    for delayed in [false, true] {
        let w = World::new(Presence::Seen, false).await;
        let request = w.request(&json!({"acceptDelayedBroadcast": delayed}));
        assert_eq!(
            mapped_options(&request).accept_delayed_broadcast,
            Some(delayed)
        );
        let (status, answer) = w.serve("/createAction", request).await;
        assert_eq!(status, StatusCode::OK, "{answer}");
        assert_eq!(w.calls.posts.load(Ordering::SeqCst) > 0, !delayed);
    }
}

#[tokio::test]
async fn delete_randomize_outputs_option_mutant() {
    let w = World::new(Presence::Seen, false).await;
    let request = w.request(&json!({"noSend": true, "randomizeOutputs": false}));
    // Toolbox 0.13.0 currently preserves order even without this option.
    // Assert the exact argument too, so its omission cannot pass by accident.
    assert_eq!(mapped_options(&request).randomize_outputs, Some(false));
    let (status, answer) = w.serve("/createAction", request).await;
    assert_eq!(status, StatusCode::OK, "{answer}");
    let bytes: Vec<u8> = serde_json::from_value(answer["tx"].clone()).unwrap();
    let beef = Beef::from_binary(&bytes).unwrap();
    let tx = beef
        .find_atomic_transaction(answer["txid"].as_str().unwrap())
        .unwrap();
    assert_eq!(tx.outputs[0].locking_script.to_hex(), "51");
    assert_eq!(tx.outputs[1].locking_script.to_hex(), "52");
}

#[tokio::test]
async fn delete_protocol_security_level_zero_arm_mutant() {
    let protocol = parse_protocol_id(&json!([0, "name"])).unwrap();
    assert_eq!(protocol.security_level, SecurityLevel::Silent);
    let w = World::new(Presence::Seen, false).await;
    let (status, answer) = w
        .serve(
            "/getPublicKey",
            json!({
                "protocolID": [0, "names"], "keyID": "witness", "counterparty": "self"
            }),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{answer}");
    assert!(PublicKey::from_hex(answer["publicKey"].as_str().unwrap()).is_ok());
}
