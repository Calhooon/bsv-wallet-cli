//! The doors read a body as the client (bsv-desktop) reads it (#122 item 3): a
//! JSON body under any content type is the call, cleaned and refused as a
//! JSON-typed one is, and a request with no content type runs the call
//! with `{}` whatever it carries.

use crate::support::{post_json, setup};
use reqwest::Client;
use serde_json::{json, Value};

const BODY: &str =
    r#"{"data":[1],"protocolID":[2,"body reader"],"keyID":"1","counterparty":"self"}"#;

/// `body` posted to `path` under `content_type` (none when `None`): the
/// status and the JSON answer.
async fn post_typed(
    client: &Client,
    url: &str,
    content_type: Option<&str>,
    body: &str,
) -> (u16, Value) {
    let mut req = client
        .post(url)
        .header("Origin", "http://test.local")
        .body(body.to_string());
    if let Some(ct) = content_type {
        req = req.header("content-type", ct);
    }
    let resp = req.send().await.unwrap();
    (resp.status().as_u16(), resp.json().await.unwrap())
}

/// A valid body as text is the call, the same answer as the body as JSON;
/// a lone surrogate in it is cleaned as in a JSON-typed body, and a body
/// that does not parse is the same refusal.
#[tokio::test]
async fn a_text_typed_body_is_the_call_as_a_json_typed_one_is() {
    let (base, client, _tmp) = setup().await;
    let url = format!("{base}/createSignature");
    let surrogate = BODY.replace(r#""keyID":"1""#, r#""keyID":"\ud800""#);
    for body in [BODY, surrogate.as_str(), r#"{"data": [1"#] {
        let as_json = post_typed(&client, &url, Some("application/json"), body).await;
        for ct in [
            "text/plain",
            "text/plain; charset=utf-8",
            "application/x-www-form-urlencoded",
        ] {
            let as_text = post_typed(&client, &url, Some(ct), body).await;
            assert_eq!(as_text, as_json, "{ct}: {body}");
        }
    }
    let (status, signed) = post_typed(&client, &url, Some("text/plain"), BODY).await;
    assert_eq!(status, 200, "{signed}");
    assert!(signed["signature"].is_array(), "{signed}");
    let (status, cleaned) = post_typed(&client, &url, Some("text/plain"), &surrogate).await;
    assert_eq!(status, 200, "{cleaned}");
    let (status, refused) = post_typed(&client, &url, Some("text/plain"), r#"{"data": [1"#).await;
    assert_eq!(
        (status, &refused["code"]),
        (400, &json!("VALIDATION_ERROR"))
    );
}

/// With no content type the body is never parsed: the call answers as it
/// does for `{}`, at two doors.
#[tokio::test]
async fn no_content_type_is_the_call_with_an_empty_body() {
    let (base, client, _tmp) = setup().await;
    let url = format!("{base}/createSignature");
    let empty = post_json(&client, &url, json!({})).await;
    let empty = (
        empty.status().as_u16(),
        empty.json::<Value>().await.unwrap(),
    );
    assert_eq!(empty.0, 400, "{}", empty.1);
    assert_eq!(empty.1["code"], "VALIDATION_ERROR");
    assert!(empty.1["message"]
        .as_str()
        .unwrap()
        .contains("missing field `protocolID`"));
    for body in [BODY, "", "not json"] {
        assert_eq!(
            post_typed(&client, &url, None, body).await,
            empty,
            "{body:?}"
        );
    }
    let url = format!("{base}/getPublicKey");
    let identity = post_json(&client, &url, json!({})).await;
    let identity = (
        identity.status().as_u16(),
        identity.json::<Value>().await.unwrap(),
    );
    assert_eq!(post_typed(&client, &url, None, "not json").await, identity);
}
