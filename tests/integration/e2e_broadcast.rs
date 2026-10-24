//! Tests against a running wallet (`WALLET_URL`) that broadcast: they spend
//! mining fees, so they run only when it is set.

use crate::support::{e2e_setup, post_json, post_json_ok};
use serde_json::{json, Value};

// =============================================================================
// E2E: BEEF broadcast test — createAction with OP_RETURN (real broadcast)
// =============================================================================

/// E2E: createAction with a 0-sat OP_RETURN output — verifies real broadcast succeeds.
///
/// This test actually broadcasts a transaction to the BSV network (~200 sats mining fee).
/// It validates the full BEEF serialization → signing → broadcast pipeline.
///
/// Run:  WALLET_URL=http://localhost:3322 cargo test --test integration e2e_create_action_broadcast_beef -- --test-threads=1
#[tokio::test]
async fn e2e_create_action_broadcast_beef() {
    let Some((base, client)) = e2e_setup() else {
        eprintln!("Skipping e2e test: WALLET_URL not set");
        return;
    };

    // OP_RETURN script: OP_0 OP_RETURN <"beef-broadcast-test">
    // 00 6a 13 626565662d62726f6164636173742d74657374
    let op_return_script = "006a13626565662d62726f6164636173742d74657374";

    // Step 1: Create a broadcast action with OP_RETURN (signAndProcess:true, default)
    let resp = post_json(
        &client,
        &format!("{base}/createAction"),
        json!({
            "description": "e2e BEEF broadcast test",
            "outputs": [{
                "lockingScript": op_return_script,
                "satoshis": 0,
                "outputDescription": "BEEF broadcast verification"
            }],
            "labels": ["e2e-test-broadcast"]
        }),
    )
    .await;

    let status = resp.status().as_u16();
    let body: Value = resp.json().await.unwrap();

    // Must succeed — if BEEF serialization or broadcast is broken, this fails
    assert_eq!(
        status, 200,
        "createAction broadcast should succeed (status {}): {:?}",
        status, body
    );

    // Step 2: Verify txid is a valid 64-char hex string
    let txid = body["txid"]
        .as_str()
        .expect("broadcast response should have txid");
    assert_eq!(txid.len(), 64, "txid should be 64 hex chars, got: {}", txid);
    assert!(
        txid.chars().all(|c| c.is_ascii_hexdigit()),
        "txid should be hex, got: {}",
        txid
    );

    // Step 3: Verify no signableTransaction (tx was signed and broadcast)
    assert!(
        body.get("signableTransaction").is_none() || body["signableTransaction"].is_null(),
        "broadcast response should not have signableTransaction"
    );

    // Step 4: Verify no error fields
    assert!(
        body.get("error").is_none() || body["error"].is_null(),
        "broadcast response should not have error: {:?}",
        body
    );

    eprintln!("BEEF broadcast succeeded! txid: {}", txid);

    // Step 5: Verify the action appears in listActions
    let actions_body = post_json_ok(
        &client,
        &format!("{base}/listActions"),
        json!({
            "labels": ["e2e-test-broadcast"],
            "includeLabels": true,
            "includeOutputs": true
        }),
    )
    .await;

    let total = actions_body["totalActions"]
        .as_u64()
        .expect("totalActions should be a number");
    assert!(
        total >= 1,
        "should have at least 1 action with e2e-test-broadcast label, got {}",
        total
    );

    // Step 6: Verify the action's txid matches
    let actions = actions_body["actions"]
        .as_array()
        .expect("actions should be an array");
    let found = actions.iter().any(|a| a["txid"].as_str() == Some(txid));
    assert!(
        found,
        "broadcast txid {} should appear in listActions results",
        txid
    );

    eprintln!(
        "BEEF broadcast E2E complete: txid={}, found in listActions=true",
        txid
    );
}

// =============================================================================
// E2E: EF broadcast tests — PushDrop-style large scripts and UTXO chaining
// =============================================================================

