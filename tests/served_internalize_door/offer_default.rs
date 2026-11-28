//! The offer rule under the cut's default (#659, lane L2a), served and
//! hermetic: what `POST /createAction` answers for a coin whose creator is
//! not mined, by the coin's kind, its creator's tracker word, the action's
//! word (`allowPending`) and the host's setting `SPEND_RECEIVED`.
//!
//! The stack's one function (bsv-stack-lean `lean/Wallet/OneFunction.lean`,
//! `offerOne`; the vectors' `offerable_one`) offers a received payment once
//! a broadcaster took it (`announced`, `seen`), with no word from the
//! action, and refuses one no broadcaster took (`unknown`, `built`) until
//! it is mined, whatever the action says. A host may keep the hold
//! (`SPEND_RECEIVED=hold`): a received unmined coin is then offered only
//! under the action's word, and never at `unknown` or `built`. Our own
//! change is offered from `built` on under both settings (the keeper's
//! answer: `liveFromBuilt`), never at `unknown`.
//!
//! What each wallet here can reach (#637). The served wallet's own
//! `Services` is the concrete type the server holds, built from the mainnet
//! options with no header service; `Wallet::new` hands it to the storage,
//! through which the toolbox's internalize door posts. Before any request
//! the storage's services are replaced by the toolbox's mock (plain, or
//! `Remembering`, which records each acceptance in the broadcast memory as
//! the concrete `Services` does), and each test reads the mock's post count.
//! Every `createAction` here is `noSend`, so no action of these wallets is
//! handed to the wallet's own services, and every non-loopback request of
//! the process goes to a closed local port (`seal_network`): the spend
//! gate's reads of an unmined creator ask no header, and a request that
//! did leave would be a fault at once. Nothing is sent.

use crate::remembering::Remembering;
use crate::seal::seal_network;
use crate::support::*;

use std::sync::Arc;

use axum::http::StatusCode;
use bsv_sdk::primitives::PrivateKey;
use bsv_wallet_cli::config::Config;
use bsv_wallet_cli::server::{self, ServerConfig};
use bsv_wallet_toolbox::{
    Chain, Services, ServicesOptions, Wallet, WalletServices, WalletStorageProvider,
};
use serde_json::{json, Value};

/// The host's settings under test: the default, and the hold.
const DEFAULT: &[(&str, &str)] = &[];
const HOLD: &[(&str, &str)] = &[("SPEND_RECEIVED", "hold")];

/// The reason of a received coin no broadcaster took.
fn not_taken(word: &str) -> String {
    format!(
        "{word}; received, not taken by a broadcaster: offered once a broadcaster takes it or \
         once mined"
    )
}

/// The reason of a received unmined coin held for want of the action's word.
const NO_WORD: &str = "depth 1: 0 blocks; received, no proof; offered under allowPending: received";

/// `support::served_rooted`, the server's settings read from `pairs` as the
/// environment at start gives them.
async fn served_under(
    root: &PrivateKey,
    parent: &str,
    posts: Arc<dyn WalletServices>,
    pairs: &[(&str, &str)],
) -> (WalletState, axum::Router) {
    seal_network();
    let roots = [(PARENT_HEIGHT, parent)];
    let storage = rooted_store(root, Some(&roots)).await;
    let services =
        Services::with_options(Chain::Main, ServicesOptions::mainnet()).expect("services");
    assert!(services.chaintracks.is_none(), "no header service");
    let wallet = Wallet::new(Some(root.clone()), storage, services)
        .await
        .expect("wallet");
    let state = server::make_wallet_state(wallet);
    WalletStorageProvider::set_services(state.storage(), posts);
    let config = ServerConfig {
        settings: Arc::new(Config::from_pairs(pairs).expect("the settings")),
        ..ServerConfig::admitting(["test.local"]).expect("an originator")
    };
    let router = server::make_router(state.clone(), config);
    (state, router)
}

