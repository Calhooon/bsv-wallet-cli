//! The lineage rule at the door (#163): a coin whose creator's lineage is
//! dead is held under every rule and word, and a createAction naming one is
//! refused with nothing stored.

use super::*;

/// A wallet's store and its (user, basket).
type Store = (StorageSqlx, (i64, i64));

async fn wallet_store() -> Store {
    store_for(&("02".to_string() + &"ab".repeat(32))).await
}

/// The store for `identity`, with the tracker's tables as a first read
/// makes them.
async fn store_for(identity: &str) -> Store {
    let (storage, user, basket) = storage_for(identity).await;
    crate::tracker_host::stored_word(&storage, "")
        .await
        .unwrap();
    (storage, (user, basket))
}

/// `child` (a transaction row) spends the output `output_id`: the toolbox's
/// `spent_by`, the edge the walk reads.
async fn spends(storage: &StorageSqlx, child: &str, output_id: i64) {
    sqlx::query(
        "UPDATE outputs SET spent_by = (SELECT transaction_id FROM transactions WHERE txid = ?) \
         WHERE output_id = ?",
    )
    .bind(child)
    .bind(output_id)
    .execute(storage.writer())
    .await
    .unwrap();
}

/// How an ancestor of ours died.
#[derive(Clone, Copy)]
enum Death {
    /// The tracker's stored word.
    Word(&'static str),
    /// The toolbox's `failed` status.
    Failed,
}

async fn kill(storage: &StorageSqlx, txid: &str, death: Death) {
    match death {
        Death::Word(word) => {
            sqlx::query("INSERT INTO tracker_states (txid, word) VALUES (?, ?)")
                .bind(txid)
                .bind(word)
                .execute(storage.writer())
                .await
                .unwrap();
        }
        Death::Failed => fail(storage, txid).await,
    }
}

/// Our own unproven change `txid` (made, a pending creator): its output id.
async fn change(store: &Store, txid: &str) -> i64 {
    coin(&store.0, store.1, txid, true, None, 6_000, 3).await
}

/// The scenario's shape: our parent P (its change P0 spent by D) and our
/// child D, whose change D0 is the coin judged. (P's txid, D's txid, D0.)
async fn parent_and_child(store: &Store, death: Option<Death>) -> (String, String, i64) {
    let (p, d) = ("e1".repeat(32), "e2".repeat(32));
    let p0 = change(store, &p).await;
    let d0 = change(store, &d).await;
    spends(&store.0, &d, p0).await;
    if let Some(death) = death {
        kill(&store.0, &p, death).await;
    }
    (p, d, d0)
}

fn quiet_headers() -> MockWalletServices {
    headers_with(&"0c".repeat(32), 900, 900)
}

/// The gate's reason for `txid`'s coin through both doors, under `opts`:
/// (the allocation's, the named input's); `None` when offered.
async fn both_doors(
    store: &Store,
    services: &MockWalletServices,
    opts: &GateOptions,
    txid: &str,
) -> (Option<String>, Option<String>) {
    doors_at(store, services, opts, (txid, 0)).await
}

/// [`both_doors`] for the coin at `vout`.
async fn doors_at(
    store: &Store,
    services: &MockWalletServices,
    opts: &GateOptions,
    (txid, vout): (&str, u32),
) -> (Option<String>, Option<String>) {
    let a = assess(UserCoins::new(&store.0, store.1 .0), services, opts, None)
        .await
        .unwrap();
    let named = assess_named(
        UserCoins::new(&store.0, store.1 .0),
        services,
        opts,
        &[(txid.to_string(), vout).into()],
        &[],
        None,
        false,
    )
    .await
    .unwrap();
    (reason_of(&a, txid), named.first().map(|c| c.reason.clone()))
}

/// Both doors hold `txid`'s coin under `opts`, naming `ancestor` dead by
/// `word`.
async fn held_naming(
    store: &Store,
    services: &MockWalletServices,
    opts: &GateOptions,
    txid: &str,
    (ancestor, word): (&str, &str),
) {
    let want = Some(format!("dead lineage: ancestor {ancestor} {word}"));
    assert_eq!(
        both_doors(store, services, opts, txid).await,
        (want.clone(), want)
    );
}

/// The scenario (P conflicted, D still announced): D's change is held
/// "dead lineage", naming P, on both doors, under the own-change rule and
/// under the received word; mined too, deep enough (the lineage holds a
/// coin under every word). The view writes nothing.
#[tokio::test]
async fn a_child_of_a_conflicted_parent_is_held_under_every_rule_and_word() {
    let store = wallet_store().await;
    let (p, d, _) = parent_and_child(&store, Some(Death::Word("conflicted"))).await;
    let services = quiet_headers();
    let before = stored_rows(&store.0).await;
    for o in every_word(&d) {
        held_naming(&store, &services, &o, &d, (&p, "conflicted")).await;
    }
    assert_eq!(stored_rows(&store.0).await, before, "the view wrote a row");

    // D mined, its block 101 deep: still held.
    let (q, m) = ("e3".repeat(32), "e4".repeat(32));
    let q0 = change(&store, &q).await;
    coin(&store.0, store.1, &m, true, Some(800), 6_000, 3).await;
    spends(&store.0, &m, q0).await;
    kill(&store.0, &q, Death::Word("conflicted")).await;
    let services = headers_with(&m, 800, 900);
    held_naming(&store, &services, &opts(1, true), &m, (&q, "conflicted")).await;
}

/// D's child E (spending D0) is held through two steps, naming P.
#[tokio::test]
async fn a_grandchild_is_held_through_two_steps() {
    let store = wallet_store().await;
    let (p, _, d0) = parent_and_child(&store, Some(Death::Word("conflicted"))).await;
    let e = "e5".repeat(32);
    change(&store, &e).await;
    spends(&store.0, &e, d0).await;
    let services = quiet_headers();
    held_naming(&store, &services, &opts(1, true), &e, (&p, "conflicted")).await;
}

/// The other dead words: a parent `rejected` by the tracker, a parent the
/// toolbox holds `failed` (the CLI's abandon).
#[tokio::test]
async fn a_child_of_a_rejected_or_failed_parent_is_held() {
    for (death, word) in [
        (Death::Word("rejected"), "rejected"),
        (Death::Failed, "failed"),
    ] {
        let store = wallet_store().await;
        let (p, d, _) = parent_and_child(&store, Some(death)).await;
        held_naming(&store, &quiet_headers(), &opts(1, true), &d, (&p, word)).await;
    }
}

/// A child of a live parent is offered as before; with the rule off, the
/// door's own reason, byte for byte.
#[tokio::test]
async fn a_child_of_a_live_parent_is_offered_as_before() {
    let store = wallet_store().await;
    let (_, d, _) = parent_and_child(&store, None).await;
    let services = quiet_headers();
    assert_eq!(
        both_doors(&store, &services, &opts(1, true), &d).await,
        (None, None)
    );
    let off = Some("depth 1: 0 blocks; own change, SPEND_OWN_CHANGE=0".to_string());
    assert_eq!(
        both_doors(&store, &services, &opts(1, false), &d).await,
        (off.clone(), off)
    );
}

/// A received coin's creator is not ours: its lineage is never read, even
/// where the store plants an edge from a conflicted action of ours into
/// it. Held without the word by the door's own reason, offered under it.
#[tokio::test]
async fn a_received_coin_is_never_walked() {
    let store = wallet_store().await;
    let (p, r) = ("e8".repeat(32), "e9".repeat(32));
    let p0 = change(&store, &p).await;
    coin(&store.0, store.1, &r, false, None, 6_000, 3).await;
    spends(&store.0, &r, p0).await;
    kill(&store.0, &p, Death::Word("conflicted")).await;
    let services = quiet_headers();
    let held = Some(
        "depth 1: 0 blocks; received, no proof; offered under allowPending: received".to_string(),
    );
    assert_eq!(
        both_doors(&store, &services, &opts(1, true), &r).await,
        (held.clone(), held)
    );
    let word = opts(1, true).with_allow_received(true);
    assert_eq!(both_doors(&store, &services, &word, &r).await, (None, None));
}

/// Another output of the transaction row `txid` at `vout`, free.
async fn second_output(store: &Store, txid: &str, vout: i64) -> i64 {
    sqlx::query(
        "INSERT INTO outputs (user_id, transaction_id, basket_id, vout, satoshis, locking_script, txid, type, spendable, change, provided_by, purpose, output_description) \
         SELECT user_id, transaction_id, ?, ?, 4000, ?, txid, 'P2PKH', 1, 1, 'storage', 'change', 'c' \
         FROM transactions WHERE txid = ?",
    )
    .bind(store.1 .1)
    .bind(vout)
    .bind(hex::decode(LOCK).unwrap())
    .bind(txid)
    .execute(store.0.writer())
    .await
    .unwrap()
    .last_insert_rowid()
}

/// A malformed store: A and B spend each other's output (a cycle no chain
/// can hold). With no dead word the walk holds nothing; reached from a
/// conflicted P it ends, and holds the cycle's free coin. Neither hangs.
#[tokio::test]
async fn a_cycle_in_spent_by_ends_the_walk_and_holds_only_what_a_dead_word_reaches() {
    let store = wallet_store().await;
    let coins = UserCoins::new(&store.0, store.1 .0);
    let (a, b) = ("ea".repeat(32), "eb".repeat(32));
    let a0 = change(&store, &a).await;
    let b0 = change(&store, &b).await;
    spends(&store.0, &b, a0).await;
    spends(&store.0, &a, b0).await;
    second_output(&store, &b, 1).await;
    let walk = || async {
        tokio::time::timeout(
            Duration::from_secs(5),
            lineage_candidates(&store.0, store.1 .0),
        )
        .await
        .expect("the walk hung")
        .unwrap()
        .dead(&HashMap::new())
    };
    assert!(walk().await.is_empty());
    let a1 = assess(coins, &quiet_headers(), &opts(1, true), None)
        .await
        .unwrap();
    assert_eq!(a1.offered.len(), 1, "B's free coin: {a1:?}");

    let p = "ec".repeat(32);
    let p0 = change(&store, &p).await;
    second_output(&store, &a, 1).await;
    // P0 spent by A as a second input beside B0.
    spends(&store.0, &a, p0).await;
    kill(&store.0, &p, Death::Word("conflicted")).await;
    let dead = walk().await;
    assert_eq!(
        dead.keys()
            .cloned()
            .collect::<std::collections::BTreeSet<_>>(),
        [a.clone(), b.clone()].into_iter().collect()
    );
    let held = assess(coins, &quiet_headers(), &opts(1, true), None)
        .await
        .unwrap();
    assert!(held.offered.is_empty(), "{held:?}");
    assert_eq!(held.held.len(), 2, "A's and B's free coins: {held:?}");
}

/// `dead_lineage_create_refused`: a createAction naming D0 is refused as a
/// held coin is (403 `INPUT_NOT_OFFERED`, the held listing naming P),
/// before the toolbox is asked, and nothing is stored. Under an outage the
/// lineage is the verdict, never "could not verify now".
#[tokio::test]
async fn a_named_coin_of_a_dead_lineage_refuses_the_action_and_stores_nothing() {
    let root = PrivateKey::random();
    let store = store_for(&root.public_key().to_hex()).await;
    let (p, d, _) = parent_and_child(&store, Some(Death::Word("conflicted"))).await;
    let wallet = Wallet::new(Some(root), store.0, quiet_headers())
        .await
        .unwrap();
    let mut args = pay(1_000);
    args.inputs = Some(vec![named_input(&d)]);
    let before = stored_rows(wallet.storage()).await;
    let (result, asked) =
        at_the_door(create_action(&wallet, args, "gate.local", &opts(1, true))).await;
    let Err(GateError::Refused(refusal)) = result else {
        panic!("the dead lineage's coin passed the gate: {result:?}");
    };
    assert!(refusal.is_named_input());
    assert_eq!(refusal.code(), "INPUT_NOT_OFFERED");
    assert_eq!(refusal.held.len(), 1);
    assert_eq!(
        refusal.held[0].reason,
        format!("dead lineage: ancestor {p} conflicted")
    );
    assert!(!asked, "the toolbox was asked");
    assert_eq!(stored_rows(wallet.storage()).await, before, "a row stored");

    wallet.services().set_tip_unavailable();
    let held = assess_named(
        UserCoins::from_wallet(&wallet).await.unwrap(),
        wallet.services(),
        &opts(1, true),
        &[(d, 0).into()],
        &[],
        None,
        false,
    )
    .await
    .unwrap();
    assert_eq!(
        held[0].reason,
        format!("dead lineage: ancestor {p} conflicted")
    );
    assert_eq!(held[0].headers_fault, None);
}

/// The planted shape of the round-two review: our parent P mined at 800
/// with its proof in `proven_txs` (its root is P's txid), dead as `death`
/// (a tracker word stored as `node_verdict` stores it, beside its verdict,
/// or the toolbox's `failed`), its change P0 spent by our child D and a
/// second coin P1 free. (P's txid, D's txid.)
async fn dead_parent_with_proof(store: &Store, death: Death) -> (String, String) {
    let (p, d) = ("f3".repeat(32), "f4".repeat(32));
    let p0 = coin(&store.0, store.1, &p, true, Some(800), 6_000, 3).await;
    second_output(store, &p, 1).await;
    change(store, &d).await;
    spends(&store.0, &d, p0).await;
    match death {
        Death::Word(word) => node_verdict(&store.0, &p, word).await,
        Death::Failed => kill(&store.0, &p, death).await,
    }
    (p, d)
}

/// A fresh wallet with [`dead_parent_with_proof`]'s P `conflicted`, and
/// headers under which P's proof checks at 800, 101 deep.
async fn conflicted_with_proof() -> (Store, (String, String), MockWalletServices) {
    let store = wallet_store().await;
    let (p, d) = dead_parent_with_proof(&store, Death::Word("conflicted")).await;
    let services = headers_with(&p, 800, 900);
    (store, (p, d), services)
}

/// The tracker's dead `word` for `txid`, stored as `node_verdict` stores
/// it: beside its verdict, so a replay reads it back.
async fn node_verdict(storage: &StorageSqlx, txid: &str, word: &str) {
    let verdict = serde_json::json!({"kind": word, "competitors": ["fd".repeat(32)]});
    sqlx::query("INSERT INTO tracker_states (txid, word, verdict) VALUES (?, ?, ?)")
        .bind(txid)
        .bind(word)
        .bind(verdict.to_string())
        .execute(storage.writer())
        .await
        .unwrap();
}

/// P1's reasons and D's, each (the allocation's, the named input's).
type Reasons = (
    (Option<String>, Option<String>),
    (Option<String>, Option<String>),
);

/// One creator, one word: through each door, P's free coin P1 is held
/// exactly when D's coin is held by P's lineage.
fn agree((parent, child): &Reasons) {
    let by_lineage = |r: &Option<String>| r.as_deref().is_some_and(|r| r.starts_with(DEAD_LINEAGE));
    assert_eq!(
        parent.0.is_some(),
        by_lineage(&child.0),
        "the allocation: {parent:?} {child:?}"
    );
    assert_eq!(
        parent.1.is_some(),
        by_lineage(&child.1),
        "the named input: {parent:?} {child:?}"
    );
}

/// The named door over P1 and D in one call: (P1's reason, D's).
async fn named_pair(
    store: &Store,
    services: &MockWalletServices,
    opts: &GateOptions,
    (p, d): (&str, &str),
) -> (Option<String>, Option<String>) {
    let named = assess_named(
        UserCoins::new(&store.0, store.1 .0),
        services,
        opts,
        &[(p.to_string(), 1).into(), (d.to_string(), 0).into()],
        &[],
        None,
        false,
    )
    .await
    .unwrap();
    let reason = |t: &str| {
        named
            .iter()
            .find(|c| c.outpoint.starts_with(t))
            .map(|c| c.reason.clone())
    };
    (reason(p), reason(d))
}

/// P1 and D through each door, both coins in one call (the allocation
/// with the door's retry when `retry`), [`agree`]d. (The reasons, whether
/// the allocation read the header service once more.)
async fn agreeing_with(
    store: &Store,
    services: &MockWalletServices,
    opts: &GateOptions,
    (p, d): (&str, &str),
    retry: bool,
) -> (Reasons, bool) {
    let a = assess_retrying(
        UserCoins::new(&store.0, store.1 .0),
        services,
        opts,
        None,
        retry,
    )
    .await
    .unwrap();
    let named = named_pair(store, services, opts, (p, d)).await;
    let read = ((reason_of(&a, p), named.0), (reason_of(&a, d), named.1));
    agree(&read);
    (read, a.headers_retried)
}

/// [`agreeing_with`] without the retry.
async fn agreeing(
    store: &Store,
    services: &MockWalletServices,
    opts: &GateOptions,
    (p, d): (&str, &str),
) -> Reasons {
    agreeing_with(store, services, opts, (p, d), false).await.0
}

/// A checked proof outranks a stored dead word (`Tracker.lean`
/// `onProof_of_rootChecks`), for the walk as for the door (`Wallet.lean`
/// `State.skel` reads the one word): P conflicted beside a proof whose root
/// checks at 800, 101 deep, is offered as mined, and D with it, through
/// both doors, under every word.
#[tokio::test]
async fn a_checked_proof_outranks_a_stored_dead_word_for_the_walk_as_for_the_door() {
    let (store, (p, d), services) = conflicted_with_proof().await;
    for o in every_word(&d) {
        let read = agreeing(&store, &services, &o, (&p, &d)).await;
        assert_eq!(read, ((None, None), (None, None)), "under {o:?}");
    }
}

/// P conflicted beside a proof `services` does not clear: both readings
/// keep the stored word, the door holding P1 `held` and D held by P's
/// lineage, the allocation with the door's retry when `retry`. Whether it
/// read the header service once more.
async fn the_stored_word_stands(services: &MockWalletServices, held: &str, retry: bool) -> bool {
    let (store, (p, d), _) = conflicted_with_proof().await;
    let held = Some(held.to_string());
    let lineage = Some(format!("dead lineage: ancestor {p} conflicted"));
    let (read, retried) = agreeing_with(&store, services, &opts(1, true), (&p, &d), retry).await;
    assert_eq!(read, ((held.clone(), held), (lineage.clone(), lineage)));
    retried
}

/// The header service silent: P's proof is not checked, so P keeps its
/// stored word for both readings: the door holds P closed on the fault,
/// and D is held by P's lineage, never "could not verify now".
#[tokio::test]
async fn a_proof_the_headers_could_not_check_leaves_the_stored_word() {
    assert!(!the_stored_word_stands(&silent_tip(), &tip_fault(), false).await);
}

/// The planted proof's headers with the tip unreadable.
fn silent_tip() -> MockWalletServices {
    let services = headers_with(&"f3".repeat(32), 800, 900);
    services.set_tip_unavailable();
    services
}

/// The hold of a creator the silent tip left unchecked.
fn tip_fault() -> String {
    let fault = "Service error: mock: the chain tip header is unavailable";
    format!("{HEADERS_UNAVAILABLE_PREFIX}{fault}")
}

/// A proof whose root does not match the header at its height: P keeps
/// its stored word, `conflicted: never offered` at the door, and D is held
/// by P's lineage.
#[tokio::test]
async fn a_proof_whose_root_does_not_match_leaves_the_stored_word() {
    let services = headers_with(&"0d".repeat(32), 800, 900);
    the_stored_word_stands(&services, "conflicted: never offered", false).await;
}

/// A `failed` parent whose proof checks reads mined, and its child is
/// offered as before, now by the reading. The allocation never lists a
/// `failed` creator's coin (the toolbox's candidates are `completed` or
/// `unproven`), so P1 is absent there. The named door holds P1 on its failed
/// transaction's status before reading the proof; D still reads the proof.
/// This PR 213 agreement test moves for P: beta offered its named coin,
/// which the toolbox's `FailedAncestor` would refuse anyway. `kill` now uses
/// the toolbox's failure write, including `spendable = 0`, instead of a raw
/// transaction-status UPDATE.
#[tokio::test]
async fn a_failed_parent_whose_proof_checks_leaves_its_child_offered() {
    let store = wallet_store().await;
    let (p, d) = dead_parent_with_proof(&store, Death::Failed).await;
    let services = headers_with(&p, 800, 900);
    assert_eq!(
        doors_at(&store, &services, &opts(1, true), (&p, 1)).await,
        (
            None,
            Some("its transaction is failed: never offered".to_string())
        )
    );
    assert_eq!(
        both_doors(&store, &services, &opts(1, true), &d).await,
        (None, None)
    );
    // The same proof against a header it does not match: D held.
    let services = headers_with(&"0d".repeat(32), 800, 900);
    held_naming(&store, &services, &opts(1, true), &d, (&p, "failed")).await;
}

/// A parent `p` (made, or received) whose tracker row, keyed by the
/// lowercase txid as the tick writes it, says `conflicted`, and our child
/// spending its coin: the child is held naming `p`.
async fn a_conflicted_parent_holds_our_child(p: &str, made: bool) {
    let store = wallet_store().await;
    let d = "f7".repeat(32);
    let p0 = coin(&store.0, store.1, p, made, None, 6_000, 3).await;
    change(&store, &d).await;
    spends(&store.0, &d, p0).await;
    kill(&store.0, &p.to_ascii_lowercase(), Death::Word("conflicted")).await;
    let services = quiet_headers();
    held_naming(&store, &services, &opts(1, true), &d, (p, "conflicted")).await;
}

/// The toolbox's txid in upper case beside the tracker's lowercase row
/// (the tick's adoption writes `lower(txid)`): the walk still finds P.
#[tokio::test]
async fn an_uppercase_parent_txid_is_walked() {
    a_conflicted_parent_holds_our_child(&"F6".repeat(32), true).await;
}

/// A received parent whose word is conflicted: our D spending its coin is
/// dead (the Lean's `w.actions` holds received actions, and `skel` reads a
/// parent's word whatever its origin).
#[tokio::test]
async fn a_received_dead_parent_kills_our_child() {
    a_conflicted_parent_holds_our_child(&"f1".repeat(32), false).await;
}

/// The door's retry (#59), round three's finding: the tip silent on the
/// first read and back before the re-read. P, a candidate holding a proof
/// and P1's creator, is read once with the creators and re-read with them,
/// so the walk and the door take the re-read's word: P1 offered, and D
/// with it, through both doors.
#[tokio::test]
async fn the_retry_re_reads_a_candidate_with_the_creators() {
    let (store, (p, d), _) = conflicted_with_proof().await;
    let services = silent_tip();
    let gate = opts(1, true);
    let ((read, retried), ()) = tokio::join!(
        agreeing_with(&store, &services, &gate, (&p, &d), true),
        recover_after_300_ms(&services)
    );
    assert!(retried, "the header service was not read once more");
    assert_eq!(read, ((None, None), (None, None)));
}

/// The tip back 300 ms after the first read, before the retry's pause
/// ends.
async fn recover_after_300_ms(services: &MockWalletServices) {
    tokio::time::sleep(Duration::from_millis(300)).await;
    services.set_tip_header(header_with_hash(900, &"0c".repeat(32), 0));
}

/// The tip silent on the read and on the re-read: P keeps its stored word
/// for both readings, held on the fault at the door, and D is held by P's
/// lineage, through both doors.
#[tokio::test]
async fn a_tip_silent_on_the_retry_leaves_the_stored_word() {
    assert!(the_stored_word_stands(&silent_tip(), &tip_fault(), true).await);
}

/// The named door takes one read of the headers for the creators and the
/// candidates: a tip that answers once and fails after splits nothing. P1
/// and D agree, whichever read answered, and the tip is asked once.
#[tokio::test]
async fn the_named_door_reads_the_creators_and_the_candidates_once() {
    let (store, (p, d), services) = conflicted_with_proof().await;
    let done = std::sync::atomic::AtomicBool::new(false);
    let fail_after_one = async {
        let tip = || services.call_count("get_chain_tip_header");
        while tip() == 0 && !done.load(std::sync::atomic::Ordering::SeqCst) {
            tokio::task::yield_now().await;
        }
        services.set_tip_unavailable();
    };
    let named = async {
        let named = named_pair(&store, &services, &opts(1, true), (&p, &d)).await;
        done.store(true, std::sync::atomic::Ordering::SeqCst);
        named
    };
    let ((parent, child), ()) = tokio::join!(named, fail_after_one);
    agree(&((None, parent), (None, child)));
    assert_eq!(services.call_count("get_chain_tip_header"), 1);
}

/// The header service's calls for one read through each door.
async fn header_calls(store: &Store, services: &MockWalletServices, (p, d): (&str, &str)) -> usize {
    services.reset_history();
    agreeing(store, services, &opts(1, true), (p, d)).await;
    services.call_count("get_chain_tip_header") + services.call_count("get_header_for_height")
}

/// The walk asks the header service nothing of its own: a candidate with
/// no proof costs no call, and a candidate holding a proof that is also
/// the creator of a judged coin (P1) is read once, as that creator alone.
#[tokio::test]
async fn a_candidate_is_read_no_more_than_a_creator() {
    let store = wallet_store().await;
    let (p, d, _) = parent_and_child(&store, None).await;
    second_output(&store, &p, 1).await;
    let services = quiet_headers();
    let live = header_calls(&store, &services, (&p, &d)).await;
    node_verdict(&store.0, &p, "conflicted").await;
    assert_eq!(header_calls(&store, &services, (&p, &d)).await, live);

    let (store, (p, d), services) = conflicted_with_proof().await;
    let candidate = header_calls(&store, &services, (&p, &d)).await;
    sqlx::query("DELETE FROM tracker_states WHERE txid = ?")
        .bind(&p)
        .execute(store.0.writer())
        .await
        .unwrap();
    assert_eq!(header_calls(&store, &services, (&p, &d)).await, candidate);
}

/// A wallet over [`conflicted_with_proof`]'s plant with the tip silent,
/// and a createAction naming D's coin alone, P only its lineage's
/// candidate: the gate's answer, whether the toolbox was asked, P's txid,
/// and the number of tip calls. The tip comes back 300 ms in when `recovers`.
async fn named_through_create_action(
    recovers: bool,
) -> (
    std::result::Result<CreateActionResult, GateError>,
    bool,
    String,
    usize,
) {
    let root = PrivateKey::random();
    let store = store_for(&root.public_key().to_hex()).await;
    let (p, d) = dead_parent_with_proof(&store, Death::Word("conflicted")).await;
    let wallet = Wallet::new(Some(root), store.0, silent_tip())
        .await
        .unwrap();
    let mut args = pay(1_000);
    args.inputs = Some(vec![named_input(&d)]);
    let gate = opts(1, true);
    let door = at_the_door(create_action(&wallet, args, "gate.local", &gate));
    let (result, asked) = if recovers {
        tokio::join!(door, recover_after_300_ms(wallet.services())).0
    } else {
        door.await
    };
    (
        result,
        asked,
        p,
        wallet.services().call_count("get_chain_tip_header"),
    )
}

/// Round four's finding: the named door's retry re-reads a lineage
/// candidate as the allocation's does. The tip silent at first and back
/// before the retry: P's proof checks on the re-read, so D named alone
/// passes the gate to the toolbox, as the allocation offers it
/// ([`the_retry_re_reads_a_candidate_with_the_creators`]).
#[tokio::test]
async fn the_named_door_retries_a_candidate_as_the_allocation_does() {
    let (result, asked, _, _) = named_through_create_action(true).await;
    if let Err(GateError::Refused(refusal)) = &result {
        panic!(
            "the named door refused D after the retry: {:?}",
            refusal.held
        );
    }
    assert!(asked, "the toolbox was not asked: {result:?}");
}

/// The tip silent throughout: P keeps its stored word after the retry, and
/// D named alone is refused 403 naming P, never "could not verify now".
#[tokio::test]
async fn the_named_door_silent_throughout_refuses_naming_the_ancestor() {
    let (result, asked, p, tip_calls) = named_through_create_action(false).await;
    assert_eq!(tip_calls, 2, "the named door did not retry the silent tip");
    let Err(GateError::Refused(refusal)) = result else {
        panic!("D passed the gate with P unread: {result:?}");
    };
    assert_eq!(refusal.code(), "INPUT_NOT_OFFERED");
    assert_eq!(refusal.headers_fault, None);
    assert_eq!(
        refusal
            .held
            .iter()
            .map(|c| c.reason.clone())
            .collect::<Vec<_>>(),
        vec![format!("dead lineage: ancestor {p} conflicted")]
    );
    assert!(!asked, "the toolbox was asked");
}

/// The header service's calls for one read through each door with the
/// retry, the tip silent throughout: the allocation over P1 and D, the
/// named door over D alone (P its candidate). Both read P and D, and
/// re-read them once: the same calls.
#[tokio::test]
async fn the_named_door_retries_with_the_allocations_header_calls() {
    let (store, (p, d), _) = conflicted_with_proof().await;
    let services = silent_tip();
    let calls = || {
        services.call_count("get_chain_tip_header") + services.call_count("get_header_for_height")
    };
    let gate = opts(1, true);
    services.reset_history();
    let a = assess_retrying(
        UserCoins::new(&store.0, store.1 .0),
        &services,
        &gate,
        None,
        true,
    )
    .await
    .unwrap();
    assert!(a.headers_retried);
    let allocation = calls();
    services.reset_history();
    let held = assess_named(
        UserCoins::new(&store.0, store.1 .0),
        &services,
        &gate,
        &[(d, 0).into()],
        &[],
        None,
        true,
    )
    .await
    .unwrap();
    assert_eq!(
        held[0].reason,
        format!("dead lineage: ancestor {p} conflicted")
    );
    assert_eq!(calls(), allocation);
}

mod proof_scope;
