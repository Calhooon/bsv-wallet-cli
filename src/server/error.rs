//! The served layer's one error (#81): every refusal a door answers is an
//! [`AppError`], its status and code fixed by its [`Kind`]. A kind is
//! reached from a typed error by its variant (bsv-rs's `Error`, the
//! toolbox's `Error`, the spend gate's `GateError`, the store busy by its
//! type) or named by the handler that refuses; never from the words of a
//! message. The one exception is a toolbox refusal the `WalletInterface`
//! boundary erases to a `WalletError`'s text: its class is read in
//! [`crate::toolbox_words`], the crate's one place that reads an error's
//! words, with the upstream issue that would type it. The message served
//! is the error's Display, as it was.

use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde_json::json;

use crate::toolbox_words::{self, Erased};

/// What a refusal is: one status and one code each (`answer`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// The caller's request does not parse or is refused as invalid.
    Validation,
    /// A BEEF's bytes are invalid (the toolbox's `InvalidBeef`).
    InvalidBeef,
    /// An error with no class of its own: bsv-rs's primitives, a transport
    /// fault (`WalletTransport`), a request field that does not decode, a
    /// toolbox refusal outside the table, the store's read before a door.
    Unclassified,
    /// The toolbox's shortfall, or the gate's refusal listing the coins it
    /// held.
    InsufficientFunds,
    /// A coin the caller named is held by the gate.
    InputNotOffered,
    /// The toolbox asks for an identity it does not hold.
    AuthenticationRequired,
    /// The toolbox's permission refusal.
    AccessDenied,
    /// The toolbox's entity not found.
    NotFound,
    /// The wallet's own coin, its stored ancestry incomplete: neither a
    /// shortfall nor the caller's bad input (toolbox 0.11.0).
    UnspendableCoin,
    /// The wallet's state refuses a well-formed request.
    Conflict,
    /// Completed action results require review, with their typed evidence.
    ReviewActions,
    /// A store or internal fault.
    Internal,
    /// A service, network or broadcast fault.
    Service,
    /// No broadcaster took the transaction and the chain index says it is
    /// absent; this wallet retired it.
    BroadcastRejected,
    /// The store busy (#44), by its type: nothing was written, built,
    /// locked or posted.
    StoreBusy,
    /// The header service did not answer for the coins that could have
    /// covered the action (#59): could not verify now.
    HeadersUnavailable,
    /// A request with `privileged: true` (BRC-100's privileged keyring,
    /// #145): this wallet holds no privileged key, so the call is refused,
    /// never answered with the everyday key.
    PrivilegedNotSupported,
}

impl Kind {
    /// The status and the code the kind is served with.
    pub(crate) const fn answer(self) -> (StatusCode, &'static str) {
        match self {
            Kind::Validation => (StatusCode::BAD_REQUEST, "VALIDATION_ERROR"),
            Kind::InvalidBeef => (StatusCode::BAD_REQUEST, "INVALID_BEEF"),
            Kind::Unclassified => (StatusCode::BAD_REQUEST, "ERROR"),
            Kind::InsufficientFunds => (StatusCode::PAYMENT_REQUIRED, "INSUFFICIENT_FUNDS"),
            Kind::InputNotOffered => (StatusCode::FORBIDDEN, "INPUT_NOT_OFFERED"),
            Kind::AuthenticationRequired => (StatusCode::UNAUTHORIZED, "AUTHENTICATION_REQUIRED"),
            Kind::AccessDenied => (StatusCode::FORBIDDEN, "ACCESS_DENIED"),
            Kind::NotFound => (StatusCode::NOT_FOUND, "NOT_FOUND"),
            Kind::UnspendableCoin => (StatusCode::CONFLICT, "UNSPENDABLE_COIN"),
            Kind::Conflict => (StatusCode::CONFLICT, "CONFLICT"),
            Kind::ReviewActions => (StatusCode::BAD_REQUEST, "5"),
            Kind::Internal => (StatusCode::INTERNAL_SERVER_ERROR, "INTERNAL_ERROR"),
            Kind::Service => (StatusCode::BAD_GATEWAY, "SERVICE_ERROR"),
            Kind::BroadcastRejected => (StatusCode::BAD_GATEWAY, "BROADCAST_REJECTED"),
            Kind::StoreBusy => (StatusCode::SERVICE_UNAVAILABLE, "STORE_BUSY"),
            Kind::HeadersUnavailable => (StatusCode::SERVICE_UNAVAILABLE, "HEADERS_UNAVAILABLE"),
            Kind::PrivilegedNotSupported => {
                (StatusCode::NOT_IMPLEMENTED, "PRIVILEGED_NOT_SUPPORTED")
            }
        }
    }

