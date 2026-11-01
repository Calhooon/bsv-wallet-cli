//! The doors' request body (#122 item 3): axum's `Json`, its own
//! rejections answered as every other refusal, `{code, message}` with the
//! framework's words, never its plain text. A body that does not parse, a
//! field missing or of the wrong type, and a body without a JSON content
//! type are the caller's request refused: 400 `VALIDATION_ERROR`, the
//! status the MetaNet Client answers for a request its wallet cannot read
//! (`src/onWalletReady.ts`, every call's `catch`), where axum answered 400,
//! 422 and 415 in text. The SDK's `HTTPWalletJSON` reads a refusal's body
//! as JSON before its status (ts-stack `edf6e03`,
//! `packages/sdk/src/wallet/substrates/HTTPWalletJSON.ts:319-340`), so a
//! text body reached it as a parse failure. A body that cannot be read is
//! the transport's and keeps axum's answer (a JSON body over the cap is
//! `lenient_json_body`'s 413 `TOO_LARGE` before this).

use axum::extract::rejection::JsonRejection;
use axum::extract::{FromRequest, Request};
use axum::response::{IntoResponse, Response};

use crate::server::error::{AppError, Kind};

/// A request body read as `T`, or a response body written from it.
pub struct Json<T>(pub T);

impl<T, S> FromRequest<S> for Json<T>
where
    axum::Json<T>: FromRequest<S, Rejection = JsonRejection>,
    S: Send + Sync,
{
    type Rejection = Response;

    async fn from_request(req: Request, state: &S) -> Result<Self, Self::Rejection> {
        match axum::Json::<T>::from_request(req, state).await {
            Ok(axum::Json(value)) => Ok(Self(value)),
            Err(rejection) => Err(refused(rejection)),
        }
    }
}

impl<T: serde::Serialize> IntoResponse for Json<T> {
    fn into_response(self) -> Response {
        axum::Json(self.0).into_response()
    }
}

/// The framework's rejection as the door answers it.
fn refused(rejection: JsonRejection) -> Response {
    match rejection {
        JsonRejection::JsonDataError(_)
        | JsonRejection::JsonSyntaxError(_)
        | JsonRejection::MissingJsonContentType(_) => {
            AppError::new(Kind::Validation, rejection.body_text()).into_response()
        }
        other => other.into_response(),
    }
}
