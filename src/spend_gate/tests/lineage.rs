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
    let (sql, value) = match death {
        Death::Word(word) => (
            "INSERT INTO tracker_states (txid, word) VALUES (?, ?)",
            word,
        ),
        Death::Failed => (
            "UPDATE transactions SET status = ? WHERE txid = ?",
            "failed",
        ),
    };
    let query = sqlx::query(sql);
    let query = match death {
        Death::Word(_) => query.bind(txid).bind(value),
        Death::Failed => query.bind(value).bind(txid),
    };
    query.execute(storage.writer()).await.unwrap();
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
    storage: &StorageSqlx,
    services: &MockWalletServices,
    opts: &GateOptions,
    txid: &str,
) -> (Option<String>, Option<String>) {
    let a = assess(storage, services, opts, None).await.unwrap();
    let named = assess_named(storage, services, opts, &[(txid.to_string(), 0)], &[], None)
        .await
        .unwrap();
    (reason_of(&a, txid), named.first().map(|c| c.reason.clone()))
}

/// Both doors hold `txid`'s coin under `opts`, naming `ancestor` dead by
/// `word`.
async fn held_naming(
    storage: &StorageSqlx,
    services: &MockWalletServices,
    opts: &GateOptions,
    txid: &str,
    (ancestor, word): (&str, &str),
) {
    let want = Some(format!("dead lineage: ancestor {ancestor} {word}"));
    assert_eq!(
        both_doors(storage, services, opts, txid).await,
        (want.clone(), want)
    );
}

/// The host's own-change rule and the action's received word, plain and
/// scoped to `txid`'s coin: every rule and word the door reads.
fn every_word(txid: &str) -> Vec<GateOptions> {
    vec![
        opts(1, true),
        opts(1, true).with_allow_received(true),
        opts(1, true).with_allow_pending(AllowPending::ReceivedOnly(vec![(txid.to_string(), 0)])),
    ]
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
        held_naming(&store.0, &services, &o, &d, (&p, "conflicted")).await;
    }
    assert_eq!(stored_rows(&store.0).await, before, "the view wrote a row");

    // D mined, its block 101 deep: still held.
    let (q, m) = ("e3".repeat(32), "e4".repeat(32));
    let q0 = change(&store, &q).await;
    coin(&store.0, store.1, &m, true, Some(800), 6_000, 3).await;
    spends(&store.0, &m, q0).await;
    kill(&store.0, &q, Death::Word("conflicted")).await;
    let services = headers_with(&m, 800, 900);
    held_naming(&store.0, &services, &opts(1, true), &m, (&q, "conflicted")).await;
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
    held_naming(&store.0, &services, &opts(1, true), &e, (&p, "conflicted")).await;
}

/// The other dead words: a parent `rejected` by the tracker, a parent the
/// toolbox holds `failed` (the CLI's abandon). A `failed` parent with a
/// proof in `proven_txs` is mined to the tracker, and its child offered.
#[tokio::test]
async fn a_child_of_a_rejected_or_failed_parent_is_held() {
    for (death, word) in [
        (Death::Word("rejected"), "rejected"),
        (Death::Failed, "failed"),
    ] {
        let store = wallet_store().await;
        let (p, d, _) = parent_and_child(&store, Some(death)).await;
        held_naming(&store.0, &quiet_headers(), &opts(1, true), &d, (&p, word)).await;
    }

    let store = wallet_store().await;
    let (p, d) = ("e6".repeat(32), "e7".repeat(32));
    let p0 = coin(&store.0, store.1, &p, true, Some(800), 6_000, 3).await;
    change(&store, &d).await;
    spends(&store.0, &d, p0).await;
    kill(&store.0, &p, Death::Failed).await;
    let (offer, named) = both_doors(&store.0, &quiet_headers(), &opts(1, true), &d).await;
    assert_eq!((offer, named), (None, None));
}

/// A child of a live parent is offered as before; with the rule off, the
/// door's own reason, byte for byte.
#[tokio::test]
async fn a_child_of_a_live_parent_is_offered_as_before() {
    let store = wallet_store().await;
    let (_, d, _) = parent_and_child(&store, None).await;
    let services = quiet_headers();
    assert_eq!(
        both_doors(&store.0, &services, &opts(1, true), &d).await,
        (None, None)
    );
    let off = Some("depth 1: 0 blocks; own change, SPEND_OWN_CHANGE=0".to_string());
    assert_eq!(
        both_doors(&store.0, &services, &opts(1, false), &d).await,
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
        both_doors(&store.0, &services, &opts(1, true), &r).await,
        (held.clone(), held)
    );
    let word = opts(1, true).with_allow_received(true);
    assert_eq!(
        both_doors(&store.0, &services, &word, &r).await,
        (None, None)
    );
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
    let (a, b) = ("ea".repeat(32), "eb".repeat(32));
    let a0 = change(&store, &a).await;
    let b0 = change(&store, &b).await;
    spends(&store.0, &b, a0).await;
    spends(&store.0, &a, b0).await;
    second_output(&store, &b, 1).await;
    let walk = || async {
        tokio::time::timeout(Duration::from_secs(5), dead_lineages(store.0.pool()))
            .await
            .expect("the walk hung")
            .unwrap()
    };
    assert!(walk().await.is_empty());
    let a1 = assess(&store.0, &quiet_headers(), &opts(1, true), None)
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
    let held = assess(&store.0, &quiet_headers(), &opts(1, true), None)
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
        wallet.storage(),
        wallet.services(),
        &opts(1, true),
        &[(d, 0)],
        &[],
        None,
    )
    .await
    .unwrap();
    assert_eq!(
        held[0].reason,
        format!("dead lineage: ancestor {p} conflicted")
    );
    assert_eq!(held[0].headers_fault, None);
}
