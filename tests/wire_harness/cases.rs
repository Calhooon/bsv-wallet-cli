//! The cases (record only), in order, and the wallet's endpoints against the
//! worlds without coins. Replay reads the fixtures, never these.

use super::cases_routes::{
    record_arc_callback, record_bearer, record_host_routes, record_host_store_busy,
    record_preflights,
};
use super::cases_spending::{
    record_abort_store_busy, record_funded_abort_and_broadcast,
    record_funded_relinquish_and_caller_input, record_funded_sign, record_gate_refusals,
    record_named_change, record_payments_and_listings, record_store_faults,
};
use super::recorder::{get, post, Recorder, CERT_TYPE, DECRYPT_CIPHERTEXT, G};
use super::worlds::{FEE_1_IN_1_OUT, FUNDED_COINS, MINED_AT, P2PKH, UNKNOWN};

/// Every case, in order: the order is the files' (and a world's state
/// runs through it).
pub(crate) async fn record_all(r: &mut Recorder) {
    record_status_and_keys(r).await;
    record_signatures(r).await;
    record_crypto(r).await;
    record_linkage(r).await;
    record_malformed(r).await;
    record_empty_wallet(r).await;
    record_unknown_references(r).await;
    record_certificate_spelling(r).await;
    record_certificates(r).await;
    record_unknown_certificates_and_discovery(r).await;
    record_host_routes(r).await;
    record_arc_callback(r).await;
    record_bearer(r).await;
    record_preflights(r).await;
    record_gate_refusals(r).await;
    record_store_faults(r).await;
    record_funded_sign(r).await;
    record_funded_abort_and_broadcast(r).await;
    record_funded_relinquish_and_caller_input(r).await;
    record_payments_and_listings(r).await;
    record_privileged(r).await;
    record_abort_store_busy(r).await;
    record_host_store_busy(r).await;
    record_named_change(r).await;
}

/// A key derivation request: `protocol` at level 2, `key_id`, counterparty
/// [`G`].
fn pk(protocol: &str, key_id: &str) -> serde_json::Value {
    serde_json::json!({"protocolID": [2, protocol], "keyID": key_id, "counterparty": G})
}

/// A certificate request's body: the certificate's type under `type_key`
/// (BRC-100's `type`, or the server's `certificateType`), then `rest`.
fn typed(type_key: &str, rest: &serde_json::Value) -> serde_json::Value {
    let mut body = serde_json::Map::new();
    body.insert(type_key.to_string(), CERT_TYPE.into());
    body.extend(rest.as_object().cloned().unwrap_or_default());
    body.into()
}

/// An `acquireCertificate` body, the `direct` protocol.
fn acquire_body(type_key: &str) -> serde_json::Value {
    let rest = serde_json::json!({"certifier": G, "acquisitionProtocol": "direct",
        "fields": {"name": "wire"}, "serialNumber": "AQID",
        "revocationOutpoint": format!("{UNKNOWN}.0"), "signature": "deadbeef",
        "keyringRevealer": "certifier", "keyringForSubject": {}});
    typed(type_key, &rest)
}

/// A `relinquishCertificate` body.
fn relinquish_body(type_key: &str) -> serde_json::Value {
    typed(
        type_key,
        &serde_json::json!({"serialNumber": "AQID", "certifier": G}),
    )
}

/// A `proveCertificate` body for a certificate the wallet does not hold.
fn prove_unheld_body(type_key: &str) -> serde_json::Value {
    let rest = serde_json::json!({"subject": G, "serialNumber": "AQID", "certifier": G,
        "revocationOutpoint": format!("{UNKNOWN}.0"), "signature": "deadbeef",
        "fields": {"name": "wire"}});
    serde_json::json!({"certificate": typed(type_key, &rest), "fieldsToReveal": ["name"], "verifier": G})
}

/// A funded coin's satoshis less the fee for spending it alone into one
/// output ([`FEE_1_IN_1_OUT`]).
pub(crate) fn exact(i: usize) -> u64 {
    FUNDED_COINS[i].2 - FEE_1_IN_1_OUT
}

