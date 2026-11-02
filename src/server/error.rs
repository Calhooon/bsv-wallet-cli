//! The served layer's one error (#81): every refusal a door answers is an
//! [`AppError`], its status and code fixed by its [`Kind`]. A kind is
//! reached from a typed error by its variant (bsv-rs's `Error`, the
//! toolbox's `Error`, the spend gate's `GateError`, the store busy by its
//! type) or named by the handler that refuses; never from the words of a
//! message. Toolbox refusals retain their reference class through the SDK
//! boundary (#330). Untyped faults stay unclassified; no message is a class.
//! The served message is the error's Display.

use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde_json::json;

use bsv_sdk::WalletRefusalClass;

/// What a refusal is: one status and one code each (`answer`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// The caller's request does not parse or is refused as invalid.
    Validation,
    /// An error with no class of its own: bsv-rs's primitives, a transport
    /// fault (`WalletTransport`), a request field that does not decode, a
    /// toolbox refusal outside the table, the store's read before a door.
    Unclassified,
    /// The toolbox's shortfall, or the gate's refusal listing the coins it
    /// held.
    InsufficientFunds,
    /// A coin the caller named is held by the gate.
    InputNotOffered,
    /// The toolbox's permission refusal.
    AccessDenied,
    /// The browser gate's refusal of an origin `ALLOWED_ORIGINS` does not
    /// name (#411): the desktop client's denial, 400 with the reference's
    /// code (ts-stack edf6e03 `WalletPermissionsManager.ts:1348-1349`).
    OriginDenied,
    /// The wallet's state refuses a well-formed request.
    Conflict,
    /// Completed action results require review, with their typed evidence.
    ReviewActions,
    /// The reference class says the operation is not implemented.
    NotImplemented,
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
            Kind::Unclassified => (StatusCode::BAD_REQUEST, "ERROR"),
            Kind::InsufficientFunds => (StatusCode::PAYMENT_REQUIRED, "INSUFFICIENT_FUNDS"),
            Kind::InputNotOffered => (StatusCode::FORBIDDEN, "INPUT_NOT_OFFERED"),
            Kind::AccessDenied => (StatusCode::FORBIDDEN, "ACCESS_DENIED"),
            Kind::OriginDenied => (StatusCode::BAD_REQUEST, "ERR_PERMISSION_DENIED"),
            Kind::Conflict => (StatusCode::CONFLICT, "CONFLICT"),
            Kind::ReviewActions => (StatusCode::BAD_REQUEST, "5"),
            Kind::NotImplemented => (StatusCode::NOT_IMPLEMENTED, "NOT_IMPLEMENTED"),
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

    /// The published reference's WERR class, never the message's words.
    const fn of_refusal(class: WalletRefusalClass) -> Self {
        use WalletRefusalClass as C;
        match class {
            C::InvalidParameter
            | C::MissingParameter
            | C::BadRequest
            | C::InvalidMerkleRoot
            | C::InvalidPublicKey
            | C::InvalidHmac
            | C::InvalidSignature => Self::Validation,
            C::InsufficientFunds => Self::InsufficientFunds,
            C::Unauthorized => Self::AccessDenied,
            C::InvalidOperation
            | C::ActionBatchState
            | C::AbortRefused
            | C::NotActive
            | C::UtxoReviewInconclusive => Self::Conflict,
            C::UnsupportedAction | C::NotImplemented => Self::NotImplemented,
            C::Internal => Self::Internal,
            C::BroadcastUnavailable | C::NetworkChain => Self::Service,
            C::ReviewActions => Self::ReviewActions,
            _ => Self::Unclassified,
        }
    }
}

/// A refusal as a door answers it: `{code, message}` at the kind's status,
/// with the gate's listing when there is one.
#[derive(Debug)]
pub struct AppError {
    kind: Kind,
    message: String,
    /// The spend gate's refusal: every coin held back, and why.
    refusal: Option<Box<crate::spend_gate::Refusal>>,
    /// The reference's five review-action members, never inferred from words.
    review: Option<Box<bsv_sdk::wallet::ReviewActionsRefusal>>,
    /// The operator's recovery hint, selected on a typed record (an
    /// abort's stored acceptance, #342) and served after the message.
    hint: Option<String>,
}

