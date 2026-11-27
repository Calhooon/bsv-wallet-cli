//! The held record (#638, #620): a claim keeps the wallet's record of it
//! on its tracker row from the post until the wallet has made it, and the
//! pass makes it once the claim is mined, through the door, once.

use super::*;
use bsv_wallet_toolbox::services::mock::{error_post_beef_result, success_post_beef_result};

/// [`claimed`], and the claim mined.
async fn claimed_and_mined() -> (Claim, String, Claimer) {
    let (gift, claim_txid, ctx) = claimed().await;
    mine_the_claim(ctx.wallet.services(), &claim_txid);
    (gift, claim_txid, ctx)
}

/// `bsv-wallet receive <claim txid> [--vout N]` by hand, its courier a
/// fixture on loopback that holds the mined claim's BEEF (the claim's
/// bytes under its own proof).
async fn receive_by_hand(ctx: &Claimer, gift: &Claim, vout: Option<u32>) -> (u32, bool) {
    use crate::commands::receive::{receive_txid_from, tests::woc_only};
    let claim_txid = txid_of(&gift.claim);
    let mut beef = Beef::new();
    let bump = beef.merge_bump(MerklePath::from_coinbase_txid(&claim_txid, CLAIM_HEIGHT));
    beef.merge_raw_tx(gift.claim.clone(), Some(bump)).unwrap();
    let route = format!("/tx/{claim_txid}/beef");
    let body = beef.to_hex().unwrap();
    let courier = crate::test_support::Fixture::start(&[(&route, 200, &body)], 500).await;
    let explorers = woc_only(&courier.base);
    receive_txid_from(ctx, &explorers, ctx.wallet.services(), &claim_txid, vout)
        .await
        .expect("receive takes the mined claim in")
}

/// [`claimed_and_mined`], and the gift's output (5,000) taken in by hand
/// with `receive <claim txid>`; the change is not.
async fn gift_received_by_hand() -> (Claim, String, Claimer) {
    let (gift, claim_txid, ctx) = claimed_and_mined().await;
    assert_eq!(receive_by_hand(&ctx, &gift, None).await, (0, true));
    assert_eq!(spendable(ctx.wallet.storage()).await, 5000, "the gift");
    (gift, claim_txid, ctx)
}

/// The pass after the claim is mined makes the record ([`assert_made`]).
async fn assert_the_pass_makes_the_record(ctx: &Claimer, gift: &Claim) {
    let report = pass(ctx, now() + 601).await;
    assert!(
        report.owed.is_empty() && report.faults.is_empty(),
        "{report:?}"
    );
    assert_made(&report, ctx, gift).await;
}

/// A pass after the record is made does nothing: the record has left the
/// row, the wallet holds the claim once, and the claim was posted once.
async fn assert_a_later_pass_does_nothing(ctx: &Claimer, gift: &Claim) {
    let again = pass(ctx, now() + 1300).await;
    assert!(
        again.stored.is_empty() && again.owed.is_empty() && again.faults.is_empty(),
        "{again:?}"
    );
    assert_eq!(ctx.wallet.services().call_count("post_beef"), 1);
    assert_recorded_once(ctx, gift).await;
}

/// While the claim is not mined the pass holds the record, names it and
/// makes nothing; after the pass that made it, a second pass does
/// nothing: the record has left the row, and the claim was posted once.
#[tokio::test]
async fn the_pass_holds_the_record_until_the_claim_is_mined_and_a_second_pass_does_nothing() {
    let (gift, claim_txid, ctx) = claimed().await;
    let waiting = pass(&ctx, now() + 1).await;
    assert_eq!(
        waiting.owed,
        vec![format!(
            "{claim_txid}: its record in the wallet is kept and made once it is mined (its word is announced)"
        )]
    );
    assert!(waiting.stored.is_empty(), "{waiting:?}");
    assert_eq!(spendable(ctx.wallet.storage()).await, 0);

    mine_the_claim(ctx.wallet.services(), &claim_txid);
    assert_the_pass_makes_the_record(&ctx, &gift).await;
    assert_a_later_pass_does_nothing(&ctx, &gift).await;
}

