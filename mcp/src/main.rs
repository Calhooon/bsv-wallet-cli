//! MCP server wrapping all 28 BRC-100 wallet HTTP endpoints.
//!
//! Talks to bsv-wallet-cli at WALLET_URL (default http://localhost:3322).
//! Every request includes the Origin header required by the wallet.

use reqwest::Client;
use rmcp::{
    handler::server::tool::ToolRouter, handler::server::wrapper::Parameters, model::*, tool,
    tool_handler, tool_router, transport::stdio, ErrorData, ServerHandler, ServiceExt,
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::env;

// ── Server struct ──────────────────────────────────────────────

#[derive(Clone)]
pub struct WalletMcp {
    client: Client,
    url: String,
    origin: String,
    tool_router: ToolRouter<Self>,
}

impl WalletMcp {
    fn err(msg: String) -> ErrorData {
        ErrorData::internal_error(msg, None)
    }

    async fn get(&self, path: &str) -> Result<CallToolResult, ErrorData> {
        let url = format!("{}/{path}", self.url);
        let resp = self
            .client
            .get(&url)
            .header("Origin", &self.origin)
            .send()
            .await
            .map_err(|e| Self::err(format!("HTTP error: {e}")))?;
        let body: Value = resp
            .json()
            .await
            .map_err(|e| Self::err(format!("JSON parse error: {e}")))?;
        Ok(CallToolResult::success(vec![Content::text(
            serde_json::to_string_pretty(&body).unwrap_or_default(),
        )]))
    }

    async fn post(&self, path: &str, body: Value) -> Result<CallToolResult, ErrorData> {
        let url = format!("{}/{path}", self.url);
        let resp = self
            .client
            .post(&url)
            .header("Origin", &self.origin)
            .header("Content-Type", "application/json")
            .json(&body)
            .send()
            .await
            .map_err(|e| Self::err(format!("HTTP error: {e}")))?;
        let status = resp.status();
        let body: Value = resp
            .json()
            .await
            .map_err(|e| Self::err(format!("JSON parse error: {e}")))?;
        if !status.is_success() {
            let msg = body
                .get("message")
                .or(body.get("error"))
                .and_then(|v| v.as_str())
                .unwrap_or("unknown error");
            return Err(Self::err(format!("Wallet HTTP {}: {msg}", status.as_u16())));
        }
        Ok(CallToolResult::success(vec![Content::text(
            serde_json::to_string_pretty(&body).unwrap_or_default(),
        )]))
    }
}

// ── Typed parameter structs ────────────────────────────────────

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
pub struct GetPublicKeyParams {
    /// If true, returns the wallet's root identity key (ignores other fields)
    #[serde(rename = "identityKey", skip_serializing_if = "Option::is_none")]
    pub identity_key: Option<bool>,
    /// Protocol ID: [securityLevel, "protocolName"], e.g. [2, "my-protocol"]
    #[serde(rename = "protocolID", skip_serializing_if = "Option::is_none")]
    pub protocol_id: Option<Value>,
    /// Key ID for derivation (default: "1")
    #[serde(rename = "keyID", skip_serializing_if = "Option::is_none")]
    pub key_id: Option<String>,
    /// Counterparty: 33-byte hex pubkey, "self", or "anyone"
    #[serde(skip_serializing_if = "Option::is_none")]
    pub counterparty: Option<String>,
    /// If true, derive key for self
    #[serde(rename = "forSelf", skip_serializing_if = "Option::is_none")]
    pub for_self: Option<bool>,
}

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
pub struct CreateSignatureParams {
    /// Data to sign as byte array, e.g. [72, 101, 108, 108, 111]
    pub data: Vec<u8>,
    /// Protocol ID: [securityLevel, "protocolName"]
    #[serde(rename = "protocolID")]
    pub protocol_id: Value,
    /// Key ID for the signing key
    #[serde(rename = "keyID")]
    pub key_id: String,
    /// Counterparty: hex pubkey, "self", or "anyone"
    pub counterparty: String,
}

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
pub struct CreateActionParams {
    /// Human-readable description of the action
    pub description: String,
    /// Transaction outputs: [{lockingScript: "hex", satoshis: number, outputDescription: "desc"}]
    #[serde(default)]
    pub outputs: Vec<Value>,
    /// Labels to tag this action with
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub labels: Vec<String>,
    /// Options: {signAndProcess: bool, noSend: bool, acceptDelayedBroadcast: bool, randomizeOutputs: bool}
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub options: Option<Value>,
}

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
pub struct ListActionsParams {
    /// Labels to filter by (empty = all)
    #[serde(default)]
    pub labels: Vec<String>,
    /// "any" or "all"
    #[serde(rename = "labelQueryMode", default = "default_any")]
    pub label_query_mode: String,
    /// Include label data in response
    #[serde(rename = "includeLabels", default)]
    pub include_labels: bool,
    /// Include input data in response
    #[serde(rename = "includeInputs", default)]
    pub include_inputs: bool,
    /// Include output data in response
    #[serde(rename = "includeOutputs", default)]
    pub include_outputs: bool,
    /// Max actions to return (default 10)
    #[serde(default = "default_10")]
    pub limit: u64,
    /// Pagination offset
    #[serde(default)]
    pub offset: u64,
}

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
pub struct ListOutputsParams {
    /// Basket name (default: "default")
    #[serde(default = "default_basket")]
    pub basket: String,
    /// Include mode: "locking scripts", "entire transactions"
    #[serde(default = "default_include")]
    pub include: String,
    /// Max outputs to return
    #[serde(default = "default_100")]
    pub limit: u64,
    /// Pagination offset
    #[serde(default)]
    pub offset: u64,
}

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
pub struct InternalizeActionParams {
    /// Transaction bytes (AtomicBEEF format) as byte array
    pub tx: Vec<u8>,
    /// Outputs: [{outputIndex, protocol, paymentRemittance: {derivationPrefix, derivationSuffix, senderIdentityKey}}]
    pub outputs: Vec<Value>,
    /// Description of the incoming payment
    pub description: String,
}

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
pub struct ReferenceParams {
    /// Reference string from a previous createAction (signAndProcess=false)
    pub reference: String,
}

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
pub struct VerifySignatureParams {
    /// Data that was signed, as byte array e.g. [72, 101, 108, 108, 111]
    pub data: Vec<u8>,
    /// Signature bytes to verify
    pub signature: Vec<u8>,
    /// Protocol ID: [securityLevel, "protocolName"], e.g. [2, "my-protocol"]
    #[serde(rename = "protocolID")]
    pub protocol_id: Value,
    /// Key ID for the signing key
    #[serde(rename = "keyID")]
    pub key_id: String,
    /// Counterparty: hex pubkey, "self", or "anyone"
    pub counterparty: String,
}

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
pub struct EncryptParams {
    /// Data to encrypt as byte array, e.g. [72, 101, 108, 108, 111]
    pub plaintext: Vec<u8>,
    /// Protocol ID: [securityLevel, "protocolName"]
    #[serde(rename = "protocolID")]
    pub protocol_id: Value,
    /// Key ID for the encryption key
    #[serde(rename = "keyID")]
    pub key_id: String,
    /// Counterparty: hex pubkey, "self", or "anyone"
    pub counterparty: String,
}

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
pub struct DecryptParams {
    /// Encrypted data as byte array
    pub ciphertext: Vec<u8>,
    /// Protocol ID: [securityLevel, "protocolName"]
    #[serde(rename = "protocolID")]
    pub protocol_id: Value,
    /// Key ID for the decryption key
    #[serde(rename = "keyID")]
    pub key_id: String,
    /// Counterparty: hex pubkey, "self", or "anyone"
    pub counterparty: String,
}

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
pub struct CreateHmacParams {
    /// Data to HMAC as byte array
    pub data: Vec<u8>,
    /// Protocol ID: [securityLevel, "protocolName"]
    #[serde(rename = "protocolID")]
    pub protocol_id: Value,
    /// Key ID for the HMAC key
    #[serde(rename = "keyID")]
    pub key_id: String,
    /// Counterparty: hex pubkey, "self", or "anyone"
    pub counterparty: String,
}

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
pub struct VerifyHmacParams {
    /// Data that was HMACed as byte array
    pub data: Vec<u8>,
    /// HMAC value to verify (32 bytes)
    pub hmac: Vec<u8>,
    /// Protocol ID: [securityLevel, "protocolName"]
    #[serde(rename = "protocolID")]
    pub protocol_id: Value,
    /// Key ID for the HMAC key
    #[serde(rename = "keyID")]
    pub key_id: String,
    /// Counterparty: hex pubkey, "self", or "anyone"
    pub counterparty: String,
}

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
pub struct GetHeaderForHeightParams {
    /// Block height to get the header for
    pub height: u64,
}

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
pub struct AcquireCertificateParams {
    /// Certificate type identifier (e.g. "agent-authorization")
    #[serde(rename = "type")]
    pub certificate_type: String,
    /// Subject's identity key (66-char hex compressed pubkey)
    pub subject: String,
    /// Certifier's identity key (66-char hex compressed pubkey)
    pub certifier: String,
    /// Unique serial number for this certificate
    #[serde(rename = "serialNumber")]
    pub serial_number: String,
    /// Revocation outpoint (txid + vout as 72-char hex), or zeros if none
    #[serde(rename = "revocationOutpoint")]
    pub revocation_outpoint: String,
    /// Certifier's signature over the certificate
    pub signature: String,
    /// Certificate fields as key-value pairs (e.g. {"name": "MyAgent", "capabilities": "tools,wallet"})
    pub fields: Value,
    /// Acquisition protocol: "direct" or "issuance"
    #[serde(rename = "acquisitionProtocol", default = "default_direct")]
    pub acquisition_protocol: String,
}

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
pub struct ListCertificatesParams {
    /// Filter by certifier identity keys (66-char hex). Empty array = all certifiers.
    #[serde(default)]
    pub certifiers: Vec<String>,
    /// Filter by certificate type IDs. Empty array = all types.
    #[serde(default)]
    pub types: Vec<String>,
    /// Max certificates to return (default 10)
    #[serde(default = "default_10")]
    pub limit: u64,
    /// Pagination offset
    #[serde(default)]
    pub offset: u64,
}

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
pub struct ProveCertificateParams {
    /// The certificate object to prove (from listCertificates)
    pub certificate: Value,
    /// Field names to selectively reveal, e.g. ["name", "capabilities"]
    #[serde(rename = "fieldsToReveal")]
    pub fields_to_reveal: Vec<String>,
}

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
pub struct RelinquishCertificateParams {
    /// Certificate type identifier
    #[serde(rename = "type")]
    pub certificate_type: String,
    /// Serial number of the certificate to relinquish
    #[serde(rename = "serialNumber")]
    pub serial_number: String,
    /// Certifier's identity key (66-char hex)
    pub certifier: String,
}

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
pub struct DiscoverByIdentityKeyParams {
    /// Identity key to look up (66-char hex compressed pubkey)
    #[serde(rename = "identityKey")]
    pub identity_key: String,
    /// Max results to return (default 10)
    #[serde(default = "default_10")]
    pub limit: u64,
    /// Optional certificate type filter
    #[serde(rename = "type", skip_serializing_if = "Option::is_none")]
    pub certificate_type: Option<String>,
}

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
pub struct DiscoverByAttributesParams {
    /// Certificate attributes to search for, e.g. {"name": "MyAgent"}
    pub attributes: Value,
    /// Max results to return (default 10)
    #[serde(default = "default_10")]
    pub limit: u64,
    /// Optional certificate type filter
    #[serde(rename = "type", skip_serializing_if = "Option::is_none")]
    pub certificate_type: Option<String>,
}

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
pub struct RevealCounterpartyKeyLinkageParams {
    /// Counterparty identity key (66-char hex)
    pub counterparty: String,
    /// Verifier identity key (66-char hex) — who will receive the linkage
    pub verifier: String,
    /// If true, use privileged key derivation
    #[serde(default)]
    pub privileged: bool,
}

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
pub struct RevealSpecificKeyLinkageParams {
    /// Counterparty identity key (66-char hex)
    pub counterparty: String,
    /// Verifier identity key (66-char hex)
    pub verifier: String,
    /// Protocol ID: [securityLevel, "protocolName"]
    #[serde(rename = "protocolID")]
    pub protocol_id: Value,
    /// Key ID for the specific key
    #[serde(rename = "keyID")]
    pub key_id: String,
    /// If true, use privileged key derivation
    #[serde(default)]
    pub privileged: bool,
}

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
pub struct RelinquishOutputParams {
    /// Basket name the output belongs to
    pub basket: String,
    /// The output to release: {txid: "64-char hex", vout: number}
    pub output: OutputRef,
}

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
pub struct OutputRef {
    /// Transaction ID (64-char hex)
    pub txid: String,
    /// Output index in the transaction
    pub vout: u32,
}

fn default_any() -> String {
    "any".into()
}
fn default_basket() -> String {
    "default".into()
}
fn default_include() -> String {
    "locking scripts".into()
}
fn default_direct() -> String {
    "direct".into()
}
fn default_10() -> u64 {
    10
}
fn default_100() -> u64 {
    100
}

// ── Tool definitions ───────────────────────────────────────────

#[tool_router]
impl WalletMcp {
    pub fn new(url: String, origin: String) -> Self {
        Self {
            client: Client::new(),
            url,
            origin,
            tool_router: Self::tool_router(),
        }
    }

    // ── Custom: Balance ────────────────────────────────────────

    #[tool(
        description = "Get the wallet's spendable balance in satoshis. Sums all spendable UTXOs."
    )]
    async fn wallet_balance(&self) -> Result<CallToolResult, ErrorData> {
        let mut total: u64 = 0;
        let mut offset: u64 = 0;
        let limit: u64 = 100;
        loop {
            let url = format!("{}/listOutputs", self.url);
            let resp = self
                .client
                .post(&url)
                .header("Origin", &self.origin)
                .header("Content-Type", "application/json")
                .json(&json!({
                    "basket": "default",
                    "include": "locking scripts",
                    "limit": limit,
                    "offset": offset,
                }))
                .send()
                .await
                .map_err(|e| Self::err(format!("HTTP error: {e}")))?;
            let body: Value = resp
                .json()
                .await
                .map_err(|e| Self::err(format!("JSON parse: {e}")))?;
            let outputs = body
                .get("outputs")
                .and_then(|v| v.as_array())
                .cloned()
                .unwrap_or_default();
            if outputs.is_empty() {
                break;
            }
            for o in &outputs {
                if o.get("spendable")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false)
                {
                    total += o.get("satoshis").and_then(|v| v.as_u64()).unwrap_or(0);
                }
            }
            if (outputs.len() as u64) < limit {
                break;
            }
            offset += limit;
        }
        let bsv = total as f64 / 100_000_000.0;
        Ok(CallToolResult::success(vec![Content::text(
            serde_json::to_string_pretty(&json!({"satoshis": total, "bsv": bsv}))
                .unwrap_or_default(),
        )]))
    }

    // ── Status (GET, no params) ────────────────────────────────

    #[tool(description = "Check if the wallet is authenticated. Returns {authenticated: true}.")]
    async fn is_authenticated(&self) -> Result<CallToolResult, ErrorData> {
        self.get("isAuthenticated").await
    }

    #[tool(description = "Get current blockchain height. Returns {height: number}.")]
    async fn get_height(&self) -> Result<CallToolResult, ErrorData> {
        self.get("getHeight").await
    }

    #[tool(description = "Get wallet network. Returns {network: \"mainnet\"}.")]
    async fn get_network(&self) -> Result<CallToolResult, ErrorData> {
        self.get("getNetwork").await
    }

    #[tool(description = "Get wallet version. Returns {version: \"wallet-brc100-1.0.0\"}.")]
    async fn get_version(&self) -> Result<CallToolResult, ErrorData> {
        self.get("getVersion").await
    }

    #[tool(description = "Wait until wallet is authenticated. Blocks until ready.")]
    async fn wait_for_authentication(&self) -> Result<CallToolResult, ErrorData> {
        self.get("waitForAuthentication").await
    }

    // ── Key & Crypto ───────────────────────────────────────────

    #[tool(
        description = "Get identity key or derive a BRC-42 public key. Set identityKey=true for root key, or provide protocolID+keyID+counterparty for derivation."
    )]
    async fn get_public_key(
        &self,
        p: Parameters<GetPublicKeyParams>,
    ) -> Result<CallToolResult, ErrorData> {
        self.post("getPublicKey", serde_json::to_value(&p.0).unwrap())
            .await
    }

    #[tool(description = "Create an ECDSA signature using a BRC-42 derived key.")]
    async fn create_signature(
        &self,
        p: Parameters<CreateSignatureParams>,
    ) -> Result<CallToolResult, ErrorData> {
        self.post("createSignature", serde_json::to_value(&p.0).unwrap())
            .await
    }

    #[tool(description = "Verify an ECDSA signature using a BRC-42 derived key.")]
    async fn verify_signature(
        &self,
        p: Parameters<VerifySignatureParams>,
    ) -> Result<CallToolResult, ErrorData> {
        self.post("verifySignature", serde_json::to_value(&p.0).unwrap())
            .await
    }

    #[tool(description = "Encrypt data using a BRC-42 derived key. Returns {ciphertext: [bytes]}.")]
    async fn encrypt(&self, p: Parameters<EncryptParams>) -> Result<CallToolResult, ErrorData> {
        self.post("encrypt", serde_json::to_value(&p.0).unwrap())
            .await
    }

    #[tool(description = "Decrypt data using a BRC-42 derived key. Returns {plaintext: [bytes]}.")]
    async fn decrypt(&self, p: Parameters<DecryptParams>) -> Result<CallToolResult, ErrorData> {
        self.post("decrypt", serde_json::to_value(&p.0).unwrap())
            .await
    }

    #[tool(description = "Create HMAC using a BRC-42 derived key. Returns {hmac: [32 bytes]}.")]
    async fn create_hmac(
        &self,
        p: Parameters<CreateHmacParams>,
    ) -> Result<CallToolResult, ErrorData> {
        self.post("createHmac", serde_json::to_value(&p.0).unwrap())
            .await
    }

    #[tool(description = "Verify HMAC using a BRC-42 derived key. Returns {valid: bool}.")]
    async fn verify_hmac(
        &self,
        p: Parameters<VerifyHmacParams>,
    ) -> Result<CallToolResult, ErrorData> {
        self.post("verifyHmac", serde_json::to_value(&p.0).unwrap())
            .await
    }

    // ── Chain ──────────────────────────────────────────────────

    #[tool(description = "Get block header for a height. Returns {header: \"hex\"}.")]
    async fn get_header_for_height(
        &self,
        p: Parameters<GetHeaderForHeightParams>,
    ) -> Result<CallToolResult, ErrorData> {
        self.post("getHeaderForHeight", serde_json::to_value(&p.0).unwrap())
            .await
    }

    // ── Transactions ───────────────────────────────────────────

    #[tool(
        description = "Create a BSV transaction. Build, sign, broadcast. Use options.signAndProcess=false for unsigned templates (sign later with sign_action). Use options.noSend=true to sign but not broadcast. A broadcaster's refusal is not an error: the answer carries sendWithResults 'sending' and broadcast {word: 'built', request, hint, inputs: 'locked'}; the inputs stay locked until a proof, a competitor's checked proof or the wallet's retire, and nothing is released."
    )]
    async fn create_action(
        &self,
        p: Parameters<CreateActionParams>,
    ) -> Result<CallToolResult, ErrorData> {
        self.post("createAction", serde_json::to_value(&p.0).unwrap())
            .await
    }

    #[tool(
        description = "Sign a previously unsigned transaction (from create_action with signAndProcess=false). A broadcaster's refusal is not an error: the answer carries sendWithResults 'sending' and broadcast {word: 'built', request, hint, inputs: 'locked'}; the inputs stay locked until a proof, a competitor's checked proof or the wallet's retire, and nothing is released."
    )]
    async fn sign_action(
        &self,
        p: Parameters<ReferenceParams>,
    ) -> Result<CallToolResult, ErrorData> {
        self.post("signAction", serde_json::to_value(&p.0).unwrap())
            .await
    }

    #[tool(
        description = "Abort an unsigned transaction and release locked UTXOs. A transaction a broadcaster accepted is never aborted (refused, 409): it stays until a proof, a node verdict or a competitor's checked proof."
    )]
    async fn abort_action(
        &self,
        p: Parameters<ReferenceParams>,
    ) -> Result<CallToolResult, ErrorData> {
        self.post("abortAction", serde_json::to_value(&p.0).unwrap())
            .await
    }

    #[tool(
        description = "Accept incoming payment. tx must be AtomicBEEF format byte array. A broadcaster's refusal of the incoming transaction is not an error: the answer is accepted, with broadcast {word: 'built', request, hint, inputs: 'locked'}; its coins are not spent by this wallet until a status source holds the transaction."
    )]
    async fn internalize_action(
        &self,
        p: Parameters<InternalizeActionParams>,
    ) -> Result<CallToolResult, ErrorData> {
        self.post("internalizeAction", serde_json::to_value(&p.0).unwrap())
            .await
    }

    #[tool(
        description = "List transaction history. Filter by labels. Returns {totalActions, actions: [{txid, satoshis, status, description, ...}]}."
    )]
    async fn list_actions(
        &self,
        p: Parameters<ListActionsParams>,
    ) -> Result<CallToolResult, ErrorData> {
        self.post("listActions", serde_json::to_value(&p.0).unwrap())
            .await
    }

    // ── Outputs (UTXOs) ────────────────────────────────────────

    #[tool(
        description = "List unspent outputs (UTXOs). Returns {totalOutputs, outputs: [{satoshis, spendable, outpoint, ...}]}."
    )]
    async fn list_outputs(
        &self,
        p: Parameters<ListOutputsParams>,
    ) -> Result<CallToolResult, ErrorData> {
        self.post("listOutputs", serde_json::to_value(&p.0).unwrap())
            .await
    }

    #[tool(description = "Release an output from a basket.")]
    async fn relinquish_output(
        &self,
        p: Parameters<RelinquishOutputParams>,
    ) -> Result<CallToolResult, ErrorData> {
        self.post("relinquishOutput", serde_json::to_value(&p.0).unwrap())
            .await
    }

    // ── Certificates ───────────────────────────────────────────

    #[tool(description = "Acquire and store a certificate in the wallet.")]
    async fn acquire_certificate(
        &self,
        p: Parameters<AcquireCertificateParams>,
    ) -> Result<CallToolResult, ErrorData> {
        self.post("acquireCertificate", serde_json::to_value(&p.0).unwrap())
            .await
    }

    #[tool(
        description = "List certificates. Filter by certifiers and/or types. Returns {certificates: [...]}."
    )]
    async fn list_certificates(
        &self,
        p: Parameters<ListCertificatesParams>,
    ) -> Result<CallToolResult, ErrorData> {
        self.post("listCertificates", serde_json::to_value(&p.0).unwrap())
            .await
    }

    #[tool(description = "Prove certificate ownership by selectively revealing fields.")]
    async fn prove_certificate(
        &self,
        p: Parameters<ProveCertificateParams>,
    ) -> Result<CallToolResult, ErrorData> {
        self.post("proveCertificate", serde_json::to_value(&p.0).unwrap())
            .await
    }

    #[tool(description = "Delete a certificate from the wallet.")]
    async fn relinquish_certificate(
        &self,
        p: Parameters<RelinquishCertificateParams>,
    ) -> Result<CallToolResult, ErrorData> {
        self.post("relinquishCertificate", serde_json::to_value(&p.0).unwrap())
            .await
    }

    // ── Discovery ──────────────────────────────────────────────

    #[tool(description = "Find certificates by identity key (BRC-56 peer discovery).")]
    async fn discover_by_identity_key(
        &self,
        p: Parameters<DiscoverByIdentityKeyParams>,
    ) -> Result<CallToolResult, ErrorData> {
        self.post("discoverByIdentityKey", serde_json::to_value(&p.0).unwrap())
            .await
    }

    #[tool(description = "Find certificates by attributes (BRC-56 peer discovery).")]
    async fn discover_by_attributes(
        &self,
        p: Parameters<DiscoverByAttributesParams>,
    ) -> Result<CallToolResult, ErrorData> {
        self.post("discoverByAttributes", serde_json::to_value(&p.0).unwrap())
            .await
    }

    // ── Key Linkage ────────────────────────────────────────────

    #[tool(description = "Reveal counterparty key linkage to a verifier (BRC-69).")]
    async fn reveal_counterparty_key_linkage(
        &self,
        p: Parameters<RevealCounterpartyKeyLinkageParams>,
    ) -> Result<CallToolResult, ErrorData> {
        self.post(
            "revealCounterpartyKeyLinkage",
            serde_json::to_value(&p.0).unwrap(),
        )
        .await
    }

    #[tool(description = "Reveal specific key linkage for a protocol+key to a verifier (BRC-70).")]
    async fn reveal_specific_key_linkage(
        &self,
        p: Parameters<RevealSpecificKeyLinkageParams>,
    ) -> Result<CallToolResult, ErrorData> {
        self.post(
            "revealSpecificKeyLinkage",
            serde_json::to_value(&p.0).unwrap(),
        )
        .await
    }
}

