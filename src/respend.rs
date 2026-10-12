//! The host's deliberate competitor (bsv-stack-lean #67).
//!
//! The host signs a different spend, never retires the announced spend.
//! Its exact inputs stay reserved until the toolbox's competitor-proof path
//! writes doubleSpend. The replacement uses the same process/post/classify
//! path as createAction; broadcaster refusals remain retryable hints.
use std::collections::HashSet;

use anyhow::{anyhow, ensure, Context, Result};
use bsv_sdk::primitives::{PrivateKey, PublicKey};
use bsv_sdk::script::template::SignOutputs;
use bsv_sdk::script::templates::P2PKH;
use bsv_sdk::transaction::{Beef, Transaction, TransactionOutput};
use bsv_sdk::wallet::{Counterparty, KeyDeriver, Protocol, SecurityLevel};
use bsv_wallet_toolbox::storage::StorageProcessActionArgs;
use bsv_wallet_toolbox::{
    classify_broadcast_results, AuthId, BroadcastOutcome, BroadcastStatus, ProofIngestOutcome,
    StorageSqlx, WalletServices, WalletStorageWriter,
};
use chrono::Utc;
use sqlx::Row;

// The pinned toolbox 0.7.4's create_action.rs:48,429. A signature can use
// 108 bytes, so reserve that upper bound rather than underpay a long DER.
const WALLET_FEE_SAT_PER_KB: u64 = 101;
const PROTOCOL: &str = "3241645161d8";

async fn schema(storage: &StorageSqlx) -> Result<()> {
    sqlx::query("CREATE TABLE IF NOT EXISTS respend_links (phantom_id INTEGER PRIMARY KEY REFERENCES transactions(transaction_id), replacement_id INTEGER NOT NULL UNIQUE REFERENCES transactions(transaction_id), settled INTEGER NOT NULL DEFAULT 0)")
        .execute(storage.pool()).await?;
    Ok(())
}

async fn require_silence(
    storage: &StorageSqlx,
    services: &dyn WalletServices,
    txid: &str,
    expected_sources: &[&str],
) -> Result<()> {
    ensure!(
        !expected_sources.is_empty(),
        "respend refused: status sources not specified"
    );
    let answers = services.get_broadcaster_statuses(txid).await;
    ensure!(
        !answers.is_empty(),
        "respend refused: status sources unavailable"
    );
    ensure!(
        !answers.iter().any(|(_, s)| matches!(
            s,
            BroadcastStatus::Accepted | BroadcastStatus::Seen | BroadcastStatus::Mined
        )),
        "respend refused: a status source holds the phantom"
    );
    for provider in expected_sources {
        ensure!(
            answers.iter().any(|(name, _)| name == provider),
            "respend refused: a configured status source is unavailable"
        );
    }
    // A provider that once accepted it must answer now as well. The toolbox
    // omits an errored provider from this API; omission is not absence.
    let known: Vec<String> = sqlx::query_scalar("SELECT provider FROM broadcast_seen WHERE txid=? AND status IN ('accepted','seen','mined')")
        .bind(txid).fetch_all(storage.pool()).await?;
    for provider in known {
        // These are aggregate memory markers, not broadcaster endpoints.
        // Current chain presence is asked by get_status_for_txids below;
        // a historical chain marker cannot make a stale proof current.
        if provider == bsv_wallet_toolbox::BROADCAST_PROVIDER_NETWORK
            || provider == bsv_wallet_toolbox::BROADCAST_PROVIDER_CHAIN
        {
            continue;
        }
        ensure!(
            answers.iter().any(|(name, _)| name == &provider),
            "respend refused: a previous status source is unavailable"
        );
    }
    let status = services
        .get_status_for_txids(&[txid.to_string()], false)
        .await
        .context("respend refused: status lookup unavailable")?;
    ensure!(
        status.status == "success" && status.error.is_none(),
        "respend refused: status lookup unavailable"
    );
    let detail = status
        .results
        .iter()
        .find(|r| r.txid.eq_ignore_ascii_case(txid))
        .context("respend refused: status lookup gave no answer")?;
    ensure!(
        detail.status == "unknown",
        "respend refused: a status source holds the phantom"
    );
    Ok(())
}

