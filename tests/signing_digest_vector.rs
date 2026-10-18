//! The digest the CLI signs with, pinned to a row the node accepts (0.13.0,
//! bsv-rs 0.7.0, whose default signing digest moved to the node's dispatch).
//!
//! The CLI signs on two paths of its own; every other signature is the
//! toolbox's `wallet::signer`, pinned by the toolbox 0.9.0 test
//! `the_digest_the_toolbox_signs_with_is_the_nodes_for_a_p2pkh_all_forkid_spend`
//! (`src/wallet/signer.rs`) on the same row:
//!
//! - the gift claim (`src/gift/claim.rs`) computes `compute_sighash_for_signing`
//!   over `SighashParams` itself, scopes 0x41 and 0xc3 (FORKID, no CHRONICLE);
//! - the respend (`src/respend.rs`) signs through `P2PKH::unlock(.., All, false)`
//!   and `Transaction::sign`, the template path 0.7.0 re-routed through the
//!   dispatcher.
//!
//! The vector is bsv-script-lean v1.1.1 `corpus/signer/p2pkh.jsonl` row
//! `signer-p2pkh-00001` (ALL|FORKID, throwaway regtest key, synthetic
//! outpoint): the digest and txid are the control column of its record
//! `docs/records/20261017-020326-s3-147-signer-column-bsv-rs-s3.json`, judged
//! valid by bitcoin-sv v1.2.3 `verifyscript` under the block and relay words,
//! as the toolbox's test cites them. Cited, not re-run here.

use bsv_sdk::primitives::bsv::sighash::{
    compute_sighash_dispatched_for_signing, compute_sighash_for_signing, parse_transaction,
    SighashParams, SIGHASH_ALL, SIGHASH_FORKID,
};
use bsv_sdk::primitives::PrivateKey;
use bsv_sdk::script::template::SignOutputs;
use bsv_sdk::script::templates::P2PKH;
use bsv_sdk::script::LockingScript;
use bsv_sdk::transaction::{Transaction, TransactionOutput};

const UNSIGNED: &str = "02000000011c6bc78a8a550405d5c1aa696da005e5a32fc2be2a3c5dc4220f17e751929cb4\
                        0100000000ffffffff01942f2a00000000001976a914e4300371dca95f736069d5c61743587b\
                        ddbf10da88ac00000000";
const LOCK: &str = "76a914995eb547f6d64595a8c91b325daa5519123a715088ac";
const SATOSHIS: u64 = 2_767_692;
const KEY: &str = "c3ab537583ef5b0072450344dd3d0d973010c5da49c525289af0e4d98dab3fe5";
const DIGEST: &str = "737019101a2340f7f56047d03ff616c4b858c6829c3fa25adac60bac72a906b3";
const TXID: &str = "e418f32d280c6e1cc6f62d881a8db88469c3599e0eeb5d2711d6601c55c97a96";

#[test]
fn the_gift_claims_digest_is_the_nodes_for_a_p2pkh_all_forkid_spend() {
    let raw = parse_transaction(&hex::decode(UNSIGNED).unwrap()).unwrap();
    let lock = hex::decode(LOCK).unwrap();
    let params = SighashParams {
        version: raw.version,
        inputs: &raw.inputs,
        outputs: &raw.outputs,
        locktime: raw.locktime,
        input_index: 0,
        subscript: &lock,
        satoshis: SATOSHIS,
        scope: SIGHASH_ALL | SIGHASH_FORKID,
    };
    let digest = compute_sighash_for_signing(&params);
    assert_eq!(hex::encode(digest), DIGEST);
    // The claim's scopes carry FORKID without CHRONICLE, so the call it
    // makes is the one the 0.7.0 dispatcher makes for them.
    assert_eq!(compute_sighash_dispatched_for_signing(&params), digest);
}

#[tokio::test]
async fn the_respends_template_signs_the_nodes_spend() {
    let mut tx = Transaction::from_hex(UNSIGNED).unwrap();
    // The outpoint is synthetic: the source is a stub holding the spent
    // output at its index, and the input keeps the row's txid (an input's
    // `source_txid` wins over its source's id).
    let mut source = Transaction::new();
    source
        .add_output(TransactionOutput::new(0, LockingScript::new()))
        .unwrap();
    source
        .add_output(TransactionOutput::new(
            SATOSHIS,
            LockingScript::from_hex(LOCK).unwrap(),
        ))
        .unwrap();
    let key = PrivateKey::from_hex(KEY).unwrap();
    let input = &mut tx.inputs[0];
    assert_eq!(input.source_output_index, 1);
    input.source_transaction = Some(Box::new(source));
    input.set_unlocking_script_template(P2PKH::unlock(&key, SignOutputs::All, false));

    tx.sign().await.unwrap();
    // RFC 6979 signatures, so the spend is the record's byte for byte.
    assert_eq!(tx.id(), TXID);
}
