//! `POST /internalizeAction` as served under bsv-wallet-toolbox-rs
//! 0.14.0-beta.8.1 (the `internalizeAction` door read whole against the
//! reference): the port of the stable line's 0.18.6
//! (`tests/served_internalize_door.rs` on `main`), in this line's way. The
//! door's refusals a caller can reach are rows 257 to 262 of the wire
//! corpus; this file holds what a fixture of the `internalize` world cannot:
//! the answer with no chain tracker (no world runs without the header
//! stub), the stable line's storage door rows through this host's door, a
//! payment merged over a spent output, and the "rows written before" found
//! and taken out; then the restores of a row an earlier release
//! reclassified, each run through to a signed spend whose unlocking script
//! is run against the coin's P2PKH; and the stable line's create-and-sign
//! row (#634), a transaction one wallet's `createAction` and `signAction`
//! built, taken in by a second. The helpers are
//! `tests/support/served_internalize.rs`, in the shapes of the toolbox's
//! published tests for this release; the spend is `Wallet::create_action`
//! with `noSend` and the `underReceivedRule` label, as the toolbox's own
//! restore tests make it (a restored row's purpose is `receive` and its
//! transaction unproven; the served `createAction`'s spend gate reads the
//! header service, and the door under test is `internalizeAction`).
//!
//! This host configures no permissions manager (`git grep -n -i
//! permissions_manager -- src mcp/src` finds none), so the Hosts rows of a
//! manager are not reachable here and have no test.
//!
//! All vectors are synthetic: random throwaway keys, a coinbase-style BUMP
//! checked against a `TestChainTracker`, a mock broadcaster. Nothing is sent.

#[path = "../src/test_support/chain_tracker.rs"]
mod chain_tracker;
#[path = "../src/test_support/no_tracker.rs"]
#[allow(dead_code)]
mod no_tracker;
#[path = "support/served_internalize.rs"]
#[allow(dead_code)]
mod support;
use support::*;

/// The door's word and the take (#645, P1).
#[path = "served_internalize_door/door_word.rs"]
mod door_word;

use axum::http::StatusCode;
use bsv_sdk::primitives::PrivateKey;
use bsv_sdk::wallet::KeyDeriver;
use serde_json::json;

// ---------------------------------------------------------------------------
// The answers
// ---------------------------------------------------------------------------

/// No chain tracker on the storage and none on its services, as
/// `CHAINTRACKS_URL=off` builds them: every `internalizeAction` is refused,
/// a wallet payment and a basket insertion alike, 502 `SERVICE_ERROR`,
/// nothing written and nothing posted. The control: the same payment to a
/// wallet with a tracker is stored and its unproven subject posted once.
#[tokio::test]
async fn with_no_chain_tracker_every_internalize_is_refused() {
    let paid = Paid::new();
    let (wallet, app, posts) = served_untracked(&paid.root).await;
    let before = stored(&wallet).await;
    for outputs in [paid.payment(), vec![insertion(0, "tokens")]] {
        let (status, body) = internalize(&app, &paid.tx, outputs).await;
        // The toolbox's hold with no header source (#509): a header fault,
        // never a verdict about the payment, served by its class as 502
        // `SERVICE_ERROR` (the stable line's 0.13.4 answers 400
        // `VALIDATION_ERROR` with its own text).
        assert_eq!(status, StatusCode::BAD_GATEWAY, "{body}");
        assert_eq!(
            body,
            json!({
                "code": "SERVICE_ERROR",
                "message": "wallet error: Service error: ChainTracker verification failed: the \
                    merkle proofs of a BEEF the wallet did not sign need headers, and no header \
                    service is configured (no chain tracker, no chaintracks_url)"
            })
        );
    }
    assert_eq!(stored(&wallet).await, before, "nothing written");
    assert_eq!(posts.call_count("post_beef"), 0, "nothing posted");

    let (wallet, app, posts) = paid.served().await;
    admitted(&app, &paid.tx, paid.payment()).await;
    assert_eq!(balance(&wallet).await, 4_500);
    assert_eq!(posts.call_count("post_beef"), 1, "stored and posted");
}

