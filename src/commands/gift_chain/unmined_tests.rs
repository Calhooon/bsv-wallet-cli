//! `gift-claim` of a gift whose deposit is not mined yet (#638): the
//! command runs through its own code path (`gift_claim::run`) over the
//! toolbox's mock services, whose broadcaster takes every post, whose
//! header service checks every root, and which hold no proof of the
//! deposit when the claim is made and the claim's own proof afterwards.

use super::genesis_tests::context;
use super::tests::{opts, spendable, storage, At, Claim};
use super::*;
use crate::context::WalletContext;
use crate::test_support::txid_of;
use bsv_wallet_toolbox::services::mock::{MockResponse, MockWalletServices};
use bsv_wallet_toolbox::services::{BlockHeader, GetMerklePathResult, GetRawTxResult};
use bsv_wallet_toolbox::Chain;

/// The height the claim is mined at; the mock's tip is five above it.
const CLAIM_HEIGHT: u32 = 900_005;

/// Services that hand out `gift`'s deposit, have no proof of it when the
/// claim is made (the one proof request of the command), and answer every
/// later proof request with the claim's own proof at [`CLAIM_HEIGHT`].
fn unmined_then_mined(gift: &Claim) -> MockWalletServices {
    let no_proof = GetMerklePathResult {
        name: Some("Services".to_string()),
        merkle_path: None,
        header: None,
        error: None,
        notes: vec![],
    };
    let claim_proof = GetMerklePathResult {
        merkle_path: MerklePath::from_coinbase_txid(&txid_of(&gift.claim), CLAIM_HEIGHT)
            .to_hex()
            .ok(),
        ..no_proof.clone()
    };
    MockWalletServices::builder()
        .height(CLAIM_HEIGHT + 5)
        .get_raw_tx_response(MockResponse::Success(GetRawTxResult {
            name: "Services".to_string(),
            txid: txid_of(&gift.deposit),
            raw_tx: Some(gift.deposit.clone()),
            error: None,
            could_not_look: false,
        }))
        .get_merkle_path_response(MockResponse::Sequence(vec![
            MockResponse::Success(no_proof),
            MockResponse::Success(claim_proof),
        ]))
        .build()
}

/// The claim is mined: the header service's header at [`CLAIM_HEIGHT`]
/// carries the root of the claim's proof (the claim alone in its block).
fn mine_the_claim(services: &MockWalletServices, claim_txid: &str) {
    services.set_header_for_height(BlockHeader {
        version: 1,
        previous_hash: "00".repeat(32),
        merkle_root: claim_txid.to_string(),
        time: 1_700_000_000,
        bits: 0x1d00_ffff,
        nonce: 0,
        hash: String::new(),
        height: CLAIM_HEIGHT,
    });
}

/// A wallet of the gift's recipient whose services hold no proof of the
/// deposit, `json` its output mode.
async fn claimer(gift: &Claim, json: bool) -> WalletContext<MockWalletServices> {
    let mut ctx = context(
        &gift.root,
        storage().await,
        unmined_then_mined(gift),
        Chain::Main,
    )
    .await;
    ctx.json_output = json;
    ctx
}

/// What the wallet's own storage holds of the claim: the tracker's row
/// (its word, whether it carries the wallet's pending record and a BEEF to
/// announce again), the transactions the store holds and the spendable
/// satoshis.
async fn held(ctx: &WalletContext<MockWalletServices>, claim_txid: &str) -> String {
    let storage = ctx.wallet.storage();
    let row: Option<(String, Option<String>, Option<Vec<u8>>)> = sqlx::query_as(
        "SELECT word, on_accept, reannounce_beef FROM tracker_states WHERE txid = ?",
    )
    .bind(claim_txid)
    .fetch_optional(storage.pool())
    .await
    .unwrap();
    let transactions: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM transactions")
        .fetch_one(storage.pool())
        .await
        .unwrap();
    let outputs: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM outputs")
        .fetch_one(storage.pool())
        .await
        .unwrap();
    let row = row.map(|(word, record, beef)| {
        format!(
            "word={word} record={} beef={}",
            if record.is_some() { "held" } else { "none" },
            if beef.is_some() { "held" } else { "none" }
        )
    });
    format!(
        "row: {row:?}; transactions: {transactions}; outputs: {outputs}; spendable: {}",
        spendable(storage).await
    )
}

/// One pass of the tracker as `tracker-tick`, `serve` and `daemon` run
/// it: the pass, then the records it owes the wallet.
async fn pass(ctx: &WalletContext<MockWalletServices>, at: u64) -> crate::tracker_host::TickReport {
    let mut report = tracker_host::tick(
        ctx.wallet.storage(),
        ctx.wallet.services(),
        &At(at),
        &opts(),
    )
    .await
    .unwrap();
    tracker_host::record_accepted(&ctx.wallet, &mut report).await;
    report
}

/// Seconds since the epoch, as the command's own clock reads them.
fn now() -> u64 {
    chrono::Utc::now().timestamp().cast_unsigned()
}

