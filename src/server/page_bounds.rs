//! The five paging doors' bounds, before any toolbox call (#198).

use super::error::{AppError, Kind};

/// Check supplied values without replacing the toolbox's absent defaults.
pub(super) fn check_page_bounds(limit: Option<u32>) -> Result<(), AppError> {
    let problem = match limit {
        Some(0) => Some(("limit", "at least 1 length.", "1 to 10000", 0)),
        Some(value @ 10001..) => Some(("limit", "no more than 10000 length.", "1 to 10000", value)),
        _ => None,
    };
    match problem {
        Some((field, must_be, bounds, value)) => Err(AppError::new(
            Kind::Validation,
            format!("Validation error: {field}: {must_be} ({field} {bounds}, got {value})"),
        )),
        None => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn page_bounds_table() {
        use axum::http::StatusCode;
        use axum::response::IntoResponse;
        for (limit, error) in [
            (None, None),
            (
                Some(0),
                Some("limit: at least 1 length. (limit 1 to 10000, got 0)"),
            ),
            (Some(1), None),
            (Some(10000), None),
            (
                Some(10001),
                Some("limit: no more than 10000 length. (limit 1 to 10000, got 10001)"),
            ),
            (
                Some(u32::MAX),
                Some("limit: no more than 10000 length. (limit 1 to 10000, got 4294967295)"),
            ),
        ] {
            let result = check_page_bounds(limit);
            if let Some(error) = error {
                let (status, _, body) =
                    crate::test_support::response_answer(result.unwrap_err().into_response()).await;
                assert_eq!(status, StatusCode::BAD_REQUEST);
                assert_eq!(
                    body,
                    serde_json::json!({"code": "VALIDATION_ERROR", "message": format!("Validation error: {error}")})
                );
            } else {
                assert!(result.is_ok(), "limit={limit:?}: {result:?}");
            }
        }
    }
}
