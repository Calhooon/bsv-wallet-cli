//! The spending power in `createAction` (#414, the design's L6, sections
//! (1) and (2), W6 and W7), over the served router on a loopback listener
//! and a store holding mined coins the calls name as their inputs: the
//! ceiling at its edge, two concurrent spends against one ceiling, `once`,
//! a grant with a ceiling and its renewal, the tally across a UTC month,
//! the deferred path's reference then txid, and the abort's delete.

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use axum::{Json, Router};
use bsv_sdk::primitives::PrivateKey;
use bsv_wallet_toolbox::Wallet;
use serde_json::{json, Value};

use crate::server::powers_tests::{page_call, serve_at, Read};
use crate::server::*;
use crate::store::origin_grants::{self as grants, Answer, Flow, Granting, PendingRow, Power};
use crate::test_support::coins::{mined_coin_with_lock, storage_for};
use crate::test_support::{mock_services, serve_router};

const PAGE: &str = "http://shop.example";
const SHOP: &str = "shop.example";
/// The coins' puzzle (`OP_1 OP_EQUAL`), unlocked by `OP_1`.
const PUZZLE: &str = "5187";

/// A served wallet holding `coins` mined coins of 50,000 satoshis, each a
/// caller input a call names (`coin(n)`), the seed unset: every page is
/// asked per power.
struct World {
    wallet: WalletState,
    addr: SocketAddr,
    held: powers::Held,
    coins: Vec<String>,
    _headers: tokio::task::JoinHandle<()>,
}

impl World {
    async fn new(coins: u8) -> Self {
        let key = PrivateKey::from_hex(&"03".repeat(32)).unwrap();
        let (storage, user, basket) = storage_for(&key.public_key().to_hex()).await;
        let mut roots = HashMap::new();
        let mut txids = Vec::new();
        for n in 0..coins {
            let height = 800 + u32::from(n);
            let (txid, _) =
                mined_coin_with_lock(&storage, (user, basket), 0xa0 + n, 50_000, height, PUZZLE)
                    .await;
            roots.insert(height, txid.clone());
            txids.push(txid);
        }
        // The grants' tables made before the router serves: the in-memory
        // store's shared cache answers "database schema is locked" to a read
        // racing their creation, which a wallet's file never does.
        grants::ensure(&storage).await.unwrap();
        // A caller input: its script is the call's, not the wallet's change.
        for txid in &txids {
            sqlx::query("UPDATE outputs SET change = 0 WHERE txid = ?")
                .bind(txid)
                .execute(storage.writer())
                .await
                .unwrap();
        }
        let (url, headers) = serve_router(header_service(roots)).await;
        let wallet: WalletState = Arc::new(
            Wallet::new(Some(key), storage, mock_services(&url))
                .await
                .unwrap(),
        );
        let config = ServerConfig::default();
        let held = config.held.clone();
        let addr = serve_at(&wallet, config).await;
        Self {
            wallet,
            addr,
            held,
            coins: txids,
            _headers: headers,
        }
    }

    /// A spend of `satoshis` on coin `n`, `options` its options.
    fn spend(&self, n: usize, satoshis: u64, options: &Value) -> Value {
        json!({"description": "a spending witness",
            "inputs": [{"outpoint": format!("{}.0", self.coins[n]), "unlockingScript": "51",
                "inputDescription": "a coin"}],
            "outputs": [{"lockingScript": "51", "satoshis": satoshis, "outputDescription": "a spend"}],
            "options": options})
    }

    /// The page's signed `noSend` spend of `satoshis` on coin `n`.
    async fn call(&self, n: usize, satoshis: u64) -> Read {
        let body = self.spend(
            n,
            satoshis,
            &json!({"noSend": true, "randomizeOutputs": false}),
        );
        page_call(self.addr, PAGE, "/createAction", body).await
    }

    /// The same, sent at once and answered later (a held call).
    fn begin(&self, n: usize, satoshis: u64) -> tokio::task::JoinHandle<Read> {
        let body = self.spend(
            n,
            satoshis,
            &json!({"noSend": true, "randomizeOutputs": false}),
        );
        let addr = self.addr;
        tokio::spawn(async move { page_call(addr, PAGE, "/createAction", body).await })
    }

