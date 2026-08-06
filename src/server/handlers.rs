use anyhow::{Context, Result};
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use bsv_sdk::primitives::{to_hex, PublicKey};
use bsv_sdk::wallet::{
    // Batch 4: Transaction types (have serde — pass through)
    AbortActionArgs,
    AbortActionResult,
    // Batch 5: Certificate types (have serde — pass through)
    AcquireCertificateArgs,
    // Batch 1: Status result types
    AuthenticatedResult,
    // Existing endpoint types
    BasketInsertion,
    // Core types
    Counterparty,
    CreateActionArgs,
    CreateActionInput,
    CreateActionOptions,
    CreateActionOutput,
    CreateHmacArgs,
    CreateSignatureArgs,
    // Batch 3: Crypto arg types (no serde — manual construction)
    DecryptArgs,
    // Batch 6: Discovery types (have serde — pass through)
    DiscoverByAttributesArgs,
    DiscoverByIdentityKeyArgs,
    DiscoverCertificatesResult,
    EncryptArgs,
    // Batch 2: Header types
    GetHeaderArgs,
    GetHeaderResult,
    GetHeightResult,
    GetNetworkResult,
    GetPublicKeyArgs,
    GetVersionResult,
    InternalizeActionArgs,
    InternalizeOutput,
    ListActionsArgs,
    ListActionsResult,
    ListCertificatesArgs,
    ListCertificatesResult,
    // Output types
    ListOutputsArgs,
    ListOutputsResult,
    Outpoint,
    Protocol,
    ProveCertificateArgs,
    ProveCertificateResult,
    RelinquishCertificateArgs,
    RelinquishCertificateResult,
    RelinquishOutputArgs,
    RelinquishOutputResult,
    // Key linkage types
    RevealCounterpartyKeyLinkageResult,
    RevealSpecificKeyLinkageResult,
    SecurityLevel,
    SignActionArgs,
    SignActionResult,
    TrustSelf,
    VerifyHmacArgs,
    VerifySignatureArgs,
    WalletCertificate,
    WalletInterface,
    WalletPayment,
    WalletRevealCounterpartyArgs,
    WalletRevealSpecificArgs,
};
use bsv_wallet_toolbox::{Services, StorageSqlx, Wallet};
use serde_json::json;
use std::sync::Arc;

use super::types::*;

pub type WalletState = Arc<Wallet<StorageSqlx, Services>>;

/// Extract originator from Origin or Originator header
fn extract_originator(
    headers: &HeaderMap,
) -> Result<String, (StatusCode, Json<serde_json::Value>)> {
    if let Some(origin) = headers.get("origin").and_then(|v| v.to_str().ok()) {
        if let Ok(url) = url::Url::parse(origin) {
            if let Some(host) = url.host_str() {
                return Ok(host.to_string());
            }
        }
    }
    if let Some(originator) = headers.get("originator").and_then(|v| v.to_str().ok()) {
        return Ok(originator.to_string());
    }
    Err((
        StatusCode::BAD_REQUEST,
        Json(json!({"message": "Origin header required"})),
    ))
}

/// Classified error response with proper HTTP status codes.
pub struct AppError {
    status: StatusCode,
    code: &'static str,
    message: String,
}

