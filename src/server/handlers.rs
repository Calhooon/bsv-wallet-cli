use anyhow::{Context, Result};
use axum::extract::State;
use axum::http::HeaderMap;
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

pub use super::error::AppError;
use super::error::Kind;
use super::page_bounds::check_page_bounds;
use super::types::*;

pub type WalletState = Arc<Wallet<StorageSqlx, Services>>;

use super::audit;

/// BRC-43 security level as its number. The enum names are the spec's own
/// meanings: Silent = 0 (never prompts), App = 1 (once per protocol),
/// Counterparty = 2 (once per protocol AND counterparty).
fn cp_str(c: &Counterparty) -> String {
    // The wire spelling, not Rust's Debug. This field exists so a caller can
    // compare a level-2 counterparty against a manifest entry; `Other(PublicKey
    // { compressed: "02…" })` cannot be compared with anything.
    match c {
        Counterparty::Self_ => "self".to_string(),
        Counterparty::Anyone => "anyone".to_string(),
        Counterparty::Other(pk) => pk.to_string().to_lowercase(),
    }
}

fn level_num(l: SecurityLevel) -> u8 {
    match l {
        SecurityLevel::Silent => 0,
        SecurityLevel::App => 1,
        SecurityLevel::Counterparty => 2,
    }
}

/// BRC-100 gives HMAC and signature verification the same result shape.
fn valid_response(valid: bool) -> Json<serde_json::Value> {
    Json(json!({ "valid": valid }))
}

/// Extract originator from Origin or Originator header
fn extract_originator(headers: &HeaderMap) -> Result<String, AppError> {
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
    Err(AppError::new(Kind::Validation, "Origin header required"))
}