/// THE TEST THAT DECIDES (#638): a gift whose deposit has no proof yet,
/// claimed. The claim is posted once and the command exits zero; the door
/// declines the claim's record while the deposit has no proof (the BEEF
/// in hand carries the deposit without its parents), so nothing is in the
/// balance yet. Then the claim is mined and one pass of the tracker runs:
/// the claimer did nothing more, and the claim's two outputs (the gift and
/// the change) are the wallet's balance.
#[tokio::test]
async fn a_claim_of_an_unmined_deposit_is_recorded_by_the_pass_once_the_claim_is_mined() {
    let gift = Claim::new();
    let claim_txid = txid_of(&gift.claim);
    let ctx = claimer(&gift, true).await;
    let services = ctx.wallet.services();

    let answer = crate::commands::gift_claim::run(&ctx, &txid_of(&gift.deposit), false).await;
    let after_claim = format!(
        "answer: {answer:?}; posts: {}; proofs asked: {}; {}",
        services.call_count("post_beef"),
        services.call_count("get_merkle_path"),
        held(&ctx, &claim_txid).await
    );
    println!("after the claim: {after_claim}");
    assert!(answer.is_ok(), "{after_claim}");
    assert_eq!(services.call_count("post_beef"), 1, "{after_claim}");
    assert_eq!(spendable(ctx.wallet.storage()).await, 0, "{after_claim}");

    // The claim is mined; the claimer runs nothing but the pass.
    mine_the_claim(services, &claim_txid);
    let report = pass(&ctx, now() + 601).await;
    let after_pass = format!(
        "mined: {:?}; stored: {:?}; faults: {:?}; posts: {}; {}",
        report.mined,
        report.stored,
        report.faults,
        services.call_count("post_beef"),
        held(&ctx, &claim_txid).await
    );
    println!("after the pass: {after_pass}");
    assert_eq!(report.mined, vec![claim_txid.clone()], "{after_pass}");
    assert_eq!(
        spendable(ctx.wallet.storage()).await.cast_unsigned(),
        gift.plan.covenant.amount + gift.plan.change,
        "the claim's coin is recorded once the claim is mined: after the claim: {after_claim}; after the pass: {after_pass}"
    );
}

/// What the door answers when the claim's record is handed to it while
/// the deposit has no proof: the toolbox's `internalizeAction` refuses the
/// BEEF (the deposit's input names a transaction the BEEF does not carry)
/// as an invalid parameter, and writes nothing.
#[tokio::test]
async fn the_door_declines_the_record_of_a_claim_whose_deposit_has_no_proof() {
    use bsv_sdk::wallet::WalletInterface;

    let gift = Claim::new();
    let ctx = claimer(&gift, true).await;
    let outcome = announce_claim(
        ctx.wallet.storage(),
        ctx.wallet.services(),
        &At(now()),
        &opts(),
        &gift.deposit,
        &gift.claim,
        &gift.pay,
        gift.lock,
        false,
    )
    .await
    .unwrap();
    assert_eq!(outcome.word, "announced");
    let record = outcome.record.expect("an accepted claim has a record");
    assert_eq!(record.outputs.len(), 2, "the gift and the change");

    let refusal = ctx
        .wallet
        .internalize_action(record, Some("bsv-wallet-cli"))
        .await
        .expect_err("the door declines the record");
    println!("the door: {refusal}");
    assert!(
        matches!(
            refusal,
            bsv_sdk::Error::WalletRefusal {
                class: bsv_sdk::WalletRefusalClass::InvalidParameter,
                ..
            }
        ),
        "{refusal:?}"
    );
    assert_eq!(
        refusal.to_string(),
        "Invalid BEEF at byte 48: InputNamesNoElement"
    );
    assert_eq!(ctx.wallet.services().call_count("post_beef"), 1);
    for table in ["transactions", "outputs"] {
        let count: i64 = sqlx::query_scalar(&format!("SELECT COUNT(*) FROM {table}"))
            .fetch_one(ctx.wallet.storage().pool())
            .await
            .unwrap();
        assert_eq!(count, 0, "the refusal wrote {table}");
    }
}