/// Status (each served on GET and POST), and the public keys.
async fn record_status_and_keys(r: &mut Recorder) {
    use serde_json::json;
    for (path, name) in [
        ("/isAuthenticated", "isAuthenticated"),
        ("/getHeight", "getHeight"),
        ("/getNetwork", "getNetwork"),
        ("/getVersion", "getVersion"),
        ("/waitForAuthentication", "waitForAuthentication"),
    ] {
        r.case(&format!("{name}.get"), "fresh", get(path)).await;
        r.case(&format!("{name}.post"), "fresh", post(path, json!({})))
            .await;
    }
    r.case(
        "getHeaderForHeight.ok",
        "fresh",
        post("/getHeaderForHeight", json!({"height": MINED_AT})),
    )
    .await;

    // --- Keys and crypto.
    r.case(
        "getPublicKey.identity",
        "fresh",
        post("/getPublicKey", json!({"identityKey": true})),
    )
    .await;
    r.case(
        "getPublicKey.derived",
        "fresh",
        post("/getPublicKey", pk("wire test", "1")),
    )
    .await;
    r.case(
        "getPublicKey.forSelf",
        "fresh",
        post(
            "/getPublicKey",
            json!({"protocolID": [1, "wire test"], "keyID": "1", "counterparty": "self", "forSelf": true}),
        ),
    )
    .await;
}

/// Signatures: made over data and over a hash, then verified.
async fn record_signatures(r: &mut Recorder) {
    use serde_json::json;
    let data = b"wire".to_vec();
    let mut sign = pk("wire test", "1");
    sign["data"] = json!(data);
    let sig = r
        .case(
            "createSignature.data",
            "fresh",
            post("/createSignature", sign),
        )
        .await;
    let mut sign = pk("wire test", "1");
    sign["hashToDirectlySign"] = json!(vec![7u8; 32]);
    r.case(
        "createSignature.hashToDirectlySign",
        "fresh",
        post("/createSignature", sign),
    )
    .await;
    let mut verify =
        json!({"protocolID": [2, "wire test"], "keyID": "1", "counterparty": G, "forSelf": true});
    verify["data"] = json!(data);
    verify["signature"] = sig.value("/signature");
    r.case(
        "verifySignature.valid",
        "fresh",
        post("/verifySignature", verify.clone()),
    )
    .await;
    verify["data"] = json!(b"other".to_vec());
    r.case(
        "verifySignature.invalid",
        "fresh",
        post("/verifySignature", verify),
    )
    .await;
}

/// Encryption, decryption and HMACs.
async fn record_crypto(r: &mut Recorder) {
    use serde_json::json;
    let data = b"wire".to_vec();
    let mut enc = pk("wire test", "1");
    enc["plaintext"] = json!(data);
    r.case("encrypt.ok", "fresh", post("/encrypt", enc)).await;
    // A ciphertext of `wire` under the fixed key (one encrypt's answer, kept:
    // a new one carries a new IV, and the fixture would move on every
    // recording).
    let mut dec = pk("wire test", "1");
    dec["ciphertext"] = json!(DECRYPT_CIPHERTEXT.to_vec());
    r.case("decrypt.ok", "fresh", post("/decrypt", dec)).await;
    let mut hmac = pk("wire test", "1");
    hmac["data"] = json!(data);
    let made = r
        .case("createHmac.ok", "fresh", post("/createHmac", hmac.clone()))
        .await;
    hmac["hmac"] = made.value("/hmac");
    r.case(
        "verifyHmac.valid",
        "fresh",
        post("/verifyHmac", hmac.clone()),
    )
    .await;
    hmac["data"] = json!(b"other".to_vec());
    r.case("verifyHmac.invalid", "fresh", post("/verifyHmac", hmac))
        .await;
    let mut short = pk("wire test", "1");
    short["data"] = json!(data);
    short["hmac"] = json!([1, 2, 3]);
    r.case("verifyHmac.short", "fresh", post("/verifyHmac", short))
        .await;
}

/// Key linkage revealed, and a counterparty key that is not one.
async fn record_linkage(r: &mut Recorder) {
    use serde_json::json;
    r.case(
        "revealCounterpartyKeyLinkage.ok",
        "fresh",
        post(
            "/revealCounterpartyKeyLinkage",
            json!({"counterparty": G, "verifier": G}),
        ),
    )
    .await;
    r.case(
        "revealCounterpartyKeyLinkage.badKey",
        "fresh",
        post(
            "/revealCounterpartyKeyLinkage",
            json!({"counterparty": "02zz", "verifier": G}),
        ),
    )
    .await;
    r.case(
        "revealSpecificKeyLinkage.ok",
        "fresh",
        post(
            "/revealSpecificKeyLinkage",
            json!({"counterparty": G, "verifier": G, "protocolID": [2, "wire test"], "keyID": "1"}),
        ),
    )
    .await;
}