impl AppError {
    /// Classify a wallet error message into an appropriate HTTP status + code.
    fn classify(msg: &str) -> (StatusCode, &'static str) {
        // Strip "wallet error: " prefix the SDK wraps errors with
        let clean = msg.strip_prefix("wallet error: ").unwrap_or(msg);

        if clean.starts_with("Insufficient funds:") || clean.starts_with("insufficient funds:") {
            (StatusCode::PAYMENT_REQUIRED, "INSUFFICIENT_FUNDS")
        } else if clean.starts_with("Authentication required")
            || clean.starts_with("Invalid identity key:")
        {
            (StatusCode::UNAUTHORIZED, "AUTHENTICATION_REQUIRED")
        } else if clean.starts_with("Access denied:") {
            (StatusCode::FORBIDDEN, "ACCESS_DENIED")
        } else if clean.starts_with("Entity not found:") || clean.starts_with("not found:") {
            (StatusCode::NOT_FOUND, "NOT_FOUND")
        } else if clean.starts_with("Duplicate entity:") || clean.starts_with("Invalid operation:")
        {
            (StatusCode::CONFLICT, "CONFLICT")
        } else if clean.starts_with("Validation error:")
            || clean.starts_with("Invalid argument:")
            || clean.starts_with("Origin header required")
        {
            (StatusCode::BAD_REQUEST, "VALIDATION_ERROR")
        } else if clean.starts_with("Storage error:")
            || clean.starts_with("Database error:")
            || clean.starts_with("Internal error:")
        {
            (StatusCode::INTERNAL_SERVER_ERROR, "INTERNAL_ERROR")
        } else if clean.starts_with("Service error:")
            || clean.starts_with("Network error:")
            || clean.starts_with("Broadcast failed:")
        {
            (StatusCode::BAD_GATEWAY, "SERVICE_ERROR")
        } else {
            (StatusCode::BAD_REQUEST, "ERROR")
        }
    }

    /// Create from a wallet/SDK error (the most common path).
    fn from_wallet_error(e: impl std::fmt::Display) -> Self {
        let message = e.to_string();
        let (status, code) = Self::classify(&message);
        Self {
            status,
            code,
            message,
        }
    }

    fn message(&self) -> &str {
        &self.message
    }

    /// Append an operator-facing recovery hint to the error message.
    fn with_hint(mut self, hint: &str) -> Self {
        self.message = format!("{} (hint: {})", self.message, hint);
        self
    }

    /// A broadcast that `create_action` reported as `Ok`, but which the network
    /// confirms never landed (silently dropped — typically ARC 465 fee-too-low
    /// on a deep unconfirmed BEEF). Surfaced as a gateway error so callers see a
    /// failure instead of a phantom txid.
    fn broadcast_rejected(txid: &str) -> Self {
        Self {
            status: StatusCode::BAD_GATEWAY,
            code: "BROADCAST_REJECTED",
            message: format!(
                "broadcast rejected: transaction {txid} is absent from BOTH the broadcaster \
                 it was submitted to AND an independent chain index, after the full probe \
                 window. The broadcaster dropped it — most likely error 465 \"fee too low\" \
                 on a deep unconfirmed BEEF. The funds were NOT sent."
            ),
        }
    }
}

impl From<anyhow::Error> for AppError {
    fn from(err: anyhow::Error) -> Self {
        Self::from_wallet_error(err)
    }
}

impl From<(StatusCode, Json<serde_json::Value>)> for AppError {
    fn from((status, json): (StatusCode, Json<serde_json::Value>)) -> Self {
        let message = json
            .0
            .get("message")
            .and_then(|m| m.as_str())
            .unwrap_or_default()
            .to_string();
        let (_, code) = Self::classify(&message);
        Self {
            status,
            code,
            message,
        }
    }
}

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        tracing::warn!(
            status = self.status.as_u16(),
            code = self.code,
            message = %self.message,
            "request error"
        );
        (
            self.status,
            Json(json!({"code": self.code, "message": self.message})),
        )
            .into_response()
    }
}

/// GET /isAuthenticated
pub async fn is_authenticated() -> Json<serde_json::Value> {
    Json(json!({"authenticated": true}))
}

/// POST /getPublicKey
pub async fn get_public_key(
    State(wallet): State<WalletState>,
    headers: HeaderMap,
    Json(req): Json<McGetPublicKeyReq>,
) -> Result<Json<McGetPublicKeyRes>, AppError> {
    let originator = extract_originator(&headers)?;

    let args = GetPublicKeyArgs {
        identity_key: req.identity_key,
        protocol_id: req.protocol_id.map(|v| parse_protocol_id(&v)).transpose()?,
        key_id: req.key_id,
        counterparty: req
            .counterparty
            .map(|s| parse_counterparty(&s))
            .transpose()?,
        for_self: req.for_self,
    };

    let result = wallet
        .get_public_key(args, &originator)
        .await
        .map_err(AppError::from_wallet_error)?;

    Ok(Json(McGetPublicKeyRes {
        public_key: result.public_key,
    }))
}