/// THE HINT (#638), in text: a claim posted and not recorded yet says
/// what happened (the claim is posted, its coin is not in the balance, the
/// deposit had no checked proof, the door's own words) and the commands
/// that take the coin in once the claim is mined, one per output of the
/// claim that pays the wallet. Exit zero: the claim is on the network.
#[tokio::test]
async fn the_text_output_says_what_happened_and_the_commands_that_take_the_coin_in() {
    let gift = Claim::new();
    let (claim, deposit) = (txid_of(&gift.claim), txid_of(&gift.deposit));
    let ctx = claimer(&gift, false).await;
    let report = crate::commands::gift_claim::claim(&ctx, &deposit, false)
        .await
        .expect("the claim is posted: exit zero");
    assert_eq!(ctx.wallet.services().call_count("post_beef"), 1);
    assert_eq!(spendable(ctx.wallet.storage()).await, 0);
    assert_eq!(
        report.text(),
        format!(
            "🔓 Gift claim broadcast
   amount:     5000 sats → your address
   fee:        156 sats (change 844 sats back to you)
   unlocks:    2020-09-13T12:26:40+00:00
   claim txid: {claim}
   word:       announced (the tracker's)
   wallet:     NOT recorded yet. The claim is posted and its coin is not in your balance:
               no checked proof of the gift's deposit {deposit} was to be had, so the wallet could not record the claim (Invalid BEEF at byte 48: InputNamesNoElement).
               Once the claim is mined, take it in with:
                 bsv-wallet receive {claim}
                 bsv-wallet receive {claim} --vout 1

   `bsv-wallet tracker-tick` (or `serve`) asks for its proof; the word turns
   `mined` when the proof checks against the header service."
        )
    );
}

/// THE HINT (#638), in JSON: the same as fields a caller can read.
/// `recorded` is false and `recordError` the door's words, as before;
/// `recordHint` says what happened and what takes the coin in, and
/// `receiveCommands` lists the commands, one per output that pays the
/// wallet.
#[tokio::test]
async fn the_json_output_carries_the_hint_and_the_commands_as_fields() {
    let gift = Claim::new();
    let (claim, deposit) = (txid_of(&gift.claim), txid_of(&gift.deposit));
    let ctx = claimer(&gift, true).await;
    let report = crate::commands::gift_claim::claim(&ctx, &deposit, false)
        .await
        .expect("the claim is posted: exit zero");
    assert_eq!(
        report.json(),
        serde_json::json!({
            "claimTxid": claim,
            "depositTxid": deposit,
            "amount": 5000,
            "fee": 156,
            "change": 844,
            "lockUntil": 1_600_000_000,
            "broadcast": "MockProvider",
            "word": "announced",
            "reask": null,
            "heldUntil": null,
            "notFinal": null,
            "recorded": false,
            "recordError": "Invalid BEEF at byte 48: InputNamesNoElement",
            "recordHint": format!(
                "the claim is posted and its coin is not in your balance yet: no checked proof of the gift's deposit {deposit} was to be had, so the wallet could not record the claim (Invalid BEEF at byte 48: InputNamesNoElement); once the claim is mined, run `bsv-wallet receive {claim}` and `bsv-wallet receive {claim} --vout 1`"
            ),
            "receiveCommands": [
                format!("bsv-wallet receive {claim}"),
                format!("bsv-wallet receive {claim} --vout 1"),
            ],
        })
    );
}

/// A claim the wallet recorded at once (its deposit mined) carries no
/// hint and no command, in either mode; a claim held as not final says
/// when it is recorded and keeps the commands for a claim that is mined
/// and still not in the balance.
#[tokio::test]
async fn a_recorded_claim_carries_no_hint_and_a_held_one_says_when_it_is_recorded() {
    use crate::commands::gift_claim::Recorded;

    let gift = Claim::new();
    let claim = txid_of(&gift.claim);
    let ctx = context(
        &gift.root,
        storage().await,
        super::genesis_tests::proving(&gift.deposit, 900_000),
        Chain::Main,
    )
    .await;
    let mut report = crate::commands::gift_claim::claim(&ctx, &txid_of(&gift.deposit), false)
        .await
        .unwrap();
    assert_eq!(report.recorded, Recorded::Yes);
    let json = report.json();
    assert_eq!(json["recorded"], true);
    assert_eq!(json["recordError"], serde_json::Value::Null);
    assert_eq!(json["recordHint"], serde_json::Value::Null);
    assert_eq!(json["receiveCommands"], serde_json::json!([]));
    assert!(
        report
            .text()
            .contains("   wallet:     recorded; 5844 sats are in your balance\n"),
        "{}",
        report.text()
    );
    assert!(!report.text().contains("receive"), "{}", report.text());

    // The same claim as `--force` leaves it when a broadcaster calls it
    // not final: nothing is recorded before one takes it.
    report.recorded = Recorded::WhenTaken;
    report.outcome.not_final = Some("Arcade: 476 non-final".to_string());
    let json = report.json();
    assert_eq!(json["recorded"], false);
    assert_eq!(json["recordError"], serde_json::Value::Null);
    assert_eq!(
        json["recordHint"],
        format!(
            "the claim is held as not final and nothing is recorded before a broadcaster takes it; if it is mined and still not in your balance, run `bsv-wallet receive {claim}` and `bsv-wallet receive {claim} --vout 1`"
        )
    );
    assert_eq!(
        json["receiveCommands"],
        serde_json::json!([
            format!("bsv-wallet receive {claim}"),
            format!("bsv-wallet receive {claim} --vout 1"),
        ])
    );
    assert!(
        report
            .text()
            .contains("   wallet:     recorded when a broadcaster takes the claim\n"),
        "{}",
        report.text()
    );
}
