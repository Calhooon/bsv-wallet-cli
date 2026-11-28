//! The create door's word (#659, lane L1), by command: the tracker's word
//! for an action this wallet signs, read straight after the command
//! returns and before any pass, as the host serves it
//! (`tracker_host::stored_word`, what `GET /tx/{txid}` answers) and as the
//! spend gate reads the creator of the action's own change
//! (`tracker_host::read_creators`).
//!
//! The stack's wallet model writes `built` when the wallet signs an action
//! of its own (bsv-stack-lean `lean/Wallet/Default.lean`, `onCreateD`: a
//! signed action takes the tracker's host act `build`); a signed action of
//! ours whose creator reads `unknown` is a host that did not record its own
//! act (the keeper's answer K3). Each road here is one this host signs by:
//! `send` (to another party and to the wallet's own address), `split`,
//! `drain`, `gift-send`, and the spend gate's door itself for a `noSend`
//! action, one that accepts delayed broadcast and an unsigned template
//! (which is not signed and gets no row).
//!
//! Every wallet here is `split`'s tests' (`wallet_with_proven`): the
//! toolbox's mock services, its header service carrying each coin's block
//! and the tip, its broadcaster the mock's, which keeps no broadcast
//! memory, so no take is recorded and a signed action reads `built`. The
//! post-broadcast verifier is off. Nothing reaches the network.

use crate::commands::split::genesis_tests::wallet_with_proven;
use crate::context::WalletContext;
use crate::spend_gate::AllowPending;
use bsv_sdk::primitives::PrivateKey;
use bsv_sdk::wallet::{CreateActionArgs, CreateActionOptions, CreateActionOutput};
use bsv_wallet_cli::tracker_host::{self, TickOptions};
use bsv_wallet_toolbox::services::mock::MockWalletServices;
use bsv_wallet_toolbox::Chain;

type Ctx = WalletContext<MockWalletServices>;

/// A mainnet wallet holding one coin of 50,000 sats proven at 700,000, the
/// post-broadcast verifier off.
async fn wallet() -> (PrivateKey, Ctx) {
    let root = PrivateKey::random();
    let (mut ctx, _) = wallet_with_proven(&root, &[(700_000, 50_000)], Chain::Main).await;
    let mut config = crate::config::Config::default();
    config.verify.skip = true;
    ctx.config = std::sync::Arc::new(config);
    (root, ctx)
}

/// Another party's P2PKH address.
fn elsewhere() -> String {
    PrivateKey::random().public_key().to_address()
}

/// The txid of the action the wallet made last, and its status.
async fn last_made(ctx: &Ctx) -> (String, String) {
    sqlx::query_as(
        "SELECT lower(txid), status FROM transactions WHERE is_outgoing = 1 AND txid IS NOT NULL \
         ORDER BY transaction_id DESC LIMIT 1",
    )
    .fetch_one(ctx.wallet.storage().pool())
    .await
    .expect("an action of the wallet's own")
}

/// The creator's word of `txid`: as the host serves it (`None` with no
/// row, the route's 404) and as the gate reads it for the action's change.
async fn words(ctx: &Ctx, txid: &str) -> (Option<String>, &'static str) {
    let served = tracker_host::stored_word(ctx.wallet.storage(), txid)
        .await
        .unwrap()
        .map(|(word, ..)| word);
    let readings = tracker_host::read_creators(
        ctx.wallet.storage(),
        ctx.wallet.services(),
        &TickOptions::default(),
        &[(txid.to_string(), None)],
    )
    .await
    .unwrap();
    (served, readings[txid].word)
}

/// The road's action reads `built`, served and at the gate. The readings
/// are printed for the record of a run.
async fn reads_built(ctx: &Ctx, road: &str) {
    let (txid, status) = last_made(ctx).await;
    let read = words(ctx, &txid).await;
    println!(
        "{road}: status {status}; served {:?}; the gate reads {}",
        read.0, read.1
    );
    assert_eq!(
        read,
        (Some("built".to_string()), "built"),
        "{road}: status {status}"
    );
}

/// `send` to another party's address: signed and posted.
#[tokio::test]
async fn a_send_reads_built_at_signing() {
    let (_, ctx) = wallet().await;
    crate::commands::send::run(&ctx, &elsewhere(), 1_000, AllowPending::Off, &[])
        .await
        .expect("sent");
    reads_built(&ctx, "send").await;
}

/// `send` to the wallet's own address: signed, posted and taken in.
#[tokio::test]
async fn a_self_send_reads_built_at_signing() {
    let (root, ctx) = wallet().await;
    let own = crate::brc29::deposit_address(&root, Chain::Main).unwrap();
    crate::commands::send::run(&ctx, &own, 1_000, AllowPending::Off, &[])
        .await
        .expect("sent");
    reads_built(&ctx, "send to its own address").await;
}

/// `split` of two outputs: signed, posted and taken in.
#[tokio::test]
async fn a_split_reads_built_at_signing() {
    let (_, ctx) = wallet().await;
    crate::commands::split::run(&ctx, 2, AllowPending::Off, &[])
        .await
        .expect("split");
    reads_built(&ctx, "split").await;
}

/// `drain` to another party's address: signed and posted.
#[tokio::test]
async fn a_drain_reads_built_at_signing() {
    let (_, ctx) = wallet().await;
    crate::commands::drain::run(&ctx, &elsewhere(), AllowPending::Off)
        .await
        .expect("drained");
    reads_built(&ctx, "drain").await;
}