/// POST /createSignature
pub async fn create_signature(
    State(wallet): State<WalletState>,
    headers: HeaderMap,
    Json(req): Json<McCreateSignatureReq>,
) -> Result<Json<McCreateSignatureRes>, AppError> {
    let originator = extract_originator(&headers)?;

    // ts-sdk sends exactly one of `data` (hash-then-sign) or
    // `hashToDirectlySign` (sign the 32-byte digest as-is).
    let hash_to_directly_sign = req
        .hash_to_directly_sign
        .map(|h| {
            <[u8; 32]>::try_from(h.as_slice()).map_err(|_| {
                AppError::from_wallet_error("Validation error: hashToDirectlySign must be 32 bytes")
            })
        })
        .transpose()?;
    if req.data.is_none() && hash_to_directly_sign.is_none() {
        return Err(AppError::from_wallet_error(
            "Validation error: one of data or hashToDirectlySign is required",
        ));
    }

    let args = CreateSignatureArgs {
        data: req.data,
        hash_to_directly_sign,
        protocol_id: parse_protocol_id(&req.protocol_id)?,
        key_id: req.key_id,
        counterparty: Some(parse_counterparty(&req.counterparty)?),
    };

    let result = wallet
        .create_signature(args, &originator)
        .await
        .map_err(AppError::from_wallet_error)?;

    Ok(Json(McCreateSignatureRes {
        signature: result.signature,
    }))
}

