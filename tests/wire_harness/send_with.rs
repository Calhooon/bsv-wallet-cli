//! The reference's paying client through the HTTP door (#147): its two
//! wallet calls (`AuthFetch.ts`, ts-stack `57c5f13`, `@bsv/sdk` from
//! 4.1.0): `createAction` with `noSend: true` and
//! `acceptDelayedBroadcast: false`, then `createAction` with
//! `options.sendWith: [txid]` and no inputs or outputs, which the client
//! requires to answer exactly one `unproven` result. Also a create naming
//! `noSendChange`, and a `signAction` naming an earlier `noSend`
//! transaction in its `sendWith`.
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

/// The fee of `txid`: the outputs it spends less the outputs it makes.
fn fee_of(store: &Store, txid: &str) -> i64 {
    let spent: i64 = spent_by(store, txid).iter().map(|o| o.2).sum();
    let made: i64 = store
        .outputs
        .iter()
        .filter(|o| o.0 == txid)
        .map(|o| o.2)
        .sum();
    spent - made
}

/// The prepared payment (call 1): its txid, with no post made.
async fn prepared(world: &World) -> (String, Value) {
    let (status, body) = call(world, "/createAction", prepare()).await;
    assert_eq!(status, 200, "call 1: {body}");
    assert!(posts(world).is_empty(), "call 1 posts nothing");
    let txid = body["txid"].as_str().unwrap().to_string();
    (txid, body)
}

#[test]
fn the_payers_second_call_at_this_host() {
    on_runtime(|| async {
        let world = World::build("funded").await;
        let (prepared_txid, _) = prepared(&world).await;
        let before = store(&world).await;
        assert_eq!(
            status_of(&before, &prepared_txid).as_deref(),
            Some("nosend")
        );

        let (status, body) = call(&world, "/createAction", submit(&prepared_txid)).await;
        let after = store(&world).await;
        let posts = posts(&world);
        let made: Vec<_> = after
            .transactions
            .iter()
            .filter(|t| !before.transactions.contains(t) && t.0 != prepared_txid)
            .cloned()
            .collect();
        eprintln!("#147 call 2: status {status}");
        eprintln!("#147 call 2: body {body}");
        eprintln!("#147 call 2: posts {posts:?}");
        eprintln!("#147 call 2: transactions made {made:?}");
        for (txid, _) in &made {
            eprintln!(
                "#147 call 2: {txid} spends {:?}, fee {}",
                spent_by(&after, txid),
                fee_of(&after, txid)
            );
        }
        eprintln!(
            "#147 call 2: the prepared payment reads {:?}",
            status_of(&after, &prepared_txid)
        );

        // The base: `sendWith` is dropped at the door, so call 2 is an
        // empty `createAction`: the toolbox builds a new transaction from
        // the wallet's coins (its change alone), posts it, and answers its
        // own txid; the prepared payment is never sent.
        assert_eq!(status, 200);
        let new_txid = body["txid"].as_str().unwrap();
        assert_ne!(new_txid, prepared_txid);
        assert_eq!(posts.len(), 1);
        assert_eq!(posts[0].1, vec![new_txid.to_string()]);
        assert_eq!(made.len(), 1);
        assert!(fee_of(&after, new_txid) > 0);
        let results = body["sendWithResults"].as_array().unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0]["txid"], new_txid);
        assert_eq!(status_of(&after, &prepared_txid).as_deref(), Some("nosend"));
    });
}

#[test]
fn no_send_change_at_this_host() {
    on_runtime(|| async {
        let world = World::build("funded").await;
        let (prepared_txid, prepared_body) = prepared(&world).await;
        let change = prepared_body["noSendChange"].clone();
        eprintln!("#147 noSendChange: call 1 answered noSendChange {change}");
        let named = change[0]
            .as_str()
            .map_or_else(|| format!("{prepared_txid}.1"), str::to_string);
        let before = store(&world).await;

        let (status, body) = call(
            &world,
            "/createAction",
            json!({
                "description": "wire chained payment",
                "outputs": [{"lockingScript": P2PKH, "satoshis": PAYMENT,
                             "outputDescription": "a second payment"}],
                "options": {"randomizeOutputs": false, "noSend": true,
                            "acceptDelayedBroadcast": false, "noSendChange": [named]}
            }),
        )
        .await;
        let after = store(&world).await;
        eprintln!("#147 noSendChange: status {status}");
        eprintln!("#147 noSendChange: body {body}");
        eprintln!("#147 noSendChange: posts {}", posts(&world).len());
        let new_txid = body["txid"].as_str().unwrap_or_default().to_string();
        eprintln!(
            "#147 noSendChange: {new_txid} spends {:?}",
            spent_by(&after, &new_txid)
        );

        // The base: `noSendChange` is dropped at the door; the create is
        // answered as a plain `noSend` from whatever coins the toolbox
        // takes, the named change unread: it spends another coin.
        assert_eq!(status, 200);
        assert_eq!(posts(&world).len(), 0);
        assert_ne!(before, after);
        let (named_txid, named_vout) = named.split_once('.').unwrap();
        assert!(!spent_by(&after, &new_txid)
            .iter()
            .any(|o| o.0 == named_txid && o.1.to_string() == named_vout));
    });
}

#[test]
fn a_sign_action_naming_a_prepared_payment() {
    on_runtime(|| async {
        let world = World::build("funded").await;
        let (prepared_txid, _) = prepared(&world).await;
        let (status, template) = call(
            &world,
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
        let reference = template["signableTransaction"]["reference"].clone();
        let before = store(&world).await;

        let (status, body) = call(
            &world,
            "/signAction",
            json!({"reference": reference, "spends": {},
                   "options": {"sendWith": [prepared_txid], "acceptDelayedBroadcast": false}}),
        )
        .await;
        let after = store(&world).await;
        let posts = posts(&world);
        let made: Vec<_> = after
            .transactions
            .iter()
            .filter(|t| !before.transactions.iter().any(|b| b.0 == t.0))
            .cloned()
            .collect();
        let signed = body["txid"].as_str().unwrap_or_default().to_string();
        eprintln!("#147 signAction: status {status}");
        eprintln!("#147 signAction: body {body}");
        eprintln!("#147 signAction: posts {posts:?}");
        eprintln!("#147 signAction: transactions made {made:?}");
        eprintln!(
            "#147 signAction: the prepared payment reads {:?}, the template {:?}",
            status_of(&after, &prepared_txid),
            status_of(&after, &signed)
        );

        // The pin's answer (toolbox 0.14.0-beta.10, the world's classic
        // ARC): the template's transaction is posted alone, as its EF (its
        // parent is proven); the prepared payment in the same post's BEEF
        // is never sent, and is marked `sending` and answered `sending`,
        // never `unproven`. Nothing is made or posted beyond what the
        // caller asked; the door changes nothing here (#147).
        assert_eq!(status, 200);
        assert_eq!(posts, vec![("/v1/tx".to_string(), vec![signed.clone()])]);
        assert_eq!(made, vec![(signed.clone(), "unproven".to_string())]);
        let results = body["sendWithResults"].as_array().unwrap();
        let mut named: Vec<_> = results
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