/// A wallet payment merged over a custom row the store holds as spent (the
/// payment's row reclassified to `tokens` as an earlier release's basket
/// insertion left it, then marked spent by SQL, as the toolbox's
/// `a_payment_merge_over_a_spent_row_leaves_it_spent` does): the merge
/// rewrites the row to managed change of `default` (`P2PKH`, `change = 1`)
/// and leaves `spendable` and `spent_by` as the row holds them, out of the
/// balance. The toolbox's Fixed entry names an existing custom output, so
/// the test starts from one, not from a row already change. The control: the unspent custom row merged the same way becomes spendable
/// change and the balance counts it. The opening internalize posts the
/// unproven subject once; no merge posts again.
#[tokio::test]
async fn a_payment_merged_over_a_spent_output_leaves_it_spent() {
    let (held, after, balance, posts) = merged_over_custom(true).await;
    assert!(!held.0 && held.1.is_some(), "held as spent");
    assert_eq!(after, held, "left spent");
    assert_eq!(balance, 0, "out of the balance");
    assert_eq!(posts, 1, "no merge posts");

    let (held, after, balance, posts) = merged_over_custom(false).await;
    assert_eq!(held, (true, None));
    assert_eq!(after, (true, None), "spendable");
    assert_eq!(balance, 4_500, "counted");
    assert_eq!(posts, 1, "no merge posts");
}

/// A payment's row reclassified to a custom row, marked spent by SQL when
/// `mark_spent`, then the payment merged over it: the row as the merge
/// writes it (managed change of `default`, `P2PKH`, `change = 1`), then its
/// `spendable` and `spent_by` before and after the merge, the balance and
/// the posts.
async fn merged_over_custom(
    mark_spent: bool,
) -> ((bool, Option<i64>), (bool, Option<i64>), u64, usize) {
    let paid = Paid::new();
    let (wallet, app, posts) = paid.reclassified().await;
    if mark_spent {
        write(
            &wallet,
            "UPDATE outputs SET spendable = 0, spent_by = transaction_id \
             WHERE txid = ? AND vout = 0",
            &paid.txid,
        )
        .await;
    }
    let held = spent(&wallet, &paid.txid).await;
    assert_eq!(row(&wallet, &paid.txid, 0).await.1, "custom");

    admitted(&app, &paid.tx, paid.payment()).await;
    let (basket, kind, change, ..) = row(&wallet, &paid.txid, 0).await;
    assert_eq!(
        (basket.as_deref(), kind.as_str(), change),
        (Some("default"), "P2PKH", 1)
    );
    let after = spent(&wallet, &paid.txid).await;
    (
        held,
        after,
        balance(&wallet).await,
        posts.call_count("post_beef"),
    )
}

/// The storage door's rows on the stable line (a payment with an empty
/// derivation prefix or suffix, a sender that is 66 hex digits and not a
/// point), through this host's wallet door: refused by the argument
/// validator, nothing written or posted. The control: the payment made
/// well is stored.
#[tokio::test]
async fn the_storage_door_rows_are_refused_by_the_argument_validator() {
    let paid = Paid::new();
    let (wallet, app, posts) = paid.served().await;
    let before = stored(&wallet).await;
    let me = paid.sender.to_hex();
    let not_a_point = format!("02{}", "ff".repeat(32));
    for (output, message) in [
        (
            payment_of(0, "", SUFFIX, &me),
            "Validation error: The derivationPrefix parameter must be valid base64 string",
        ),
        (
            payment_of(0, PREFIX, "", &me),
            "Validation error: The derivationSuffix parameter must be valid base64 string",
        ),
        (
            payment_of(0, PREFIX, SUFFIX, &not_a_point),
            "Validation error: The senderIdentityKey parameter must be a valid compressed \
             secp256k1 public key",
        ),
    ] {
        refused(&app, &paid.tx, vec![output], message).await;
    }
    assert_eq!(stored(&wallet).await, before, "nothing written");
    assert_eq!(posts.call_count("post_beef"), 0, "nothing posted");
    admitted(&app, &paid.tx, paid.payment()).await;
}

// ---------------------------------------------------------------------------
// The rows written before
// ---------------------------------------------------------------------------