    /// The status the kind is served with.
    pub(crate) const fn status(self) -> StatusCode {
        self.answer().0
    }

    /// The code the kind is served with, as the body and `--json` give it.
    pub(crate) const fn code(self) -> &'static str {
        self.answer().1
    }

    /// The `Retry-After` the kind is served with, in seconds: the store
    /// busy's one second (the writer's turn), the header service's pause.
    const fn retry_after_secs(self) -> Option<u64> {
        match self {
            Kind::StoreBusy => Some(1),
            Kind::HeadersUnavailable => Some(crate::spend_gate::HEADERS_RETRY_AFTER_SECS),
            _ => None,
        }
    }

    /// The kind of a toolbox refusal's class.
    const fn of_erased(class: Erased) -> Self {
        match class {
            Erased::InsufficientFunds => Kind::InsufficientFunds,
            Erased::AuthenticationRequired => Kind::AuthenticationRequired,
            Erased::AccessDenied => Kind::AccessDenied,
            Erased::NotFound => Kind::NotFound,
            Erased::UnspendableCoin => Kind::UnspendableCoin,
            Erased::Conflict => Kind::Conflict,
            Erased::InvalidBeef => Kind::InvalidBeef,
            Erased::Validation => Kind::Validation,
            Erased::Internal => Kind::Internal,
            Erased::Service => Kind::Service,
            Erased::Unread => Kind::Unclassified,
        }
    }
}

/// A refusal as a door answers it: `{code, message}` at the kind's status,
/// with the gate's listing or the unspendable coin when there is one.
#[derive(Debug)]
pub struct AppError {
    kind: Kind,
    message: String,
    /// The spend gate's refusal: every coin held back, and why.
    refusal: Option<Box<crate::spend_gate::Refusal>>,
    /// The coin an `UnspendableCoin` refusal names.
    coin: Option<String>,
    /// The reference's five review-action members, never inferred from words.
    review: Option<Box<bsv_sdk::wallet::ReviewActionsRefusal>>,
}

