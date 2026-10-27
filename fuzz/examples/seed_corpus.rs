//! Writes the seed corpus under `corpus/<target>/seed-<name>` from the
//! shapes the tree's tests and e2e scenarios already use: the BEEF frames
//! of `src/atomic_beef.rs`'s tests, the request bodies of
//! `tests/e2e/lib/wallet-client.js` and `tests/e2e/scenarios/`, the
//! explorer bodies of `explorer_bodies`'s readers, the outpoints of
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

    // explorer_bodies: [reader, status, body], each reader's body valid,
    // malformed and about another transaction. Readers: 0 WoC `/beef`, 1 BB
    // `/hex`, 2 WoC chain index, 3 Bitails chain index, 4 BB chain index;
    // status 0 is 200. The txid asked is `control`'s.
    let seed = |reader: u8, body: &[u8]| {
        let mut bytes = vec![reader, 0];
        bytes.extend_from_slice(body);
        bytes
    };
    let explorer = |name: &str, reader: u8, body: &[u8]| {
        write("explorer_bodies", name, &seed(reader, body));
    };
    explorer("woc-beef", 0, hex::encode(&proven).as_bytes());
    explorer("woc-beef-not-hex", 0, b"{\"error\":\"not found\"}");
    explorer("woc-beef-other-tx", 0, hex::encode(&atomic).as_bytes());
    explorer("bb-hex", 1, hex::encode(&control).as_bytes());
    explorer("bb-hex-not-hex", 1, b"zz");
    explorer("bb-hex-other-tx", 1, hex::encode(&child).as_bytes());
    let block = "00".repeat(31) + "01";
    for (reader, name, mined, unmined) in [
        (
            2u8,
            "woc-index",
            format!(r#"{{"txid":"{control_id}","confirmations":3}}"#),
            format!(r#"{{"txid":"{control_id}","confirmations":0}}"#),
        ),
        (
            3u8,
            "bitails-index",
            format!(r#"{{"txid":"{control_id}","blockHeight":900000}}"#),
            format!(r#"{{"txid":"{control_id}"}}"#),
        ),
        (
            4u8,
            "bb-index",
            format!(r#"{{"txid":"{control_id}","block_hash":"{block}","block_height":900000}}"#),
            format!(r#"{{"txid":"{control_id}"}}"#),
        ),
    ] {
        explorer(&format!("{name}-mined"), reader, mined.as_bytes());
        explorer(&format!("{name}-unmined"), reader, unmined.as_bytes());
        explorer(&format!("{name}-malformed"), reader, b"{\"txid\":");
        explorer(
            &format!("{name}-other-tx"),
            reader,
            mined.replace(&control_id, &child_id).as_bytes(),
        );
    }

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