/// A spent output an earlier merge made spendable (`spendable = 1` with
/// `spent_by` set, reproduced by SQL): the balance counts it, the
/// toolbox's query finds it, and `POST /relinquishOutput` on `default`
/// takes it out of the balance (the toolbox's Hosts: 4500 before, 0 after).
#[tokio::test]
async fn a_spent_row_an_earlier_merge_made_spendable_is_found_and_relinquished() {
    let paid = Paid::new();
    let (wallet, app, _) = paid.stored().await;
    write(
        &wallet,
        "UPDATE outputs SET spent_by = transaction_id WHERE txid = ? AND vout = 0",
        &paid.txid,
    )
    .await;
    assert_eq!(balance(&wallet).await, 4_500, "counted");
    let found: Vec<(i64, String, i64)> = sqlx::query_as(
        "SELECT user_id, txid, vout FROM outputs WHERE spendable = 1 AND spent_by IS NOT NULL",
    )
    .fetch_all(wallet.storage().writer())
    .await
    .unwrap();
    assert_eq!(found.len(), 1);
    assert_eq!((found[0].1.as_str(), found[0].2), (paid.txid.as_str(), 0));
    relinquish(&app, &paid.txid, 0).await;
    assert_eq!(balance(&wallet).await, 0, "out of the balance");
}

/// What an internalize of `outputs: []` admitted before left (the
/// toolbox's Hosts: a transaction row with no `outputs` row), found by the
/// query the Hosts names: on this release the request is refused (row
/// 258), so a fresh wallet holds none.
#[tokio::test]
async fn an_empty_outputs_leaves_no_transaction_without_an_output() {
    let paid = Paid::new();
    let (wallet, app, posts) = paid.served().await;
    let before = stored(&wallet).await;
    refused(
        &app,
        &paid.tx,
        vec![],
        "Validation error: The outputs parameter must be at least one output to internalize \
         from the transaction",
    )
    .await;
    assert_eq!(stored(&wallet).await, before, "nothing written");
    assert_eq!(posts.call_count("post_beef"), 0, "nothing posted");
    let orphans: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM transactions t WHERE NOT EXISTS \
         (SELECT 1 FROM outputs o WHERE o.transaction_id = t.transaction_id)",
    )
    .fetch_one(wallet.storage().writer())
    .await
    .unwrap();
    assert_eq!(orphans, 0);
}

// ---------------------------------------------------------------------------
// The restores, through to a signed spend
// ---------------------------------------------------------------------------

/// The wallet's own change an earlier release reclassified, restored
/// through `POST /internalizeAction` as a wallet payment with the prefix
/// and suffix the row held and the wallet's own identity key as the
/// sender: change again in `default`, its sender that key, and a spend
/// that selects it signs. An empty sender, the stable line's 0.13.3
/// restore, is refused on this line, nothing stored; an action without the
/// `underReceivedRule` label does not select the restored coin while its
/// transaction is unproven.
#[tokio::test]
async fn own_change_restored_with_the_wallets_identity_key_signs() {
    let root = PrivateKey::random();
    let (wallet, app, _, bytes, txid, vout) = own_change(&root).await;
    let (_, _, _, prefix, suffix, _) = row(&wallet, &txid, vout).await;
    let (prefix, suffix) = (prefix.unwrap(), suffix.unwrap());
    reclassify(&wallet, &txid, vout).await;
    assert_eq!(balance(&wallet).await, 0);

    let before = stored(&wallet).await;
    let restore = |sender: &str| vec![payment_of(vout, &prefix, &suffix, sender)];
    refused(&app, &bytes, restore(""), EMPTY_SENDER).await;
    assert_eq!(stored(&wallet).await, before, "nothing stored");
    assert_eq!(row(&wallet, &txid, vout).await.1, "custom");

    let me = root.public_key().to_hex();
    admitted(&app, &bytes, restore(&me)).await;
    let restored = row(&wallet, &txid, vout).await;
    assert_eq!(restored.0.as_deref(), Some("default"));
    assert_eq!((restored.1.as_str(), restored.2), ("P2PKH", 1));
    assert_eq!(restored.5, Some(me), "the stored sender");
    assert!(balance(&wallet).await > 0, "counted");

    passed_over_without_the_label(&wallet).await;
    signs(&wallet, &txid, vout).await;
}