impl AppError {
    /// A refusal of `kind` in the handler's own words.
    pub(crate) fn new(kind: Kind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
            refusal: None,
            review: None,
            hint: None,
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

    /// `signAction`'s failure. The reference's not-implemented class is
    /// raised there for one case, a reference with no pending action in
    /// this process (absent or expired: toolbox 0.14.0-beta.5
    /// `wallet.rs:2016-2041`, after ts-stack `edf6e03` `signAction.ts:57-60`
    /// "recovery of out-of-session signAction reference data"): the
    /// caller's reference names nothing, the 400 `VALIDATION_ERROR`
    /// `abortAction` answers for the same reference (#122 item 5). Any other
    /// failure keeps its class.
    pub(crate) fn from_sign_action(e: bsv_sdk::Error) -> Self {
        match &e {
            bsv_sdk::Error::WalletRefusal {
                class: WalletRefusalClass::NotImplemented,
                ..
            } => Self::new(Kind::Validation, e.to_string()),
            _ => Self::from(e),
        }
    }

    fn with_refusal(mut self, refusal: crate::spend_gate::Refusal) -> Self {
        self.refusal = Some(Box::new(refusal));
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

    /// A classed refusal or fault. Generic `WalletError` has no typed family;
    /// a `WalletFault` has one (bsv-rs #439, #342).
    fn of_sdk(e: &bsv_sdk::Error) -> Kind {
        match e {
            bsv_sdk::Error::WalletBusy { .. } => Kind::StoreBusy,
            bsv_sdk::Error::WalletReviewActions(_) => Kind::ReviewActions,
            bsv_sdk::Error::WalletRefusal { class, .. } => Kind::of_refusal(*class),
            bsv_sdk::Error::AbortAccepted(record) => Kind::of_refusal(record.class),
            bsv_sdk::Error::TransportFault { .. } => Kind::Service,
            bsv_sdk::Error::WalletFault(fault) => Self::of_fault(fault.family),
            bsv_sdk::Error::TransactionNotJudged { cause, .. } => Self::of_not_judged(cause),
            _ => Kind::Unclassified,
        }
    }

    /// A typed toolbox fault by its family: the far side's (a transport or
    /// a service) is 502, the wallet's own (its store, its build) is 500. A
    /// family a later package adds is the stack's own, never the caller's.
    fn of_fault(family: bsv_sdk::WalletFaultFamily) -> Kind {
        use bsv_sdk::WalletFaultFamily as F;
        match family {
            F::Transport | F::Service => Kind::Service,
            // Storage, Internal, and any family a later package adds.
            _ => Kind::Internal,
        }
    }

    /// An input the stack could not judge (bsv-rs #452), by its cause and
    /// never its words. A coin or a spend before Genesis is the wallet's own
    /// coin unjudgeable under this stack's word, permanent for that coin: the
    /// wallet's state refuses a well-formed request (409). A tip, an
    /// activation height, a source height or the work budget is the stack's
    /// own configuration or policy, never the coin's or the caller's (500);
    /// a cause a later package adds is the stack's too.
    fn of_not_judged(cause: &bsv_sdk::script::NotJudgedCause) -> Kind {
        use bsv_sdk::script::NotJudgedCause as C;
        match cause {
            C::PreGenesisCoin { .. } | C::PreGenesisSpend { .. } => Kind::Conflict,
            // TipHeightOverflow, ZeroActivationHeight, SourceHeightOverflow,
            // WorkBudgetExceeded, TransactionWorkBudgetExceeded (bsv-rs
            // 0.9.0-beta.5, #519: the per-transaction budget), and any cause
            // a later package adds.
            _ => Kind::Internal,
        }
    }

    /// The recovery hint an abort refused on stored acceptance carries
    /// (#342): selected on the typed record, its transaction, status and
    /// accepting word as the toolbox knew them, never on the message.
    fn abort_hint(record: &bsv_sdk::AbortAcceptance) -> String {
        let mut known = Vec::new();
        if let Some(txid) = &record.txid {
            known.push(format!("transaction {txid}"));
        }
        if let Some(status) = &record.status {
            known.push(format!("status '{status}'"));
        }
        if let Some(word) = &record.accepting_word {
            known.push(format!("accepting word {word}"));
        }
        let known = if known.is_empty() {
            String::new()
        } else {
            format!(" ({})", known.join(", "))
        };
        format!(
            "the wallet holds an accepting word for this transaction{known}; it stays until \
             a proof, node verdict or competitor's checked proof decides it"
        )
    }

    /// Storage callers retain their fault type; wallet callers use `of_sdk`.
    fn of_toolbox(e: &bsv_wallet_toolbox::Error) -> Kind {
        use bsv_wallet_toolbox::Error as T;
        match e {
            T::InsufficientFunds { .. } => Kind::InsufficientFunds,
            T::AuthenticationRequired | T::AccessDenied(_) => Kind::AccessDenied,
            T::InvalidIdentityKey(_)
            | T::NotFound { .. }
            | T::UserNotFound(_)
            | T::InvalidBeef { .. }
            | T::SpendRefused { .. }
            | T::ValidationError(_)
            | T::InvalidArgument(_) => Kind::Validation,
            T::UnspendableCoin { .. }
            | T::FailedAncestor { .. }
            | T::Duplicate { .. }
            | T::InvalidOperation(_)
            | T::InvalidTransactionStatus(_)
            | T::StorageNotAvailable => Kind::Conflict,
            T::ReviewActions(_) => Kind::ReviewActions,
            T::StorageError(_) | T::DatabaseError(_) | T::Internal(_) => Kind::Internal,
            T::ServiceError(_) | T::NetworkError(_) | T::BroadcastFailed(_) => Kind::Service,
            T::StoreBusy { .. } => Kind::StoreBusy,
            T::SdkError(e) => Self::of_sdk(e),
            _ => Kind::Unclassified,
        }
    }

    fn from_sdk(e: &bsv_sdk::Error) -> Self {
        let mut error = Self::new(Self::of_sdk(e), e.to_string());
        match e {
            bsv_sdk::Error::WalletReviewActions(review) => error.review = Some(review.clone()),
            bsv_sdk::Error::AbortAccepted(record) => error.hint = Some(Self::abort_hint(record)),
            _ => {}
        }
        error
    }

    fn from_toolbox(e: &bsv_wallet_toolbox::Error) -> Self {
        if let bsv_wallet_toolbox::Error::SdkError(sdk) = e {
            let mut error = Self::from_sdk(sdk);
            error.message = e.to_string();
            return error;
        }
        let mut error = Self::new(Self::of_toolbox(e), e.to_string());
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
        let message = match &self.hint {
            Some(hint) => format!("{} (hint: {hint})", self.message),
            None => self.message,
        };
        // Log the response's words too: a callback deadline cannot identify
        // whether the store or the header service was still answering.
        tracing::warn!(
            status = status.as_u16(),
            code = code,
            message = %message,
            "request error"
        );
        let mut body = json!({"code": code, "message": message});
        if let Some(review) = &self.review {
            body = review_body(review);
        }
        if let Some(refusal) = &self.refusal {
            body["offeredSatoshis"] = json!(refusal.offered_satoshis);
            body["held"] = refusal.held_json();
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

/// #140's host map: `HTTPWalletJSON.ts` at edf6e03 reads HTTP 400/code 5.
/// Keep absent optional members absent. The reference's competing BEEF is
/// a `Uint8Array`, whose JSON has numeric object keys, unlike the action's tx.
fn review_body(review: &bsv_sdk::wallet::ReviewActionsRefusal) -> serde_json::Value {
    let results: Vec<_> = review
        .review_action_results
        .iter()
        .map(|typed| {
            let mut result = json!(typed);
            if let Some(bytes) = &typed.competing_beef {
                result["competingBeef"] = serde_json::Value::Object(
                    bytes
                        .iter()
                        .enumerate()
                        .map(|(i, byte)| (i.to_string(), json!(byte)))
                        .collect(),
                );
            }
            result
        })
        .collect();
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

#[cfg(test)]
mod tests {
    use super::*;

    /// #122 item 5: signAction's unknown reference is the caller's 400, as
    /// abortAction's; any other class at that door keeps its own answer.
    #[test]
    fn a_sign_action_reference_miss_is_the_callers_400() {
        let miss = bsv_sdk::Error::WalletRefusal {
            class: WalletRefusalClass::NotImplemented,
            message: "No pending transaction found for reference: cmVm".to_string(),
        };
        let served = AppError::from_sign_action(miss);
        assert_eq!(
            served.kind.answer(),
            (StatusCode::BAD_REQUEST, "VALIDATION_ERROR")
        );
        assert_eq!(
            served.message,
            "No pending transaction found for reference: cmVm"
        );
        let other = bsv_sdk::Error::WalletRefusal {
            class: WalletRefusalClass::InvalidOperation,
            message: "not signable".to_string(),
        };
        assert_eq!(AppError::from_sign_action(other).kind, Kind::Conflict);
    }

    /// #386 (bsv-rs #582 at toolbox 0.14.0-beta.6): a certifier whose
    /// general answer names its identity key in upper case is refused by
    /// bsv-rs's transport (`assert_auth_identity_key`); the toolbox's
    /// issuance door (`certificate_issuance_auth.rs` `send_error`) carries
    /// that refusal as `NetworkError`, a `WalletFault` of the Transport
    /// family at the boundary, so `acquireCertificate` serves the far side's
    /// 502 `SERVICE_ERROR` in the toolbox's words, never 400 `ERROR`.
    #[test]
    fn an_upper_case_certifier_answer_at_issuance_is_the_far_sides_502() {
        let upper = "0279be667ef9dcbbac55a06295ce870b07029bfcdb2dce28d959f2815b16f81798"
            .to_ascii_uppercase();
        let refusal = bsv_sdk::auth::assert_auth_identity_key(&upper).unwrap_err();
        assert!(matches!(refusal, bsv_sdk::Error::AuthError(_)));
        assert!(refusal.transport_fault().is_none());
        let toolbox =
            bsv_wallet_toolbox::Error::NetworkError(format!("Failed to send request: {refusal}"));
        let served = AppError::from(bsv_sdk::Error::from(toolbox));
        assert_eq!(
            served.kind.answer(),
            (StatusCode::BAD_GATEWAY, "SERVICE_ERROR")
        );
        assert_eq!(
            served.message,
            "wallet error: Network error: Failed to send request: authentication error: \
             Authentication identityKey must be a canonical compressed public key."
        );
    }
}