/// POST /createAction
///
/// Spending operations are serialized via `SpendingLock` so that concurrent
/// requests queue up ("bus stop" pattern) rather than racing on SQLite's write
/// lock. Non-spending endpoints remain fully concurrent.
pub async fn create_action(
    State(wallet): State<WalletState>,
    axum::Extension(spending_lock): axum::Extension<super::SpendingLock>,
    axum::Extension(verifier): axum::Extension<crate::broadcast_verify::BroadcastVerifier>,
    headers: HeaderMap,
    Json(req): Json<McCreateActionReq>,
) -> Result<Json<McCreateActionRes>, AppError> {
    let originator = extract_originator(&headers)?;

    // Whether this request performs an immediate broadcast (vs. no-send /
    // delayed / external-signing). Only immediate broadcasts are verified.
    let (opt_no_send, opt_accept_delayed) = req
        .options
        .as_ref()
        .map(|o| {
            (
                o.no_send.unwrap_or(false),
                o.accept_delayed_broadcast.unwrap_or(false),
            )
        })
        .unwrap_or((false, false));

    let outputs = req
        .outputs
        .map(|outs| {
            outs.into_iter()
                .map(|o| {
                    Ok(CreateActionOutput {
                        locking_script: hex::decode(&o.locking_script)?,
                        satoshis: o.satoshis,
                        output_description: o.output_description,
                        basket: o.basket,
                        custom_instructions: o.custom_instructions,
                        tags: o.tags,
                    })
                })
                .collect::<Result<Vec<_>>>()
        })
        .transpose()?;

    // Map caller-provided explicit inputs (e.g. a covenant spend carrying a full
    // unlockingScript). An absent/empty list stays `Some(vec![])` so the wallet
    // auto-selects its own funding UTXOs — the prior behaviour for plain sends.
    // Before this, the handler discarded `inputs`/`inputBEEF`/`lockTime`, so any
    // covenant input was silently dropped and the tx was funded from change.
    let inputs = match req.inputs {
        Some(reqs) if !reqs.is_empty() => {
            let mut mapped = Vec::with_capacity(reqs.len());
            for i in reqs {
                let outpoint = Outpoint::from_string(&i.outpoint)
                    .map_err(|e| anyhow::anyhow!("invalid input outpoint '{}': {e}", i.outpoint))?;
                let unlocking_script = match i.unlocking_script {
                    Some(h) => Some(hex::decode(&h).context("invalid unlockingScript hex")?),
                    None => None,
                };
                mapped.push(CreateActionInput {
                    outpoint,
                    input_description: i.input_description.unwrap_or_else(|| "input".to_string()),
                    unlocking_script,
                    unlocking_script_length: i.unlocking_script_length,
                    sequence_number: i.sequence_number,
                });
            }
            Some(mapped)
        }
        _ => Some(vec![]),
    };

    let args = CreateActionArgs {
        description: req.description,
        input_beef: req.input_beef,
        inputs,
        outputs,
        lock_time: req.lock_time,
        version: None,
        labels: req.labels,
        options: req.options.map(|o| CreateActionOptions {
            accept_delayed_broadcast: o.accept_delayed_broadcast,
            randomize_outputs: o.randomize_outputs,
            sign_and_process: o.sign_and_process,
            no_send: o.no_send,
            trust_self: match o.trust_self.as_deref() {
                Some("known") => Some(TrustSelf::Known),
                _ => None,
            },
            ..Default::default()
        }),
    };

    let output_count = args.outputs.as_ref().map(|o| o.len()).unwrap_or(0);
    let total_sats: u64 = args
        .outputs
        .as_ref()
        .map(|o| o.iter().map(|x| x.satoshis).sum())
        .unwrap_or(0);

    // Acquire spending lock — queues behind any in-flight createAction.
    // Once the previous tx completes (and its change UTXO exists), we proceed.
    let _guard = spending_lock.lock().await;
    let result = wallet
        .create_action(args, &originator)
        .await
        .map_err(AppError::from_wallet_error)?;
    drop(_guard);

    tracing::info!(
        originator = %originator,
        outputs = output_count,
        total_satoshis = total_sats,
        txid = ?result.txid.as_ref().map(|t| to_hex(t)),
        "createAction"
    );

    // Build AtomicBEEF from result.beef (which has ancestors) + txid.
    // result.tx is raw tx bytes; clients expect AtomicBEEF.
    let atomic_beef = match (result.beef.as_ref(), result.txid.as_ref()) {
        (Some(beef_bytes), Some(txid_bytes)) => {
            let txid_hex = to_hex(txid_bytes);
            match bsv_sdk::transaction::Beef::from_binary(beef_bytes) {
                Ok(mut beef) => match beef.to_binary_atomic(&txid_hex) {
                    Ok(ab) => Some(ab),
                    Err(e) => {
                        tracing::warn!(error = %e, "Failed to build AtomicBEEF, falling back to raw tx");
                        result.tx.clone()
                    }
                },
                Err(e) => {
                    tracing::warn!(error = %e, "Failed to parse BEEF, falling back to raw tx");
                    result.tx.clone()
                }
            }
        }
        _ => result.tx.clone(),
    };

    // Fail loud if an immediate broadcast was silently dropped. `create_action`
    // returns Ok with a txid even when ARC rejected the tx (e.g. 465 fee-too-low
    // on a deep unconfirmed BEEF), because the toolbox misclassifies a 465 as a
    // transient service error. Only immediate broadcasts (not no-send / delayed /
    // external-signing) are verified. A definitive network absence → 502, so the
    // caller sees a failure instead of a phantom (never-mined) txid.
    if !opt_no_send && !opt_accept_delayed && result.signable_transaction.is_none() {
        if let Some(txid_bytes) = result.txid.as_ref() {
            let txid_hex = to_hex(txid_bytes);
            if verifier.verify(&txid_hex).await
                == crate::broadcast_verify::BroadcastVerification::Rejected
            {
                return Err(AppError::broadcast_rejected(&txid_hex));
            }
        }
    }

    Ok(Json(McCreateActionRes {
        txid: result.txid.map(|t| to_hex(&t)),
        tx: atomic_beef,
        send_with_results: result
            .send_with_results
            .and_then(|r| serde_json::to_value(r).ok()),
        signable_transaction: result.signable_transaction.map(|st| {
            // The SDK stores reference as Vec<u8> from String.into_bytes().
            // Reverse that to get the original reference string that
            // signAction/abortAction can look up in the pending tx cache.
            let reference =
                String::from_utf8(st.reference).unwrap_or_else(|e| hex::encode(e.into_bytes()));
            McSignableTransaction {
                tx: st.tx,
                reference,
            }
        }),
        no_send_change: result
            .no_send_change
            .and_then(|v| serde_json::to_value(v).ok()),
    }))
}