/// The toolbox's 0.14.0-beta.8.1 `### Hosts`: a restored row "is selected
/// as any received coin (an unproven one only under the action's
/// `underReceivedRule` label)". The same spend without the label is passed
/// over, the coin the wallet's only one (the stable line's 0.18.8 pins its
/// restores' answer the same way).
async fn passed_over_without_the_label(wallet: &WalletState) {
    let unselected = spend(wallet, false).await.expect_err("not selected");
    assert_eq!(unselected, "Insufficient funds: need 1005, have 0");
}

/// A spend that selects `txid.vout` signs it, its unlocking script checked.
async fn signs(wallet: &WalletState, txid: &str, vout: u32) {
    let tx = spend(wallet, true).await.expect("the coin signs");
    spends_the_coin(wallet, &tx, txid, vout).await;
}

/// A received payment reclassified the same way: with its prefix and
/// suffix under the wallet's own key the door derives another key and
/// refuses (`Derivation mismatch`, 400 `VALIDATION_ERROR`); with an empty
/// sender it is refused as a parameter (400 `VALIDATION_ERROR`); with the
/// sender the row held it is change again and a spend signs.
#[tokio::test]
async fn a_reclassified_received_payment_restored_with_its_sender_signs() {
    let paid = Paid::new();
    let (wallet, app, posts) = paid.reclassified().await;
    let before = stored(&wallet).await;

    let me = paid.root.public_key();
    let derived_hash = hex::encode(derived(&paid.root, &me, PREFIX, SUFFIX).hash160());
    let mismatch = format!(
        "Derivation mismatch for output 0: derived {derived_hash} != script {}",
        hex::encode(&paid.lock[3..23])
    );
    refused(&app, &paid.tx, paid.payment_naming(&me.to_hex()), &mismatch).await;
    refused(&app, &paid.tx, paid.payment_naming(""), EMPTY_SENDER).await;
    assert_eq!(stored(&wallet).await, before, "nothing stored");
    assert_eq!(row(&wallet, &paid.txid, 0).await.1, "custom");
    assert_eq!(posts.call_count("post_beef"), 1, "the opening's post alone");

    restored_and_signs(&paid, &wallet, &app).await;
}

/// `paid`'s row restored with the payment as its sender made it: change of
/// `default` again, its sender the one the row held, counted, and a spend
/// that selects it signs.
async fn restored_and_signs(paid: &Paid, wallet: &WalletState, app: &axum::Router) {
    admitted(app, &paid.tx, paid.payment()).await;
    let restored = row(wallet, &paid.txid, 0).await;
    assert_eq!(restored.0.as_deref(), Some("default"));
    assert_eq!(restored.5, Some(paid.sender.to_hex()), "the stored sender");
    assert_eq!(balance(wallet).await, 4_500);
    passed_over_without_the_label(wallet).await;
    signs(wallet, &paid.txid, 0).await;
}

/// A received payment written by this CLI's own flows (`fund`, `receive`,
/// `sync`, `split`, `gift-claim` and the tracker host's record of a claim
/// name the anyone key, `KeyDeriver::anyone_key()`, as the sender, with
/// `src/brc29.rs`'s default prefix and suffix), reclassified as the others
/// are: restored with that prefix, that suffix and the anyone key as the
/// sender, it is change again, counted, holds the anyone key as its sender,
/// and a spend that selects it signs. An empty sender is refused.
#[tokio::test]
async fn a_reclassified_payment_this_cli_wrote_restored_with_the_anyone_key_signs() {
    let (_, anyone) = KeyDeriver::anyone_key();
    let paid = Paid::from(&PrivateKey::random(), anyone, CLI_PREFIX, CLI_SUFFIX);
    let (wallet, app, _) = paid.reclassified().await;
    refused(&app, &paid.tx, paid.payment_naming(""), EMPTY_SENDER).await;
    assert_eq!(row(&wallet, &paid.txid, 0).await.1, "custom");

    restored_and_signs(&paid, &wallet, &app).await;
}

/// A change row whose stored sender is an empty string (the stable line's
/// door stores an empty sender as given, its 0.13.3 restore among them, as
/// can a sync from such a store; reproduced by SQL as the toolbox's test
/// does): on this release a spend that selects it signs, with the wallet as
/// its counterparty, and the row is not rewritten.
#[tokio::test]
async fn a_change_row_holding_an_empty_sender_signs() {
    let (wallet, _, _, _, txid, vout) = own_change(&PrivateKey::random()).await;
    sqlx::query("UPDATE outputs SET sender_identity_key = '' WHERE txid = ? AND vout = ?")
        .bind(&txid)
        .bind(vout)
        .execute(wallet.storage().writer())
        .await
        .unwrap();
    signs(&wallet, &txid, vout).await;
    assert_eq!(row(&wallet, &txid, vout).await.5.as_deref(), Some(""));
}