/// BRC-100's privileged keyring (#145): this wallet holds none, so a
/// request with `privileged: true` is refused by its kind, before any key
/// is derived or the call audited; never answered with the everyday key.
/// Absent or `false` is the call as before. `privilegedReason` is read and
/// only logged: it accompanies a privileged request and changes nothing
/// without one.
///
/// The nine request structs carry both as plain fields, not a flattened
/// struct: a flattened field loses serde's path, so a non-boolean
/// `privileged` would be a 422 without its `privileged:` prefix and a
/// JSON array body an "expected struct" error.
/// They lead the seven calls' structs (an array body's `[0]` is
/// `privileged`) and close the key-linkage calls' as before.
fn refuse_privileged(
    call: &str,
    privileged: Option<bool>,
    reason: Option<&str>,
) -> Result<(), AppError> {
    if privileged != Some(true) {
        return Ok(());
    }
    tracing::info!(call, reason, "privileged request refused");
    Err(AppError::new(
        Kind::PrivilegedNotSupported,
        format!(
            "{call}: privileged is not supported: this wallet holds no privileged keyring, \
             and a privileged request is never answered with the everyday key"
        ),
    ))
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
    refuse_privileged(
        "getPublicKey",
        req.privileged,
        req.privileged_reason.as_deref(),
    )?;

    let args = GetPublicKeyArgs {
        identity_key: req.identity_key,
        protocol_id: req.protocol_id.map(|v| parse_protocol_id(&v)).transpose()?,
        key_id: req.key_id,
        counterparty: req
            .counterparty
            .as_deref()
            .map(parse_counterparty)
            .transpose()?,
        for_self: req.for_self,
        privileged: req.privileged,
        privileged_reason: req.privileged_reason,
        seek_permission: None,
    };

    if let Some(p) = args.protocol_id.as_ref() {
        audit::record(audit::protocol_entry(
            "getPublicKey",
            level_num(p.security_level),
            &p.protocol_name,
            args.counterparty.as_ref().map(cp_str),
            args.key_id.clone(),
            &originator,
        ));
    }
    let result = wallet
        .get_public_key(args, Some(&originator))
        .await
        .map_err(AppError::from)?;

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
    refuse_privileged(
        "createSignature",
        req.privileged,
        req.privileged_reason.as_deref(),
    )?;

    // ts-sdk sends exactly one of `data` (hash-then-sign) or
    // `hashToDirectlySign` (sign the 32-byte digest as-is).
    let hash_to_directly_sign = req
        .hash_to_directly_sign
        .map(|h| {
            <[u8; 32]>::try_from(h.as_slice()).map_err(|_| {
                AppError::new(
                    Kind::Validation,
                    "Validation error: hashToDirectlySign must be 32 bytes",
                )
            })
        })
        .transpose()?;
    if req.data.is_none() && hash_to_directly_sign.is_none() {
        return Err(AppError::new(
            Kind::Validation,
            "Validation error: one of data or hashToDirectlySign is required",
        ));
    }

    let args = CreateSignatureArgs {
        data: req.data,
        hash_to_directly_sign,
        protocol_id: parse_protocol_id(&req.protocol_id)?,
        key_id: req.key_id,
        counterparty: Some(parse_counterparty(&req.counterparty)?),
        privileged: req.privileged,
        privileged_reason: req.privileged_reason,
        seek_permission: None,
    };

    audit::record(audit::protocol_entry(
        "createSignature",
        level_num(args.protocol_id.security_level),
        &args.protocol_id.protocol_name,
        args.counterparty.as_ref().map(cp_str),
        Some(args.key_id.clone()),
        &originator,
    ));
    let result = wallet
        .create_signature(args, Some(&originator))
        .await
        .map_err(AppError::from)?;

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
    axum::Extension(gate): axum::Extension<crate::spend_gate::GateOptions>,
    headers: HeaderMap,
    Json(req): Json<McCreateActionReq>,
) -> Result<Json<McCreateActionRes>, AppError> {
    let originator = extract_originator(&headers)?;
    // The request to the toolbox's arguments and the door's `allowPending`
    // word (`create_action_args`; the fuzz target `served_json` calls it).
    let (args, allow_pending) = create_action_args(req)?;

    // Whether this request performs an immediate broadcast (vs. no-send /
    // delayed / external-signing). Only immediate broadcasts are verified.
    let (opt_no_send, opt_accept_delayed) = args
        .options
        .as_ref()
        .map(|o| {
            (
                o.no_send.unwrap_or(false),
                o.accept_delayed_broadcast.unwrap_or(false),
            )
        })
        .unwrap_or((false, false));

    let output_count = args.outputs.as_ref().map(|o| o.len()).unwrap_or(0);
    let total_sats: u64 = args
        .outputs
        .as_ref()
        .map(|o| o.iter().map(|x| x.satoshis).sum())
        .unwrap_or(0);

    // A real wallet prompts here unless the app declared a BRC-73
    // `spendingAuthorization` budget — including for a FEE-ONLY action (an
    // OP_RETURN marker, total_sats == 0) which still costs a miner fee.
    audit::record(audit::spend_entry(
        "createAction",
        total_sats,
        Some(args.description.clone()),
        &originator,
    ));

    // Acquire spending lock — queues behind any in-flight createAction.
    // Once the previous tx completes (and its change UTXO exists), we proceed.
    let _guard = spending_lock.lock().await;
    // A coin whose transaction drew no accepting word on its immediate post,
    // with no status source holding it, is not selected (`spend_guard`).
    crate::spend_guard::run(wallet.storage())
        .await
        .map_err(|e| AppError::from_pre_door(e, "spend guard"))?;
    // The spend gate (R1 (b), (d), R3 (b)): the toolbox allocates only
    // coins whose creator is mined at SPEND_DEPTH, our own change under
    // SPEND_OWN_CHANGE, or a received pending coin under this action's
    // `allowPending: received`; a named input passes the same judgment; a
    // refusal lists every coin held back.
    let result = crate::spend_gate::create_action(
        &wallet,
        args,
        &originator,
        &gate.with_allow_pending(allow_pending),
    )
    .await
    .map_err(AppError::from_gate_error)?;
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

    // Post-broadcast follow-up (see `broadcast_follow_up`). From the toolbox
    // 0.7.4 a broadcaster's refusal on the immediate post is a hint, not an
    // error (bsv-stack-lean #66): `create_action` returns the txid with
    // `sending`, the refusal on the request's history, the inputs locked.
    //   * ACCEPTED by the broadcaster (`sendWithResults: unproven`): answer now;
    //     the presence probe runs in the background and records evidence; it
    //     never retires a transaction a broadcaster took.
    //   * No accepting word (`sending`: a refusal, a transient word, no
    //     broadcaster reached): verify inline. A definitive absence of a
    //     transaction no broadcaster took retires it (the host's act on
    //     `built`), 502 BROADCAST_REJECTED; otherwise the answer carries the
    //     word `built` and the toolbox's hint (`broadcast`).
    //   * Not broadcast (noSend / delayed / deferred signing): nothing.
    let disposition =
        super::broadcast_follow_up::disposition(&result, opt_no_send, opt_accept_delayed);
    if let Some(txid_bytes) = result.txid.as_ref() {
        let txid_hex = to_hex(txid_bytes);
        let probe = verifier.clone();
        let report_wallet = wallet.clone();
        // The unproven ancestors the package carried: network presence of
        // the new tx vouches for them too (the broadcast memory's `seen`).
        let ancestors: Vec<String> = result
            .beef
            .as_deref()
            .map(|beef| bsv_wallet_toolbox::unproven_ancestors_in_beef(beef, &txid_hex))
            .unwrap_or_default();
        let outcome = super::broadcast_follow_up::follow_up(
            disposition,
            txid_hex.clone(),
            move |txid| async move { probe.verify_report(&txid).await },
            move |txid, report| async move {
                super::broadcast_follow_up::apply_presence_report(
                    report_wallet.storage(),
                    report_wallet.services(),
                    &txid,
                    &ancestors,
                    &report,
                )
                .await;
            },
        )
        .await;
        if outcome == super::broadcast_follow_up::FollowUp::Rejected
            && super::broadcast_follow_up::tx_status(wallet.storage(), &txid_hex).await
                == Some("failed".to_string())
        {
            return Err(AppError::broadcast_rejected(&txid_hex));
        }
    }
    let broadcast = match result.txid.as_ref() {
        Some(t) if super::broadcast_follow_up::sending(&result.send_with_results, t) => {
            super::post_word::post_word(wallet.storage(), &to_hex(t)).await
        }
        _ => None,
    };

    Ok(Json(McCreateActionRes {
        txid: result.txid.map(|t| to_hex(&t)),
        tx: atomic_beef,
        send_with_results: result
            .send_with_results
            .and_then(|r| serde_json::to_value(r).ok()),
        signable_transaction: result.signable_transaction.map(|st| {
            // BRC-100's Base64String is the storage handle's twelve bytes.
            let reference = crate::spend_gate::reference_string(&st.reference);
            McSignableTransaction {
                tx: st.tx,
                reference,
            }
        }),
        no_send_change: result
            .no_send_change
            .and_then(|v| serde_json::to_value(v).ok()),
        broadcast,
    }))
}