/// E2E: createAction with PushDrop-style script — verifies EF broadcast handles large scripts.
///
/// Tests that the BEEF->EF conversion correctly embeds parent UTXO data even when
/// the transaction has large locking scripts (PushDrop pattern used by bsv-worm proofs).
///
/// The script is: OP_FALSE OP_RETURN OP_PUSHDATA2 <512 bytes of 0xaa>
/// Total script: 517 bytes (1034 hex chars).
///
/// Run: WALLET_URL=http://localhost:3322 cargo test --test integration e2e_create_action_broadcast_pushdata -- --test-threads=1
#[tokio::test]
async fn e2e_create_action_broadcast_pushdata() {
    let Some((base, client)) = e2e_setup() else {
        eprintln!("Skipping e2e test: WALLET_URL not set");
        return;
    };

    // Build PushDrop-style OP_RETURN script:
    // 00       = OP_FALSE
    // 6a       = OP_RETURN
    // 4d       = OP_PUSHDATA2
    // 0002     = 512 in little-endian u16
    // aa * 512 = 512 bytes of data
    let data_hex = "aa".repeat(512); // 1024 hex chars = 512 bytes
    let op_return_script = format!("006a4d0002{}", data_hex);
    assert_eq!(
        op_return_script.len(),
        1034,
        "PushDrop script should be 1034 hex chars (517 bytes)"
    );

    // Step 1: Create a broadcast action with the large OP_RETURN (signAndProcess:true)
    let resp = post_json(
        &client,
        &format!("{base}/createAction"),
        json!({
            "description": "e2e EF broadcast PushDrop test",
            "outputs": [{
                "lockingScript": op_return_script,
                "satoshis": 0,
                "outputDescription": "PushDrop-style large OP_RETURN for EF broadcast verification"
            }],
            "labels": ["e2e-test-pushdata"]
        }),
    )
    .await;

    let status = resp.status().as_u16();
    let body: Value = resp.json().await.unwrap();

    // Must succeed — if BEEF->EF conversion breaks on large scripts, this fails
    assert_eq!(
        status, 200,
        "createAction broadcast should succeed with large PushDrop script (status {}): {:?}",
        status, body
    );

    // Step 2: Verify txid is a valid 64-char hex string
    let txid = body["txid"]
        .as_str()
        .expect("broadcast response should have txid");
    assert_eq!(txid.len(), 64, "txid should be 64 hex chars, got: {}", txid);
    assert!(
        txid.chars().all(|c| c.is_ascii_hexdigit()),
        "txid should be hex, got: {}",
        txid
    );

    // Step 3: Verify no signableTransaction (tx was signed and broadcast)
    assert!(
        body.get("signableTransaction").is_none() || body["signableTransaction"].is_null(),
        "broadcast response should not have signableTransaction"
    );

    // Step 4: Verify no error fields
    assert!(
        body.get("error").is_none() || body["error"].is_null(),
        "broadcast response should not have error: {:?}",
        body
    );

    eprintln!(
        "EF broadcast with PushDrop script succeeded! txid: {}",
        txid
    );

    // Step 5: Verify the action appears in listActions
    let actions_body = post_json_ok(
        &client,
        &format!("{base}/listActions"),
        json!({
            "labels": ["e2e-test-pushdata"],
            "includeLabels": true,
            "includeOutputs": true
        }),
    )
    .await;

    let total = actions_body["totalActions"]
        .as_u64()
        .expect("totalActions should be a number");
    assert!(
        total >= 1,
        "should have at least 1 action with e2e-test-pushdata label, got {}",
        total
    );

    // Step 6: Verify the action's txid matches
    let actions = actions_body["actions"]
        .as_array()
        .expect("actions should be an array");
    let found = actions.iter().any(|a| a["txid"].as_str() == Some(txid));
    assert!(
        found,
        "broadcast txid {} should appear in listActions results",
        txid
    );

    eprintln!(
        "EF broadcast PushDrop E2E complete: txid={}, script_len=517 bytes, found in listActions=true",
        txid
    );
}

