//! 0.11.0 (#29) over the served router: `GET /tx/{txid}` for a mined, a
//! pending and an unknown transaction; the frozen record in `/chain-feed`;
//! a named `createAction` input the gate holds, refused. Nothing reaches
//! the network: the rows are written into the store directly and no route
//! here reads a header.

use bsv_sdk::primitives::PrivateKey;
use bsv_wallet_cli::server::{self, ServerConfig};
use bsv_wallet_cli::tracker_host::{self, SystemClock, TickOptions};
use bsv_wallet_toolbox::{
    Chain, Services, ServicesOptions, StorageSqlx, Wallet, WalletStorageWriter,
};
use reqwest::Client;
use serde_json::{json, Value};
use tempfile::TempDir;

const MINED: &str = "1111111111111111111111111111111111111111111111111111111111111111";
const PENDING: &str = "2222222222222222222222222222222222222222222222222222222222222222";
const UNKNOWN: &str = "3333333333333333333333333333333333333333333333333333333333333333";
const COIN: &str = "4444444444444444444444444444444444444444444444444444444444444444";
const ROOT: &str = "5555555555555555555555555555555555555555555555555555555555555555";
const STALE: &str = "6666666666666666666666666666666666666666666666666666666666666666";

async fn served() -> (String, Client, TempDir) {
    let tmp = TempDir::new().unwrap();
    let storage = StorageSqlx::open(tmp.path().join("w.db").to_str().unwrap())
        .await
        .unwrap();
    let key = PrivateKey::random();
    storage
        .migrate("tx-word-route", &key.public_key().to_hex())
        .await
        .unwrap();
    storage.make_available().await.unwrap();
    let (user, _) = storage
        .find_or_insert_user(&key.public_key().to_hex())
        .await
        .unwrap();
    let basket = storage
        .find_or_create_default_basket(user.user_id)
        .await
        .unwrap()
        .basket_id;
    let pool = storage.pool().clone();

    // The tracker's rows (the table made by the first read).
    tracker_host::stored_word(&storage, UNKNOWN).await.unwrap();
    for (txid, word, height) in [(MINED, "mined", Some(900i64)), (PENDING, "seen", None)] {
        sqlx::query("INSERT INTO tracker_states (txid, word, height, reask) VALUES (?, ?, ?, ?)")
            .bind(txid)
            .bind(word)
            .bind(height)
            .bind((word == "seen").then_some("age"))
            .execute(&pool)
            .await
            .unwrap();
    }
    // A transaction a reorg took out of its block (ruling 7b (b)): `stale`,
    // its root gone, its hints `accepted` then `seen`.
    sqlx::query("INSERT INTO tracker_states (txid, word, hints, reask) VALUES (?, 'stale', ?, ?)")
        .bind(STALE)
        .bind(
            json!([
                {"source": "arcade|accepted", "status": "accepted", "observed": 1},
                {"source": "arcade|seen", "status": "seen", "observed": 2},
            ])
            .to_string(),
        )
        .bind("reorg@900")
        .execute(&pool)
        .await
        .unwrap();
    let now = chrono::Utc::now();
    sqlx::query(
        "INSERT INTO proven_txs (txid, height, idx, block_hash, merkle_root, merkle_path, raw_tx, created_at, updated_at) \
         VALUES (?, 900, 0, ?, ?, X'00', X'01000000', ?, ?)",
    )
    .bind(MINED)
    .bind("bb".repeat(32))
    .bind(ROOT)
    .bind(now)
    .bind(now)
    .execute(&pool)
    .await
    .unwrap();

    // One coin of ours, then frozen by the feed.
    let tx_row = sqlx::query(
        "INSERT INTO transactions (user_id, status, reference, is_outgoing, satoshis, version, lock_time, description, txid, raw_tx, created_at, updated_at) \
         VALUES (?, 'completed', 'ref-coin', 0, 0, 1, 0, 'coin', ?, X'01000000', ?, ?)",
    )
    .bind(user.user_id)
    .bind(COIN)
    .bind(now)
    .bind(now)
    .execute(&pool)
    .await
    .unwrap()
    .last_insert_rowid();
    sqlx::query(
        "INSERT INTO outputs (user_id, transaction_id, basket_id, vout, satoshis, locking_script, txid, type, spendable, change, provided_by, purpose, output_description, created_at, updated_at) \
         VALUES (?, ?, ?, 0, 5000, X'76a914dbc0a7c84983c5bf199b7b2d41b3acf0408ee5aa88ac', ?, 'P2PKH', 1, 1, 'storage', 'change', 'c', ?, ?)",
    )
    .bind(user.user_id)
    .bind(tx_row)
    .bind(basket)
    .bind(COIN)
    .bind(now)
    .bind(now)
    .execute(&pool)
    .await
    .unwrap();

    let services = Services::with_options(Chain::Main, ServicesOptions::mainnet()).unwrap();
    // A freeze names no tracked row: no header is read.
    tracker_host::deliver_chain_event(
        &storage,
        &services,
        &SystemClock,
        &TickOptions::from_env(),
        1,
        bsv_tracker::ChainEvent::Frozen {
            outpoint: bsv_tracker::Outpoint {
                txid: COIN.to_string(),
                vout: 0,
            },
        },
    )
    .await
    .unwrap();

    let wallet = Wallet::new(Some(key), storage, services).await.unwrap();
    let app = server::make_router(server::make_wallet_state(wallet), ServerConfig::default());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.ok();
    });
    (format!("http://{addr}"), Client::new(), tmp)
}

