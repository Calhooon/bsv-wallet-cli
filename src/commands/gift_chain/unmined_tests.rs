//! `gift-claim` of a gift whose deposit is not mined yet (#638): the
//! command runs through its own code path (`gift_claim::run`) over the
//! toolbox's mock services, whose broadcaster takes every post, whose
//! header service checks every root, and which hold no proof of the
//! deposit when the claim is made and the claim's own proof afterwards.
//!
//! Nothing here can reach a broadcaster or an explorer: every wallet is
//! built on `MockWalletServices` (no HTTP client), and the one courier a
//! test asks (`receive` by hand) is a fixture on loopback, the one rung of
//! its ladder at that fixture's base (#637).

use super::genesis_tests::context;
use super::tests::{opts, spendable, storage, At, Claim};
use super::*;
use crate::commands::gift_claim::{ClaimReport, Recorded};
use crate::context::WalletContext;
use crate::test_support::txid_of;
use bsv_wallet_toolbox::services::mock::{
    MockResponse, MockWalletServices, MockWalletServicesBuilder,
};
use bsv_wallet_toolbox::services::{BlockHeader, GetMerklePathResult, GetRawTxResult};
use bsv_wallet_toolbox::Chain;

mod held_record;

/// A wallet of a gift's recipient, on the mock services.
type Claimer = WalletContext<MockWalletServices>;

/// The height the claim is mined at; the mock's tip is five above it.
const CLAIM_HEIGHT: u32 = 900_005;

/// The services' answer to a proof request: `raw`'s one-leaf proof at
/// `height`, or no proof.
fn proof_of(raw: &[u8], height: Option<u32>) -> MockResponse<GetMerklePathResult> {
    MockResponse::Success(GetMerklePathResult {
        name: Some("Services".to_string()),
        merkle_path: height.and_then(|at| {
            MerklePath::from_coinbase_txid(&txid_of(raw), at)
                .to_hex()
                .ok()
        }),
        header: None,
        error: None,
        notes: vec![],
    })
}

/// Services that hand out `gift`'s deposit, have no proof of it when the
/// claim is made (the one proof request of the command), and answer every
/// later proof request with the claim's own proof at [`CLAIM_HEIGHT`].
fn unmined_then_mined(gift: &Claim) -> MockWalletServicesBuilder {
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
            proof_of(&gift.deposit, None),
            proof_of(&gift.claim, Some(CLAIM_HEIGHT)),
        ]))
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
async fn claimer(gift: &Claim, json: bool) -> Claimer {
    let services = unmined_then_mined(gift).build();
    let mut ctx = context(&gift.root, storage().await, services, Chain::Main).await;
    ctx.json_output = json;
    ctx
}

/// `gift-claim <deposit txid>` through `ctx`, its report in hand.
async fn claim_gift(ctx: &Claimer, gift: &Claim) -> Result<ClaimReport> {
    crate::commands::gift_claim::claim(ctx, &txid_of(&gift.deposit), false).await
}

/// A gift whose deposit has no proof, claimed in the output mode `json`:
/// the gift, the claimer's wallet and the command's report. The claim is
/// posted once (exit zero) and not recorded yet.
async fn claim_unmined(json: bool) -> (Claim, Claimer, ClaimReport) {
    let gift = Claim::new();
    let ctx = claimer(&gift, json).await;
    let report = claim_gift(&ctx, &gift).await.expect("posted: exit zero");
    assert!(matches!(report.recorded, Recorded::NotYet { .. }));
    assert_eq!(ctx.wallet.services().call_count("post_beef"), 1);
    assert_eq!(spendable(ctx.wallet.storage()).await, 0);
    (gift, ctx, report)
}

/// [`claim_unmined`]: the gift, the claim's txid and the claimer's wallet.
async fn claimed() -> (Claim, String, Claimer) {
    let (gift, ctx, _) = claim_unmined(true).await;
    let claim_txid = txid_of(&gift.claim);
    (gift, claim_txid, ctx)
}

/// What the wallet's own storage holds of the claim: the tracker's row
/// (its word, whether it carries the wallet's pending record and a BEEF to
/// announce again), the transactions the store holds and the spendable
/// satoshis.
async fn held(ctx: &Claimer, claim_txid: &str) -> String {
    let storage = ctx.wallet.storage();
    // A claim refused before the tracker heard of it made no table yet:
    // the tracker's own read makes its tables.
    tracker_host::stored_word(storage, claim_txid)
        .await
        .unwrap();
    let row: Option<(String, Option<String>, Option<Vec<u8>>)> = sqlx::query_as(
        "SELECT word, on_accept, reannounce_beef FROM tracker_states WHERE txid = ?",
    )
    .bind(claim_txid)
    .fetch_optional(storage.pool())
    .await
    .unwrap();
    let mut counts = Vec::new();
    for table in ["transactions", "outputs"] {
        let count: i64 = sqlx::query_scalar(&format!("SELECT COUNT(*) FROM {table}"))
            .fetch_one(storage.pool())
            .await
            .unwrap();
        counts.push(format!("{table}: {count}"));
    }
    let row = row.map(|(word, record, beef)| {
        let kept = |held: bool| if held { "held" } else { "none" };
        format!(
            "word={word} record={} beef={}",
            kept(record.is_some()),
            kept(beef.is_some())
        )
    });
    format!(
        "row: {row:?}; {}; spendable: {}",
        counts.join("; "),
        spendable(storage).await
    )
}

