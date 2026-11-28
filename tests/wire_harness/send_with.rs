//! The reference's paying client through the HTTP door (#147): its two
//! wallet calls (`AuthFetch.ts`, ts-stack `57c5f13`, `@bsv/sdk` from
//! 4.1.0): `createAction` with `noSend: true` and
//! `acceptDelayedBroadcast: false`, then `createAction` with
//! `options.sendWith: [txid]` and no inputs or outputs, which the client
//! requires to answer exactly one `unproven` result. Until the wallet
//! below this host answers that call, a create naming a non-empty
//! `sendWith` or `noSendChange` is refused by name, 501
//! `SEND_WITH_NOT_SUPPORTED`, with nothing built, stored or posted, and
//! the payer's own abort frees the prepared payment's coins. A
//! `signAction` naming an earlier `noSend` transaction in its `sendWith`
//! is passed whole, and its answer at the pin is pinned here.
//!
//! Each case runs in the `funded` world (its coins mined, its headers a
//! local stub, every proxy closed: `quiet_environment`), whose one
//! broadcaster is the world's local ARC stub; the stub logs every request
//! it takes, and the cases count them.

use axum::body::to_bytes;
use serde_json::{json, Value};
use sqlx::Row;
use tower::ServiceExt;

use super::recorder::post;
use super::worlds::{World, P2PKH};
use crate::wire_support::on_runtime;

/// The payment the reference's client prepares: one output, its
/// satoshis well under the funded world's largest coin (50,000), so the
/// toolbox takes that coin and makes change.
const PAYMENT: u64 = 1_000;

/// The requests the world's ARC took so far: (path, txids carried).
fn posts(world: &World) -> Vec<(String, Vec<String>)> {
    world.arc_posts.lock().unwrap().clone()
}