/// The caller's malformed requests.
async fn record_malformed(r: &mut Recorder) {
    use serde_json::json;
    let data = b"wire".to_vec();
    let mut no_origin = pk("wire test", "1");
    no_origin["plaintext"] = json!(data);
    r.case(
        "encrypt.noOrigin",
        "fresh",
        post("/encrypt", no_origin).without("origin"),
    )
    .await;
    let mut bad_protocol = pk("wire test", "1");
    bad_protocol["protocolID"] = json!("wire test");
    bad_protocol["data"] = json!(data);
    r.case(
        "createSignature.badProtocolID",
        "fresh",
        post("/createSignature", bad_protocol),
    )
    .await;
    r.case(
        "createSignature.noData",
        "fresh",
        post("/createSignature", pk("wire test", "1")),
    )
    .await;
    r.case(
        "createSignature.missingField",
        "fresh",
        post("/createSignature", json!({"data": [1]})),
    )
    .await;
    r.case(
        "createSignature.malformedJson",
        "fresh",
        post("/createSignature", json!({})).raw("{\"data\": [1"),
    )
    .await;
    r.case(
        "getPublicKey.loneSurrogate",
        "fresh",
        post("/getPublicKey", json!({})).raw(
            "{\"protocolID\":[2,\"wire test\"],\"keyID\":\"ab \\ud83d cd\",\"counterparty\":\"self\"}",
        ),
    )
    .await;
    r.case("unknownRoute", "fresh", post("/noSuchCall", json!({})))
        .await;
}

/// Actions and outputs, an empty wallet.
async fn record_empty_wallet(r: &mut Recorder) {
    use serde_json::json;
    r.case(
        "listActions.empty",
        "fresh",
        post("/listActions", json!({"labels": []})),
    )
    .await;
    r.case(
        "listOutputs.empty",
        "fresh",
        post("/listOutputs", json!({"basket": "default"})),
    )
    .await;
    r.case(
        "createAction.insufficientFunds",
        "fresh",
        post(
            "/createAction",
            json!({"description": "wire unfunded", "outputs": [
                {"lockingScript": P2PKH, "satoshis": 1000, "outputDescription": "an output"}]}),
        ),
    )
    .await;
    r.case(
        "createAction.badAllowPending",
        "fresh",
        post(
            "/createAction",
            json!({"description": "wire bad word", "allowPending": "everything", "outputs": [
                {"lockingScript": P2PKH, "satoshis": 1000, "outputDescription": "an output"}]}),
        ),
    )
    .await;
    r.case(
        "createAction.scopeNamesForeignCoin",
        "fresh",
        post(
            "/createAction",
            json!({"description": "wire foreign coin", "allowPending": {"received": [format!("{UNKNOWN}:0")]}, "outputs": [
                {"lockingScript": P2PKH, "satoshis": 1000, "outputDescription": "an output"}]}),
        ),
    )
    .await;
}

/// Actions and outputs the wallet does not know, and a BEEF that is not one.
async fn record_unknown_references(r: &mut Recorder) {
    use serde_json::json;
    r.case(
        "signAction.unknownReference",
        "fresh",
        post(
            "/signAction",
            json!({"spends": {}, "reference": "bm8tc3VjaC1yZWZlcmVuY2U="}),
        ),
    )
    .await;
    r.case(
        "abortAction.unknownReference",
        "fresh",
        post(
            "/abortAction",
            json!({"reference": "bm8tc3VjaC1yZWZlcmVuY2U="}),
        ),
    )
    .await;
    r.case(
        "relinquishOutput.unknown",
        "fresh",
        post(
            "/relinquishOutput",
            json!({"basket": "default", "output": format!("{UNKNOWN}.0")}),
        ),
    )
    .await;
    r.case(
        "internalizeAction.invalidBeef",
        "fresh",
        post(
            "/internalizeAction",
            json!({"tx": [0xde, 0xad, 0xbe, 0xef], "outputs": [{"outputIndex": 0, "protocol": "wallet payment",
                "paymentRemittance": {"derivationPrefix": "d2lyZQ==", "derivationSuffix": "d2lyZQ==", "senderIdentityKey": G}}],
                "description": "wire invalid beef"}),
        ),
    )
    .await;
}

