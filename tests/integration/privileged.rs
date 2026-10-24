//! BRC-100's privileged keyring (#145): the nine calls that name
//! `privileged` refuse `privileged: true` (this wallet holds no privileged
//! key), and answer `privileged: false`, or `privilegedReason` alone, as
//! they answer the call without either field.

use crate::support::{post_json, post_json_ok, setup};
use reqwest::Client;
use serde_json::{json, Value};

const G: &str = "0279be667ef9dcbbac55a06295ce870b07029bfcdb2dce28d959f2815b16f81798";

/// A key derivation request: level 2, key `1`, counterparty [`G`], then
/// `rest`.
fn derived(rest: &Value) -> Value {
    let base = json!({"protocolID": [2, "privileged test"], "keyID": "1", "counterparty": G});
    with(&base, rest)
}

/// `body` with `extra` merged in.
fn with(body: &Value, extra: &Value) -> Value {
    let mut body = body.clone();
    body.as_object_mut()
        .unwrap()
        .extend(extra.as_object().cloned().unwrap_or_default());
    body
}

/// A valid body for each of the nine calls, and whether its answer is the
/// same bytes on every call (encryption draws a fresh IV, the linkage
/// calls encrypt for the verifier and stamp the time).
async fn bodies(base: &str, client: &Client) -> Vec<(&'static str, Value, bool)> {
    let data = json!(b"privileged".to_vec());
    let sig = post_json_ok(
        client,
        &format!("{base}/createSignature"),
        derived(&json!({"data": data})),
    )
    .await;
    let enc = post_json_ok(
        client,
        &format!("{base}/encrypt"),
        derived(&json!({"plaintext": data})),
    )
    .await;
    let hmac = post_json_ok(
        client,
        &format!("{base}/createHmac"),
        derived(&json!({"data": data})),
    )
    .await;
    vec![
        ("getPublicKey", derived(&json!({})), true),
        ("createSignature", derived(&json!({"data": data})), true),
        (
            "verifySignature",
            derived(&json!({"data": data, "signature": sig["signature"], "forSelf": true})),
            true,
        ),
        ("encrypt", derived(&json!({"plaintext": data})), false),
        (
            "decrypt",
            derived(&json!({"ciphertext": enc["ciphertext"]})),
            true,
        ),
        ("createHmac", derived(&json!({"data": data})), true),
        (
            "verifyHmac",
            derived(&json!({"data": data, "hmac": hmac["hmac"]})),
            true,
        ),
        (
            "revealCounterpartyKeyLinkage",
            json!({"counterparty": G, "verifier": G}),
            false,
        ),
        (
            "revealSpecificKeyLinkage",
            derived(&json!({"verifier": G})),
            false,
        ),
    ]
}

/// The status and the body of one call.
async fn call(client: &Client, url: &str, body: Value) -> (u16, Value) {
    let resp = post_json(client, url, body).await;
    let status = resp.status().as_u16();
    (status, resp.json().await.unwrap())
}

/// `privileged: true` is 501 `PRIVILEGED_NOT_SUPPORTED` on each of the
/// nine calls, with or without a reason: never the everyday key's answer.
/// `privileged: false`, `privileged: null` and `privilegedReason` alone
/// are the call as it is without them: the same status, and the same
/// bytes where the call's answer does not move between calls.
#[tokio::test]
async fn privileged_true_is_refused_and_false_is_the_call_as_before() {
    let (base, client, _tmp) = setup().await;
    let reason = "a reason for it";
    let extras = [
        json!({"privileged": true}),
        json!({"privileged": true, "privilegedReason": reason}),
        json!({"privileged": false}),
        json!({"privileged": null}),
        json!({"privilegedReason": reason}),
        json!({"privileged": false, "privilegedReason": reason}),
    ];
    let keys = |v: &Value| v.as_object().unwrap().keys().cloned().collect::<Vec<_>>();
    for (path, body, stable) in bodies(&base, &client).await {
        let url = format!("{base}/{path}");
        let (status, plain) = call(&client, &url, body.clone()).await;
        assert_eq!(status, 200, "{path}: {plain}");
        for extra in &extras {
            let (status, served) = call(&client, &url, with(&body, extra)).await;
            if extra["privileged"] == json!(true) {
                assert_eq!(status, 501, "{path} {extra}: {served}");
                assert_eq!(served["code"], "PRIVILEGED_NOT_SUPPORTED", "{path}");
                assert_eq!(keys(&served), ["code", "message"], "{path}: {served}");
                let message = served["message"].as_str().unwrap();
                assert!(message.starts_with(path), "{path}: {served}");
            } else if stable {
                assert_eq!((status, &served), (200, &plain), "{path} {extra}");
            } else {
                assert_eq!(status, 200, "{path} {extra}: {served}");
                assert_eq!(keys(&served), keys(&plain), "{path} {extra}");
            }
        }
    }
}