/// A served wallet under `pairs`, paid 4,500 sats through its door. When
/// `taken`, its broadcaster's acceptance is recorded (`Remembering`) and the
/// payment reads `announced`; else the mock keeps no record of a take and
/// it reads `built`. The word is read back as the host serves it.
async fn paid_under(pairs: &[(&str, &str)], taken: bool) -> (WalletState, axum::Router, String) {
    let paid = Paid::new();
    let (wallet, app) = if taken {
        let posts = Arc::new(Remembering::new(accepting()));
        let served = served_under(&paid.root, &paid.parent, posts.clone(), pairs).await;
        admitted(&served.1, &paid.tx, paid.payment()).await;
        assert_eq!(posts.call_count("post_beef"), 1, "posted to the mock");
        served
    } else {
        let posts = Arc::new(accepting());
        let served = served_under(&paid.root, &paid.parent, posts.clone(), pairs).await;
        admitted(&served.1, &paid.tx, paid.payment()).await;
        assert_eq!(posts.call_count("post_beef"), 1, "posted to the mock");
        served
    };
    assert_eq!(balance(&wallet).await, 4_500);
    let (status, body) = get_from(&app, &format!("/tx/{}", paid.txid)).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let word = if taken { "announced" } else { "built" };
    assert_eq!(body["word"], word, "{body}");
    (wallet, app, format!("{}.0", paid.txid))
}

/// `POST /createAction` of 1,000 sats to another key, signed and not sent,
/// with `allow_pending` as the action's word (none when `Null`).
async fn create(app: &axum::Router, allow_pending: Value) -> (StatusCode, Value) {
    let mut body = json!({
        "description": "a spend under the offer rule",
        "outputs": [{
            "lockingScript": hex::encode(other_lock()),
            "satoshis": 1000,
            "outputDescription": "a payment",
        }],
        "options": { "noSend": true },
    });
    if !allow_pending.is_null() {
        body["allowPending"] = allow_pending;
    }
    let answer = post_to(app, "/createAction", body).await;
    // The status and, for a refusal, the coins held (a signed action's
    // bytes are not printed).
    println!("createAction -> {} {}", answer.0, answer.1["held"]);
    answer
}

/// The answer is the funds' refusal holding `outpoint` alone, for `reason`.
fn assert_held(answer: &(StatusCode, Value), outpoint: &str, word: &str, reason: &str) {
    let (status, body) = answer;
    assert_eq!(*status, StatusCode::PAYMENT_REQUIRED, "{body}");
    assert_eq!(body["code"], "INSUFFICIENT_FUNDS", "{body}");
    assert_eq!(body["offeredSatoshis"], 0, "{body}");
    let held = body["held"].as_array().expect("the held coins");
    assert_eq!(held.len(), 1, "{body}");
    assert_eq!(held[0]["outpoint"], outpoint, "{body}");
    assert_eq!(held[0]["word"], word, "{body}");
    assert_eq!(held[0]["reason"], reason, "{body}");
}

/// The answer is a signed action.
fn assert_spent(answer: &(StatusCode, Value)) {
    let (status, body) = answer;
    assert_eq!(*status, StatusCode::OK, "{body}");
    assert!(body["txid"].is_string(), "{body}");
}

// ---------------------------------------------------------------------------
// A received payment a broadcaster took
// ---------------------------------------------------------------------------

/// The default: a received payment a broadcaster took (`announced`) is
/// spent by an action that says no word.
#[tokio::test]
async fn a_received_payment_a_broadcaster_took_is_spent_with_no_word() {
    let (_wallet, app, _) = paid_under(DEFAULT, true).await;
    assert_spent(&create(&app, Value::Null).await);
}

/// The default: the word is still read and changes nothing for such a coin.
#[tokio::test]
async fn a_taken_payment_is_spent_with_the_word_too() {
    let (_wallet, app, _) = paid_under(DEFAULT, true).await;
    assert_spent(&create(&app, json!("received")).await);
}

