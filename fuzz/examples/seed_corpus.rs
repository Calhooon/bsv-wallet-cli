//! Writes the seed corpus under `corpus/<target>/seed-<name>` from the
//! shapes the tree's tests and e2e scenarios already use: the BEEF frames
//! of `src/atomic_beef.rs`'s tests, the request bodies of
//! `tests/e2e/lib/wallet-client.js` and `tests/e2e/scenarios/`, the
//! explorer answers of `src/explorers.rs`'s tests, the outpoints of
//! scenario 23. Run from `fuzz/`: `cargo run --example seed_corpus`. A seed's
//! name is stable, so a rerun rewrites the same files.

use std::fs;
use std::path::Path;

use bsv_wallet_cli_fuzz::{atomic_payment, p2pkh, proven_beef, raw_tx, txid_of};

/// The BRC-29 "anyone" identity key the e2e scenarios pay from.
const ANYONE_KEY: &str = "0279be667ef9dcbbac55a06295ce870b07029bfcdb2dce28d959f2815b16f81798";

fn write(target: &str, name: &str, bytes: &[u8]) {
    let dir = Path::new("corpus").join(target);
    fs::create_dir_all(&dir).expect("the corpus directory");
    fs::write(dir.join(format!("seed-{name}")), bytes).expect("a seed");
}