/// `body` posted to `path` through the world's router: the status and
/// the JSON body.
async fn call(world: &World, path: &str, body: Value) -> (u16, Value) {
    let response = world
        .router
        .clone()
        .oneshot(post(path, body).to_http())
        .await
        .unwrap();
    let status = response.status().as_u16();
    let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

/// The reference's call 1: the payment prepared, signed, not sent.
fn prepare() -> Value {
    json!({
        "description": "wire paid request",
        "outputs": [{"lockingScript": P2PKH, "satoshis": PAYMENT,
                     "outputDescription": "HTTP request payment"}],
        "options": {"randomizeOutputs": false, "noSend": true, "acceptDelayedBroadcast": false}
    })
}

/// The reference's call 2 (`submitPreparedPayment`), naming `txid`.
fn submit(txid: &str) -> Value {
    json!({
        "description": "Submit prepared HTTP payment",
        "options": {"sendWith": [txid], "acceptDelayedBroadcast": false}
    })
}

/// The store as the cases read it: every transaction (txid, status), every
/// output (its transaction's txid, vout, satoshis, spendable, its
/// spender's txid), every proof request (txid, status).
#[derive(Debug, PartialEq, Eq, Clone)]
struct Store {
    transactions: Vec<(String, String)>,
    outputs: Vec<(String, i64, i64, bool, Option<String>)>,
    requests: Vec<(String, String)>,
}

async fn store(world: &World) -> Store {
    let pool = world.wallet.storage().writer();
    let transactions =
        sqlx::query("SELECT COALESCE(txid, ''), status FROM transactions ORDER BY transaction_id")
            .fetch_all(pool)
            .await
            .unwrap()
            .iter()
            .map(|r| (r.get(0), r.get(1)))
            .collect();
    let outputs = sqlx::query(
        "SELECT COALESCE(t.txid, ''), o.vout, o.satoshis, o.spendable, s.txid \
         FROM outputs o JOIN transactions t ON t.transaction_id = o.transaction_id \
         LEFT JOIN transactions s ON s.transaction_id = o.spent_by ORDER BY o.output_id",
    )
    .fetch_all(pool)
    .await
    .unwrap()
    .iter()
    .map(|r| (r.get(0), r.get(1), r.get(2), r.get(3), r.get(4)))
    .collect();
    let requests = sqlx::query("SELECT txid, status FROM proven_tx_reqs ORDER BY proven_tx_req_id")
        .fetch_all(pool)
        .await
        .unwrap()
        .iter()
        .map(|r| (r.get(0), r.get(1)))
        .collect();
    Store {
        transactions,
        outputs,
        requests,
    }
}

/// The status the store holds for `txid`.
fn status_of(store: &Store, txid: &str) -> Option<String> {
    store
        .transactions
        .iter()
        .find(|(t, _)| t == txid)
        .map(|(_, s)| s.clone())
}

/// The outputs `txid` spends, as (their txid, vout, satoshis, spendable).
fn spent_by(store: &Store, txid: &str) -> Vec<(String, i64, i64, bool)> {
    store
        .outputs
        .iter()
        .filter(|o| o.4.as_deref() == Some(txid))
        .map(|o| (o.0.clone(), o.1, o.2, o.3))
        .collect()
}

/// The prepared payment (call 1): its txid, with no post made.
async fn prepared(world: &World) -> (String, Value) {
    let (status, body) = call(world, "/createAction", prepare()).await;
    assert_eq!(status, 200, "call 1: {body}");
    assert!(posts(world).is_empty(), "call 1 posts nothing");
    let txid = body["txid"].as_str().unwrap().to_string();
    (txid, body)
}

/// The refusal's code (#147).
const REFUSED: &str = "SEND_WITH_NOT_SUPPORTED";

/// `body` posted to `/createAction` is refused by name before anything is
/// built, stored or posted: 501, the code, its message naming `option`;
/// the ARC took nothing and the store is as it was.
async fn refused(world: &World, body: Value, option: &str) -> Value {
    let before = store(world).await;
    let posted = posts(world).len();
    let (status, served) = call(world, "/createAction", body).await;
    assert_eq!(status, 501, "{served}");
    assert_eq!(served["code"], REFUSED);
    let message = served["message"].as_str().unwrap();
    assert!(
        message.starts_with(&format!("createAction: {option} is not supported: ")),
        "{message}"
    );
    for said in [
        "this wallet cannot yet",
        "was not sent and is still reserved under its txid",
        "abortAction with its txid as the reference frees its coins",
    ] {
        assert!(message.contains(said), "{message}");
    }
    assert_eq!(posts(world).len(), posted, "nothing posted");
    assert_eq!(store(world).await, before, "nothing built or stored");
    served
}

#[test]
fn the_payers_second_call_is_refused_and_her_abort_frees_the_coins() {
    on_runtime(|| async {
        let world = World::build("funded").await;
        let (prepared_txid, _) = prepared(&world).await;
        let served = refused(&world, submit(&prepared_txid), "sendWith").await;
        eprintln!("#147 call 2: {served}");

        // The payer's leg: the reference's client never aborts by itself
        // (an unknown outcome), so her application's abort is her way out.
        // After the refusal the payment still reads `nosend`, its input
        // reserved.
        let after = store(&world).await;
        assert_eq!(status_of(&after, &prepared_txid).as_deref(), Some("nosend"));
        let reserved = spent_by(&after, &prepared_txid);
        assert_eq!(reserved.len(), 1, "{reserved:?}");
        assert!(reserved.iter().all(|o| !o.3), "{reserved:?}");
        abort_frees(&world, &prepared_txid, &reserved[0]).await;
    });
}

/// Her abort of `txid` by its txid (the reference client's own
/// `abortAction({reference: txid})`) succeeds, `coin` is spendable again,
/// and a following ordinary createAction spends it: only the freed coin
/// (50,000) covers 45,000.
async fn abort_frees(world: &World, txid: &str, coin: &(String, i64, i64, bool)) {
    let (status, aborted) = call(world, "/abortAction", json!({"reference": txid})).await;
    assert_eq!(
        (status, &aborted["aborted"]),
        (200, &json!(true)),
        "{aborted}"
    );
    let freed = store(world).await;
    assert!(spent_by(&freed, txid).is_empty());
    assert!(freed
        .outputs
        .iter()
        .any(|o| (&o.0, o.1, o.2, o.3, &o.4) == (&coin.0, coin.1, coin.2, true, &None)));

    let (status, spent) = call(
        world,
        "/createAction",
        json!({"description": "wire after the abort", "outputs": [
            {"lockingScript": P2PKH, "satoshis": 45_000, "outputDescription": "an output"}],
            "options": {"randomizeOutputs": false}}),
    )
    .await;
    assert_eq!(status, 200, "{spent}");
    let spender = spent["txid"].as_str().unwrap();
    let spends = spent_by(&store(world).await, spender);
    assert!(
        spends.iter().any(|o| (&o.0, o.1) == (&coin.0, coin.1)),
        "{spends:?}"
    );
    assert_eq!(posts(world).len(), 1);
}

#[test]
fn no_send_change_is_refused_by_name() {
    on_runtime(|| async {
        let world = World::build("funded").await;
        let (prepared_txid, prepared_body) = prepared(&world).await;
        let named = prepared_body["noSendChange"][0]
            .as_str()
            .unwrap()
            .to_string();
        assert!(named.starts_with(&prepared_txid));
        let served = refused(
            &world,
            json!({
                "description": "wire chained payment",
                "outputs": [{"lockingScript": P2PKH, "satoshis": PAYMENT,
                             "outputDescription": "a second payment"}],
                "options": {"randomizeOutputs": false, "noSend": true,
                            "acceptDelayedBroadcast": false, "noSendChange": [named]}
            }),
            "noSendChange",
        )
        .await;
        eprintln!("#147 noSendChange: {served}");
        assert_eq!(
            status_of(&store(&world).await, &prepared_txid).as_deref(),
            Some("nosend")
        );
    });
}

#[test]
fn empty_lists_are_the_call_as_before() {
    on_runtime(|| async {
        let world = World::build("funded").await;
        let (status, body) = call(
            &world,
            "/createAction",
            json!({
                "description": "wire empty lists",
                "outputs": [{"lockingScript": P2PKH, "satoshis": PAYMENT,
                             "outputDescription": "an output"}],
                "options": {"randomizeOutputs": false, "noSend": true,
                            "sendWith": [], "noSendChange": []}
            }),
        )
        .await;
        assert_eq!(status, 200, "{body}");
        let txid = body["txid"].as_str().unwrap();
        assert_eq!(
            status_of(&store(&world).await, txid).as_deref(),
            Some("nosend")
        );
        assert!(posts(&world).is_empty());
        let (status, body) = call(
            &world,
            "/createAction",
            json!({
                "description": "wire null lists",
                "outputs": [{"lockingScript": P2PKH, "satoshis": PAYMENT,
                             "outputDescription": "an output"}],
                "options": {"randomizeOutputs": false, "sendWith": null, "noSendChange": null}
            }),
        )
        .await;
        assert_eq!(status, 200, "{body}");
        assert_eq!(posts(&world).len(), 1);
    });
}

/// The funded world's unsigned template (`signAndProcess: false`): its
/// reference.
async fn template(world: &World) -> Value {
    let (status, template) = call(
        world,
        "/createAction",
        json!({
            "description": "wire template",
            "outputs": [{"lockingScript": P2PKH, "satoshis": PAYMENT,
                         "outputDescription": "an output"}],
            "options": {"signAndProcess": false, "randomizeOutputs": false}
        }),
    )
    .await;
    assert_eq!(status, 200, "{template}");
    template["signableTransaction"]["reference"].clone()
}

#[test]
fn a_sign_action_naming_a_prepared_payment() {
    on_runtime(|| async {
        let world = World::build("funded").await;
        let (prepared_txid, _) = prepared(&world).await;
        let reference = template(&world).await;
        let before = store(&world).await;

        let (status, body) = call(
            &world,
            "/signAction",
            json!({"reference": reference, "spends": {},
                   "options": {"sendWith": [prepared_txid], "acceptDelayedBroadcast": false}}),
        )
        .await;
        let after = store(&world).await;
        let made: Vec<_> = after
            .transactions
            .iter()
            .filter(|t| !before.transactions.iter().any(|b| b.0 == t.0))
            .cloned()
            .collect();
        let signed = body["txid"].as_str().unwrap_or_default().to_string();
        eprintln!("#147 signAction: status {status}, body {body}");
        eprintln!("#147 signAction: posts {:?}, made {made:?}", posts(&world));

        // The pin's answer (toolbox 0.14.0-beta.10, the world's classic
        // ARC): the template's transaction is posted alone, as its EF (its
        // parent is proven); the prepared payment in the same post's BEEF
        // is never sent, and is marked `sending` and answered `sending`,
        // never `unproven`. Nothing is made or posted beyond what the
        // caller asked; the door changes nothing here (#147).
        assert_eq!(status, 200);
        assert_eq!(
            posts(&world),
            vec![("/v1/tx".to_string(), vec![signed.clone()])]
        );
        assert_eq!(made, vec![(signed.clone(), "unproven".to_string())]);
        let mut named: Vec<_> = body["sendWithResults"]
            .as_array()
            .unwrap()
            .iter()
            .map(|r| (r["txid"].as_str().unwrap(), r["status"].as_str().unwrap()))
            .collect();
        named.sort_unstable();
        let mut want = vec![
            (signed.as_str(), "unproven"),
            (prepared_txid.as_str(), "sending"),
        ];
        want.sort_unstable();
        assert_eq!(named, want);
        assert_eq!(
            status_of(&after, &prepared_txid).as_deref(),
            Some("sending")
        );
        assert_eq!(status_of(&after, &signed).as_deref(), Some("unproven"));
    });
}
