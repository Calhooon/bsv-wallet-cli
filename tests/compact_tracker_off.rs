//! P0-1c (bsv-stack-lean #35, #48): `bsv-wallet compact` writes merkle
//! proofs into stored input BEEFs, so it is a proof store and must refuse
//! what the toolbox's ingest refuses. With no chain tracker
//! (`CHAINTRACKS_URL=off`) a stored proof that no tracker ever checked must
//! not be written into a stored BEEF.
//!
//! The binary is run as a user runs it: a throwaway wallet made by `init` in
//! a temp dir, its database seeded with one completed request whose stored
//! input BEEF carries the parent as a raw leg, and `proven_txs` holding an
//! unchecked proof for that parent; then `compact` under `off`. No network.

use std::process::Command;

use bsv_sdk::primitives::{sha256d, to_hex};
use bsv_sdk::transaction::{Beef, MerklePath, MerklePathLeaf, BEEF_V1};
use tempfile::TempDir;

fn wallet(dir: &TempDir) -> Command {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_bsv-wallet"));
    cmd.current_dir(dir.path())
        .env_remove("ROOT_KEY")
        .env_remove("ARC_MODE")
        .env_remove("ARCADE")
        .env_remove("ARC_URL")
        .env("RUST_LOG", "warn");
    cmd
}

fn txid_of(raw: &[u8]) -> String {
    let mut h = sha256d(raw).to_vec();
    h.reverse();
    to_hex(&h)
}

/// A one-input, one-output transaction spending `prev` (txid hex, display
/// order) with an output script of `script_len` bytes (OP_RETURN padding).
fn tx_spending(prev: &str, script_len: usize) -> Vec<u8> {
    let mut raw = Vec::new();
    raw.extend_from_slice(&1u32.to_le_bytes()); // version
    raw.push(1); // vin count
    let mut p = hex::decode(prev).unwrap();
    p.reverse();
    raw.extend_from_slice(&p);
    raw.extend_from_slice(&0u32.to_le_bytes()); // vout
    raw.push(0); // script len
    raw.extend_from_slice(&0xffff_ffffu32.to_le_bytes()); // sequence
    raw.push(1); // vout count
    raw.extend_from_slice(&1000u64.to_le_bytes()); // value
    if script_len == 0 {
        raw.push(0);
    } else {
        raw.push(0xfe);
        raw.extend_from_slice(&(script_len as u32).to_le_bytes());
        raw.push(0x6a); // OP_RETURN
        raw.extend(vec![0u8; script_len - 1]);
    }
    raw.extend_from_slice(&0u32.to_le_bytes()); // locktime
    raw
}

/// A two-leaf path at `height`: `a` at offset 0, `b` at offset 1; `txid`
/// flags which leaf is a proven transaction.
fn two_leaf_path(height: u32, a: (&str, bool), b: (&str, bool)) -> MerklePath {
    let leaf = |offset, (hash, txid): (&str, bool)| MerklePathLeaf {
        offset,
        hash: Some(hash.to_string()),
        txid,
        duplicate: false,
    };
    MerklePath {
        block_height: height,
        path: vec![vec![leaf(0, a), leaf(1, b)]],
    }
}

#[tokio::test]
async fn compact_with_chaintracks_off_writes_no_unchecked_proof_into_a_stored_beef() {
    let dir = TempDir::new().expect("temp dir");
    let init = wallet(&dir)
        .args(["--db", "wallet.db", "init"])
        .output()
        .expect("run init");
    assert!(
        init.status.success(),
        "init: {}",
        String::from_utf8_lossy(&init.stderr)
    );

    // The parent P, mined at 500 next to a sibling S; the child C spends P.
    let parent = tx_spending(&"11".repeat(32), 0);
    let p = txid_of(&parent);
    let s = "22".repeat(32);
    let child = tx_spending(&p, 0);
    let c = txid_of(&child);
    // Dead weight no input of C reaches, so the stored BEEF is large.
    let filler = tx_spending(&"33".repeat(32), 60_000);

    // The stored input BEEF (Atomic, for C): the block's path already
    // holds P as a sibling hash (it proves S), and P rides as a raw leg.
    let mut stored = Beef::with_version(BEEF_V1);
    stored.merge_bump(two_leaf_path(500, (&p, false), (&s, true)));
    stored.merge_raw_tx(parent.clone(), None);
    stored.merge_raw_tx(filler, None);
    stored.merge_raw_tx(child.clone(), None);
    let stored_bytes = stored.to_binary_atomic(&c).expect("atomic BEEF");
    assert!(
        Beef::from_binary(&stored_bytes)
            .unwrap()
            .find_txid(&p)
            .and_then(|t| t.bump_index())
            .is_none(),
        "precondition: P rides unproven"
    );

    // P's proof in proven_txs, stored by code that checked nothing: no
    // record in proof_root_checks.
    let db = dir.path().join("wallet.db");
    let pool = sqlx::SqlitePool::connect(&format!("sqlite:{}", db.display()))
        .await
        .expect("open wallet.db");
    let proof = two_leaf_path(500, (&p, true), (&s, false));
    let root = proof.compute_root(Some(&p)).unwrap();
    let now = chrono::Utc::now();
    sqlx::query(
        "INSERT INTO proven_txs (txid, height, idx, block_hash, merkle_root, merkle_path, raw_tx, created_at, updated_at) VALUES (?, 500, 0, ?, ?, ?, ?, ?, ?)",
    )
    .bind(&p)
    .bind("ab".repeat(32))
    .bind(&root)
    .bind(proof.to_binary())
    .bind(&parent)
    .bind(now)
    .bind(now)
    .execute(&pool)
    .await
    .expect("seed the unchecked proof");
    sqlx::query(
        "INSERT INTO proven_tx_reqs (txid, status, attempts, history, notified, notify, raw_tx, input_beef, created_at, updated_at) VALUES (?, 'completed', 0, '{}', 0, '{}', ?, ?, ?, ?)",
    )
    .bind(&c)
    .bind(&child)
    .bind(&stored_bytes)
    .bind(now)
    .bind(now)
    .execute(&pool)
    .await
    .expect("seed the completed request");
    pool.close().await;

    let out = wallet(&dir)
        .env("CHAINTRACKS_URL", "off")
        .args(["--db", "wallet.db", "compact"])
        .output()
        .expect("run compact");
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(out.status.success(), "compact: {stdout}{stderr}");

    let pool = sqlx::SqlitePool::connect(&format!("sqlite:{}", db.display()))
        .await
        .expect("reopen wallet.db");
    let (after,): (Vec<u8>,) =
        sqlx::query_as("SELECT input_beef FROM proven_tx_reqs WHERE txid = ?")
            .bind(&c)
            .fetch_one(&pool)
            .await
            .unwrap();
    let after = Beef::from_binary(&after).expect("the stored BEEF parses");
    assert!(
        after.find_txid(&p).and_then(|t| t.bump_index()).is_none(),
        "no unchecked proof is written into a stored BEEF with no tracker: {stdout}"
    );
    let unchanged: Option<(i64,)> =
        sqlx::query_as("SELECT 1 FROM proven_tx_reqs WHERE txid = ? AND input_beef = ?")
            .bind(&c)
            .bind(&stored_bytes)
            .fetch_optional(&pool)
            .await
            .unwrap();
    assert!(unchanged.is_some(), "the stored BEEF is untouched");
    pool.close().await;
}