    /// The owner's ceiling for the shop.
    async fn ceiling(&self, amount: i64) {
        let granting = Granting {
            originator: SHOP,
            power: &Power::Spending,
            amount: Some(amount),
            expiry: 0,
        };
        grants::grant(self.wallet.storage(), &granting, now())
            .await
            .unwrap();
    }

    /// The open rows once there are `n`.
    async fn open(&self, n: usize) -> Vec<PendingRow> {
        for _ in 0..500 {
            let open = grants::listing(self.wallet.storage(), false)
                .await
                .unwrap()
                .pending;
            if open.len() == n {
                return open;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        let all = grants::listing(self.wallet.storage(), true).await.unwrap();
        panic!("{n} open rows never appeared: {:?}", all.pending);
    }

    /// The owner answers row `id`, as the CLI writes it, and wakes its call.
    async fn answer(&self, id: i64, answer: Answer, amount: Option<i64>) {
        let reply = grants::Reply {
            answer,
            expiry: 0,
            amount,
        };
        grants::answer(self.wallet.storage(), id, &reply, now())
            .await
            .unwrap();
        self.held.wake(id);
    }

    async fn tally(&self) -> i64 {
        grants::month_tally(self.wallet.storage(), SHOP, now())
            .await
            .unwrap()
    }

    /// The tally's rows: (txid, reference, satoshis).
    async fn spends(&self) -> Vec<(Option<String>, Option<String>, i64)> {
        sqlx::query_as("SELECT txid, reference, satoshis FROM origin_spends ORDER BY spend_id")
            .fetch_all(self.wallet.storage().pool())
            .await
            .unwrap()
    }

    async fn grant_rows(&self) -> usize {
        grants::listing(self.wallet.storage(), true)
            .await
            .unwrap()
            .grants
            .len()
    }
}

/// The header service: each coin's height answers its txid as the root.
fn header_service(roots: HashMap<u32, String>) -> Router {
    Router::new().fallback(move |uri: axum::http::Uri| {
        let roots = roots.clone();
        async move {
            let height = uri
                .query()
                .and_then(|q| q.strip_prefix("height="))
                .and_then(|h| h.parse().ok())
                .unwrap_or(900);
            Json(json!({"status": "success", "value": {
                "height": height, "version": 1, "previousHash": "00".repeat(32),
                "merkleRoot": roots.get(&height).cloned().unwrap_or_else(|| "0c".repeat(32)),
                "time": 1_700_000_000, "bits": 486_604_799_u32, "nonce": 0,
                "hash": format!("{height:064x}")
            }}))
        }
    })
}

fn now() -> i64 {
    owner_door::now().unwrap()
}

/// A held spend's row: its kind, amount and renewal.
fn asked(row: &PendingRow) -> (&str, Option<i64>, bool) {
    (row.kind.as_str(), row.amount, row.renewal)
}

fn denied(read: &Read) -> bool {
    read.0 == 400
        && read.3["code"] == "ERR_PERMISSION_DENIED"
        && read.3["message"] == powers::PERMISSION_DENIED
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_spend_equal_to_the_ceiling_is_admitted_and_one_more_is_held_as_a_renewal() {
    let w = World::new(3).await;
    w.ceiling(3000).await;
    let admitted = w.call(0, 3000).await;
    assert_eq!(admitted.0, 200, "{}", admitted.3);
    let txid = admitted.3["txid"].as_str().unwrap().to_string();
    assert_eq!(w.spends().await, vec![(Some(txid), None, 3000)]);
    assert_eq!(w.tally().await, 3000);

    let one_more = w.begin(1, 1);
    let open = w.open(1).await;
    assert_eq!(asked(&open[0]), ("spending", Some(1), true));
    assert_eq!(open[0].key, "[]");
    w.answer(open[0].pending_id, Answer::Deny, None).await;
    let refused = one_more.await.unwrap();
    assert!(denied(&refused), "{refused:?}");
    assert_eq!(w.tally().await, 3000, "a refused spend is not tallied");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_spend_one_satoshi_past_the_ceiling_is_held_before_the_toolbox() {
    let w = World::new(1).await;
    w.ceiling(2999).await;
    let past = w.begin(0, 3000);
    let open = w.open(1).await;
    assert_eq!(asked(&open[0]), ("spending", Some(3000), true));
    assert!(w.spends().await.is_empty(), "nothing reached the toolbox");
    w.answer(open[0].pending_id, Answer::Deny, None).await;
    assert!(denied(&past.await.unwrap()));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn two_concurrent_spends_against_one_ceiling_admit_one_and_hold_the_other() {
    let w = World::new(2).await;
    w.ceiling(3000).await;
    let (first, second) = (w.begin(0, 3000), w.begin(1, 3000));
    let open = w.open(1).await;
    assert_eq!(asked(&open[0]), ("spending", Some(3000), true));
    // The held one is judged only after the admitted one tallied its spend
    // and let the lock go; a while later it is still the one held.
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(w.open(1).await.len(), 1, "never both admitted");
    assert_eq!(w.tally().await, 3000);
    w.answer(open[0].pending_id, Answer::Deny, None).await;
    let mut answers = [first.await.unwrap(), second.await.unwrap()];
    answers.sort_by_key(|r| r.0);
    assert_eq!(answers[0].0, 200, "{}", answers[0].3);
    assert!(denied(&answers[1]), "{:?}", answers[1]);
    assert_eq!(w.spends().await.len(), 1);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn once_admits_the_one_call_and_writes_no_grant_row() {
    let w = World::new(2).await;
    let call = w.begin(0, 2500);
    let open = w.open(1).await;
    assert_eq!(asked(&open[0]), ("spending", Some(2500), false));
    w.answer(open[0].pending_id, Answer::Once, None).await;
    let once = call.await.unwrap();
    assert_eq!(once.0, 200, "{}", once.3);
    assert_eq!(w.grant_rows().await, 0);
    assert_eq!(w.tally().await, 2500, "a spend answered once is tallied");

    // Once wrote no row: the next spend is held again.
    let again = w.begin(1, 2500);
    let open = w.open(1).await;
    assert_eq!(asked(&open[0]), ("spending", Some(2500), false));
    w.answer(open[0].pending_id, Answer::Deny, None).await;
    assert!(denied(&again.await.unwrap()));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_grant_below_the_call_is_checked_again_and_a_grant_with_room_admits() {
    let w = World::new(1).await;
    let call = w.begin(0, 2500);
    let first = w.open(1).await;
    assert_eq!(asked(&first[0]), ("spending", Some(2500), false));
    // A ceiling below the call's amount: checked again under the lock, so
    // the call is held once more, as a renewal.
    w.answer(first[0].pending_id, Answer::Grant, Some(2000))
        .await;
    let second = loop {
        let open = w.open(1).await;
        if open[0].pending_id != first[0].pending_id {
            break open;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    };
    assert_eq!(asked(&second[0]), ("spending", Some(2500), true));
    w.answer(second[0].pending_id, Answer::Grant, Some(60_000))
        .await;
    let granted = call.await.unwrap();
    assert_eq!(granted.0, 200, "{}", granted.3);
    let rows = grants::listing(w.wallet.storage(), true).await.unwrap();
    assert_eq!(
        (
            rows.grants.len(),
            rows.grants[0].kind.as_str(),
            rows.grants[0].amount
        ),
        (1, "spending", Some(60_000))
    );
    assert_eq!(w.tally().await, 2500);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_renewal_prompt_past_the_ceiling_names_the_ceiling_and_the_tally() {
    let w = World::new(2).await;
    w.ceiling(3000).await;
    assert_eq!(w.call(0, 1000).await.0, 200);
    let call = w.begin(1, 2500);
    let open = w.open(1).await;
    let shown = grants::Shown::of(w.wallet.storage(), vec![], open.clone(), now())
        .await
        .unwrap();
    assert_eq!(
        serde_json::to_value(shown.json()).unwrap()["pending"][0]["asked"],
        "spend 2500 satoshis past this month's ceiling of 3000 (1000 spent)"
    );
    w.answer(open[0].pending_id, Answer::Grant, Some(5000))
        .await;
    assert_eq!(call.await.unwrap().0, 200);
    assert_eq!(w.tally().await, 3500);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_tally_crossing_a_utc_month_boundary_counts_this_month_alone() {
    let w = World::new(1).await;
    w.ceiling(3000).await;
    // Last month's spend fills last month's ceiling; 31 days back is always
    // an earlier calendar month.
    let last_month = now() - 31 * 86_400;
    let earlier = "ee".repeat(32);
    grants::spent(
        w.wallet.storage(),
        SHOP,
        Flow::Signed(&earlier),
        3000,
        last_month,
    )
    .await
    .unwrap();
    assert_eq!(w.tally().await, 0);
    let admitted = w.call(0, 3000).await;
    assert_eq!(admitted.0, 200, "{}", admitted.3);
    assert_eq!(w.tally().await, 3000);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_deferred_path_tallies_its_reference_then_sign_action_fills_the_txid() {
    let w = World::new(1).await;
    w.ceiling(10_000).await;
    let mut template = w.spend(
        0,
        3000,
        &json!({"signAndProcess": false, "randomizeOutputs": false}),
    );
    template["inputs"][0] = json!({"outpoint": format!("{}.0", w.coins[0]),
        "unlockingScriptLength": 1, "inputDescription": "a coin"});
    let unsigned = page_call(w.addr, PAGE, "/createAction", template).await;
    assert_eq!(unsigned.0, 200, "{}", unsigned.3);
    let reference = unsigned.3["signableTransaction"]["reference"]
        .as_str()
        .unwrap()
        .to_string();
    assert_eq!(
        w.spends().await,
        vec![(None, Some(reference.clone()), 3000)]
    );

    let signed = page_call(
        w.addr,
        PAGE,
        "/signAction",
        json!({"reference": reference, "spends": {"0": {"unlockingScript": "51"}},
               "options": {"noSend": true}}),
    )
    .await;
    assert_eq!(signed.0, 200, "{}", signed.3);
    let txid = signed.3["txid"].as_str().unwrap().to_string();
    assert_eq!(w.spends().await, vec![(Some(txid), Some(reference), 3000)]);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_abort_deletes_the_spend_and_a_spend_a_broadcaster_took_keeps_it() {
    let w = World::new(3).await;
    w.ceiling(50_000).await;
    // A template, aborted by its reference.
    let mut template = w.spend(
        0,
        1000,
        &json!({"signAndProcess": false, "randomizeOutputs": false}),
    );
    template["inputs"][0] = json!({"outpoint": format!("{}.0", w.coins[0]),
        "unlockingScriptLength": 1, "inputDescription": "a coin"});
    let unsigned = page_call(w.addr, PAGE, "/createAction", template).await;
    let reference = unsigned.3["signableTransaction"]["reference"].clone();
    let aborted = page_call(
        w.addr,
        PAGE,
        "/abortAction",
        json!({"reference": reference}),
    )
    .await;
    assert_eq!(aborted.0, 200, "{}", aborted.3);
    assert!(
        w.spends().await.is_empty(),
        "the aborted template left the tally"
    );

    // A noSend action, aborted by its txid.
    let built = w.call(1, 2000).await;
    let txid = built.3["txid"].clone();
    let aborted = page_call(w.addr, PAGE, "/abortAction", json!({"reference": txid})).await;
    assert_eq!(aborted.0, 200, "{}", aborted.3);
    assert_eq!(w.tally().await, 0);

    // A spend a broadcaster took: the abort is refused and the row kept.
    let taken = w.call(2, 3000).await;
    let txid = taken.3["txid"].as_str().unwrap().to_string();
    sqlx::query("UPDATE transactions SET status = 'completed' WHERE txid = ?")
        .bind(&txid)
        .execute(w.wallet.storage().writer())
        .await
        .unwrap();
    let refused = page_call(w.addr, PAGE, "/abortAction", json!({"reference": txid})).await;
    assert_eq!(refused.0, 409, "{}", refused.3);
    assert_eq!(w.spends().await, vec![(Some(txid), None, 3000)]);
}