/// One pass of the tracker as `tracker-tick`, `serve` and `daemon` run
/// it: the pass, then the records it owes the wallet.
async fn pass(ctx: &Claimer, at: u64) -> tracker_host::TickReport {
    let (storage, services) = (ctx.wallet.storage(), ctx.wallet.services());
    let mut report = tracker_host::tick(storage, services, &At(at), &opts())
        .await
        .unwrap();
    tracker_host::record_accepted(storage, &ctx.wallet, &mut report).await;
    report
}

/// The pass made the claim's record: its line says so, and the wallet
/// holds the claim's one transaction, its two outputs (the gift and the
/// change), their sum the balance, and no pending record on the row.
async fn assert_made(report: &tracker_host::TickReport, ctx: &Claimer, gift: &Claim) {
    let claim_txid = txid_of(&gift.claim);
    assert!(
        report
            .stored
            .contains(&format!("{claim_txid}: recorded in the wallet")),
        "{report:?}"
    );
    assert_recorded_once(ctx, gift).await;
}

/// The wallet holds the claim, recorded once (see [`assert_made`]).
async fn assert_recorded_once(ctx: &Claimer, gift: &Claim) {
    assert_eq!(
        held(ctx, &txid_of(&gift.claim)).await,
        format!(
            "row: Some(\"word=mined record=none beef=none\"); transactions: 1; outputs: 2; spendable: {}",
            gift.plan.covenant.amount + gift.plan.change
        )
    );
}

/// Seconds since the epoch, as the command's own clock reads them.
fn now() -> u64 {
    chrono::Utc::now().timestamp().cast_unsigned()
}

/// The two commands that take a mined claim's coin in by hand.
fn receive_commands(claim_txid: &str) -> Vec<String> {
    vec![
        format!("bsv-wallet receive {claim_txid}"),
        format!("bsv-wallet receive {claim_txid} --vout 1"),
    ]
}

/// The door's words for the record of a claim whose deposit has no proof.
const DOOR_SAID: &str = "Invalid BEEF at byte 48: InputNamesNoElement";

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
    assert_eq!(services.call_count("post_beef"), 1, "posted once");
    assert_made(&report, &ctx, &gift).await;
}

/// The claim handed to the broadcasters and the tracker through `ctx`, as
/// the command hands it, at the time `at`.
async fn announce(ctx: &Claimer, gift: &Claim, at: u64, force: bool) -> ClaimOutcome {
    let (storage, services) = (ctx.wallet.storage(), ctx.wallet.services());
    let (deposit, claim) = (&gift.deposit, &gift.claim);
    announce_claim(
        storage,
        services,
        &At(at),
        &opts(),
        deposit,
        claim,
        &gift.pay,
        gift.lock,
        force,
    )
    .await
    .unwrap()
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
    let outcome = announce(&ctx, &gift, now(), false).await;
    assert_eq!(outcome.word, "announced");
    let record = outcome.record.expect("an accepted claim has a record");
    assert_eq!(record.outputs.len(), 2, "the gift and the change");

    let refusal = ctx
        .wallet
        .internalize_action(record, Some("bsv-wallet-cli"))
        .await
        .expect_err("the door declines the record");
    println!("the door: {refusal}");
    let class = bsv_sdk::WalletRefusalClass::InvalidParameter;
    assert!(
        matches!(&refusal, bsv_sdk::Error::WalletRefusal { class: of, .. } if *of == class),
        "{refusal:?}"
    );
    assert_eq!(refusal.to_string(), DOOR_SAID);
    assert_eq!(ctx.wallet.services().call_count("post_beef"), 1);
    assert_eq!(
        held(&ctx, &txid_of(&gift.claim)).await,
        "row: Some(\"word=announced record=held beef=none\"); transactions: 0; outputs: 0; spendable: 0",
        "the refusal wrote nothing, and the row keeps the record"
    );
}