/// POST /internalizeAction
pub async fn internalize_action(
    State(wallet): State<WalletState>,
    headers: HeaderMap,
    Json(req): Json<McInternalizeActionReq>,
) -> Result<Json<McInternalizeActionRes>, AppError> {
    let originator = extract_originator(&headers)?;
    validate_internalize_instructions(&req)?;
    let args = internalize_args(req);

    let output_count = args.outputs.len();
    let subject = super::post_word::atomic_subject(&args.tx);
    let result = wallet
        .internalize_action(args, Some(&originator))
        .await
        .map_err(AppError::from)?;

    tracing::info!(
        originator = %originator,
        outputs = output_count,
        accepted = result.accepted,
        "internalizeAction"
    );
    // From the toolbox 0.7.4 a refusal of the internalized transaction's
    // post is a hint and the answer is `accepted`; the word says so.
    let broadcast = match subject {
        Some(txid) => super::post_word::post_word(wallet.storage(), &txid).await,
        None => None,
    };

    Ok(Json(McInternalizeActionRes {
        accepted: result.accepted,
        broadcast,
    }))
}

/// Refuse oversized insertion instructions before any output can be stored.
fn validate_internalize_instructions(req: &McInternalizeActionReq) -> Result<(), AppError> {
    // #157: validate every output before the wallet can store any of them.
    // String::len counts UTF-8 bytes. The reference applies this bound only
    // at internalizeAction, not to createAction's output instructions.
    const CUSTOM_INSTRUCTIONS_MAX_BYTES: usize = 1000;
    for output in &req.outputs {
        if let Some(instructions) = output
            .insertion_remittance
            .as_ref()
            .and_then(|insertion| insertion.custom_instructions.as_deref())
        {
            let bytes = instructions.len();
            if bytes > CUSTOM_INSTRUCTIONS_MAX_BYTES {
                return Err(AppError::new(
                    Kind::Validation,
                    format!(
                        "Validation error: customInstructions: no more than 1000 length. \
                     (UTF-8 bytes: limit {CUSTOM_INSTRUCTIONS_MAX_BYTES}, got {bytes})"
                    ),
                ));
            }
        }
    }
    Ok(())
}