/// POST /internalizeAction
pub async fn internalize_action(
    State(wallet): State<WalletState>,
    headers: HeaderMap,
    Json(req): Json<McInternalizeActionReq>,
) -> Result<Json<McInternalizeActionRes>, AppError> {
    let originator = extract_originator(&headers)?;

    let args = InternalizeActionArgs {
        tx: req.tx,
        outputs: req
            .outputs
            .into_iter()
            .map(|o| InternalizeOutput {
                output_index: o.output_index,
                protocol: o.protocol,
                payment_remittance: o.payment_remittance.map(|p| WalletPayment {
                    derivation_prefix: p.derivation_prefix,
                    derivation_suffix: p.derivation_suffix,
                    sender_identity_key: p.sender_identity_key,
                }),
                insertion_remittance: o.insertion_remittance.map(|i| BasketInsertion {
                    basket: i.basket,
                    custom_instructions: i.custom_instructions,
                    tags: i.tags,
                }),
            })
            .collect(),
        description: req.description,
        labels: req.labels,
        seek_permission: None,
    };

    let output_count = args.outputs.len();
    let result = wallet
        .internalize_action(args, &originator)
        .await
        .map_err(AppError::from_wallet_error)?;

    tracing::info!(
        originator = %originator,
        outputs = output_count,
        accepted = result.accepted,
        "internalizeAction"
    );

    Ok(Json(McInternalizeActionRes {
        accepted: result.accepted,
    }))
}

// --- Helpers ---

/// Parse [2, "protocol_name"] → Protocol
fn parse_protocol_id(v: &serde_json::Value) -> Result<Protocol> {
    let arr = v.as_array().context("protocolID must be an array")?;
    let level = arr
        .first()
        .and_then(|v| v.as_u64())
        .context("protocolID[0] must be an integer")?;
    let name = arr
        .get(1)
        .and_then(|v| v.as_str())
        .context("protocolID[1] must be a string")?;
    Ok(Protocol::new(
        match level {
            0 => SecurityLevel::Silent,
            1 => SecurityLevel::App,
            2 => SecurityLevel::Counterparty,
            _ => anyhow::bail!("invalid security level: {level}"),
        },
        name,
    ))
}

/// Parse "03abc..." → Counterparty::Other(PublicKey)
fn parse_counterparty(s: &str) -> Result<Counterparty> {
    match s {
        "self" => Ok(Counterparty::Self_),
        "anyone" => Ok(Counterparty::Anyone),
        hex => Ok(Counterparty::Other(PublicKey::from_hex(hex)?)),
    }
}

// =============================================================================
// Batch 1: Status (GET endpoints)
// =============================================================================

/// GET /getHeight
pub async fn get_height(
    State(wallet): State<WalletState>,
) -> Result<Json<GetHeightResult>, AppError> {
    let result = wallet
        .get_height("localhost")
        .await
        .map_err(AppError::from_wallet_error)?;
    Ok(Json(result))
}

/// GET /getNetwork
pub async fn get_network(
    State(wallet): State<WalletState>,
) -> Result<Json<GetNetworkResult>, AppError> {
    let result = wallet
        .get_network("localhost")
        .await
        .map_err(AppError::from_wallet_error)?;
    Ok(Json(result))
}

/// GET /getVersion
pub async fn get_version(
    State(wallet): State<WalletState>,
) -> Result<Json<GetVersionResult>, AppError> {
    let result = wallet
        .get_version("localhost")
        .await
        .map_err(AppError::from_wallet_error)?;
    Ok(Json(result))
}

/// GET /waitForAuthentication
pub async fn wait_for_authentication(
    State(wallet): State<WalletState>,
) -> Result<Json<AuthenticatedResult>, AppError> {
    let result = wallet
        .wait_for_authentication("localhost")
        .await
        .map_err(AppError::from_wallet_error)?;
    Ok(Json(result))
}

// =============================================================================
// Batch 2: Header
// =============================================================================

/// POST /getHeaderForHeight
pub async fn get_header_for_height(
    State(wallet): State<WalletState>,
    headers: HeaderMap,
    Json(args): Json<GetHeaderArgs>,
) -> Result<Json<GetHeaderResult>, AppError> {
    let originator = extract_originator(&headers)?;
    let result = wallet
        .get_header_for_height(args, &originator)
        .await
        .map_err(AppError::from_wallet_error)?;
    Ok(Json(result))
}