/// A wallet store in the file `path`, migrated when it is new.
async fn store_in(path: &str, new: bool) -> StorageSqlx {
    use bsv_wallet_toolbox::WalletStorageWriter;
    let storage = StorageSqlx::open(path).await.unwrap();
    if new {
        let identity = "02".to_string() + &"ab".repeat(32);
        storage.migrate("gift-tests", &identity).await.unwrap();
    }
    storage.make_available().await.unwrap();
    storage
}

/// A restart between the post and the proof loses nothing: the record is
/// on the tracker's row in the wallet's own file, so a wallet opened again
/// on that file, with nothing of the claim in memory, records the coin on
/// its pass once the claim is mined.
#[tokio::test]
async fn a_restart_between_the_post_and_the_proof_loses_nothing() {
    let gift = Claim::new();
    let claim_txid = txid_of(&gift.claim);
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("wallet.db");
    let file = file.to_str().unwrap();

    let services = unmined_then_mined(&gift).build();
    let ctx = context(
        &gift.root,
        store_in(file, true).await,
        services,
        Chain::Main,
    )
    .await;
    claim_gift(&ctx, &gift).await.unwrap();
    assert_eq!(
        held(&ctx, &claim_txid).await,
        "row: Some(\"word=announced record=held beef=none\"); transactions: 0; outputs: 0; spendable: 0"
    );
    ctx.wallet.storage().pool().close().await;
    ctx.wallet.storage().writer().close().await;
    drop(ctx);

    // The process is gone. The claim is mined; a wallet opens the file.
    let services = MockWalletServices::builder()
        .height(CLAIM_HEIGHT + 5)
        .get_merkle_path_response(proof_of(&gift.claim, Some(CLAIM_HEIGHT)))
        .build();
    mine_the_claim(&services, &claim_txid);
    let ctx = context(
        &gift.root,
        store_in(file, false).await,
        services,
        Chain::Main,
    )
    .await;
    let report = pass(&ctx, now() + 601).await;
    assert_eq!(ctx.wallet.services().call_count("post_beef"), 0);
    assert_made(&report, &ctx, &gift).await;
}

/// `receive <claim txid>` and `receive <claim txid> --vout 1` by hand,
/// once the claim is mined and before any pass, still take the coin in.
/// The pass that follows hands the kept record to the door, which holds
/// both outputs already: nothing is counted twice, the record leaves the
/// row, and the pass after that does nothing.
#[tokio::test]
async fn receive_by_hand_before_the_pass_still_works_and_the_pass_then_changes_nothing() {
    let (gift, claim_txid, ctx) = gift_received_by_hand().await;
    assert_eq!(receive_by_hand(&ctx, &gift, Some(1)).await, (1, true));
    assert_eq!(
        held(&ctx, &claim_txid).await,
        "row: Some(\"word=announced record=held beef=none\"); transactions: 1; outputs: 2; spendable: 5844",
        "both outputs by hand"
    );
    assert_the_pass_makes_the_record(&ctx, &gift).await;
    assert_a_later_pass_does_nothing(&ctx, &gift).await;
}

/// `receive <claim txid>` alone by hand takes the gift's output in; the
/// pass then takes the change in through the same door and counts the
/// gift once.
#[tokio::test]
async fn the_pass_takes_in_the_output_a_receive_by_hand_left() {
    let (gift, _, ctx) = gift_received_by_hand().await;
    assert_the_pass_makes_the_record(&ctx, &gift).await;
}

