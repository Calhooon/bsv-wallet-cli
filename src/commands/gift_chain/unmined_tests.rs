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

/// The command in its text mode on the same gift: posted once, exit zero,
/// nothing in the balance yet.
#[tokio::test]
async fn the_text_mode_claim_of_an_unmined_deposit_is_posted_once_and_exits_zero() {
    let gift = Claim::new();
    let ctx = claimer(&gift, false).await;
    let answer = crate::commands::gift_claim::run(&ctx, &txid_of(&gift.deposit), false).await;
    assert!(answer.is_ok(), "{answer:?}");
    assert_eq!(ctx.wallet.services().call_count("post_beef"), 1);
    assert_eq!(spendable(ctx.wallet.storage()).await, 0);
}