// =============================================================================
// Batch 3: Crypto
// =============================================================================

/// POST /verifySignature
pub async fn verify_signature(
    State(wallet): State<WalletState>,
    headers: HeaderMap,
    Json(req): Json<McVerifySignatureReq>,
) -> Result<Json<serde_json::Value>, AppError> {
    let originator = extract_originator(&headers)?;
    let args = VerifySignatureArgs {
        data: req.data,
        hash_to_directly_verify: None,
        signature: req.signature,
        protocol_id: parse_protocol_id(&req.protocol_id)?,
        key_id: req.key_id,
        counterparty: req
            .counterparty
            .map(|s| parse_counterparty(&s))
            .transpose()?,
        for_self: req.for_self,
    };
    let result = wallet
        .verify_signature(args, &originator)
        .await
        .map_err(AppError::from_wallet_error)?;
    Ok(Json(json!({ "valid": result.valid })))
}

/// POST /encrypt
pub async fn encrypt(
    State(wallet): State<WalletState>,
    headers: HeaderMap,
    Json(req): Json<McEncryptReq>,
) -> Result<Json<serde_json::Value>, AppError> {
    let originator = extract_originator(&headers)?;
    let args = EncryptArgs {
        plaintext: req.plaintext,
        protocol_id: parse_protocol_id(&req.protocol_id)?,
        key_id: req.key_id,
        counterparty: req
            .counterparty
            .map(|s| parse_counterparty(&s))
            .transpose()?,
    };
    let result = wallet
        .encrypt(args, &originator)
        .await
        .map_err(AppError::from_wallet_error)?;
    Ok(Json(json!({ "ciphertext": result.ciphertext })))
}

/// POST /decrypt
pub async fn decrypt(
    State(wallet): State<WalletState>,
    headers: HeaderMap,
    Json(req): Json<McDecryptReq>,
) -> Result<Json<serde_json::Value>, AppError> {
    let originator = extract_originator(&headers)?;
    let args = DecryptArgs {
        ciphertext: req.ciphertext,
        protocol_id: parse_protocol_id(&req.protocol_id)?,
        key_id: req.key_id,
        counterparty: req
            .counterparty
            .map(|s| parse_counterparty(&s))
            .transpose()?,
    };
    let result = wallet
        .decrypt(args, &originator)
        .await
        .map_err(AppError::from_wallet_error)?;
    Ok(Json(json!({ "plaintext": result.plaintext })))
}

/// POST /createHmac
pub async fn create_hmac(
    State(wallet): State<WalletState>,
    headers: HeaderMap,
    Json(req): Json<McCreateHmacReq>,
) -> Result<Json<serde_json::Value>, AppError> {
    let originator = extract_originator(&headers)?;
    let args = CreateHmacArgs {
        data: req.data,
        protocol_id: parse_protocol_id(&req.protocol_id)?,
        key_id: req.key_id,
        counterparty: req
            .counterparty
            .map(|s| parse_counterparty(&s))
            .transpose()?,
    };
    let result = wallet
        .create_hmac(args, &originator)
        .await
        .map_err(AppError::from_wallet_error)?;
    Ok(Json(json!({ "hmac": result.hmac.to_vec() })))
}

/// POST /verifyHmac
pub async fn verify_hmac(
    State(wallet): State<WalletState>,
    headers: HeaderMap,
    Json(req): Json<McVerifyHmacReq>,
) -> Result<Json<serde_json::Value>, AppError> {
    let originator = extract_originator(&headers)?;
    let hmac: [u8; 32] = req
        .hmac
        .try_into()
        .map_err(|_| anyhow::anyhow!("hmac must be exactly 32 bytes"))?;
    let args = VerifyHmacArgs {
        data: req.data,
        hmac,
        protocol_id: parse_protocol_id(&req.protocol_id)?,
        key_id: req.key_id,
        counterparty: req
            .counterparty
            .map(|s| parse_counterparty(&s))
            .transpose()?,
    };
    let result = wallet
        .verify_hmac(args, &originator)
        .await
        .map_err(AppError::from_wallet_error)?;
    Ok(Json(json!({ "valid": result.valid })))
}