/// Certificates. BRC-100 names the certificate's type `type`; the
/// server's types take `certificateType` (found by this corpus, #69): the
/// BRC's spelling is recorded as served, then the server's, in order:
/// acquire, list, prove, relinquish.
async fn record_certificate_spelling(r: &mut Recorder) {
    for (path, body) in [
        ("/acquireCertificate", acquire_body("type")),
        ("/relinquishCertificate", relinquish_body("type")),
        ("/proveCertificate", prove_unheld_body("type")),
    ] {
        r.case(
            &format!("{}.brc100Type", &path[1..]),
            "fresh",
            post(path, body),
        )
        .await;
    }
}

/// A certificate acquired, then listed, proven and relinquished.
async fn record_certificates(r: &mut Recorder) {
    use serde_json::json;
    let acquired = r
        .case(
            "acquireCertificate.direct",
            "fresh",
            post("/acquireCertificate", acquire_body("certificateType")),
        )
        .await;
    r.case(
        "listCertificates.one",
        "fresh",
        post(
            "/listCertificates",
            json!({"certifiers": [G], "types": [CERT_TYPE]}),
        ),
    )
    .await;
    let certificate = acquired
        .body
        .as_ref()
        .map(|c| serde_json::to_value(c).unwrap())
        .unwrap_or_default();
    r.case(
        "proveCertificate.held",
        "fresh",
        post(
            "/proveCertificate",
            json!({"certificate": certificate, "fieldsToReveal": ["name"], "verifier": G}),
        ),
    )
    .await;
    r.case(
        "relinquishCertificate.held",
        "fresh",
        post("/relinquishCertificate", relinquish_body("certificateType")),
    )
    .await;
}

/// Certificates the wallet does not hold, and discovery.
async fn record_unknown_certificates_and_discovery(r: &mut Recorder) {
    use serde_json::json;
    r.case(
        "relinquishCertificate.unknown",
        "fresh",
        post("/relinquishCertificate", relinquish_body("certificateType")),
    )
    .await;
    r.case(
        "proveCertificate.unknown",
        "fresh",
        post("/proveCertificate", prove_unheld_body("certificateType")),
    )
    .await;
    r.case(
        "discoverByIdentityKey.ok",
        "fresh",
        post("/discoverByIdentityKey", json!({"identityKey": G})),
    )
    .await;
    r.case(
        "discoverByAttributes.ok",
        "fresh",
        post(
            "/discoverByAttributes",
            json!({"attributes": {"name": "wire"}}),
        ),
    )
    .await;
}

/// BRC-100's privileged keyring asked for (#145): each of the nine calls
/// that name `privileged` refuses `privileged: true`, this wallet holding
/// no privileged key; the refusal comes before any field past the
/// originator is read, so the bodies carry what a caller would send.
async fn record_privileged(r: &mut Recorder) {
    use serde_json::json;
    let data = json!(b"wire".to_vec());
    let privileged = |mut body: serde_json::Value| {
        body["privileged"] = json!(true);
        body["privilegedReason"] = json!("wire privileged");
        body
    };
    let mut cases = vec![("getPublicKey", pk("wire test", "1"))];
    for (path, field) in [
        ("createSignature", "data"),
        ("encrypt", "plaintext"),
        ("decrypt", "ciphertext"),
        ("createHmac", "data"),
    ] {
        let mut body = pk("wire test", "1");
        body[field] = if path == "decrypt" {
            json!(DECRYPT_CIPHERTEXT.to_vec())
        } else {
            data.clone()
        };
        cases.push((path, body));
    }
    let mut verify = pk("wire test", "1");
    verify["data"] = data.clone();
    verify["signature"] = json!([48, 6, 2, 1, 1, 2, 1, 1]);
    cases.push(("verifySignature", verify));
    let mut verify = pk("wire test", "1");
    verify["data"] = data;
    verify["hmac"] = json!(vec![0u8; 32]);
    cases.push(("verifyHmac", verify));
    cases.push((
        "revealCounterpartyKeyLinkage",
        json!({"counterparty": G, "verifier": G}),
    ));
    let mut specific = pk("wire test", "1");
    specific["verifier"] = json!(G);
    cases.push(("revealSpecificKeyLinkage", specific));
    for (path, body) in cases {
        r.case(
            &format!("{path}.privileged"),
            "fresh",
            post(&format!("/{path}"), privileged(body)),
        )
        .await;
    }
}
