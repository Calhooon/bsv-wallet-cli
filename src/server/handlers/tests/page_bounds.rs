//! Paging bounds through the served router (#198).

use super::*;
use tower::ServiceExt;

fn page_requests() -> [(&'static str, serde_json::Value); 5] {
    [
        ("listActions", json!({"labels": []})),
        ("listOutputs", json!({"basket": "default"})),
        ("listCertificates", json!({"certifiers": [], "types": []})),
        (
            "discoverByIdentityKey",
            json!({"identityKey": "0279be667ef9dcbbac55a06295ce870b07029bfcdb2dce28d959f2815b16f81798"}),
        ),
        (
            "discoverByAttributes",
            json!({"attributes": {"name": "bounds"}}),
        ),
    ]
}

async fn page_router() -> axum::Router {
    let storage = StorageSqlx::in_memory().await.unwrap();
    let key = bsv_sdk::primitives::PrivateKey::random();
    let wallet = crate::test_support::test_wallet(storage, "page-bounds", key).await;
    crate::server::make_router(wallet, crate::server::ServerConfig::default())
}

async fn post_page(
    app: axum::Router,
    call: &str,
    args: serde_json::Value,
) -> (StatusCode, serde_json::Value) {
    post_page_from(app, call, args, "bounds.test").await
}

async fn post_page_from(
    app: axum::Router,
    call: &str,
    args: serde_json::Value,
    originator: &str,
) -> (StatusCode, serde_json::Value) {
    let response = app
        .oneshot(
            axum::http::Request::post(format!("/{call}"))
                .header("Originator", originator)
                .header("Content-Type", "application/json")
                .body(axum::body::Body::from(serde_json::to_vec(&args).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();
    let (status, _, body) = crate::test_support::response_answer(response).await;
    (status, body)
}

async fn check_pages(limit: Option<u32>, refusal: Option<&str>) {
    let app = page_router().await;
    for (call, mut args) in page_requests() {
        if let Some(limit) = limit {
            args["limit"] = json!(limit);
        }
        let (status, body) = post_page(app.clone(), call, args).await;
        println!("{call} limit={limit:?}: {status} {body}");
        if let Some(refusal) = refusal {
            assert_eq!(status, StatusCode::BAD_REQUEST, "{call}: {body}");
            assert_eq!(
                body,
                json!({
                    "code": "VALIDATION_ERROR", "message": refusal,
                }),
                "{call}"
            );
        } else {
            assert_eq!(status, StatusCode::OK, "{call}: {body}");
        }
    }
}

#[tokio::test]
async fn page_limit_at_bound() {
    check_pages(Some(10000), None).await;
}

#[tokio::test]
async fn page_limit_zero() {
    check_pages(
        Some(0),
        Some("Validation error: limit: at least 1 length. (limit 1 to 10000, got 0)"),
    )
    .await;
}

#[tokio::test]
async fn page_limit_above_bound() {
    for limit in [10001, u32::MAX] {
        check_pages(Some(limit), Some(&format!("Validation error: limit: no more than 10000 length. (limit 1 to 10000, got {limit})"))).await;
    }
}

#[tokio::test]
async fn page_defaults_stay_absent() {
    check_pages(None, None).await;
}

#[tokio::test]
async fn page_negative_offset_reaches_toolbox_newest_first() {
    use crate::test_support::coins::{coin, storage_for_named};

    let key = bsv_sdk::primitives::PrivateKey::random();
    let (storage, user, basket) =
        storage_for_named(&key.public_key().to_hex(), "page-bounds").await;
    for (txid, satoshis) in [("ab".repeat(32), 100), ("cd".repeat(32), 200)] {
        coin(&storage, (user, basket), &txid, false, None, satoshis, 1).await;
    }
    let wallet = crate::test_support::test_wallet(storage, "page-bounds", key).await;
    let app = crate::server::make_router(wallet, crate::server::ServerConfig::default());
    for (offset, satoshis) in [(-1, 200), (0, 100)] {
        let (status, body) = post_page(
            app.clone(),
            "listOutputs",
            json!({"basket": "default", "offset": offset, "limit": 1}),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert_eq!(body["totalOutputs"], 2);
        assert_eq!(body["outputs"].as_array().unwrap().len(), 1);
        assert_eq!(body["outputs"][0]["satoshis"], satoshis);
    }
}

#[test]
fn page_handlers_share_one_guard_and_forward_original_args() {
    let source = include_str!("../../handlers.rs");
    let guard = "check_page_bounds(args.limit)?;";
    assert_eq!(source.matches(guard).count(), 5);
    for call in [
        "list_actions",
        "list_outputs",
        "list_certificates",
        "discover_by_identity_key",
        "discover_by_attributes",
    ] {
        let start = source.find(&format!("pub async fn {call}(")).unwrap();
        let handler = source[start..].split("\n}\n").next().unwrap();
        let check = handler.find(guard).unwrap();
        let forwarded = handler
            .find(&format!(".{call}(args, &extract_originator(&headers)?)"))
            .unwrap();
        assert!(check < forwarded, "{call}: guard must precede the toolbox");
        assert!(
            handler.contains("Json(args):"),
            "{call}: immutable original args"
        );
        assert!(
            !handler.contains("let args")
                && !handler.contains("args =")
                && !handler.contains("unwrap_or"),
            "{call}: no rebuilding, clamp or default"
        );
    }
}

// The concrete router cannot take MockWallet. All five real toolbox methods
// reject this originator on entry, making it a tripwire for a call past the
// guard. The accepted-limit control proves that the tripwire is live.
#[tokio::test]
async fn page_refusals_precede_every_toolbox_entry() {
    let app = page_router().await;
    for (call, args) in page_requests() {
        for limit in [0, 10001, 10000] {
            let mut body = args.clone();
            body["limit"] = json!(limit);
            let (_, body) = post_page_from(app.clone(), call, body, &"x".repeat(254)).await;
            let answer = body;
            let message = answer["message"].as_str().unwrap();
            if limit == 10000 {
                assert!(
                    message.contains("originator exceeds maximum length"),
                    "{call}: {answer}"
                );
            } else {
                assert!(
                    message.starts_with("Validation error: limit:"),
                    "{call}: {answer}"
                );
                assert!(!message.contains("originator"), "{call}: {answer}");
            }
        }
    }
}