// =============================================================================
// Batch 4: Transaction Workflow
// =============================================================================

/// POST /signAction
pub async fn sign_action(
    State(wallet): State<WalletState>,
    headers: HeaderMap,
    Json(args): Json<SignActionArgs>,
) -> Result<Json<SignActionResult>, AppError> {
    let originator = extract_originator(&headers)?;
    let reference = args.reference.clone();
    let result = wallet
        .sign_action(args, &originator)
        .await
        .map_err(AppError::from_wallet_error)?;
    tracing::info!(originator = %originator, reference = %reference, "signAction");
    Ok(Json(result))
}

/// POST /abortAction
pub async fn abort_action(
    State(wallet): State<WalletState>,
    headers: HeaderMap,
    Json(args): Json<AbortActionArgs>,
) -> Result<Json<AbortActionResult>, AppError> {
    let originator = extract_originator(&headers)?;
    let result = wallet.abort_action(args, &originator).await.map_err(|e| {
        // The toolbox (correctly) refuses to abort a broadcast tx blindly — it
        // could be on-chain, and failing it here would fabricate a phantom
        // double-spend. The sanctioned recovery for a broadcast-lie is the
        // chain-checked sweep: point the operator at it.
        let mut err = AppError::from_wallet_error(e);
        if err.message().contains("unproven") {
            err = err.with_hint(
                "an 'unproven' tx may already be on-chain; if its broadcast \
                 never landed, run `bsv-wallet cleanup-abandoned --execute` \
                 (verifies chain absence before restoring inputs) or wait for \
                 the daemon's auto-reconcile tick",
            );
        }
        err
    })?;
    Ok(Json(result))
}

/// POST /listActions
pub async fn list_actions(
    State(wallet): State<WalletState>,
    headers: HeaderMap,
    Json(args): Json<ListActionsArgs>,
) -> Result<Json<ListActionsResult>, AppError> {
    let originator = extract_originator(&headers)?;
    let result = wallet
        .list_actions(args, &originator)
        .await
        .map_err(AppError::from_wallet_error)?;
    Ok(Json(result))
}

/// POST /listOutputs
pub async fn list_outputs(
    State(wallet): State<WalletState>,
    headers: HeaderMap,
    Json(args): Json<ListOutputsArgs>,
) -> Result<Json<ListOutputsResult>, AppError> {
    let originator = extract_originator(&headers)?;
    let result = wallet
        .list_outputs(args, &originator)
        .await
        .map_err(AppError::from_wallet_error)?;
    Ok(Json(result))
}

/// POST /relinquishOutput
///
/// Acquires SpendingLock because relinquish modifies UTXO state and can race
/// with createAction on SQLite writes (previously caused SQLITE_BUSY errors).
pub async fn relinquish_output(
    State(wallet): State<WalletState>,
    axum::Extension(spending_lock): axum::Extension<super::SpendingLock>,
    headers: HeaderMap,
    Json(args): Json<RelinquishOutputArgs>,
) -> Result<Json<RelinquishOutputResult>, AppError> {
    let originator = extract_originator(&headers)?;
    let _guard = spending_lock.lock().await;
    let result = wallet
        .relinquish_output(args, &originator)
        .await
        .map_err(AppError::from_wallet_error)?;
    Ok(Json(result))
}

// =============================================================================
// Batch 5: Certificates
// =============================================================================

/// POST /acquireCertificate
pub async fn acquire_certificate(
    State(wallet): State<WalletState>,
    headers: HeaderMap,
    Json(args): Json<AcquireCertificateArgs>,
) -> Result<Json<WalletCertificate>, AppError> {
    let originator = extract_originator(&headers)?;
    let result = wallet
        .acquire_certificate(args, &originator)
        .await
        .map_err(AppError::from_wallet_error)?;
    Ok(Json(result))
}