/// THE HINT (#638), in text: a claim posted and not recorded yet says
/// what happened (the claim is posted, its coin is not in the balance, the
/// deposit had no checked proof, the door's own words), that the wallet
/// keeps the record and who makes it, and the commands that take the coin
/// in by hand once the claim is mined, one per output of the claim that
/// pays the wallet. Exit zero: the claim is on the network.
#[tokio::test]
async fn the_text_output_says_what_happened_and_the_commands_that_take_the_coin_in() {
    let (gift, _, report) = claim_unmined(false).await;
    let (claim, deposit) = (txid_of(&gift.claim), txid_of(&gift.deposit));
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
               no checked proof of the gift's deposit {deposit} was to be had, so the wallet could not record the claim ({DOOR_SAID}).
               The wallet keeps the claim's record and makes it once the claim is mined:
               `daemon` and `serve` do that in their tracker pass; with neither running,
               run `bsv-wallet tracker-tick` after the claim is mined. Or take it in by hand then:
                 bsv-wallet receive {claim}
                 bsv-wallet receive {claim} --vout 1

   `bsv-wallet tracker-tick` (or `serve`) asks for its proof; the word turns
   `mined` when the proof checks against the header service."
        )
    );
}

/// THE HINT (#638), in JSON: the same as fields a caller can read.
/// `recorded` is false and `recordError` the door's words, as before;
/// `recordHeld` says the wallet keeps the claim's record and makes it by
/// itself, `recordHint` says what happened and what takes the coin in, and
/// `receiveCommands` lists the commands, one per output that pays the
/// wallet.
#[tokio::test]
async fn the_json_output_carries_the_hint_and_the_commands_as_fields() {
    let (gift, _, report) = claim_unmined(true).await;
    let (deposit, claim) = (txid_of(&gift.deposit), txid_of(&gift.claim));
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
            "recordError": DOOR_SAID,
            "recordHeld": true,
            "recordHint": format!(
                "the claim is posted and its coin is not in your balance yet: no checked proof of the gift's deposit {deposit} was to be had, so the wallet could not record the claim ({DOOR_SAID}); the wallet keeps the claim's record and makes it once the claim is mined: `daemon` and `serve` do that in their tracker pass, and with neither running `bsv-wallet tracker-tick` does; by hand, once the claim is mined, run `bsv-wallet receive {claim}` and `bsv-wallet receive {claim} --vout 1`"
            ),
            "receiveCommands": receive_commands(&claim),
        })
    );
}

/// `recorded` and `recordHeld` of the command's JSON, which carries no
/// `recordError` (the door declined nothing).
fn flags(json: &serde_json::Value) -> (bool, bool) {
    assert_eq!(json["recordError"], serde_json::Value::Null);
    (json["recorded"] == true, json["recordHeld"] == true)
}

/// A claim the wallet recorded at once (its deposit mined) carries no
/// hint and no command, in either mode, and leaves no pending record; a
/// claim held as not final says when it is recorded and keeps the commands
/// for a claim that is mined and still not in the balance.
#[tokio::test]
async fn a_recorded_claim_carries_no_hint_and_a_held_one_says_when_it_is_recorded() {
    let gift = Claim::new();
    let claim = txid_of(&gift.claim);
    let services = super::genesis_tests::proving(&gift.deposit, 900_000);
    let ctx = context(&gift.root, storage().await, services, Chain::Main).await;
    let mut report = claim_gift(&ctx, &gift).await.unwrap();
    assert_eq!(report.recorded, Recorded::Yes);
    let (json, text) = (report.json(), report.text());
    assert_eq!(flags(&json), (true, false), "recorded, and nothing held");
    assert_eq!(json["recordHint"], serde_json::Value::Null);
    assert_eq!(json["receiveCommands"], serde_json::json!([]));
    assert!(
        text.contains("   wallet:     recorded; 5844 sats are in your balance\n")
            && !text.contains("receive"),
        "{text}"
    );
    assert_eq!(
        held(&ctx, &claim).await,
        "row: Some(\"word=announced record=none beef=none\"); transactions: 1; outputs: 2; spendable: 5844",
        "a claim recorded at once leaves no pending record"
    );

    // The same claim as `--force` leaves it when a broadcaster calls it
    // not final: nothing is recorded before one takes it.
    report.recorded = Recorded::WhenTaken;
    report.outcome.not_final = Some("Arcade: 476 non-final".to_string());
    let (text, json) = (report.text(), report.json());
    assert_eq!(flags(&json), (false, true), "not recorded, the record held");
    assert_eq!(
        json["recordHint"],
        format!(
            "the claim is held as not final and nothing is recorded before a broadcaster takes it; the wallet keeps the claim's record and makes it then, in the tracker pass of `daemon` and `serve` or by `bsv-wallet tracker-tick`; if the claim is mined and still not in your balance, run `bsv-wallet receive {claim}` and `bsv-wallet receive {claim} --vout 1`"
        )
    );
    assert_eq!(
        json["receiveCommands"],
        serde_json::json!(receive_commands(&claim))
    );
    assert!(
        text.contains("   wallet:     recorded when a broadcaster takes the claim\n"),
        "{text}"
    );
}