// ── ServerHandler ──────────────────────────────────────────────

#[tool_handler]
impl ServerHandler for WalletMcp {
    fn get_info(&self) -> ServerInfo {
        ServerInfo {
            protocol_version: ProtocolVersion::LATEST,
            capabilities: ServerCapabilities::builder().enable_tools().build(),
            server_info: Implementation::from_build_env(),
            instructions: Some(
                "BSV wallet MCP server. All 28 BRC-100 wallet endpoints plus wallet_balance."
                    .into(),
            ),
        }
    }
}

// ── main ───────────────────────────────────────────────────────

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::from_default_env()
                .add_directive("bsv_wallet_mcp=info".parse().unwrap()),
        )
        .with_writer(std::io::stderr)
        .with_ansi(false)
        .init();

    let url = env::var("WALLET_URL").unwrap_or_else(|_| "http://localhost:3322".into());
    let origin = env::var("WALLET_ORIGIN").unwrap_or_else(|_| "http://localhost".into());

    tracing::info!("Starting bsv-wallet-mcp → {url}");

    let server = WalletMcp::new(url, origin);
    let service = server.serve(stdio()).await.inspect_err(|e| {
        tracing::error!("Failed to start MCP server: {e}");
    })?;

    service.waiting().await?;
    Ok(())
}