/// The pass never gives back a coin the wallet has spent. The gift's
/// output is taken in by hand and then spent (its row no longer
/// spendable, as a spend leaves it); the pass hands the kept record, which
/// names both outputs, to the door: the change is taken in, and the spent
/// output stays spent.
#[tokio::test]
async fn the_pass_leaves_an_output_the_wallet_spent_spent() {
    let (_, claim_txid, ctx) = gift_received_by_hand().await;
    sqlx::query("UPDATE outputs SET spendable = 0 WHERE txid = ? AND vout = 0")
        .bind(&claim_txid)
        .execute(ctx.wallet.storage().writer())
        .await
        .unwrap();
    assert_eq!(spendable(ctx.wallet.storage()).await, 0);

    let report = pass(&ctx, now() + 601).await;
    let made = format!("{claim_txid}: recorded in the wallet");
    assert!(report.stored.contains(&made), "{report:?}");
    assert_eq!(
        held(&ctx, &claim_txid).await,
        "row: Some(\"word=mined record=none beef=none\"); transactions: 1; outputs: 2; spendable: 844",
        "the change alone: the spent output stays spent"
    );
}

/// The door counts a claim once however often its record is handed over.
/// A gift whose deposit is mined is recorded at once; here the row's write
/// after the door's yes never happened (the process ended there), so the
/// record is still on the row. The pass hands it over once the claim is
/// mined: the wallet holds the claim's one transaction and its two outputs
/// as before, and the record leaves the row.
#[tokio::test]
async fn a_record_handed_to_the_door_again_counts_the_coin_once() {
    use bsv_sdk::wallet::WalletInterface;

    let gift = Claim::new();
    let claim_txid = txid_of(&gift.claim);
    let services = MockWalletServices::builder()
        .height(CLAIM_HEIGHT + 5)
        .get_merkle_path_response(MockResponse::Sequence(vec![
            proof_of(&gift.deposit, Some(900_000)),
            proof_of(&gift.claim, Some(CLAIM_HEIGHT)),
        ]))
        .build();
    let ctx = context(&gift.root, storage().await, services, Chain::Main).await;
    let record = announce(&ctx, &gift, now(), false).await.record.unwrap();
    let taken = ctx
        .wallet
        .internalize_action(record, Some("bsv-wallet-cli"))
        .await
        .unwrap();
    assert!(taken.accepted);
    assert_eq!(
        held(&ctx, &claim_txid).await,
        "row: Some(\"word=announced record=held beef=none\"); transactions: 1; outputs: 2; spendable: 5844"
    );

    mine_the_claim(ctx.wallet.services(), &claim_txid);
    assert_the_pass_makes_the_record(&ctx, &gift).await;
}

/// A claim every broadcaster refuses leaves no coin, no tracked row and
/// nothing owed: the command's error says what they said, and a pass has
/// nothing to make.
#[tokio::test]
async fn a_claim_every_broadcaster_refuses_leaves_no_coin_and_nothing_owed() {
    let gift = Claim::new();
    let refused = vec![error_post_beef_result("Arcade", "fee too low")];
    let services = unmined_then_mined(&gift)
        .post_beef_response(MockResponse::Success(refused))
        .build();
    let ctx = context(&gift.root, storage().await, services, Chain::Main).await;
    let refusal = claim_gift(&ctx, &gift)
        .await
        .expect_err("refused by every broadcaster");
    assert_eq!(
        refusal.to_string(),
        "broadcast rejected by every broadcaster (Arcade: fee too low)"
    );
    assert_eq!(ctx.wallet.services().call_count("post_beef"), 1);
    let report = pass(&ctx, now() + 601).await;
    assert!(
        report.stored.is_empty() && report.owed.is_empty(),
        "{report:?}"
    );
    assert_eq!(
        held(&ctx, &txid_of(&gift.claim)).await,
        "row: None; transactions: 0; outputs: 0; spendable: 0"
    );
}