async fn get(client: &Client, url: String) -> (u16, Value) {
    let resp = client.get(url).send().await.unwrap();
    (resp.status().as_u16(), resp.json().await.unwrap())
}

#[tokio::test]
async fn the_route_serves_the_stored_word_and_its_evidence() {
    let (base, client, _tmp) = served().await;

    let (status, mined) = get(&client, format!("{base}/tx/{}", MINED.to_uppercase())).await;
    assert_eq!(status, 200, "{mined}");
    assert_eq!(
        mined,
        json!({"txid": MINED, "word": "mined", "height": 900, "reask": null,
               "evidence": {"root": ROOT, "height": 900}})
    );

    let (status, pending) = get(&client, format!("{base}/tx/{PENDING}")).await;
    assert_eq!(status, 200, "{pending}");
    assert_eq!(
        pending,
        json!({"txid": PENDING, "word": "seen", "height": null, "reask": "age", "evidence": null})
    );

    let (status, unknown) = get(&client, format!("{base}/tx/{UNKNOWN}")).await;
    assert_eq!(status, 404, "{unknown}");
    let (status, _) = get(&client, format!("{base}/tx/nothex")).await;
    assert_eq!(status, 400);
}

/// Ruling 7b (b) of 2026-10-20 20:1xZ (bsv-stack-lean
/// `docs/DECISIONS-2026-10-19.md` section 8c), the draft
/// `corpus/scenarios/2026-10-20-reorg-below-the-user.md` step 3: a `stale`
/// word is served as `stale` with its tier (the word its hints give, its
/// status before the proof: "confirming") and the re-ask the reorg left.
#[tokio::test]
async fn the_route_serves_a_stale_word_with_its_tier_and_its_reask() {
    let (base, client, _tmp) = served().await;
    let (status, stale) = get(&client, format!("{base}/tx/{STALE}")).await;
    assert_eq!(status, 200, "{stale}");
    assert_eq!(
        stale,
        json!({"txid": STALE, "word": "stale", "tier": "seen", "height": null,
               "reask": "reorg@900", "evidence": null})
    );
}

#[tokio::test]
async fn the_route_is_behind_the_wallet_auth() {
    let tmp = TempDir::new().unwrap();
    let storage = StorageSqlx::open(tmp.path().join("w.db").to_str().unwrap())
        .await
        .unwrap();
    let key = PrivateKey::random();
    storage
        .migrate("tx-word-auth", &key.public_key().to_hex())
        .await
        .unwrap();
    storage.make_available().await.unwrap();
    let services = Services::with_options(Chain::Main, ServicesOptions::mainnet()).unwrap();
    let wallet = Wallet::new(Some(key), storage, services).await.unwrap();
    let config = ServerConfig {
        auth_token: Some("secret".into()),
        ..ServerConfig::default()
    };
    let app = server::make_router(server::make_wallet_state(wallet), config);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.ok();
    });
    let client = Client::new();
    for path in ["chain-feed", &format!("tx/{MINED}")] {
        let resp = client
            .get(format!("http://{addr}/{path}"))
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status().as_u16(), 401, "{path}");
    }
}

#[tokio::test]
async fn the_frozen_record_is_counted_and_a_named_frozen_coin_is_refused() {
    let (base, client, _tmp) = served().await;

    let (status, feed) = get(&client, format!("{base}/chain-feed")).await;
    assert_eq!(status, 200, "{feed}");
    assert_eq!(feed["frozenCount"], 1);
    assert_eq!(feed["cursor"], "1");

    let resp = client
        .post(format!("{base}/createAction"))
        .header("Origin", "http://localhost")
        .json(&json!({
            "description": "spend a frozen coin",
            "inputs": [{"outpoint": format!("{COIN}.0"), "unlockingScriptLength": 107,
                        "inputDescription": "named"}],
            "outputs": [{"lockingScript": "76a914dbc0a7c84983c5bf199b7b2d41b3acf0408ee5aa88ac",
                         "satoshis": 1000, "outputDescription": "out"}],
            "allowPending": "received",
        }))
        .send()
        .await
        .unwrap();
    let status = resp.status().as_u16();
    let body: Value = resp.json().await.unwrap();
    assert_eq!(status, 403, "{body}");
    assert_eq!(body["code"], "INPUT_NOT_OFFERED");
    assert_eq!(body["held"][0]["outpoint"], format!("{COIN}.0"));
    assert_eq!(body["held"][0]["reason"], "frozen: never offered");
    assert_eq!(body["held"][0]["word"], "frozen");
}