/// `gift-send` of 1,000 sats: signed and posted.
#[tokio::test]
async fn a_gift_send_reads_built_at_signing() {
    let (_, ctx) = wallet().await;
    let recipient = PrivateKey::random().public_key().to_hex();
    crate::commands::gift_send::run(&ctx, &recipient, 1_000, "+90d", 1_000, AllowPending::Off)
        .await
        .expect("the gift is sent");
    reads_built(&ctx, "gift-send").await;
}

/// An action through the spend gate's door with `options`: 1,000 sats to
/// another party.
async fn gated(ctx: &Ctx, options: CreateActionOptions) {
    let lock = bsv_sdk::script::templates::P2PKH::lock_from_address(&elsewhere()).unwrap();
    let args = CreateActionArgs {
        description: "an action of the create door's tests".to_string(),
        input_beef: None,
        inputs: None,
        outputs: Some(vec![CreateActionOutput {
            locking_script: lock.to_binary(),
            satoshis: 1_000,
            output_description: "a payment".to_string(),
            basket: None,
            custom_instructions: None,
            tags: None,
        }]),
        lock_time: None,
        version: None,
        labels: None,
        options: Some(options),
    };
    crate::spend_gate::create_action(&ctx.wallet, args, "create.door", &ctx.gate())
        .await
        .expect("the action");
}

/// A `noSend` action: signed, not posted. It reads `built` and stays
/// there until it is sent.
#[tokio::test]
async fn a_no_send_action_reads_built_at_signing() {
    let (_, ctx) = wallet().await;
    gated(
        &ctx,
        CreateActionOptions {
            no_send: Some(true),
            ..Default::default()
        },
    )
    .await;
    assert_eq!(
        ctx.wallet.services().call_count("post_beef"),
        0,
        "not posted"
    );
    reads_built(&ctx, "noSend").await;
}

/// An action that accepts delayed broadcast: signed, not posted by the
/// call. It reads `built` until it is sent.
#[tokio::test]
async fn a_delayed_action_reads_built_at_signing() {
    let (_, ctx) = wallet().await;
    gated(
        &ctx,
        CreateActionOptions {
            accept_delayed_broadcast: Some(true),
            ..Default::default()
        },
    )
    .await;
    reads_built(&ctx, "delayed").await;
}

/// An unsigned template (`signAndProcess` false): nothing is signed, so
/// the tracker holds no row for it.
#[tokio::test]
async fn an_unsigned_template_has_no_row() {
    let (_, ctx) = wallet().await;
    gated(
        &ctx,
        CreateActionOptions {
            sign_and_process: Some(false),
            ..Default::default()
        },
    )
    .await;
    let rows: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM tracker_states t JOIN transactions x ON lower(x.txid) = t.txid \
         WHERE x.is_outgoing = 1",
    )
    .fetch_one(ctx.wallet.storage().pool())
    .await
    .unwrap_or(0);
    assert_eq!(rows, 0, "no row for a template");
}

/// The tracker's row of `txid`, every column the host keeps.
async fn row(ctx: &Ctx, txid: &str) -> Option<(i64, String, String, i64)> {
    sqlx::query_as("SELECT built, word, hints, updated_at FROM tracker_states WHERE txid = ?")
        .bind(txid)
        .fetch_optional(ctx.wallet.storage().pool())
        .await
        .unwrap()
}

/// Written once: a second write (the internalize door's, on the same
/// transaction) and the pass's adoption change nothing. The pass adopts no row for the action (it has one) and it
/// still reads `built`.
#[tokio::test]
async fn a_second_write_and_the_pass_change_nothing() {
    let (_, ctx) = wallet().await;
    crate::commands::send::run(&ctx, &elsewhere(), 1_000, AllowPending::Off, &[])
        .await
        .expect("sent");
    let (txid, _) = last_made(&ctx).await;
    let written = row(&ctx, &txid).await;
    // The internalize door's word on the same transaction: nothing moves.
    tracker_host::door_word(ctx.wallet.storage(), &txid).await;
    assert_eq!(row(&ctx, &txid).await, written, "a second write");
    let report = tracker_host::tick(
        ctx.wallet.storage(),
        ctx.wallet.services(),
        &tracker_host::SystemClock,
        &TickOptions::default(),
    )
    .await
    .unwrap();
    assert_eq!(report.adopted, 0, "{report:?}");
    let (word, ..) = tracker_host::stored_word(ctx.wallet.storage(), &txid)
        .await
        .unwrap()
        .expect("a row");
    assert_eq!(word, "built");
}

/// A fault writing the row is logged by name and never fails the action:
/// the send answers, its action is stored, the tracker holds no row.
#[tokio::test]
async fn a_fault_writing_the_row_is_logged_and_the_action_stands() {
    let (markers, _guard) = crate::test_support::Markers::capture();
    let (_, ctx) = wallet().await;
    // The tracker's tables exist (a read creates them), then refuse a row.
    tracker_host::stored_word(ctx.wallet.storage(), &"00".repeat(32))
        .await
        .unwrap();
    sqlx::query(
        "CREATE TRIGGER no_row BEFORE INSERT ON tracker_states \
         BEGIN SELECT RAISE(FAIL, 'no row'); END",
    )
    .execute(ctx.wallet.storage().writer())
    .await
    .unwrap();
    crate::commands::send::run(&ctx, &elsewhere(), 1_000, AllowPending::Off, &[])
        .await
        .expect("the send stands");
    let (txid, status) = last_made(&ctx).await;
    assert_eq!(status, "unproven");
    assert_eq!(row(&ctx, &txid).await, None);
    let logged = markers.named("signed_word_not_written");
    assert_eq!(logged.len(), 1, "{:?}", markers.all());
    assert_eq!(logged[0].txid.as_deref(), Some(txid.as_str()));
}