/// Build a competitor from the wallet's exact reserved inputs and post it.
/// No input is released and no transaction word is written on the phantom.
pub async fn build_and_post(
    storage: &StorageSqlx,
    services: &dyn WalletServices,
    root: &PrivateKey,
    txid: &str,
    expected_sources: &[&str],
) -> Result<String> {
    let decoded = hex::decode(txid).context("respend requires a hex txid")?;
    ensure!(decoded.len() == 32, "respend requires a 32-byte txid");
    let txid = txid.to_ascii_lowercase();
    let identity = root.public_key().to_hex();
    let row = sqlx::query("SELECT t.transaction_id,t.user_id,t.status,t.raw_tx,COALESCE(t.input_beef,r.input_beef) AS input_beef,r.status AS req_status,r.history FROM transactions t JOIN users u ON u.user_id=t.user_id JOIN proven_tx_reqs r ON r.txid=t.txid WHERE t.txid=? AND u.identity_key=?")
        .bind(&txid).bind(&identity).fetch_optional(storage.pool()).await?
        .context("respend refused: phantom is not this wallet's tracked transaction")?;
    let phantom_id: i64 = row.get("transaction_id");
    let user_id: i64 = row.get("user_id");
    let word: String = row.get("status");
    let req_word: String = row.get("req_status");
    ensure!(
        matches!(word.as_str(), "unproven" | "sending")
            && !matches!(req_word.as_str(), "completed" | "doubleSpend" | "invalid"),
        "respend refused: phantom already has proof or a terminal word"
    );
    ensure!(
        crate::retire_guard::broadcaster_took(storage.pool(), &txid).await?,
        "respend refused: no broadcaster took this transaction"
    );
    schema(storage).await?;
    let previous: Option<(i64,String,String)> = sqlx::query_as("SELECT l.replacement_id,t.status,t.txid FROM respend_links l JOIN transactions t ON t.transaction_id=l.replacement_id WHERE l.phantom_id=?").bind(phantom_id).fetch_optional(storage.pool()).await?;
    if let Some((_, status, previous_txid)) = &previous {
        ensure!(
            status == "failed"
                && !crate::retire_guard::broadcaster_took(storage.pool(), previous_txid).await?,
            "respend refused: replacement already tracked; its cadence retries it"
        );
    }
    require_silence(storage, services, &txid, expected_sources).await?;

    let raw: Vec<u8> = row
        .get::<Option<Vec<u8>>, _>("raw_tx")
        .context("respend refused: phantom bytes unavailable")?;
    let phantom = Transaction::from_binary(&raw)?;
    ensure!(
        phantom.id() == txid && !phantom.inputs.is_empty(),
        "respend refused: invalid phantom bytes"
    );
    let input_beef: Vec<u8> = row
        .get::<Option<Vec<u8>>, _>("input_beef")
        .context("respend refused: input BEEF unavailable")?;
    let mut beef = Beef::from_binary(&input_beef)?;
    // Keep only the original inputs' ancestor closure; the phantom and its
    // outputs must never ride in the replacement's ancestry.
    let mut keep = HashSet::new();
    let mut pending: Vec<String> = phantom
        .inputs
        .iter()
        .map(|i| i.get_source_txid())
        .collect::<bsv_sdk::Result<_>>()?;
    while let Some(id) = pending.pop() {
        if !keep.insert(id.clone()) {
            continue;
        }
        let tx = beef
            .find_txid(&id)
            .and_then(|t| t.tx())
            .context("respend refused: input ancestry unavailable")?;
        if tx.merkle_path.is_none()
            && beef
                .find_txid(&id)
                .is_some_and(|t| t.bump_index().is_none())
        {
            for input in &tx.inputs {
                pending.push(input.get_source_txid()?);
            }
        }
    }
    beef.txs.retain(|t| keep.contains(&t.txid()));
    let roots = beef
        .verify_structure()
        .map_err(|e| anyhow!("respend refused: invalid input BEEF: {e:?}"))?;
    let tracker = services.get_chain_tracker().await?;
    for (height, root_hash) in roots {
        ensure!(
            tracker.is_valid_root_for_height(&root_hash, height).await?,
            "respend refused: ancestry proof off our headers"
        );
    }
    let deriver = KeyDeriver::new(Some(root.clone()));
    let protocol = Protocol::new(SecurityLevel::Counterparty, PROTOCOL);
    let mut replacement = Transaction::new();
    let mut total = 0u64;
    let mut coin_ids = Vec::new();
    let mut outpoints = HashSet::new();
    for input in &phantom.inputs {
        let source = input.get_source_txid()?;
        ensure!(
            outpoints.insert((source.clone(), input.source_output_index)),
            "respend refused: duplicate input"
        );
        let coin = sqlx::query("SELECT output_id,satoshis,locking_script,derivation_prefix,derivation_suffix,sender_identity_key,spent_by,spendable FROM outputs WHERE user_id=? AND txid=? AND vout=?")
            .bind(user_id).bind(&source).bind(input.source_output_index as i64).fetch_optional(storage.pool()).await?
            .context("respend refused: an input is not the wallet's")?;
        ensure!(
            coin.get::<Option<i64>, _>("spent_by") == Some(phantom_id)
                && !coin.get::<bool, _>("spendable"),
            "respend refused: input is not reserved to the phantom"
        );
        let prefix: String = coin
            .get::<Option<String>, _>("derivation_prefix")
            .context("respend refused: input derivation unavailable")?;
        let suffix: String = coin
            .get::<Option<String>, _>("derivation_suffix")
            .context("respend refused: input derivation unavailable")?;
        let sender: Option<String> = coin.get("sender_identity_key");
        let counterparty = match sender.filter(|s| !s.is_empty()) {
            Some(s) => Counterparty::Other(PublicKey::from_hex(&s)?),
            None => Counterparty::Self_,
        };
        let key =
            deriver.derive_private_key(&protocol, &format!("{prefix} {suffix}"), &counterparty)?;
        let lock = P2PKH::lock_from_address(&key.public_key().to_address())?;
        ensure!(
            coin.get::<Option<Vec<u8>>, _>("locking_script") == Some(lock.to_binary()),
            "respend refused: an input is not the wallet's P2PKH key"
        );
        let parent = beef
            .find_txid(&source)
            .and_then(|t| t.tx())
            .context("respend refused: source transaction unavailable")?;
        ensure!(
            parent.id() == source,
            "respend refused: source bytes mismatch"
        );
        let output = parent
            .outputs
            .get(input.source_output_index as usize)
            .context("respend refused: source output unavailable")?;
        let sats = u64::try_from(coin.get::<i64, _>("satoshis"))?;
        ensure!(
            output.satoshis == Some(sats) && output.locking_script == lock,
            "respend refused: stored input differs from source bytes"
        );
        total = total
            .checked_add(sats)
            .context("respend input value overflow")?;
        coin_ids.push(coin.get::<i64, _>("output_id"));
        replacement.add_input_from_tx(
            parent.clone(),
            input.source_output_index,
            P2PKH::unlock(&key, SignOutputs::All, false),
        )?;
    }
    let prefix = hex::encode(PrivateKey::random().public_key().hash160());
    let suffix = hex::encode(PrivateKey::random().public_key().hash160());
    let change_key = deriver.derive_private_key(
        &protocol,
        &format!("{prefix} {suffix}"),
        &Counterparty::Self_,
    )?;
    let change_lock = P2PKH::lock_from_address(&change_key.public_key().to_address())?;
    replacement.add_output(TransactionOutput::new(0, change_lock.clone()))?;
    let fee = (u64::try_from(replacement.estimate_size())? * WALLET_FEE_SAT_PER_KB).div_ceil(1000);
    let amount = total
        .checked_sub(fee)
        .filter(|s| *s > 0)
        .context("respend refused: inputs cannot cover wallet fee")?;
    replacement.outputs[0].satoshis = Some(amount);
    replacement.sign().await?;
    let replacement_id = replacement.id();
    ensure!(
        replacement_id != txid,
        "respend refused: replacement must differ from phantom"
    );
    require_silence(storage, services, &txid, expected_sources).await?;
    let input_beef = beef.to_binary();
    let reference = format!("respend-{replacement_id}");
    let now = Utc::now();

    // One SQLite transaction reserves the act, output and proof ask. Recheck
    // every reservation and word under the write lock before committing.
    let basket = storage
        .find_or_create_default_basket(user_id)
        .await?
        .basket_id;
    let mut transaction = storage.pool().begin().await?;
    let current: (String,String,String) = sqlx::query_as("SELECT t.status,r.status,r.history FROM transactions t JOIN proven_tx_reqs r ON r.txid=t.txid WHERE t.transaction_id=?")
        .bind(phantom_id).fetch_one(&mut *transaction).await?;
    ensure!(
        current.0 == word && current.1 == req_word,
        "respend refused: phantom changed during construction"
    );
    for coin in coin_ids {
        let locked: (bool, Option<i64>) =
            sqlx::query_as("SELECT spendable,spent_by FROM outputs WHERE output_id=?")
                .bind(coin)
                .fetch_one(&mut *transaction)
                .await?;
        ensure!(
            locked == (false, Some(phantom_id)),
            "respend refused: input reservation changed"
        );
    }
    if let Some((old_id, _, _)) = previous {
        let removed = sqlx::query("DELETE FROM respend_links WHERE phantom_id=? AND replacement_id=? AND replacement_id IN (SELECT transaction_id FROM transactions WHERE status='failed')")
            .bind(phantom_id).bind(old_id).execute(&mut *transaction).await?.rows_affected();
        ensure!(
            removed == 1,
            "respend refused: previous replacement changed"
        );
    }
    let new_row: i64 = sqlx::query_scalar("INSERT INTO transactions (user_id,status,reference,is_outgoing,satoshis,version,lock_time,description,input_beef,txid,raw_tx,created_at,updated_at) VALUES (?,'unsigned',?,1,?,1,0,'Deliberate replacement spend',?,?,?,?,?) RETURNING transaction_id")
        .bind(user_id).bind(&reference).bind(-(fee as i64)).bind(&input_beef).bind(&replacement_id).bind(replacement.to_binary()).bind(now).bind(now).fetch_one(&mut *transaction).await?;
    sqlx::query("INSERT INTO outputs (user_id,transaction_id,basket_id,vout,satoshis,locking_script,txid,type,spendable,change,derivation_prefix,derivation_suffix,provided_by,purpose,output_description,created_at,updated_at) VALUES (?,?,?,0,?,?,'','P2PKH',0,1,?,?,'storage','change','Replacement change',?,?)")
        .bind(user_id).bind(new_row).bind(basket).bind(amount as i64).bind(change_lock.to_binary()).bind(prefix).bind(suffix).bind(now).bind(now).execute(&mut *transaction).await?;
    sqlx::query("INSERT INTO respend_links (phantom_id,replacement_id) VALUES (?,?)")
        .bind(phantom_id)
        .bind(new_row)
        .execute(&mut *transaction)
        .await?;
    // Queue only a hint. The existing proof pass owns doubleSpend.
    let mut history: serde_json::Value =
        serde_json::from_str(&current.2).context("respend refused: request history malformed")?;
    ensure!(
        history.is_object(),
        "respend refused: request history malformed"
    );
    let queued = history
        .as_object_mut()
        .unwrap()
        .entry("competitorsQueued")
        .or_insert_with(|| serde_json::json!([]));
    queued
        .as_array_mut()
        .context("respend refused: competitor queue malformed")?
        .push(serde_json::json!(replacement_id));
    sqlx::query("UPDATE proven_tx_reqs SET history=? WHERE txid=?")
        .bind(history.to_string())
        .bind(&txid)
        .execute(&mut *transaction)
        .await?;
    transaction.commit().await?;
    storage
        .process_action(
            &AuthId::with_user_id(identity, user_id),
            StorageProcessActionArgs {
                is_new_tx: true,
                is_send_with: false,
                is_no_send: false,
                is_delayed: false,
                reference: Some(reference),
                txid: Some(replacement_id.clone()),
                raw_tx: Some(replacement.to_binary()),
                send_with: vec![],
            },
        )
        .await?;
    beef.merge_raw_tx(replacement.to_binary(), None);
    let outcome = match services
        .post_beef(&beef.to_binary(), std::slice::from_ref(&replacement_id))
        .await
    {
        Ok(results) => classify_broadcast_results(&results),
        Err(e) => BroadcastOutcome::ServiceError {
            details: vec![e.to_string()],
        },
    };
    storage
        .update_transaction_status_after_broadcast(&replacement_id, &outcome)
        .await?;
    crate::spend_guard::run(storage.pool()).await?;
    Ok(replacement_id)
}

