//! The `internalize-held` world (toolbox #823's `### Hosts`): two change
//! rows of the default basket, kept apart from the `internalize` world so
//! its listings do not move (wire rows 251 to 254).

use std::collections::HashMap;

use bsv_sdk::primitives::PrivateKey;
use bsv_wallet_toolbox::StorageSqlx;

use super::worlds::{funded_key, funded_lock, parent_tx, Rows};

/// The `internalize-held` world's two change rows of the default basket
/// (toolbox #823's `### Hosts`): (seed of the parent's input, height mined,
/// satoshis, derivation suffix under `FUNDED_PREFIX`). Each is its own
/// mined transaction, as `funded`'s coins are.
pub(crate) const HELD_COINS: [(u8, u32, u64, &str); 2] = [
    (0xf1, 899_881, 8_000, "Rg=="),
    (0xf2, 899_882, 6_000, "Rw=="),
];

/// A [`HELD_COINS`] transaction: coin 0 pays the P2PKH of its derived key,
/// coin 1 that key's P2PK, a script that is not the P2PKH of its derived
/// key.
pub(crate) fn held_coin(key: &PrivateKey, i: usize) -> bsv_sdk::transaction::Transaction {
    let (seed, _, satoshis, suffix) = HELD_COINS[i];
    let lock = if i == 0 {
        funded_lock(key, suffix)
    } else {
        [
            &[0x21][..],
            &funded_key(key, suffix).to_compressed(),
            &[0xac][..],
        ]
        .concat()
    };
    parent_tx(seed, satoshis, &lock)
}

/// The `internalize-held` world, the two rows toolbox #823's `### Hosts`
/// names, each a mined coin written as change of the default basket
/// (`P2PKH`, change, its derivation) and then changed with an SQL update,
/// as the `internalize` world's earlier insertions are: [`HELD_COINS`] 0,
/// change with a derivation prefix and suffix and another `provided_by`
/// and `purpose` than `storage` and `change`; and [`HELD_COINS`] 1, a
/// payment admitted before the wallet payment rule (#819), change of the
/// default basket whose script is not the P2PKH of its derived key.
/// Kept apart from `internalize`, whose listings it would otherwise move.
pub(crate) async fn seed_internalize_held(
    storage: &StorageSqlx,
    identity: &str,
    key: &PrivateKey,
) -> HashMap<u32, String> {
    let rows = Rows::open(storage, identity).await;
    let mut roots = HashMap::new();
    for (i, (_, height, _, suffix)) in HELD_COINS.into_iter().enumerate() {
        let txid = rows
            .mined_coin(
                &held_coin(key, i),
                height,
                "a held change row",
                Some(suffix),
            )
            .await;
        roots.insert(height, txid.clone());
        let (provided_by, purpose) = if i == 0 {
            ("you", "receive")
        } else {
            ("storage", "receive")
        };
        sqlx::query("UPDATE outputs SET provided_by = ?, purpose = ? WHERE txid = ?")
            .bind(provided_by)
            .bind(purpose)
            .bind(&txid)
            .execute(&rows.pool)
            .await
            .unwrap();
    }
    roots
}