fn main() {
    let lock = p2pkh([0xdb; 20]);
    let funding = "11".repeat(32);
    let control = raw_tx(&[(&funding, 0)], &[(1000, lock.clone())]);
    let control_id = txid_of(&control);
    let proven = proven_beef(control.clone(), 900_000);
    let child = raw_tx(
        &[(&control_id, 0)],
        &[(600, p2pkh([0x01; 20])), (300, p2pkh([0x02; 20]))],
    );
    let child_id = txid_of(&child);
    let atomic = atomic_payment(control.clone(), child.clone());
    // No input, one output: the same bytes as `atomic_beef.rs`'s
    // `no_input_tx`.
    let no_input = raw_tx(&[], &[(1000, lock)]);
    let no_output = raw_tx(&[(&funding, 0)], &[]);

    // beef_reader: the frames atomic_beef.rs's tests read.
    write("beef_reader", "proven-one-tx", &proven);
    write("beef_reader", "atomic-payment", &atomic);
    write("beef_reader", "no-input", &proven_beef(no_input, 900_000));
    write("beef_reader", "no-output", &proven_beef(no_output, 900_000));
    write("beef_reader", "cut", &proven[..proven.len() - 7]);
    write("beef_reader", "raw-tx", &control);
    write("beef_reader", "empty-v1", &[0x01, 0x00, 0xbe, 0xef, 0, 0]);

    // served_json: the e2e client's bodies.
    let tx_array = serde_json::to_string(&atomic).expect("json");
    let create = |extra: &str| {
        format!(
            r#"{{"outputs":[{{"lockingScript":"{}","satoshis":1000,"outputDescription":"E2E payment","basket":"default","tags":["e2e"]}}],"description":"E2E.1: A pays B",{extra}"options":{{"signAndProcess":true,"acceptDelayedBroadcast":false,"randomizeOutputs":false,"noSend":false}},"labels":["e2e"]}}"#,
            hex::encode(p2pkh([0x01; 20]))
        )
    };
    write("served_json", "create-plain", create("").as_bytes());
    write(
        "served_json",
        "create-allow-received",
        create(r#""allowPending":"received","#).as_bytes(),
    );
    write(
        "served_json",
        "create-allow-scoped",
        create(&format!(
            r#""allowPending":{{"received":["{child_id}:0"]}},"#
        ))
        .as_bytes(),
    );
    write(
        "served_json",
        "create-inputs",
        format!(
            r#"{{"description":"covenant spend","inputBEEF":{tx_array},"inputs":[{{"outpoint":"{child_id}.1","unlockingScript":"51","inputDescription":"covenant","sequenceNumber":4294967295}}],"outputs":[{{"lockingScript":"6a","satoshis":0,"outputDescription":"marker"}}],"lockTime":0,"options":{{"trustSelf":"known","noSend":true}}}}"#
        )
        .as_bytes(),
    );
    let internalize = |labels: &str| {
        format!(
            r#"{{"tx":{tx_array},"outputs":[{{"outputIndex":0,"protocol":"wallet payment","paymentRemittance":{{"derivationPrefix":"SfKxPIJNgdI=","derivationSuffix":"NaGLC6fMH50=","senderIdentityKey":"{ANYONE_KEY}"}}}},{{"outputIndex":1,"protocol":"basket insertion","insertionRemittance":{{"basket":"worm-proofs","customInstructions":"{{}}","tags":["e2e"]}}}}],"description":"E2E.2: received from A"{labels}}}"#
        )
    };
    write(
        "served_json",
        "internalize-payment",
        internalize("").as_bytes(),
    );
    write(
        "served_json",
        "internalize-forged-rule-label",
        internalize(r#","labels":["spend-gate:received-pending","e2e"]"#).as_bytes(),
    );
    write(
        "served_json",
        "lone-surrogate",
        br#"{"description":"a \ud83d lone lead and a \udc00 lone trail, a \ud83d\ude00 pair","outputs":[]}"#,
    );

    // allow_pending: scenario 23's words and the outpoint spellings.
    let outpoint = format!("{child_id}:0");
    write("allow_pending", "received", br#""received""#);
    write(
        "allow_pending",
        "scoped",
        format!(
            r#"{{"received":["{outpoint}","{}.1"]}}"#,
            child_id.to_uppercase()
        )
        .as_bytes(),
    );
    write("allow_pending", "scoped-empty", br#"{"received":[]}"#);
    write(
        "allow_pending",
        "field",
        format!(r#"{{"allowPending":{{"received":["{outpoint}"]}}}}"#).as_bytes(),
    );
    write("allow_pending", "outpoint-colon", outpoint.as_bytes());
    write(
        "allow_pending",
        "outpoint-lines",
        format!("{outpoint}\n {child_id}.1 \n{outpoint}").as_bytes(),
    );

    // explorer_bodies: [call, status, body], the answers explorers.rs's
    // tests serve. Calls: 0 WoC spent, 1 BB txo, 2 BB txo with the script,
    // 3 WoC beef, 4 BB hex; status 0 is 200, 4 is 404.
    let held = hex::encode(p2pkh([7; 20]));
    let other = hex::encode(p2pkh([8; 20]));
    let seed = |call: u8, status: u8, body: &str| {
        let mut bytes = vec![call, status];
        bytes.extend_from_slice(body.as_bytes());
        bytes
    };
    let upper = child_id.to_uppercase();
    write(
        "explorer_bodies",
        "woc-spent",
        &seed(0, 0, &format!(r#"{{"txid":"{upper}","vin":0}}"#)),
    );
    write(
        "explorer_bodies",
        "woc-spent-no-txid",
        &seed(0, 0, r#"{"vin":0}"#),
    );
    write("explorer_bodies", "woc-spent-404", &seed(0, 4, ""));
    for (call, name) in [(1u8, "bb-txo"), (2u8, "bb-txo-script")] {
        write(
            "explorer_bodies",
            &format!("{name}-spent"),
            &seed(
                call,
                0,
                &format!(
                    r#"{{"script_pubkey":"{held}","spent":true,"spent_txid":"{child_id}","spend_scope":"mempool"}}"#
                ),
            ),
        );
        write(
            "explorer_bodies",
            &format!("{name}-unspent"),
            &seed(
                call,
                0,
                &format!(r#"{{"script_pubkey":"{held}","spent":false}}"#),
            ),
        );
        write(
            "explorer_bodies",
            &format!("{name}-other-script"),
            &seed(
                call,
                0,
                &format!(r#"{{"script_pubkey":"{other}","spent":true,"spent_txid":"{child_id}"}}"#),
            ),
        );
        write(
            "explorer_bodies",
            &format!("{name}-empty-script"),
            &seed(call, 0, r#"{"script_pubkey":"","spent":false}"#),
        );
    }
    write(
        "explorer_bodies",
        "woc-beef",
        &seed(3, 0, &hex::encode(&proven)),
    );
    write(
        "explorer_bodies",
        "bb-hex",
        &seed(4, 0, &hex::encode(&control)),
    );

    // brc29_remittance: structured input (arbitrary); seeds are the e2e
    // derivation strings as bytes, for the fuzzer to cut from.
    write(
        "brc29_remittance",
        "e2e-default",
        b"SfKxPIJNgdI=\x0cNaGLC6fMH50=\x0c\x00\x00\x00\x00\x01\x02\x01",
    );
    write(
        "brc29_remittance",
        "e2e-beef-14",
        b"e2e-beef-14\x0be2e-beef-14\x0b\x01\x02\x00\x00\x00\x00\x00\x03\x00\x01",
    );
    write("brc29_remittance", "zeros", &[0u8; 64]);
}
