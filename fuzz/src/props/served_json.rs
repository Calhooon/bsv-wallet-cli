//! `fuzz_targets/served_json.rs`'s property.

use bsv_wallet_cli::server::handlers::{create_action_args, internalize_args};
use bsv_wallet_cli::server::sanitize_lone_surrogates;
use bsv_wallet_cli::server::types::{McCreateActionReq, McInternalizeActionReq};
use bsv_wallet_cli::spend_gate::rule_of_label;

pub fn check(data: &[u8]) {
    let body = sanitize_lone_surrogates(data);
    assert_eq!(
        sanitize_lone_surrogates(&body).as_ref(),
        body.as_ref(),
        "the sanitizer is not idempotent"
    );

    if let Ok(req) = serde_json::from_slice::<McCreateActionReq>(&body) {
        let _ = create_action_args(req);
    }
    if let Ok(req) = serde_json::from_slice::<McInternalizeActionReq>(&body) {
        let outputs = req.outputs.len();
        let args = internalize_args(req);
        assert_eq!(args.outputs.len(), outputs);
        for label in args.labels.iter().flatten() {
            assert!(
                rule_of_label(label).is_none(),
                "a caller's rule label survived"
            );
        }
    }
}