/// A claim a broadcaster took and the network then refused (its row's
/// word a node's verdict, as the follow-up of a `stale` word writes one)
/// leaves no coin, and its record says so: the pass names it, with the
/// word, and hands nothing to the door.
#[tokio::test]
async fn a_claim_the_network_refused_leaves_no_coin_and_a_record_that_says_so() {
    let (_, claim_txid, ctx) = claimed_and_mined().await;
    sqlx::query("UPDATE tracker_states SET word = 'rejected' WHERE txid = ?")
        .bind(&claim_txid)
        .execute(ctx.wallet.storage().writer())
        .await
        .unwrap();

    for at in [now() + 601, now() + 1300] {
        let report = pass(&ctx, at).await;
        assert_eq!(
            report.owed,
            vec![format!(
                "{claim_txid}: its record in the wallet is not made: the transaction is rejected, and no coin of it is the wallet's"
            )]
        );
        assert!(report.stored.is_empty(), "{report:?}");
        assert_eq!(
            held(&ctx, &claim_txid).await,
            "row: Some(\"word=rejected record=held beef=none\"); transactions: 0; outputs: 0; spendable: 0"
        );
    }
}

/// The tally of `claim_txid`'s owed record, as `tracker_meta` holds it.
async fn tally(ctx: &Claimer, claim_txid: &str) -> Option<String> {
    sqlx::query_scalar("SELECT value FROM tracker_meta WHERE key = 'owed:' || ?")
        .bind(claim_txid)
        .fetch_optional(ctx.wallet.storage().pool())
        .await
        .unwrap()
}

/// The tally after `tries` tries, the next not before `next_at`.
fn tried(tries: u32, next_at: u64) -> String {
    format!("{{\"tries\":{tries},\"nextAt\":{next_at}}}")
}

/// A mined claim whose record a pass handed over and never took to the
/// door (the process ended there): the gift, the claim's txid, the wallet
/// and the time of that pass. One try is counted, the next not before the
/// re-ask's pause (600 s) is over.
async fn handed_over_and_not_made() -> (Claim, String, Claimer, u64) {
    let (gift, claim_txid, ctx) = claimed_and_mined().await;
    let (storage, services) = (ctx.wallet.storage(), ctx.wallet.services());
    let start = now() + 601;
    let report = tracker_host::tick(storage, services, &At(start), &opts())
        .await
        .unwrap();
    assert_eq!(report.mined, vec![claim_txid.clone()]);
    assert_eq!(report.on_accept.len(), 1, "handed over once");
    assert_eq!(tally(&ctx, &claim_txid).await, Some(tried(1, start + 600)));
    (gift, claim_txid, ctx, start)
}

/// A try that did not make the record is counted and waits its pause:
/// nothing is handed over before it, and the try after it reaches the
/// door and makes the record once; the tally leaves with the record.
#[tokio::test]
async fn a_try_that_did_not_make_the_record_waits_its_pause_and_the_next_makes_it() {
    let (gift, claim_txid, ctx, start) = handed_over_and_not_made().await;
    let (storage, services) = (ctx.wallet.storage(), ctx.wallet.services());
    let early = tracker_host::tick(storage, services, &At(start + 599), &opts())
        .await
        .unwrap();
    assert!(
        early.on_accept.is_empty() && early.owed.is_empty(),
        "{early:?}"
    );
    assert_eq!(spendable(storage).await, 0);

    let report = pass(&ctx, start + 600).await;
    assert_eq!(report.stored.len(), 1, "{report:?}");
    assert_made(&report, &ctx, &gift).await;
    assert_eq!(tally(&ctx, &claim_txid).await, None);
}