impl AppError {
    /// A refusal of `kind` in the handler's own words.
    pub(crate) fn new(kind: Kind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
            refusal: None,
            coin: None,
            review: None,
        }
    }

    /// A store busy by its type, preserving its Display at the host routes
    /// and the abort pre-check. Any other fault keeps the caller's mapping.
    pub(crate) fn store_busy(e: &anyhow::Error) -> Option<Self> {
        crate::store::is_store_busy(e).then(|| Self::new(Kind::StoreBusy, e.to_string()))
    }

    /// A pre-door fault of the spend gate or the spend guard: the store
    /// busy by its type ([`crate::store::is_store_busy`]: SQLite's busy or
    /// the writer's acquire ceiling), else a 500 in the gate's words.
    pub(crate) fn from_pre_door(e: anyhow::Error, words: &str) -> Self {
        let kind = if crate::store::is_store_busy(&e) {
            Kind::StoreBusy
        } else {
            Kind::Internal
        };
        Self::new(kind, format!("Storage error: {words}: {e}"))
    }

    /// A gated `createAction`'s failure: the refusal keeps its listing.
    /// "Could not verify now" (#59: the header service's fault held the
    /// coins that could have covered it) is 503 `HEADERS_UNAVAILABLE` with
    /// `Retry-After`, never a verdict; a scoped word naming a coin that is
    /// not one the wallet could spend under it (#63) is the caller's 400.
    pub(crate) fn from_gate_error(e: crate::spend_gate::GateError) -> Self {
        use crate::spend_gate::GateError;
        match e {
            GateError::Refused(refusal) => {
                let kind = if refusal.is_headers_fault() {
                    Kind::HeadersUnavailable
                } else if refusal.is_named_input() {
                    Kind::InputNotOffered
                } else {
                    Kind::InsufficientFunds
                };
                Self::new(kind, refusal.to_string()).with_refusal(refusal)
            }
            GateError::Scope(message) => Self::new(Kind::Validation, message),
            GateError::Wallet(e) => Self::from(e),
            GateError::Gate(e) => Self::from_pre_door(e, "spend gate"),
        }
    }

    fn with_refusal(mut self, refusal: crate::spend_gate::Refusal) -> Self {
        self.refusal = Some(Box::new(refusal));
        self
    }

    /// Append an operator-facing recovery hint to the error message.
    pub(crate) fn with_hint(mut self, hint: &str) -> Self {
        self.message = format!("{} (hint: {})", self.message, hint);
        self
    }

    /// A broadcast no broadcaster took (`sendWithResults` `sending`), absent
    /// from the broadcaster it was submitted to and from an independent chain
    /// index after the full probe window, which this wallet then retired (the
    /// host's act on a template never announced). The retire releases an
    /// input only where the chain vouches it unspent; the message promises
    /// nothing more.
    pub(crate) fn broadcast_rejected(txid: &str) -> Self {
        Self::new(
            Kind::BroadcastRejected,
            format!(
                "broadcast rejected: no broadcaster accepted transaction {txid}, and it is \
                 absent from the broadcaster it was submitted to and from an independent chain \
                 index after the full probe window; this wallet retired it (failed). An input \
                 it spent is released only where the chain vouches it unspent; any other stays \
                 locked and is re-checked."
            ),
        )
    }

    /// The kind of a bsv-rs error, by its variant, and the coin an
    /// unspendable refusal names.
    fn of_sdk(e: &bsv_sdk::Error) -> (Kind, Option<String>) {
        match e {
            // The toolbox's `StoreBusy`, typed across the boundary (#44).
            bsv_sdk::Error::WalletBusy { .. } => (Kind::StoreBusy, None),
            bsv_sdk::Error::WalletReviewActions(_) => (Kind::ReviewActions, None),
            // A toolbox refusal whose variant the boundary erased.
            bsv_sdk::Error::WalletError(text) => {
                let kind = Kind::of_erased(toolbox_words::class(text));
                let coin = (kind == Kind::UnspendableCoin)
                    .then(|| toolbox_words::unspendable_coin(text))
                    .flatten();
                (kind, coin)
            }
            // `WalletTransport` (bsv-rs #73: no answer in a wallet's word)
            // among them, served as before it had a type; the served
            // wallet is in process and never gives one. bsv-rs's
            // primitives, keys and BEEF faults likewise.
            _ => (Kind::Unclassified, None),
        }
    }

    /// The kind of a toolbox error, by its variant; `StoreBusy` and
    /// `SdkError` as the `WalletInterface` boundary gives them (bsv-rs's
    /// `WalletBusy`, the bsv-rs error itself).
    fn of_toolbox(e: &bsv_wallet_toolbox::Error) -> (Kind, Option<String>) {
        use bsv_wallet_toolbox::Error as T;
        let kind = match e {
            T::InsufficientFunds { .. } => Kind::InsufficientFunds,
            T::AuthenticationRequired | T::InvalidIdentityKey(_) => Kind::AuthenticationRequired,
            T::AccessDenied(_) => Kind::AccessDenied,
            T::NotFound { .. } => Kind::NotFound,
            T::UnspendableCoin { outpoint, .. } => {
                let coin = Some(outpoint.trim().to_string()).filter(|c| !c.is_empty());
                return (Kind::UnspendableCoin, coin);
            }
            T::Duplicate { .. } | T::InvalidOperation(_) | T::InvalidTransactionStatus(_) => {
                Kind::Conflict
            }
            T::ReviewActions(_) => Kind::ReviewActions,
            T::InvalidBeef { .. } | T::SpendRefused { .. } => Kind::InvalidBeef,
            T::ValidationError(_) | T::InvalidArgument(_) => Kind::Validation,
            T::StorageError(_) | T::DatabaseError(_) | T::Internal(_) => Kind::Internal,
            T::ServiceError(_) | T::NetworkError(_) | T::BroadcastFailed(_) => Kind::Service,
            T::StoreBusy { .. } => Kind::StoreBusy,
            T::SdkError(e) => return Self::of_sdk(e),
            _ => Kind::Unclassified,
        };
        (kind, None)
    }

    fn typed(kind: Kind, coin: Option<String>, message: String) -> Self {
        Self {
            coin,
            ..Self::new(kind, message)
        }
    }

    fn from_sdk(e: &bsv_sdk::Error) -> Self {
        let (kind, coin) = Self::of_sdk(e);
        let mut error = Self::typed(kind, coin, e.to_string());
        if let bsv_sdk::Error::WalletReviewActions(review) = e {
            error.review = Some(review.clone());
        }
        error
    }

    fn from_toolbox(e: &bsv_wallet_toolbox::Error) -> Self {
        if let bsv_wallet_toolbox::Error::SdkError(sdk) = e {
            return Self::from_sdk(sdk);
        }
        let (kind, coin) = Self::of_toolbox(e);
        let mut error = Self::typed(kind, coin, e.to_string());
        if let bsv_wallet_toolbox::Error::ReviewActions(review) = e {
            error.review = Some(review.clone());
        }
        error
    }
}

