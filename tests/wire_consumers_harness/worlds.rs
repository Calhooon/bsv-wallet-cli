//! The worlds a consumer's fixtures are served from.

use std::collections::HashMap;

use bsv_sdk::primitives::PrivateKey;
use bsv_wallet_cli::server::{self, ServerConfig};
use bsv_wallet_toolbox::{StorageSqlx, Wallet, WalletStorageWriter};

use super::cases::G;
use super::fixture::NORMALIZER;
use crate::wire_support::{header_stub, services, Req, Resp, CLOSED, ROOT_KEY};

// ---------------------------------------------------------------------------
// The worlds.
// ---------------------------------------------------------------------------

/// The bearer of the `auth` world.
pub(crate) const BEARER: &str = "wire-bearer";

/// The header stub's root, at every height.
const HEADER_ROOT: &str = "5555555555555555555555555555555555555555555555555555555555555555";

/// The seeded coins (hand-written rows, never broadcast): a received
/// payment not yet mined, and a second one (the scoped word names one).
pub(crate) const PENDING_A: &str =
    "7777777777777777777777777777777777777777777777777777777777777777";
pub(crate) const PENDING_B: &str =
    "8888888888888888888888888888888888888888888888888888888888888888";
/// A txid the wallet has never seen.
pub(crate) const UNKNOWN: &str = "3333333333333333333333333333333333333333333333333333333333333333";
/// The received coins' script: not the key their derivation gives, so a
/// spend the gate lets through stops at signing with a stable answer
/// (signed, its change's fresh derivation would move the txid each run).
const P2PKH: &str = "76a914dbc0a7c84983c5bf199b7b2d41b3acf0408ee5aa88ac";

pub(crate) struct World {
    router: axum::Router,
}

/// Two received coins not yet mined (no proof stored), as an
/// `internalizeAction` of a payment leaves them: 30,000 and 20,000 sats.
async fn seed_pending(storage: &StorageSqlx, identity: &str) {
    let user = storage
        .find_or_insert_user(identity)
        .await
        .unwrap()
        .0
        .user_id;
    let basket = storage.find_or_create_default_basket(user).await.unwrap();
    let pool = storage.writer().clone();
    let when = chrono::DateTime::parse_from_rfc3339("2026-10-23T00:00:00Z")
        .unwrap()
        .with_timezone(&chrono::Utc);
    for (txid, satoshis) in [(PENDING_A, 30_000i64), (PENDING_B, 20_000)] {
        let tx_row = sqlx::query(
            "INSERT INTO transactions (user_id, status, reference, is_outgoing, satoshis, version, lock_time, description, txid, raw_tx, created_at, updated_at) \
             VALUES (?, 'unproven', ?, 0, ?, 1, 0, 'a received payment', ?, X'01000000', ?, ?)",
        )
        .bind(user)
        .bind(format!("ref-{}", &txid[..8]))
        .bind(satoshis)
        .bind(txid)
        .bind(when)
        .bind(when)
        .execute(&pool)
        .await
        .unwrap()
        .last_insert_rowid();
        sqlx::query(
            "INSERT INTO outputs (user_id, transaction_id, basket_id, vout, satoshis, locking_script, txid, type, spendable, change, provided_by, purpose, output_description, derivation_prefix, derivation_suffix, sender_identity_key, created_at, updated_at) \
             VALUES (?, ?, ?, 0, ?, ?, ?, 'P2PKH', 1, 1, 'storage', 'change', 'c', 'SfKxPIJNgdI=', 'NaGLC6fMH50=', ?, ?, ?)",
        )
        .bind(user)
        .bind(tx_row)
        .bind(basket.basket_id)
        .bind(satoshis)
        .bind(hex::decode(P2PKH).unwrap())
        .bind(txid)
        .bind(G)
        .bind(when)
        .bind(when)
        .execute(&pool)
        .await
        .unwrap();
    }
}

impl World {
    /// The worlds, by name:
    /// - `fresh`: an empty wallet, no bearer, the header stub up.
    /// - `auth`: `fresh` behind the bearer [`BEARER`].
    /// - `pending`: two received coins not yet mined ([`PENDING_A`],
    ///   [`PENDING_B`]), the payments a funder's sweeps leave.
    pub(crate) async fn build(name: &str) -> World {
        let key = PrivateKey::from_hex(ROOT_KEY).unwrap();
        let identity = key.public_key().to_hex();
        let storage = StorageSqlx::in_memory().await.unwrap();
        storage.migrate("wire", &identity).await.unwrap();
        storage.make_available().await.unwrap();
        // The tracker's table, made by a read of it.
        bsv_wallet_cli::tracker_host::stored_word(&storage, UNKNOWN)
            .await
            .unwrap();
        match name {
            "fresh" | "auth" => {}
            "pending" => seed_pending(&storage, &identity).await,
            other => panic!("no world named {other}"),
        }
        let services = services(
            header_stub(false, HashMap::new(), HEADER_ROOT).await,
            CLOSED.to_string(),
        );
        let wallet = Wallet::new(Some(key), storage, services).await.unwrap();
        let mut config = ServerConfig {
            settings: crate::wire_support::quiet_config(),
            ..ServerConfig::default()
        };
        if name == "auth" {
            config.auth_token = Some(BEARER.to_string());
        }
        World {
            router: server::make_router(server::make_wallet_state(wallet), config),
        }
    }

    pub(crate) async fn serve(&self, req: &Req) -> Resp {
        NORMALIZER.serve(&self.router, req).await.0
    }
}
