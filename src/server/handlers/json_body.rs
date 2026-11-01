//! The doors' request body (#122 item 3): read as the MetaNet Client reads
//! it, its rejections answered as every other refusal, `{code, message}`
//! with the framework's words, never its plain text. The client's Express
//! app runs `express.json()` then `express.text({ type: '*/*' })` and every
//! call `JSON.parse`s the string (bsv-desktop `b791cb2`,
//! `electron/httpServer.ts:89-90` and `183-189`, `src/walletByteJson.ts:140`),
//! so a body is JSON under any content type, and a request with none is
//! parsed by nothing and runs the call with `{}`; this reader does the
//! same. A body that does not parse and a field missing or of the wrong
//! type are the caller's request refused: 400 `VALIDATION_ERROR`, the
//! status the MetaNet Client answers for a request its wallet cannot read
//! (`src/onWalletReady.ts`, every call's `catch`), where axum answered 400
//! and 422 in text. The SDK's `HTTPWalletJSON` reads a refusal's body as
//! JSON before its status (ts-stack `edf6e03`,
//! `packages/sdk/src/wallet/substrates/HTTPWalletJSON.ts:319-340`), so a
//! text body reached it as a parse failure. A body that cannot be read is
//! the transport's and keeps axum's answer (a body over the cap is
//! `lenient_json_body`'s 413 `TOO_LARGE` before this).

use axum::body::Bytes;
use axum::extract::rejection::JsonRejection;
use axum::extract::{FromRequest, Request};
use axum::http::header::CONTENT_TYPE;
use axum::response::{IntoResponse, Response};
use serde::de::DeserializeOwned;

use crate::server::error::{AppError, Kind};

/// A request body read as `T`, or a response body written from it.
pub struct Json<T>(pub T);

impl<T, S> FromRequest<S> for Json<T>
where
    T: DeserializeOwned,
    S: Send + Sync,
{
    type Rejection = Response;

    async fn from_request(req: Request, state: &S) -> Result<Self, Self::Rejection> {
        if !req.headers().contains_key(CONTENT_TYPE) {
            return read(b"{}");
        }
        let bytes = Bytes::from_request(req, state)
            .await
            .map_err(IntoResponse::into_response)?;
        read(&bytes)
    }
}

/// `bytes` read as `T` whatever the content type, as the client's
/// `JSON.parse` reads the string.
fn read<T: DeserializeOwned>(bytes: &[u8]) -> Result<Json<T>, Response> {
    match axum::Json::<T>::from_bytes(bytes) {
        Ok(axum::Json(value)) => Ok(Json(value)),
        Err(rejection) => Err(refused(rejection)),
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
        JsonRejection::JsonDataError(_) | JsonRejection::JsonSyntaxError(_) => {
            AppError::new(Kind::Validation, rejection.body_text()).into_response()
        }
        other => other.into_response(),
    }
}
