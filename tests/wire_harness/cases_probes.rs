//! The probe record (#539): a page holding no row answered at an open
//! door twice and under a level-0 protocol once, then the owner's
//! `GET /origins` with its `probes` section (two rows, counts 2 and 1; the
//! times volatile keys). Served from the `fresh` world after the owner's
//! door's fixtures, so its listing holds their one revoked row too;
//! recorded last, so no file before them moves.
use serde_json::json;

use super::recorder::{get, post, Recorder};

/// A page the seed does not list, on a port.
const PROBE: &str = "http://probe.test:8080";

pub(crate) async fn record(r: &mut Recorder) {
    for name in ["probe.getVersion.once", "probe.getVersion.again"] {
        r.case(name, "fresh", get("/getVersion").header("origin", PROBE))
            .await;
    }
    r.case(
        "probe.createSignature.levelZero",
        "fresh",
        post(
            "/createSignature",
            json!({"protocolID": [0, "wire probe"], "keyID": "1", "counterparty": "self",
                   "data": [1, 2, 3]}),
        )
        .header("origin", PROBE),
    )
    .await;
    let owner = get("/origins")
        .without("origin")
        .header("host", "127.0.0.1:3322");
    let listed = r.case("owner.origins.probes", "fresh", owner).await;
    assert_eq!(listed.value("/probes/0/door"), "createSignature");
    assert_eq!(
        listed.value("/probes/0/power"),
        r#"[false,0,"wire probe",""]"#
    );
    assert_eq!(listed.value("/probes/1/count"), 2);
    assert_eq!(listed.value("/pending"), json!([]));
}
