//! The brief reconnect's window (#534, the design's L8, section (2)): the
//! owner's answer to a row whose caller disconnected, refused while the row
//! waits for its page to ask again. The owner step opens the row as a held
//! call opens it and marks it as the disconnect marks it (`abandon`), so
//! the answer meets an abandoned row inside its window. Recorded last, so
//! no file before it moves.
use serde_json::json;

use super::owner::{Act, Owner};
use super::recorder::{post, Recorder};

pub(crate) async fn record(r: &mut Recorder) {
    let owner = Owner {
        before: vec![Act::Abandon {
            originator: "back.test".into(),
            kind: "basket".into(),
            key: "[\"wire basket\"]".into(),
            usage: "listing".into(),
            call: "/listOutputs".into(),
        }],
        ..Owner::default()
    };
    let answer = post(
        "/origins/answer",
        json!({"pendingId": 1, "answer": "grant"}),
    )
    .without("origin")
    .header("host", "127.0.0.1:3322");
    let refused = r
        .case_owned("owner.answer.abandoned", "fresh", answer, Some(owner))
        .await;
    assert_eq!(refused.value("/code"), "CONFLICT");
}