/// Retain the exact input reservations of an unsettled replacement before
/// selection. The toolbox's rollback can restore an input on an unspent
/// hint; the host has deliberately reserved that input for two tracked
/// competitors, so that hint must not let a third spend select it.
pub async fn reserve_inputs(pool: &sqlx::SqlitePool) -> Result<()> {
    let exists: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='respend_links'",
    )
    .fetch_one(pool)
    .await?;
    if exists == 0 {
        return Ok(());
    }
    let rows = sqlx::query("SELECT l.phantom_id,t.user_id,COALESCE(t.raw_tx,r.raw_tx,p.raw_tx) AS raw_tx FROM respend_links l JOIN transactions t ON t.transaction_id=l.replacement_id LEFT JOIN proven_tx_reqs r ON r.txid=t.txid LEFT JOIN proven_txs p ON p.proven_tx_id=t.proven_tx_id WHERE l.settled=0").fetch_all(pool).await?;
    for row in rows {
        let phantom: i64 = row.get("phantom_id");
        let user_id: i64 = row.get("user_id");
        let raw: Vec<u8> = row
            .get::<Option<Vec<u8>>, _>("raw_tx")
            .context("respend reservation bytes unavailable")?;
        let inputs: Vec<(String, u32)> = {
            let tx = Transaction::from_binary(&raw)?;
            tx.inputs
                .iter()
                .map(|i| Ok((i.get_source_txid()?, i.source_output_index)))
                .collect::<bsv_sdk::Result<_>>()?
        };
        for (source, vout) in inputs {
            sqlx::query("UPDATE outputs SET spendable=0,spent_by=? WHERE user_id=? AND txid=? AND vout=? AND (spent_by IS NULL OR spent_by=?)")
                .bind(phantom).bind(user_id).bind(source).bind(vout as i64).bind(phantom).execute(pool).await?;
        }
    }
    Ok(())
}