/// The default: the scoped form still narrows. A scope naming another coin
/// than the wallet's one is the caller's error; a scope naming the coin
/// spends it.
#[tokio::test]
async fn the_scoped_word_names_the_taken_payment() {
    let (_wallet, app, coin) = paid_under(DEFAULT, true).await;
    let scope = json!({ "received": [coin.replace('.', ":")] });
    assert_spent(&create(&app, scope).await);
}

/// `SPEND_RECEIVED=hold` keeps the word's rule: the taken payment is held
/// from an action with no word, and spent under the word.
#[tokio::test]
async fn under_hold_a_taken_payment_needs_the_word() {
    let (_wallet, app, coin) = paid_under(HOLD, true).await;
    assert_held(&create(&app, Value::Null).await, &coin, "announced", NO_WORD);
    assert_spent(&create(&app, json!("received")).await);
}

// ---------------------------------------------------------------------------
// A received payment no broadcaster took
// ---------------------------------------------------------------------------

/// The default: a received payment at `built` (no record of a take) is held
/// until a broadcaster takes it or it is mined, with or without the word.
#[tokio::test]
async fn a_received_payment_no_broadcaster_took_is_held() {
    let (_wallet, app, coin) = paid_under(DEFAULT, false).await;
    let reason = not_taken("built");
    assert_held(&create(&app, Value::Null).await, &coin, "built", &reason);
    assert_held(
        &create(&app, json!("received")).await,
        &coin,
        "built",
        &reason,
    );
}

/// `SPEND_RECEIVED=hold`: the same payment at `built` WITH the word is
/// held, the hold stricter than the word's old rule here; without the word
/// it is held for want of the word, as before.
#[tokio::test]
async fn under_hold_a_built_payment_is_held_even_with_the_word() {
    let (_wallet, app, coin) = paid_under(HOLD, false).await;
    assert_held(
        &create(&app, json!("received")).await,
        &coin,
        "built",
        &not_taken("built"),
    );
    assert_held(&create(&app, Value::Null).await, &coin, "built", NO_WORD);
}

// ---------------------------------------------------------------------------
// Our own change at `unknown`
// ---------------------------------------------------------------------------

/// A served wallet under `pairs` whose one coin is its own unmined change
/// with no tracker row: the action that made it was signed through the
/// wallet library alone (`support::spend`, no door of this host), as a
/// store from before the create door wrote `built` holds one. Its outpoint.
async fn own_change_at_unknown(pairs: &[(&str, &str)]) -> (axum::Router, String) {
    let funding = Paid::new();
    let posts = Arc::new(Remembering::new(accepting()));
    let (wallet, app) = served_under(&funding.root, &funding.parent, posts, pairs).await;
    admitted(&app, &funding.tx, funding.payment()).await;
    let made = spend(&wallet, true).await.expect("the action");
    let txid = made.id();
    write(
        &wallet,
        "UPDATE transactions SET status = 'unproven' WHERE txid = ?",
        &txid,
    )
    .await;
    let vout: i64 = sqlx::query_scalar("SELECT vout FROM outputs WHERE txid = ? AND change = 1")
        .bind(&txid)
        .fetch_one(wallet.storage().pool())
        .await
        .expect("the change output");
    let (status, body) = get_from(&app, &format!("/tx/{txid}")).await;
    assert_eq!(status, StatusCode::NOT_FOUND, "no tracker row: {body}");
    (app, format!("{txid}.{vout}"))
}

/// Our own change whose creator reads `unknown` is held under every
/// setting, with or without the word: the rule for our own change is the
/// words from `built` on.
#[tokio::test]
async fn our_own_change_at_unknown_is_held_under_every_setting() {
    let reason = "unknown; own change not signed: never offered";
    for pairs in [DEFAULT, HOLD] {
        let (app, coin) = own_change_at_unknown(pairs).await;
        assert_held(&create(&app, Value::Null).await, &coin, "unknown", reason);
        assert_held(
            &create(&app, json!("received")).await,
            &coin,
            "unknown",
            reason,
        );
    }
}