/// E2E: Two sequential createActions — verifies UTXO chaining works with EF broadcast.
///
/// The second transaction must use the change output from the first. This tests
/// that the wallet correctly manages UTXOs between consecutive broadcasts and
/// that EF format works for chained transactions.
///
/// Run: WALLET_URL=http://localhost:3322 cargo test --test integration e2e_sequential_broadcasts -- --test-threads=1
#[tokio::test]
async fn e2e_sequential_broadcasts() {
    let Some((base, client)) = e2e_setup() else {
        eprintln!("Skipping e2e test: WALLET_URL not set");
        return;
    };

    // --- First broadcast ---

    // OP_RETURN script: OP_0 OP_RETURN <"seq-broadcast-1">
    // 00 6a 0f 7365712d62726f6164636173742d31
    let script_1 = "006a0f7365712d62726f6164636173742d31";

    let resp1 = post_json(
        &client,
        &format!("{base}/createAction"),
        json!({
            "description": "e2e sequential broadcast 1",
            "outputs": [{
                "lockingScript": script_1,
                "satoshis": 0,
                "outputDescription": "sequential broadcast test 1"
            }],
            "labels": ["e2e-test-sequential"]
        }),
    )
    .await;

    let status1 = resp1.status().as_u16();
    let body1: Value = resp1.json().await.unwrap();
    assert_eq!(
        status1, 200,
        "first sequential broadcast should succeed (status {}): {:?}",
        status1, body1
    );

    let txid1 = body1["txid"]
        .as_str()
        .expect("first broadcast should have txid");
    assert_eq!(
        txid1.len(),
        64,
        "first txid should be 64 hex chars, got: {}",
        txid1
    );
    assert!(
        txid1.chars().all(|c| c.is_ascii_hexdigit()),
        "first txid should be hex, got: {}",
        txid1
    );
    assert!(
        body1.get("error").is_none() || body1["error"].is_null(),
        "first broadcast should not have error: {:?}",
        body1
    );

    eprintln!("Sequential broadcast 1 succeeded: txid={}", txid1);

    // --- Second broadcast (immediately after first) ---

    // OP_RETURN script: OP_0 OP_RETURN <"seq-broadcast-2">
    // 00 6a 0f 7365712d62726f6164636173742d32
    let script_2 = "006a0f7365712d62726f6164636173742d32";

    let resp2 = post_json(
        &client,
        &format!("{base}/createAction"),
        json!({
            "description": "e2e sequential broadcast 2",
            "outputs": [{
                "lockingScript": script_2,
                "satoshis": 0,
                "outputDescription": "sequential broadcast test 2"
            }],
            "labels": ["e2e-test-sequential"]
        }),
    )
    .await;

    let status2 = resp2.status().as_u16();
    let body2: Value = resp2.json().await.unwrap();
    assert_eq!(
        status2, 200,
        "second sequential broadcast should succeed (status {}): {:?}",
        status2, body2
    );

    let txid2 = body2["txid"]
        .as_str()
        .expect("second broadcast should have txid");
    assert_eq!(
        txid2.len(),
        64,
        "second txid should be 64 hex chars, got: {}",
        txid2
    );
    assert!(
        txid2.chars().all(|c| c.is_ascii_hexdigit()),
        "second txid should be hex, got: {}",
        txid2
    );
    assert!(
        body2.get("error").is_none() || body2["error"].is_null(),
        "second broadcast should not have error: {:?}",
        body2
    );

    // Verify the two txids are different (distinct transactions)
    assert_ne!(
        txid1, txid2,
        "sequential broadcasts should produce different txids"
    );

    eprintln!("Sequential broadcast 2 succeeded: txid={}", txid2);

    // Verify both actions appear in listActions
    let actions_body = post_json_ok(
        &client,
        &format!("{base}/listActions"),
        json!({
            "labels": ["e2e-test-sequential"],
            "includeLabels": true,
            "includeOutputs": true
        }),
    )
    .await;

    let total = actions_body["totalActions"]
        .as_u64()
        .expect("totalActions should be a number");
    assert!(
        total >= 2,
        "should have at least 2 actions with e2e-test-sequential label, got {}",
        total
    );

    let actions = actions_body["actions"]
        .as_array()
        .expect("actions should be an array");
    let found1 = actions.iter().any(|a| a["txid"].as_str() == Some(txid1));
    let found2 = actions.iter().any(|a| a["txid"].as_str() == Some(txid2));
    assert!(found1, "first txid {} should appear in listActions", txid1);
    assert!(found2, "second txid {} should appear in listActions", txid2);

    eprintln!(
        "Sequential broadcasts E2E complete: txid1={}, txid2={}, both found in listActions",
        txid1, txid2
    );
}
