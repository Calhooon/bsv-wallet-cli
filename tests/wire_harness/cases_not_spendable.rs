//! A coin the store marks not spendable with no spender, at the served
//! `POST /createAction` under `options.acceptDelayedBroadcast: true`
//! (#646, the hold of #165), in the `not-spendable` world: 402
//! `INSUFFICIENT_FUNDS`, nothing offered, the coin in `held[]` with the
//! word `lost`. The stable line's 0.18.9, which has no hold, answers this
//! request with a signed spend of the coin stored `sending` (#646's
//! table). The world has no broadcaster (a closed port) and the action is
//! signed, so an answer other than the refusal would be that spend, kept
//! for a later post. Recorded last, in a world of its own, so no file before it moves.
//! What a served request cannot show (the store after the refusal, the
//! monitor's send pass, the action posted at once, a coin the spend guard
//! holds) is `src/spend_gate/tests/not_spendable.rs`.
use std::collections::HashMap;

use bsv_sdk::primitives::PrivateKey;
use bsv_wallet_toolbox::StorageSqlx;
use serde_json::json;

use super::recorder::{post, Recorder};
use super::worlds::{funded_lock, parent_tx, Rows, P2PKH};

const WORLD: &str = "not-spendable";
/// The world's one coin: (seed of the parent's input, height mined,
/// satoshis, derivation suffix under the funded coins' prefix).
const COIN: (u8, u32, u64, &str) = (0xb6, 899_871, 50_000, "SA==");

/// The `not-spendable` world's rows: one mined coin the wallet can sign
/// for, written as `funded`'s are (a one-leaf block of its own, which the
/// header stub answers), then marked as `reconcile-outputs --execute`,
/// `cleanup-abandoned --execute` and the spend guard mark a coin never to
/// be selected: `spendable = 0`, no spender.
pub(crate) async fn world(
    storage: &StorageSqlx,
    identity: &str,
    key: &PrivateKey,
) -> HashMap<u32, String> {
    let (seed, height, satoshis, suffix) = COIN;
    let rows = Rows::open(storage, identity).await;
    let parent = parent_tx(seed, satoshis, &funded_lock(key, suffix));
    let txid = rows
        .mined_coin(&parent, height, "a coin marked lost", Some(suffix))
        .await;
    let marked =
        sqlx::query("UPDATE outputs SET spendable = 0 WHERE txid = ? AND spent_by IS NULL")
            .bind(&txid)
            .execute(&rows.pool)
            .await
            .unwrap()
            .rows_affected();
    assert_eq!(marked, 1, "the world's one coin");
    HashMap::from([(height, txid)])
}

pub(crate) async fn record(r: &mut Recorder) {
    let refused = r
        .case(
            "createAction.notSpendable.delayedBroadcast",
            WORLD,
            post(
                "/createAction",
                json!({"description": "wire not spendable",
                    "outputs": [{"lockingScript": P2PKH, "outputDescription": "an output",
                        "satoshis": 1000}],
                    "options": {"acceptDelayedBroadcast": true, "signAndProcess": true,
                        "randomizeOutputs": false}}),
            ),
        )
        .await;
    assert_eq!(refused.value("/code"), "INSUFFICIENT_FUNDS");
    assert_eq!(refused.value("/offeredSatoshis"), 0);
    assert_eq!(refused.value("/held/0/word"), "lost");
    assert_eq!(
        refused.value("/held/0/reason"),
        "lost: the store marks it not spendable"
    );
}