/// The toolbox's `internalizeAction` arguments for a caller's request, the
/// gate's rule label stripped from its labels.
#[must_use]
pub fn internalize_args(req: McInternalizeActionReq) -> InternalizeActionArgs {
    let mut args = InternalizeActionArgs {
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
    // The rule's label is the gate's to write, never a caller's (F4 of the
    // #44 CLI review): a forged one would read as underRule and grant a
    // respend the received rule.
    crate::spend_gate::strip_rule_label(&mut args.labels);
    args
}

/// The toolbox's `createAction` arguments for a caller's request, with the
/// door's `allowPending` word. The handler calls it (then the lock, the
/// guard, the gate); the fuzz target `served_json` calls it with no store.
///
/// # Errors
///
/// An `allowPending` word the gate does not read, a locking or unlocking
/// script that is not hex, or an input outpoint that does not parse.
pub fn create_action_args(
    req: McCreateActionReq,
) -> Result<(CreateActionArgs, crate::spend_gate::AllowPending), AppError> {
    // R1 (d): the action's word, read here at the door and nowhere else:
    // `"received"`, or the scoped `{"received": ["txid:vout", ...]}` (#63).
    let allow_pending = crate::spend_gate::AllowPending::from_json(req.allow_pending.as_ref())
        .map_err(bsv_wallet_toolbox::Error::InvalidArgument)?;

    let outputs = create_action_outputs(req.outputs)?;
    let inputs = create_action_inputs(req.inputs)?;

    let args = CreateActionArgs {
        description: req.description,
        input_beef: req.input_beef,
        inputs,
        outputs,
        lock_time: req.lock_time,
        version: None,
        labels: req.labels,
        options: req.options.as_ref().map(create_action_options),
    };
    Ok((args, allow_pending))
}

/// A request's outputs, each locking script decoded from hex.
fn create_action_outputs(
    outputs: Option<Vec<McCreateActionOutput>>,
) -> Result<Option<Vec<CreateActionOutput>>> {
    outputs
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
        .transpose()
}

/// Map caller-provided explicit inputs (e.g. a covenant spend carrying a full
/// unlockingScript). An absent/empty list stays `Some(vec![])` so the wallet
/// auto-selects its own funding UTXOs: the prior behaviour for plain sends.
/// Before this, the handler discarded `inputs`/`inputBEEF`/`lockTime`, so any
/// covenant input was silently dropped and the tx was funded from change.
fn create_action_inputs(
    inputs: Option<Vec<McCreateActionInput>>,
) -> Result<Option<Vec<CreateActionInput>>, AppError> {
    let Some(reqs) = inputs.filter(|reqs| !reqs.is_empty()) else {
        return Ok(Some(vec![]));
    };
    let mut mapped = Vec::with_capacity(reqs.len());
    for i in reqs {
        let outpoint = Outpoint::from_string(&i.outpoint)
            .map_err(|e| anyhow::anyhow!("invalid input outpoint '{}': {e}", i.outpoint))?;
        let unlocking_script = match i.unlocking_script {
            // [SRC] ts-stack `edf6e03`, `packages/sdk/src/wallet/validationHelpers.ts:522-528`.
            Some(h) => {
                // Include JavaScript's U+FEFF trim and count UTF-16 code units.
                // Rust also trims U+0085 via is_whitespace; JavaScript's trim does not.
                let h = h.trim_matches(|c: char| c.is_whitespace() || c == '\u{feff}');
                let length = h.encode_utf16().count();
                if length % 2 != 0 {
                    return Err(AppError::new(
                        Kind::Validation,
                        format!("The unlockingScript parameter must be even length, not {length}."),
                    ));
                }
                Some(hex::decode(h).map_err(|_| {
                    AppError::new(
                        Kind::Validation,
                        "The unlockingScript parameter must be hexadecimal string.",
                    )
                })?)
            }
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
    Ok(Some(mapped))
}

/// A request's options; `trustSelf` is read only as `"known"`.
fn create_action_options(o: &McCreateActionOptions) -> CreateActionOptions {
    CreateActionOptions {
        accept_delayed_broadcast: o.accept_delayed_broadcast,
        randomize_outputs: o.randomize_outputs,
        sign_and_process: o.sign_and_process,
        no_send: o.no_send,
        trust_self: match o.trust_self.as_deref() {
            Some("known") => Some(TrustSelf::Known),
            _ => None,
        },
        ..Default::default()
    }
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
        hex => Ok(Counterparty::Other(strict_public_key(hex)?)),
    }
}

/// A caller's key argument read strictly: even-length hex, then
/// `PublicKey::from_bytes`, as `PublicKey::from_hex` read it through bsv-rs
/// 0.9.0-beta.4 and in its words (#377). From beta.5 `from_hex` reads the
/// reference's lenient `fromString` forms (bsv-rs #498: odd-length, hybrid,
/// a coordinate at or above p), which the reference's wallet refuses at
/// these arguments (`validatePublicKey`, ts-stack@edf6e03
/// `sdk/src/wallet/validationHelpers.ts:185-196`); bsv-rs keeps its own
/// strict reading crate-private.
fn strict_public_key(hex: &str) -> bsv_sdk::Result<PublicKey> {
    PublicKey::from_bytes(&bsv_sdk::primitives::from_hex(hex)?)
}

// =============================================================================
// Batch 1: Status (GET endpoints)
// =============================================================================

/// GET /getHeight
pub async fn get_height(
    State(wallet): State<WalletState>,
) -> Result<Json<GetHeightResult>, AppError> {
    let result = wallet
        .get_height(Some("localhost"))
        .await
        .map_err(AppError::from)?;
    Ok(Json(result))
}

/// GET /getNetwork
pub async fn get_network(
    State(wallet): State<WalletState>,
) -> Result<Json<GetNetworkResult>, AppError> {
    let result = wallet
        .get_network(Some("localhost"))
        .await
        .map_err(AppError::from)?;
    Ok(Json(result))
}

/// GET /getVersion
pub async fn get_version(
    State(wallet): State<WalletState>,
) -> Result<Json<GetVersionResult>, AppError> {
    let result = wallet
        .get_version(Some("localhost"))
        .await
        .map_err(AppError::from)?;
    Ok(Json(result))
}

/// GET /waitForAuthentication
pub async fn wait_for_authentication(
    State(wallet): State<WalletState>,
) -> Result<Json<AuthenticatedResult>, AppError> {
    let result = wallet
        .wait_for_authentication(Some("localhost"))
        .await
        .map_err(AppError::from)?;
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
        .get_header_for_height(args, Some(&originator))
        .await
        .map_err(AppError::from)?;
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
    refuse_privileged(
        "verifySignature",
        req.privileged,
        req.privileged_reason.as_deref(),
    )?;
    let args = VerifySignatureArgs {
        data: req.data,
        hash_to_directly_verify: None,
        signature: req.signature,
        protocol_id: parse_protocol_id(&req.protocol_id)?,
        key_id: req.key_id,
        counterparty: req
            .counterparty
            .as_deref()
            .map(parse_counterparty)
            .transpose()?,
        for_self: req.for_self,
        privileged: req.privileged,
        privileged_reason: req.privileged_reason,
        seek_permission: None,
    };
    let result = wallet
        .verify_signature(args, Some(&originator))
        .await
        .map_err(AppError::from)?;
    Ok(valid_response(result.valid))
}

/// POST /encrypt
pub async fn encrypt(
    State(wallet): State<WalletState>,
    headers: HeaderMap,
    Json(req): Json<McEncryptReq>,
) -> Result<Json<serde_json::Value>, AppError> {
    let originator = extract_originator(&headers)?;
    refuse_privileged("encrypt", req.privileged, req.privileged_reason.as_deref())?;
    let args = EncryptArgs {
        plaintext: req.plaintext,
        protocol_id: parse_protocol_id(&req.protocol_id)?,
        key_id: req.key_id,
        counterparty: req
            .counterparty
            .as_deref()
            .map(parse_counterparty)
            .transpose()?,
        privileged: req.privileged,
        privileged_reason: req.privileged_reason,
        seek_permission: None,
    };
    audit::record(audit::protocol_entry(
        "encrypt",
        level_num(args.protocol_id.security_level),
        &args.protocol_id.protocol_name,
        args.counterparty.as_ref().map(cp_str),
        Some(args.key_id.clone()),
        &originator,
    ));
    let result = wallet
        .encrypt(args, Some(&originator))
        .await
        .map_err(AppError::from)?;
    Ok(Json(json!({ "ciphertext": result.ciphertext })))
}

/// POST /decrypt
pub async fn decrypt(
    State(wallet): State<WalletState>,
    headers: HeaderMap,
    Json(req): Json<McDecryptReq>,
) -> Result<Json<serde_json::Value>, AppError> {
    let originator = extract_originator(&headers)?;
    refuse_privileged("decrypt", req.privileged, req.privileged_reason.as_deref())?;
    let args = DecryptArgs {
        ciphertext: req.ciphertext,
        protocol_id: parse_protocol_id(&req.protocol_id)?,
        key_id: req.key_id,
        counterparty: req
            .counterparty
            .as_deref()
            .map(parse_counterparty)
            .transpose()?,
        privileged: req.privileged,
        privileged_reason: req.privileged_reason,
        seek_permission: None,
    };
    audit::record(audit::protocol_entry(
        "decrypt",
        level_num(args.protocol_id.security_level),
        &args.protocol_id.protocol_name,
        args.counterparty.as_ref().map(cp_str),
        Some(args.key_id.clone()),
        &originator,
    ));
    let result = wallet
        .decrypt(args, Some(&originator))
        .await
        .map_err(AppError::from)?;
    Ok(Json(json!({ "plaintext": result.plaintext })))
}

/// POST /createHmac
pub async fn create_hmac(
    State(wallet): State<WalletState>,
    headers: HeaderMap,
    Json(req): Json<McCreateHmacReq>,
) -> Result<Json<serde_json::Value>, AppError> {
    let originator = extract_originator(&headers)?;
    refuse_privileged(
        "createHmac",
        req.privileged,
        req.privileged_reason.as_deref(),
    )?;
    let args = CreateHmacArgs {
        data: req.data,
        protocol_id: parse_protocol_id(&req.protocol_id)?,
        key_id: req.key_id,
        counterparty: req
            .counterparty
            .as_deref()
            .map(parse_counterparty)
            .transpose()?,
        privileged: req.privileged,
        privileged_reason: req.privileged_reason,
        seek_permission: None,
    };
    audit::record(audit::protocol_entry(
        "createHmac",
        level_num(args.protocol_id.security_level),
        &args.protocol_id.protocol_name,
        args.counterparty.as_ref().map(cp_str),
        Some(args.key_id.clone()),
        &originator,
    ));
    let result = wallet
        .create_hmac(args, Some(&originator))
        .await
        .map_err(AppError::from)?;
    Ok(Json(json!({ "hmac": result.hmac.to_vec() })))
}

/// POST /verifyHmac
pub async fn verify_hmac(
    State(wallet): State<WalletState>,
    headers: HeaderMap,
    Json(req): Json<McVerifyHmacReq>,
) -> Result<Json<serde_json::Value>, AppError> {
    let originator = extract_originator(&headers)?;
    refuse_privileged(
        "verifyHmac",
        req.privileged,
        req.privileged_reason.as_deref(),
    )?;
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
            .as_deref()
            .map(parse_counterparty)
            .transpose()?,
        privileged: req.privileged,
        privileged_reason: req.privileged_reason,
        seek_permission: None,
    };
    let result = wallet
        .verify_hmac(args, Some(&originator))
        .await
        .map_err(AppError::from)?;
    Ok(valid_response(result.valid))
}

// =============================================================================
// Batch 4: Transaction Workflow
// =============================================================================

/// POST /signAction
pub async fn sign_action(
    State(wallet): State<WalletState>,
    headers: HeaderMap,
    Json(args): Json<SignActionArgs>,
) -> Result<Json<McSignActionRes>, AppError> {
    let originator = extract_originator(&headers)?;
    let reference = args.reference.clone();
    let result = wallet
        .sign_action(args, Some(&originator))
        .await
        .map_err(AppError::from)?;
    tracing::info!(originator = %originator, reference = %reference, "signAction");
    let broadcast = match result.txid.as_ref() {
        Some(t) if super::broadcast_follow_up::sending(&result.send_with_results, t) => {
            super::post_word::post_word(wallet.storage(), &to_hex(t)).await
        }
        _ => None,
    };
    // Same wire shape as create_action: txid as hex, tx (AtomicBEEF) as a
    // number array — see McSignActionRes for why the toolbox struct cannot be
    // serialized directly.
    Ok(Json(McSignActionRes {
        txid: result.txid.map(|t| to_hex(&t)),
        tx: result.tx,
        send_with_results: result
            .send_with_results
            .and_then(|r| serde_json::to_value(r).ok()),
        broadcast,
    }))
}

/// POST /abortAction
///
/// Refuses a transaction a broadcaster took, including an `unfail` request
/// or any request holding a proof, whatever its status says.
pub async fn abort_action(
    State(wallet): State<WalletState>,
    headers: HeaderMap,
    Json(args): Json<AbortActionArgs>,
) -> Result<Json<AbortActionResult>, AppError> {
    let originator = extract_originator(&headers)?;
    // A transaction a broadcaster took is never aborted here, whatever the
    // caller knows: it stays until a proof, a node verdict or a competitor's
    // checked proof (bsv-stack-lean #66; #24). One no broadcaster took is
    // the toolbox's to abort.
    if let Some(txid) =
        crate::retire_guard::abort_refused_for(wallet.storage().pool(), &args.reference)
            .await
            .map_err(abort_pre_check_fault)?
    {
        return Err(AppError::new(
            Kind::Conflict,
            format!(
                "Invalid operation: a broadcaster took transaction {txid}; it is not aborted. It stays, \
             re-asked, until a proof, a node verdict or a competitor's checked proof decides it"
            ),
        ));
    }
    let result = wallet
        .abort_action(args, Some(&originator))
        .await
        .map_err(AppError::from)?;
    Ok(Json(result))
}

/// The abort's pre-check fault: the store busy by its type
/// ([`crate::store::is_store_busy`]) is 503 `STORE_BUSY` with
/// `Retry-After: 1`, as at every door (#168); any other keeps its word.
fn abort_pre_check_fault(e: anyhow::Error) -> AppError {
    AppError::store_busy(&e).unwrap_or_else(|| AppError::from(e))
}

/// POST /listActions
pub async fn list_actions(
    State(wallet): State<WalletState>,
    headers: HeaderMap,
    Json(args): Json<ListActionsArgs>,
) -> Result<Json<serde_json::Value>, AppError> {
    let originator = extract_originator(&headers)?;
    check_page_bounds(args.limit)?;
    let result = wallet
        .list_actions(args, Some(&originator))
        .await
        .map_err(AppError::from)?;
    let user = crate::spend_gate::UserCoins::from_wallet(&wallet)
        .await
        .map_err(|e| AppError::new(Kind::Internal, format!("Storage error: {e}")))?;
    let rules = crate::spend_gate::action_rules(wallet.storage(), user.user_id())
        .await
        .map_err(|e| AppError::new(Kind::Internal, format!("Storage error: {e}")))?;
    let result = serde_json::to_value(result)
        .map_err(|e| AppError::new(Kind::Internal, format!("Internal error: {e}")))?;
    Ok(Json(with_rules(result, &rules)))
}

/// Each action created under a rule the action named (R1 (d)) carries it
/// as `underRule`, by its txid or, a template not yet signed, by its
/// reference; the toolbox's shape is otherwise as it gave it.
pub fn with_rules(
    mut actions: serde_json::Value,
    rules: &crate::spend_gate::ActionRules,
) -> serde_json::Value {
    if let Some(list) = actions.get_mut("actions").and_then(|a| a.as_array_mut()) {
        for action in list {
            let txid = action
                .get("txid")
                .and_then(|t| t.as_str())
                .map(str::to_ascii_lowercase);
            let rule = match txid {
                Some(t) if t.bytes().all(|b| b == b'0') => {
                    let key = crate::spend_gate::template_key(
                        action.get("status").and_then(|v| v.as_str()).unwrap_or(""),
                        action
                            .get("description")
                            .and_then(|v| v.as_str())
                            .unwrap_or(""),
                        action.get("satoshis").and_then(|v| v.as_i64()).unwrap_or(0),
                    );
                    rules.by_template.get(&key)
                }
                Some(t) => rules.by_txid.get(&t),
                None => None,
            }
            .cloned();
            if let (Some(rule), Some(obj)) = (rule, action.as_object_mut()) {
                obj.insert("underRule".into(), json!(rule));
            }
        }
    }
    actions
}

/// POST /listOutputs
pub async fn list_outputs(
    State(wallet): State<WalletState>,
    headers: HeaderMap,
    Json(args): Json<ListOutputsArgs>,
) -> Result<Json<ListOutputsResult>, AppError> {
    let originator = extract_originator(&headers)?;
    check_page_bounds(args.limit)?;
    let result = wallet
        .list_outputs(args, Some(&originator))
        .await
        .map_err(AppError::from)?;
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
        .relinquish_output(args, Some(&originator))
        .await
        .map_err(AppError::from)?;
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
        .acquire_certificate(args, Some(&originator))
        .await
        .map_err(AppError::from)?;
    Ok(Json(result))
}

/// POST /listCertificates
pub async fn list_certificates(
    State(wallet): State<WalletState>,
    headers: HeaderMap,
    Json(args): Json<ListCertificatesArgs>,
) -> Result<Json<ListCertificatesResult>, AppError> {
    let originator = extract_originator(&headers)?;
    check_page_bounds(args.limit)?;
    let result = wallet
        .list_certificates(args, Some(&originator))
        .await
        .map_err(AppError::from)?;
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
        .prove_certificate(args, Some(&originator))
        .await
        .map_err(AppError::from)?;
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
        .relinquish_certificate(args, Some(&originator))
        .await
        .map_err(AppError::from)?;
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
    check_page_bounds(args.limit)?;
    let result = wallet
        .discover_by_identity_key(args, Some(&originator))
        .await
        .map_err(AppError::from)?;
    Ok(Json(result))
}

/// POST /discoverByAttributes
pub async fn discover_by_attributes(
    State(wallet): State<WalletState>,
    headers: HeaderMap,
    Json(args): Json<DiscoverByAttributesArgs>,
) -> Result<Json<DiscoverCertificatesResult>, AppError> {
    let originator = extract_originator(&headers)?;
    check_page_bounds(args.limit)?;
    let result = wallet
        .discover_by_attributes(args, Some(&originator))
        .await
        .map_err(AppError::from)?;
    Ok(Json(result))
}

/// POST /revealCounterpartyKeyLinkage
pub async fn reveal_counterparty_key_linkage(
    State(wallet): State<WalletState>,
    headers: HeaderMap,
    Json(req): Json<McRevealCounterpartyKeyLinkageReq>,
) -> Result<Json<RevealCounterpartyKeyLinkageResult>, AppError> {
    let originator = extract_originator(&headers)?;
    refuse_privileged(
        "revealCounterpartyKeyLinkage",
        req.privileged,
        req.privileged_reason.as_deref(),
    )?;
    let args = WalletRevealCounterpartyArgs {
        counterparty: strict_public_key(&req.counterparty).map_err(|e| anyhow::anyhow!("{}", e))?,
        verifier: strict_public_key(&req.verifier).map_err(|e| anyhow::anyhow!("{}", e))?,
        privileged: req.privileged,
        privileged_reason: req.privileged_reason,
    };
    let result = wallet
        .reveal_counterparty_key_linkage(args, Some(&originator))
        .await
        .map_err(AppError::from)?;
    Ok(Json(result))
}

/// POST /revealSpecificKeyLinkage
pub async fn reveal_specific_key_linkage(
    State(wallet): State<WalletState>,
    headers: HeaderMap,
    Json(req): Json<McRevealSpecificKeyLinkageReq>,
) -> Result<Json<RevealSpecificKeyLinkageResult>, AppError> {
    let originator = extract_originator(&headers)?;
    refuse_privileged(
        "revealSpecificKeyLinkage",
        req.privileged,
        req.privileged_reason.as_deref(),
    )?;
    let args = WalletRevealSpecificArgs {
        counterparty: parse_counterparty(&req.counterparty)?,
        verifier: strict_public_key(&req.verifier).map_err(|e| anyhow::anyhow!("{}", e))?,
        protocol_id: parse_protocol_id(&req.protocol_id)?,
        key_id: req.key_id,
        privileged: req.privileged,
        privileged_reason: req.privileged_reason,
    };
    let result = wallet
        .reveal_specific_key_linkage(args, Some(&originator))
        .await
        .map_err(AppError::from)?;
    Ok(Json(result))
}

#[cfg(test)]
mod tests;
