//! A prepared payment's send at the create door (#147): a non-empty
//! `sendWith` or `noSendChange` is refused by name, 501
//! `SEND_WITH_NOT_SUPPORTED`, before anything is built, stored or posted,
//! until the wallet below this host answers the call. The world is the
//! funded one, whose coins would otherwise fund a new transaction.
use super::recorder::{post, Recorder};
use super::worlds::{P2PKH, UNKNOWN};
use serde_json::json;

pub(crate) async fn record(r: &mut Recorder) {
    // The reference's call 2 (`AuthFetch.ts` `submitPreparedPayment`,
    // ts-stack `57c5f13`): no inputs, no outputs, the prepared txid.
    let send_with = r
        .case(
            "createAction.sendWith",
            "funded",
            post(
                "/createAction",
                json!({"description": "Submit prepared HTTP payment",
                       "options": {"sendWith": [UNKNOWN], "acceptDelayedBroadcast": false}}),
            ),
        )
        .await;
    assert_eq!(send_with.value("/code"), "SEND_WITH_NOT_SUPPORTED");
    let change = r
        .case(
            "createAction.noSendChange",
            "funded",
            post(
                "/createAction",
                json!({"description": "wire chained payment", "outputs": [
                    {"lockingScript": P2PKH, "satoshis": 1000, "outputDescription": "a payment"}],
                    "options": {"noSend": true, "randomizeOutputs": false,
                                "noSendChange": [format!("{UNKNOWN}.1")]}}),
            ),
        )
        .await;
    assert_eq!(change.value("/code"), "SEND_WITH_NOT_SUPPORTED");
}