/// Reassign already locked inputs only after the existing proof funnel has
/// checked the replacement and written doubleSpend on the phantom.
pub async fn settle_proven(storage: &StorageSqlx, _services: &dyn WalletServices) -> Result<()> {
    let exists: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='respend_links'",
    )
    .fetch_one(storage.pool())
    .await?;
    if exists == 0 {
        return Ok(());
    }
    // A crash after the durable build but before process_action leaves an
    // unsigned row. Finish only that recorded act; no new inputs are picked.
    let pending = sqlx::query("SELECT t.user_id,t.reference,t.txid,t.raw_tx,u.identity_key FROM respend_links l JOIN transactions t ON t.transaction_id=l.replacement_id JOIN users u ON u.user_id=t.user_id WHERE t.status='unsigned'").fetch_all(storage.pool()).await?;
    for row in pending {
        storage
            .process_action(
                &AuthId::with_user_id(row.get::<String, _>("identity_key"), row.get("user_id")),
                StorageProcessActionArgs {
                    is_new_tx: true,
                    is_send_with: false,
                    is_no_send: false,
                    is_delayed: true,
                    reference: Some(row.get("reference")),
                    txid: Some(row.get("txid")),
                    raw_tx: Some(row.get("raw_tx")),
                    send_with: vec![],
                },
            )
            .await?;
    }
    let rows = sqlx::query("SELECT l.phantom_id,l.replacement_id,t.txid,p.raw_tx,p.merkle_path,p.height,p.block_hash FROM respend_links l JOIN transactions t ON t.transaction_id=l.replacement_id JOIN transactions f ON f.transaction_id=l.phantom_id JOIN proven_tx_reqs r ON r.txid=f.txid JOIN proven_txs p ON p.proven_tx_id=t.proven_tx_id WHERE l.settled=0 AND r.status='doubleSpend' AND t.status='completed'")
        .fetch_all(storage.pool()).await?;
    for row in rows {
        let txid: String = row.get("txid");
        let path: Vec<u8> = row.get("merkle_path");
        let height: i64 = row.get("height");
        let block: String = row.get("block_hash");
        if !matches!(
            storage
                .ingest_merkle_proof(&txid, &path, u32::try_from(height)?, &block, None)
                .await?,
            ProofIngestOutcome::Ingested(_)
        ) {
            continue;
        }
        let phantom: i64 = row.get("phantom_id");
        let replacement: i64 = row.get("replacement_id");
        let mut transaction = storage.pool().begin().await?;
        let raw: Vec<u8> = row
            .get::<Option<Vec<u8>>, _>("raw_tx")
            .context("respend reservation bytes unavailable")?;
        let inputs: Vec<(String, u32)> = {
            let tx = Transaction::from_binary(&raw)?;
            tx.inputs
                .iter()
                .map(|input| Ok((input.get_source_txid()?, input.source_output_index)))
                .collect::<bsv_sdk::Result<_>>()?
        };
        for (source, vout) in inputs {
            sqlx::query("UPDATE outputs SET spent_by=?,spendable=0 WHERE (spent_by=? OR spent_by IS NULL) AND txid=? AND vout=? AND user_id=(SELECT user_id FROM transactions WHERE transaction_id=?)")
                .bind(replacement).bind(phantom).bind(source).bind(vout as i64).bind(replacement).execute(&mut *transaction).await?;
        }
        sqlx::query("UPDATE respend_links SET settled=1 WHERE phantom_id=?")
            .bind(phantom)
            .execute(&mut *transaction)
            .await?;
        transaction.commit().await?;
    }
    Ok(())
}