/// POST /listCertificates
pub async fn list_certificates(
    State(wallet): State<WalletState>,
    headers: HeaderMap,
    Json(args): Json<ListCertificatesArgs>,
) -> Result<Json<ListCertificatesResult>, AppError> {
    let originator = extract_originator(&headers)?;
    let result = wallet
        .list_certificates(args, &originator)
        .await
        .map_err(AppError::from_wallet_error)?;
    Ok(Json(result))
}

/// POST /proveCertificate
pub async fn prove_certificate(
    State(wallet): State<WalletState>,
    headers: HeaderMap,
    Json(args): Json<ProveCertificateArgs>,
) -> Result<Json<ProveCertificateResult>, AppError> {
    let originator = extract_originator(&headers)?;
    let result = wallet
        .prove_certificate(args, &originator)
        .await
        .map_err(AppError::from_wallet_error)?;
    Ok(Json(result))
}

/// POST /relinquishCertificate
pub async fn relinquish_certificate(
    State(wallet): State<WalletState>,
    headers: HeaderMap,
    Json(args): Json<RelinquishCertificateArgs>,
) -> Result<Json<RelinquishCertificateResult>, AppError> {
    let originator = extract_originator(&headers)?;
    let result = wallet
        .relinquish_certificate(args, &originator)
        .await
        .map_err(AppError::from_wallet_error)?;
    Ok(Json(result))
}

// =============================================================================
// Batch 6: Discovery + Key Linkage
// =============================================================================

/// POST /discoverByIdentityKey
pub async fn discover_by_identity_key(
    State(wallet): State<WalletState>,
    headers: HeaderMap,
    Json(args): Json<DiscoverByIdentityKeyArgs>,
) -> Result<Json<DiscoverCertificatesResult>, AppError> {
    let originator = extract_originator(&headers)?;
    let result = wallet
        .discover_by_identity_key(args, &originator)
        .await
        .map_err(AppError::from_wallet_error)?;
    Ok(Json(result))
}

/// POST /discoverByAttributes
pub async fn discover_by_attributes(
    State(wallet): State<WalletState>,
    headers: HeaderMap,
    Json(args): Json<DiscoverByAttributesArgs>,
) -> Result<Json<DiscoverCertificatesResult>, AppError> {
    let originator = extract_originator(&headers)?;
    let result = wallet
        .discover_by_attributes(args, &originator)
        .await
        .map_err(AppError::from_wallet_error)?;
    Ok(Json(result))
}

/// POST /revealCounterpartyKeyLinkage
pub async fn reveal_counterparty_key_linkage(
    State(wallet): State<WalletState>,
    headers: HeaderMap,
    Json(req): Json<McRevealCounterpartyKeyLinkageReq>,
) -> Result<Json<RevealCounterpartyKeyLinkageResult>, AppError> {
    let originator = extract_originator(&headers)?;
    let args = WalletRevealCounterpartyArgs {
        counterparty: PublicKey::from_hex(&req.counterparty)
            .map_err(|e| anyhow::anyhow!("{}", e))?,
        verifier: PublicKey::from_hex(&req.verifier).map_err(|e| anyhow::anyhow!("{}", e))?,
        privileged: req.privileged,
        privileged_reason: req.privileged_reason,
    };
    let result = wallet
        .reveal_counterparty_key_linkage(args, &originator)
        .await
        .map_err(AppError::from_wallet_error)?;
    Ok(Json(result))
}

/// POST /revealSpecificKeyLinkage
pub async fn reveal_specific_key_linkage(
    State(wallet): State<WalletState>,
    headers: HeaderMap,
    Json(req): Json<McRevealSpecificKeyLinkageReq>,
) -> Result<Json<RevealSpecificKeyLinkageResult>, AppError> {
    let originator = extract_originator(&headers)?;
    let args = WalletRevealSpecificArgs {
        counterparty: parse_counterparty(&req.counterparty)?,
        verifier: PublicKey::from_hex(&req.verifier).map_err(|e| anyhow::anyhow!("{}", e))?,
        protocol_id: parse_protocol_id(&req.protocol_id)?,
        key_id: req.key_id,
        privileged: req.privileged,
        privileged_reason: req.privileged_reason,
    };
    let result = wallet
        .reveal_specific_key_linkage(args, &originator)
        .await
        .map_err(AppError::from_wallet_error)?;
    Ok(Json(result))
}
