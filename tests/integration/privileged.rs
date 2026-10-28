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

/// This test's own originator: the permission audit is process-global and
/// the other tests' servers record into it too, so its entries are read
/// by this host alone.
const AUDITED: &str = "privileged-audit.test";

/// The audit's entries recorded for [`AUDITED`].
async fn audited(client: &Client, base: &str) -> Vec<Value> {
    let audit: Value = client
        .get(format!("{base}/permission-audit"))
        .send()
        .await
        .expect("request failed")
        .json()
        .await
        .unwrap();
    let entries = audit["entries"].as_array().unwrap();
    entries
        .iter()
        .filter(|e| e["originator"] == AUDITED)
        .cloned()
        .collect()
}

/// One call from [`AUDITED`]: its status.
async fn call_audited(client: &Client, url: &str, body: Value) -> u16 {
    let resp = client
        .post(url)
        .header("Origin", format!("http://{AUDITED}"))
        .json(&body)
        .send()
        .await
        .expect("request failed");
    resp.status().as_u16()
}

/// `privileged: true` is refused before the call is audited: the nine
/// refusals add no entry to the permission audit, while the plain call
/// adds one on each of the five audited routes (a wallet with a human
/// behind it would prompt for those, and never for a refused call).
#[tokio::test]
async fn privileged_true_is_refused_before_the_call_is_audited() {
    let (base, client, _tmp) = setup().await;
    let bodies = bodies(&base, &client).await;
    let reset = client
        .post(format!("{base}/permission-audit/reset"))
        .send()
        .await
        .expect("request failed");
    assert_eq!(reset.status(), 200);
    assert_eq!(audited(&client, &base).await, Vec::<Value>::new());
    for (path, body, _) in &bodies {
        let url = format!("{base}/{path}");
        let refused = with(body, &json!({"privileged": true, "privilegedReason": "r"}));
        assert_eq!(call_audited(&client, &url, refused).await, 501, "{path}");
    }
    let after = audited(&client, &base).await;
    assert_eq!(after, Vec::<Value>::new(), "the refusals were audited");
    let five = [
        "getPublicKey",
        "createSignature",
        "encrypt",
        "decrypt",
        "createHmac",
    ];
    for (path, body, _) in bodies.iter().filter(|(p, _, _)| five.contains(p)) {
        let before = audited(&client, &base).await.len();
        let url = format!("{base}/{path}");
        assert_eq!(
            call_audited(&client, &url, body.clone()).await,
            200,
            "{path}"
        );
        let entries = audited(&client, &base).await;
        assert_eq!(entries.len(), before + 1, "{path}: {entries:?}");
        assert_eq!(entries[before]["method"], *path, "{path}");
    }
}

/// A non-boolean `privileged` is the framework's 422 on each of the nine
/// calls, never the everyday key's answer, and its text names the field's
/// path (`privileged: invalid type`); a JSON array body is judged by
/// position (`[0]`), as a struct with plain fields is.
#[tokio::test]
async fn a_non_boolean_privileged_is_a_422_that_names_its_path() {
    let (base, client, _tmp) = setup().await;
    for (path, body, _) in bodies(&base, &client).await {
        let url = format!("{base}/{path}");
        for bad in [json!("true"), json!(1)] {
            let resp = post_json(&client, &url, with(&body, &json!({"privileged": bad}))).await;
            let status = resp.status().as_u16();
            let text = resp.text().await.unwrap();
            assert_eq!(status, 422, "{path} {bad}: {text}");
            assert!(text.contains("privileged: invalid type"), "{path}: {text}");
        }
        let resp = post_json(&client, &url, json!([body])).await;
        let status = resp.status().as_u16();
        let text = resp.text().await.unwrap();
        assert_eq!(status, 422, "{path} array: {text}");
        assert!(text.contains("[0]"), "{path} array: {text}");
        assert!(!text.contains("expected struct"), "{path} array: {text}");
    }
}

/// The key-linkage calls read a JSON array body by position as they did
/// before #145: `privileged` and `privilegedReason` close their structs
/// and default when the array stops short of them, so the required
/// fields alone are 200, as is the array with `false` and a reason, and
/// a `true` in `privileged`'s place is 501.
#[tokio::test]
async fn the_key_linkage_calls_read_an_array_body_by_position_as_before() {
    let (base, client, _tmp) = setup().await;
    let proto = json!([2, "privileged test"]);
    let calls = [
        ("revealCounterpartyKeyLinkage", vec![json!(G), json!(G)]),
        (
            "revealSpecificKeyLinkage",
            vec![json!(G), json!(G), proto, json!("1")],
        ),
    ];
    for (path, required) in calls {
        let url = format!("{base}/{path}");
        let full = [required.clone(), vec![json!(false), json!("reason")]].concat();
        for body in [required.clone(), full] {
            let resp = post_json(&client, &url, json!(body)).await;
            let status = resp.status().as_u16();
            let text = resp.text().await.unwrap();
            assert_eq!(status, 200, "{path} {body:?}: {text}");
        }
        let short = [required.clone(), vec![json!(false), json!("r")]].concat();
        let (status, served) = call(&client, &url, json!(short)).await;
        assert_eq!(status, 400, "{path} {short:?}: {served}");
        assert_eq!(served["code"], "VALIDATION_ERROR", "{path}");
        let refused = [required, vec![json!(true)]].concat();
        let (status, served) = call(&client, &url, json!(refused)).await;
        assert_eq!(status, 501, "{path} {refused:?}: {served}");
        assert_eq!(served["code"], "PRIVILEGED_NOT_SUPPORTED", "{path}");
    }
}
