//! `createAction`'s `sendWith` and `noSendChange` at the door's mapping
//! (#147): a non-empty list in either is refused by name before anything
//! is asked or built; absent, `null` and `[]` map as before.

use super::super::{create_action_args, McCreateActionReq};
use super::answer;
use axum::http::StatusCode;
use serde_json::json;

/// A non-empty `sendWith` or `noSendChange` (#147) is 501
/// `SEND_WITH_NOT_SUPPORTED` from the request's mapping, before anything
/// is asked or built, naming the option; absent, `null` and `[]` map as
/// before.
#[tokio::test]
async fn a_named_send_with_or_no_send_change_is_refused_and_an_empty_one_passes() {
    let request = |options: serde_json::Value| -> McCreateActionReq {
        serde_json::from_value(json!({"description": "a payment", "options": options})).unwrap()
    };
    for passes in [
        json!({}),
        json!({"sendWith": null, "noSendChange": null}),
        json!({"sendWith": [], "noSendChange": []}),
        json!({"noSend": true, "sendWith": []}),
    ] {
        assert!(
            create_action_args(request(passes.clone())).is_ok(),
            "{passes}"
        );
    }
    let txid = "ab".repeat(32);
    for (options, named) in [
        (json!({"sendWith": [txid]}), "sendWith"),
        (json!({"sendWith": [txid], "noSendChange": []}), "sendWith"),
        (
            json!({"noSendChange": [format!("{txid}.1")]}),
            "noSendChange",
        ),
        (
            json!({"sendWith": [txid], "noSendChange": [format!("{txid}.1")]}),
            "sendWith",
        ),
        (json!({"sendWith": "not a list"}), "sendWith"),
    ] {
        let Err(e) = create_action_args(request(options.clone())) else {
            panic!("{options} passed");
        };
        let (status, retry, body) = answer(e).await;
        assert_eq!(status, StatusCode::NOT_IMPLEMENTED, "{options}");
        assert!(retry.is_none());
        assert_eq!(body["code"], "SEND_WITH_NOT_SUPPORTED");
        assert_eq!(body.as_object().unwrap().len(), 2, "{body}");
        let message = body["message"].as_str().unwrap();
        assert!(
            message.starts_with(&format!("createAction: {named} is not supported: ")),
            "{message}"
        );
    }
}