/// With no chain tracker (`CHAINTRACKS_URL=off`) the door holds every
/// `internalizeAction`, so a mined claim's record is not handed over: the
/// pass names it in its faults, counts no try, and the record stays.
#[tokio::test]
async fn a_mined_claims_record_is_held_and_named_while_no_chain_tracker_is_configured() {
    use crate::test_support::NoTracker;

    let (_, claim_txid, ctx, start) = handed_over_and_not_made().await;
    let off = NoTracker(
        MockWalletServices::builder()
            .height(CLAIM_HEIGHT + 5)
            .build(),
    );
    let report = tracker_host::tick(ctx.wallet.storage(), &off, &At(start + 600), &opts())
        .await
        .unwrap();
    assert!(report.on_accept.is_empty(), "{report:?}");
    assert_eq!(
        report.faults,
        vec![format!(
            "{claim_txid}: {}",
            tracker_host::RECORD_HELD_WITHOUT_A_TRACKER
        )]
    );
    assert_eq!(tally(&ctx, &claim_txid).await, Some(tried(1, start + 600)));
    assert_eq!(
        held(&ctx, &claim_txid).await,
        "row: Some(\"word=mined record=held beef=none\"); transactions: 0; outputs: 0; spendable: 0"
    );
}

/// A record tried sixteen times is not handed over again: the pass names
/// it, with the command that records it by hand.
#[tokio::test]
async fn a_record_tried_sixteen_times_is_named_and_left_to_receive() {
    let (_, claim_txid, ctx, start) = handed_over_and_not_made().await;
    let storage = ctx.wallet.storage();
    sqlx::query("UPDATE tracker_meta SET value = ? WHERE key = 'owed:' || ?")
        .bind(tried(16, 0))
        .bind(&claim_txid)
        .execute(storage.writer())
        .await
        .unwrap();
    let report = pass(&ctx, start + 600).await;
    assert!(report.stored.is_empty(), "{report:?}");
    assert_eq!(
        report.owed,
        vec![format!(
            "{claim_txid}: not recorded in the wallet after 16 tries; `bsv-wallet receive {claim_txid}` records it by hand, one output at a time (`--vout`)"
        )]
    );
    assert_eq!(spendable(storage).await, 0);
}

/// #620 on the forced road: a claim held as not final whose deposit has
/// no proof. Its re-announce at the lock time is accepted and the door
/// declines the record (the stored BEEF carries the deposit without its
/// parents): the record stays on the row and is not dropped. Once the
/// claim is mined the pass makes it. One announce, one re-announce, one
/// record.
#[tokio::test]
async fn a_held_claims_record_the_door_declined_at_its_re_announce_is_made_once_mined() {
    let gift = Claim::new();
    let claim_txid = txid_of(&gift.claim);
    let not_final = vec![error_post_beef_result("Arcade", "476 non-final")];
    let taken = vec![success_post_beef_result("Arcade", &[&claim_txid])];
    let services = unmined_then_mined(&gift)
        .post_beef_response(MockResponse::Sequence(vec![
            MockResponse::Success(not_final),
            MockResponse::Success(taken),
        ]))
        .build();
    let ctx = context(&gift.root, storage().await, services, Chain::Main).await;
    let lock = gift.lock;
    assert!(announce(&ctx, &gift, lock - 3600, true)
        .await
        .record
        .is_none());
    assert_eq!(
        held(&ctx, &claim_txid).await,
        "row: Some(\"word=built record=held beef=held\"); transactions: 0; outputs: 0; spendable: 0"
    );

    // The lock time: announced again and taken; the door declines.
    let report = pass(&ctx, lock).await;
    assert_eq!(report.reannounced, vec![claim_txid.clone()]);
    assert_eq!(
        report.stored,
        vec![format!(
            "{claim_txid}: not recorded in the wallet ({DOOR_SAID}); its record is kept, and a pass makes it once it is mined, or `bsv-wallet receive {claim_txid}` records it by hand then"
        )]
    );
    assert_eq!(
        held(&ctx, &claim_txid).await,
        "row: Some(\"word=announced record=held beef=none\"); transactions: 0; outputs: 0; spendable: 0"
    );

    // Mined: the pass makes the record.
    mine_the_claim(ctx.wallet.services(), &claim_txid);
    let report = pass(&ctx, lock + 601).await;
    assert_eq!(report.mined, vec![claim_txid.clone()]);
    assert_eq!(ctx.wallet.services().call_count("post_beef"), 2);
    assert_made(&report, &ctx, &gift).await;
}
