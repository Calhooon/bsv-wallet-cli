use serde::{Deserialize, Serialize};

/// MetaNet Client request for getPublicKey
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct McGetPublicKeyReq {
    #[serde(default)]
    pub identity_key: bool,
    #[serde(default, alias = "protocolID")]
    pub protocol_id: Option<serde_json::Value>,
    #[serde(default, alias = "keyID")]
    pub key_id: Option<String>,
    #[serde(default)]
    pub counterparty: Option<String>,
    #[serde(default)]
    pub for_self: Option<bool>,
}

/// MetaNet Client response for getPublicKey
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct McGetPublicKeyRes {
    pub public_key: String,
}

/// MetaNet Client request for createSignature
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct McCreateSignatureReq {
    /// Raw data to hash-then-sign. Mutually exclusive with
    /// `hash_to_directly_sign` (ts-sdk sends exactly one of the two).
    #[serde(default)]
    pub data: Option<Vec<u8>>,
    /// A 32-byte digest to sign as-is (ts-sdk `hashToDirectlySign`).
    #[serde(default)]
    pub hash_to_directly_sign: Option<Vec<u8>>,
    #[serde(alias = "protocolID")]
    pub protocol_id: serde_json::Value,
    #[serde(alias = "keyID")]
    pub key_id: String,
    pub counterparty: String,
}

/// MetaNet Client response for createSignature
#[derive(Serialize)]
pub struct McCreateSignatureRes {
    pub signature: Vec<u8>,
}

/// MetaNet Client request for createAction
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct McCreateActionReq {
    pub description: String,
    #[serde(default)]
    pub outputs: Option<Vec<McCreateActionOutput>>,
    #[serde(default)]
    pub options: Option<McCreateActionOptions>,
    #[serde(default)]
    pub labels: Option<Vec<String>>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct McCreateActionOutput {
    pub locking_script: String,
    pub satoshis: u64,
    pub output_description: String,
    #[serde(default)]
    pub basket: Option<String>,
    #[serde(default)]
    pub custom_instructions: Option<String>,
    #[serde(default)]
    pub tags: Option<Vec<String>>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct McCreateActionOptions {
    #[serde(default)]
    pub accept_delayed_broadcast: Option<bool>,
    #[serde(default)]
    pub randomize_outputs: Option<bool>,
    #[serde(default)]
    pub sign_and_process: Option<bool>,
    #[serde(default)]
    pub no_send: Option<bool>,
}

/// MetaNet Client response for createAction
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct McCreateActionRes {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub txid: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tx: Option<Vec<u8>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub send_with_results: Option<serde_json::Value>,
    /// For signAndProcess: false — contains the unsigned tx and reference for signAction/abortAction.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub signable_transaction: Option<McSignableTransaction>,
    /// Change outpoints for noSend transactions.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub no_send_change: Option<serde_json::Value>,
}

/// Unsigned transaction + reference for deferred signing flow.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct McSignableTransaction {
    /// The unsigned transaction bytes (number array).
    pub tx: Vec<u8>,
    /// The reference string for signAction/abortAction.
    /// NOTE: The SDK stores this as Vec<u8> (from String.into_bytes()) then hex-encodes it.
    /// We reverse that: convert bytes back to the original String so signAction/abortAction
    /// can look it up correctly in the wallet's pending transaction cache.
    pub reference: String,
}

/// MetaNet Client request for internalizeAction
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct McInternalizeActionReq {
    pub tx: Vec<u8>,
    pub outputs: Vec<McInternalizeOutput>,
    pub description: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct McInternalizeOutput {
    pub output_index: u32,
    pub protocol: String,
    #[serde(default)]
    pub payment_remittance: Option<McWalletPayment>,
    #[serde(default)]
    pub insertion_remittance: Option<McBasketInsertion>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct McBasketInsertion {
    pub basket: String,
    #[serde(default)]
    pub custom_instructions: Option<String>,
    #[serde(default)]
    pub tags: Option<Vec<String>>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct McWalletPayment {
    pub derivation_prefix: String,
    pub derivation_suffix: String,
    pub sender_identity_key: String,
}

/// MetaNet Client response for internalizeAction
#[derive(Serialize)]
pub struct McInternalizeActionRes {
    pub accepted: bool,
}

// =============================================================================
// Batch 3: Crypto request types (SDK args lack serde)
// =============================================================================

/// MetaNet Client request for verifySignature
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct McVerifySignatureReq {
    #[serde(default)]
    pub data: Option<Vec<u8>>,
    pub signature: Vec<u8>,
    #[serde(alias = "protocolID")]
    pub protocol_id: serde_json::Value,
    #[serde(alias = "keyID")]
    pub key_id: String,
    #[serde(default)]
    pub counterparty: Option<String>,
    #[serde(default)]
    pub for_self: Option<bool>,
}

/// MetaNet Client request for encrypt
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct McEncryptReq {
    pub plaintext: Vec<u8>,
    #[serde(alias = "protocolID")]
    pub protocol_id: serde_json::Value,
    #[serde(alias = "keyID")]
    pub key_id: String,
    #[serde(default)]
    pub counterparty: Option<String>,
}

/// MetaNet Client request for decrypt
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct McDecryptReq {
    pub ciphertext: Vec<u8>,
    #[serde(alias = "protocolID")]
    pub protocol_id: serde_json::Value,
    #[serde(alias = "keyID")]
    pub key_id: String,
    #[serde(default)]
    pub counterparty: Option<String>,
}

/// MetaNet Client request for createHmac
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct McCreateHmacReq {
    pub data: Vec<u8>,
    #[serde(alias = "protocolID")]
    pub protocol_id: serde_json::Value,
    #[serde(alias = "keyID")]
    pub key_id: String,
    #[serde(default)]
    pub counterparty: Option<String>,
}

/// MetaNet Client request for verifyHmac
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct McVerifyHmacReq {
    pub data: Vec<u8>,
    pub hmac: Vec<u8>,
    #[serde(alias = "protocolID")]
    pub protocol_id: serde_json::Value,
    #[serde(alias = "keyID")]
    pub key_id: String,
    #[serde(default)]
    pub counterparty: Option<String>,
}

// =============================================================================
// Batch 6: Key linkage request types (SDK args lack serde)
// =============================================================================

/// MetaNet Client request for revealCounterpartyKeyLinkage
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct McRevealCounterpartyKeyLinkageReq {
    pub counterparty: String,
    pub verifier: String,
    #[serde(default)]
    pub privileged: Option<bool>,
    #[serde(default)]
    pub privileged_reason: Option<String>,
}

/// MetaNet Client request for revealSpecificKeyLinkage
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct McRevealSpecificKeyLinkageReq {
    pub counterparty: String,
    pub verifier: String,
    #[serde(alias = "protocolID")]
    pub protocol_id: serde_json::Value,
    #[serde(alias = "keyID")]
    pub key_id: String,
    #[serde(default)]
    pub privileged: Option<bool>,
    #[serde(default)]
    pub privileged_reason: Option<String>,
}