impl From<bsv_sdk::Error> for AppError {
    fn from(e: bsv_sdk::Error) -> Self {
        Self::from_sdk(&e)
    }
}

impl From<bsv_wallet_toolbox::Error> for AppError {
    fn from(e: bsv_wallet_toolbox::Error) -> Self {
        Self::from_toolbox(&e)
    }
}

/// A handler's own `anyhow` error (a request field that does not decode):
/// the bsv-rs or toolbox error it carries, by its variant; any other is
/// unclassified.
impl From<anyhow::Error> for AppError {
    fn from(e: anyhow::Error) -> Self {
        let mut error = if let Some(sdk) = e.downcast_ref::<bsv_sdk::Error>() {
            Self::from_sdk(sdk)
        } else if let Some(toolbox) = e.downcast_ref::<bsv_wallet_toolbox::Error>() {
            Self::from_toolbox(toolbox)
        } else {
            Self::new(Kind::Unclassified, e.to_string())
        };
        error.message = e.to_string();
        error
    }
}

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        let (status, code) = (self.kind.status(), self.kind.code());
        // Log the response's words too: a callback deadline cannot identify
        // whether the store or the header service was still answering.
        tracing::warn!(
            status = status.as_u16(),
            code = code,
            message = %self.message,
            "request error"
        );
        let mut body = json!({"code": code, "message": self.message});
        if let Some(review) = &self.review {
            body = review_body(review);
        }
        if let Some(refusal) = &self.refusal {
            body["offeredSatoshis"] = json!(refusal.offered_satoshis);
            body["held"] = refusal.held_json();
        }
        if let Some(coin) = &self.coin {
            body["coin"] = json!(coin);
        }
        match self.kind.retry_after_secs() {
            // Retry after the pause. A callback deadline can leave partial
            // status writes, which repeated delivery converges.
            Some(secs) => (
                status,
                [(axum::http::header::RETRY_AFTER, secs.to_string())],
                Json(body),
            )
                .into_response(),
            None => (status, Json(body)).into_response(),
        }
    }
}

/// #140's host map: HTTPWalletJSON.ts at edf6e03 reads HTTP 400/code 5.
/// Keep absent optional members absent. The reference's competing BEEF is
/// a Uint8Array, whose JSON has numeric object keys, unlike the action's tx.
fn review_body(review: &bsv_sdk::wallet::ReviewActionsRefusal) -> serde_json::Value {
    let mut results = json!(review.review_action_results);
    for (result, typed) in results
        .as_array_mut()
        .unwrap()
        .iter_mut()
        .zip(&review.review_action_results)
    {
        if let Some(bytes) = &typed.competing_beef {
            result["competingBeef"] = serde_json::Value::Object(
                bytes
                    .iter()
                    .enumerate()
                    .map(|(i, byte)| (i.to_string(), json!(byte)))
                    .collect(),
            );
        }
    }
    let mut body = json!({
        "isError": true, "code": 5, "message": review.message,
        "reviewActionResults": results, "sendWithResults": review.send_with_results,
    });
    if let Some(txid) = &review.txid {
        body["txid"] = json!(hex::encode(txid));
    }
    if let Some(tx) = &review.tx {
        body["tx"] = json!(tx);
    }
    if let Some(change) = &review.no_send_change {
        body["noSendChange"] = json!(change);
    }
    body
}