// ---------------------------------------------------------------------------
// An entry carried by txid alone that no BUMP proves (the stable line's
// 0.18.8 row)
// ---------------------------------------------------------------------------

/// `beef_paying`'s payment of one input with its parent carried by txid
/// alone and no BUMP in the BEEF: its `AtomicBEEF` and its parent's txid.
fn unproven_stub_paying(lock: &[u8]) -> (Vec<u8>, String) {
    let (parent, child) = paying(lock, 1);
    let mut beef = bsv_sdk::transaction::Beef::new();
    beef.merge_txid_only(parent.id());
    beef.merge_raw_tx(child.to_binary(), None).unwrap();
    (beef.to_binary_atomic(&child.id()).unwrap(), parent.id())
}

/// An entry carried by txid alone that no BUMP of the BEEF proves is
/// refused as an invalid BEEF (the toolbox's line: "refused as
/// `StubNotProven`"), 400 `VALIDATION_ERROR` naming the byte, nothing
/// written and nothing posted; the stable line answers it 400
/// `VALIDATION_ERROR`, `BEEF structure is invalid`.
#[tokio::test]
async fn a_parent_carried_by_txid_alone_with_no_bump_is_refused() {
    let root = PrivateKey::random();
    let sender = PrivateKey::random().public_key();
    let lock = p2pkh(&derived(&root, &sender, PREFIX, SUFFIX));
    let (tx, parent) = unproven_stub_paying(&lock);
    let (wallet, app, posts) = served(&root, Some(&parent)).await;
    let before = stored(&wallet).await;
    refused(
        &app,
        &tx,
        vec![payment_of(0, PREFIX, SUFFIX, &sender.to_hex())],
        "Invalid BEEF at byte 43: StubNotProven",
    )
    .await;
    assert_eq!(stored(&wallet).await, before, "nothing written");
    assert_eq!(balance(&wallet).await, 0, "the balance unmoved");
    assert_eq!(posts.call_count("post_beef"), 0, "nothing posted");
}

// ---------------------------------------------------------------------------
// The create-and-sign row (#634)
// ---------------------------------------------------------------------------

/// The stable line's last passing shape (main's
/// `tests/served_internalize_scripts.rs`), served on this line: a
/// transaction one wallet's own `createAction` (`signAndProcess: false`)
/// and `signAction` built and signed, paying a second wallet's derived
/// P2PKH, is taken in by that second wallet through its served
/// `POST /internalizeAction`: its scripts are run (the paying wallet's
/// P2PKH signature against its received coin, and that coin's own
/// unproven transaction over its proven parent) and pass. 200, stored, the
/// receiver's mock posted once.
#[tokio::test]
async fn a_transaction_a_wallets_own_create_and_sign_built_is_stored_by_another() {
    // The sender holds one received coin of 4,500, taken in through its door.
    let funding = Paid::new();
    let (sender, _sender_app, _) = funding.stored().await;
    assert_eq!(balance(&sender).await, 4_500);

    // The receiver's key for the sender's remittance.
    let receiver = PrivateKey::random();
    let sender_identity = funding.root.public_key();
    let lock = p2pkh(&derived(&receiver, &sender_identity, PREFIX, SUFFIX));
    let (tx, txid) = create_and_sign(&sender, &lock, 300).await;

    let (wallet, app, posts) = served(&receiver, Some(&funding.parent)).await;
    let (status, body) = internalize(
        &app,
        &tx,
        vec![payment_of(0, PREFIX, SUFFIX, &sender_identity.to_hex())],
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["accepted"], true, "{body}");
    assert_eq!(balance(&wallet).await, 300, "stored");
    let kept: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM outputs WHERE txid = ? AND vout = 0")
        .bind(&txid)
        .fetch_one(wallet.storage().pool())
        .await
        .unwrap();
    assert_eq!(kept, 1, "the payment's row");
    assert_eq!(posts.call_count("post_beef"), 1, "posted once");
}
