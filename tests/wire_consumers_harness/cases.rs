//! The cases (record only): each consumer's calls, in table order. Replay
//! reads the fixtures, never these.

use std::collections::BTreeMap;

use super::worlds::{BEARER, PENDING_A, PENDING_B, UNKNOWN};
use crate::wire_support::{Req, J};

/// A request body from its JSON text, its keys in the order the consumer
/// writes them (`serde_json::json!` would sort them).
fn body(text: &str) -> J {
    serde_json::from_str(text).unwrap_or_else(|e| panic!("{text}: {e}"))
}

// ---------------------------------------------------------------------------
// The cases (record only). Replay reads the fixtures, never these.
// ---------------------------------------------------------------------------

/// The secp256k1 generator's key: the counterparty "anyone", and the
/// stand-in for a server's identity key.
pub(crate) const G: &str = "0279be667ef9dcbbac55a06295ce870b07029bfcdb2dce28d959f2815b16f81798";
/// A pay-to-public-key-hash output script (a payee's).
const PAYEE: &str = "76a91489abcdefabbaabbaabbaabbaabbaabbaabbaabba88ac";
/// The x402 server's response signature (fixture 04's answer: made for the
/// server, not by it).
const SERVER_SIGNATURE: &str = "30440220016d22b66a0ffb4489cfe9334921368f1b0b19cc51861a9da7091a81c6b2ddac02206743cbd5919e91afec26e8f2d95487d7da34c797ff86796d2fe329a5d93485bb";
/// A `tx` that is not an atomic BEEF (the shape is the call's, the refusal
/// the parser's).
const NOT_BEEF: &[u8] = &[1, 1, 1, 1, 0, 0, 0, 0];

/// One call a consumer sends: a row of its table below.
#[derive(Clone, Copy)]
pub(crate) struct Case {
    /// The consumer's own name for the call (the fixture's file name).
    pub(crate) call: &'static str,
    /// The file in the consumer that sends this shape.
    pub(crate) source: &'static str,
    pub(crate) world: &'static str,
    /// The `Origin` header; empty sends none.
    origin: &'static str,
    pub(crate) path: &'static str,
    /// A POST's JSON body, its keys in the order the consumer writes them;
    /// none is a GET. [`fill`] replaces the placeholders in it and in the
    /// path.
    body: Option<&'static str>,
    /// The case's own placeholders: the bytes each stands for, sent as the
    /// JSON number array the `MetaNet` Client's shape carries.
    bytes: &'static [(&'static str, &'static [u8])],
    /// Sends `authorization: Bearer` [`BEARER`].
    pub(crate) bearer: bool,
}

impl Case {
    /// What every case starts from: a GET with no header, `fresh` world.
    const BASE: Case = Case {
        call: "",
        source: "",
        world: "fresh",
        origin: "",
        path: "",
        body: None,
        bytes: &[],
        bearer: false,
    };

    pub(crate) fn request(&self) -> Req {
        let mut headers = BTreeMap::new();
        if !self.origin.is_empty() {
            headers.insert("origin".to_string(), self.origin.to_string());
        }
        if self.bearer {
            headers.insert("authorization".to_string(), format!("Bearer {BEARER}"));
        }
        Req {
            method: if self.body.is_some() { "POST" } else { "GET" }.into(),
            path: fill(self.path, self.bytes),
            headers,
            json: self.body.map(|text| body(&fill(text, self.bytes))),
            ..Req::default()
        }
    }
}

/// A case's text with its placeholders replaced: the constants by name
/// (`{G}`, `{PAYEE}`, `{PENDING_A}`, `{PENDING_B}`, `{UNKNOWN}`,
/// `{SERVER_SIGNATURE}` as bytes), then the case's own bytes.
fn fill(text: &str, bytes: &[(&str, &[u8])]) -> String {
    let signature = hex::decode(SERVER_SIGNATURE).unwrap();
    let mut out = text.to_string();
    for (name, value) in [
        ("{G}", G),
        ("{PAYEE}", PAYEE),
        ("{PENDING_A}", PENDING_A),
        ("{PENDING_B}", PENDING_B),
        ("{UNKNOWN}", UNKNOWN),
    ] {
        out = out.replace(name, value);
    }
    for (name, value) in bytes
        .iter()
        .copied()
        .chain([("{SERVER_SIGNATURE}", &signature[..])])
    {
        out = out.replace(name, &serde_json::to_string(value).unwrap());
    }
    out
}

/// Every consumer's cases, each consumer's in the order it is recorded.
pub(crate) const CASES: [(&str, &[Case]); 7] = [
    ("low-fleet", LOW_FLEET),
    ("stack-witness", STACK_WITNESS),
    ("x402-skill", X402_SKILL),
    ("mcp", MCP),
    ("app-payers", APP_PAYERS),
    ("dolphinmilk", DOLPHINMILK),
    ("e2e-harness", E2E_HARNESS),
];

/// LOW's fleet: its app's `WalletClient` over @bsv/sdk 2.1.4's
/// `HTTPWalletJSON` (POST for every call, the args as JSON), served to a
/// seat page whose origin the browser sets; its e2e route adds
/// `allowPending: "received"` to a seat's `/createAction` in transit; its
/// harness's own fetch carries the bearer. The scoped word is #63's text
/// (no LOW code sends it yet). LOW's funder drives the CLI (`--json`),
/// which `docs/DEPENDENTS.md` lists and this test does not reach.
const LOW_FLEET_SEAT: Case = Case {
    world: "pending",
    origin: "http://localhost:5173",
    ..Case::BASE
};
const LOW_FLEET: &[Case] = &[
    Case {
        call: "isAuthenticated",
        source: "app/src/components/ConnectGate.tsx (WalletClient over HTTPWalletJSON)",
        path: "/isAuthenticated",
        body: Some("{}"),
        ..LOW_FLEET_SEAT
    },
    Case {
        call: "getVersion",
        source: "app/src/components/ConnectGate.tsx (WalletClient over HTTPWalletJSON)",
        path: "/getVersion",
        body: Some("{}"),
        ..LOW_FLEET_SEAT
    },
    Case {
        call: "getPublicKey.identity",
        source: "app/src/components/ConnectGate.tsx",
        path: "/getPublicKey",
        body: Some(r#"{"identityKey":true}"#),
        ..LOW_FLEET_SEAT
    },
    Case {
        call: "getPublicKey.brc29Self",
        source: "app/src/lib/stake.ts (the payout key, forSelf)",
        path: "/getPublicKey",
        body: Some(
            r#"{"protocolID":[2,"3241645161d8"],"counterparty":"self","keyID":"SfKxPIJNgdI= NaGLC6fMH50=","forSelf":true}"#,
        ),
        ..LOW_FLEET_SEAT
    },
    Case {
        call: "createSignature.hashToDirectlySign",
        source: "app/src/lib/stake.ts signStakeDigest (STAKE_PROTOCOL)",
        path: "/createSignature",
        body: Some(
            r#"{"protocolID":[2,"low escrow settle"],"keyID":"game-1","counterparty":"{G}","hashToDirectlySign":{hash}}"#,
        ),
        bytes: &[("{hash}", &[7u8; 32])],
        ..LOW_FLEET_SEAT
    },
    Case {
        call: "createHmac.handSecret",
        source: "app/src/lib/handSecrets.ts deriveHandSecrets",
        path: "/createHmac",
        body: Some(
            r#"{"protocolID":[2,"low hand secret"],"keyID":"game-1:{G}:v1","data":{data},"counterparty":"self"}"#,
        ),
        bytes: &[("{data}", b"low-hand-secret:v1:game-1")],
        ..LOW_FLEET_SEAT
    },
    Case {
        call: "listActions.hopLedger",
        source: "app/src/lib/stake.ts fetchWalletHopLedger",
        path: "/listActions",
        body: Some(r#"{"labels":[],"includeLabels":true,"limit":10000,"offset":0}"#),
        ..LOW_FLEET_SEAT
    },
    // The app's own spend: no word, so every received unproven coin is held.
    Case {
        call: "createAction.stake",
        source: "app/src/lib/stake.ts (the escrow funding)",
        path: "/createAction",
        body: Some(
            r#"{"description":"LOW stake escrow funding","outputs":[{"lockingScript":"{PAYEE}","satoshis":2300,"outputDescription":"LOW stake hop"}],"options":{"randomizeOutputs":false,"acceptDelayedBroadcast":false}}"#,
        ),
        ..LOW_FLEET_SEAT
    },
    // The same body with the e2e route's word: a received coin is offered
    // and chosen, and the build stops at signing (see [`P2PKH`]): the
    // answer is past the gate, where without the word it is 402.
    //
    // A known wrong shape, pinned as the server's answer today: a signing
    // fault is a 400 `ERROR` carrying the toolbox's internal message (a
    // derived hash160, "Transaction aborted"). Typed errors (#81) and the
    // fixtures oracle (#69) fix it; this fixture and the scoped one below
    // change with that fix, in its own commit.
    Case {
        call: "createAction.allowPending",
        source: "app/e2e/lib/allowPendingRoute.ts withAllowPending",
        path: "/createAction",
        body: Some(
            r#"{"description":"LOW stake escrow funding","outputs":[{"lockingScript":"{PAYEE}","satoshis":2300,"outputDescription":"LOW stake hop"}],"options":{"randomizeOutputs":false,"acceptDelayedBroadcast":false},"allowPending":"received"}"#,
        ),
        ..LOW_FLEET_SEAT
    },
    // #63's scoped word: only the funder's top-up (PENDING_B) is offered.
    // The answer is the same known wrong shape as the word's above (#81).
    Case {
        call: "createAction.scoped",
        source: "bsv-wallet-cli #63 (LOW's fleet, loop 46, pair 32): allowPending {received: [txid:vout]}",
        path: "/createAction",
        body: Some(
            r#"{"description":"LOW stake escrow funding","outputs":[{"lockingScript":"{PAYEE}","satoshis":2300,"outputDescription":"LOW stake hop"}],"options":{"randomizeOutputs":false,"acceptDelayedBroadcast":false},"allowPending":{"received":["{PENDING_B}:0"]}}"#,
        ),
        ..LOW_FLEET_SEAT
    },
    Case {
        call: "internalizeAction.potPayout",
        source: "app/src/lib/stake.ts (the pot payout)",
        path: "/internalizeAction",
        body: Some(
            r#"{"tx":{tx},"outputs":[{"outputIndex":0,"protocol":"wallet payment","paymentRemittance":{"derivationPrefix":"SfKxPIJNgdI=","derivationSuffix":"NaGLC6fMH50=","senderIdentityKey":"{G}"}}],"description":"LOW pot payout"}"#,
        ),
        bytes: &[("{tx}", NOT_BEEF)],
        ..LOW_FLEET_SEAT
    },
    Case {
        call: "getPublicKey.harnessBearer",
        source: "app/e2e/lib/cli.ts (fetch with authorization when AUTH_TOKEN is set)",
        world: "auth",
        origin: "http://localhost",
        path: "/getPublicKey",
        body: Some(
            r#"{"protocolID":[2,"3241645161d8"],"keyID":"SfKxPIJNgdI= NaGLC6fMH50=","counterparty":"{G}","forSelf":true}"#,
        ),
        bearer: true,
        ..LOW_FLEET_SEAT
    },
];

/// The stack's witness spend wallet: its daily job drives the CLI
/// (`address`, `--json split --count 2`, `balance`); its one read over
/// HTTP is the tracker's word for the spend, a plain GET on loopback.
const STACK_WITNESS: &[Case] = &[Case {
    call: "txWord",
    source: "bsv-stack-lean docs/p0/witness-spend.md (GET /tx/{txid} on loopback)",
    path: "/tx/{UNKNOWN}",
    ..Case::BASE
}];

/// The x402 skill (calgooon-x402 1.1.4 at `dc00cde`): every call goes
/// through `lib/metanet.py`'s `_call` (POST, `Origin: http://localhost`, JSON),
/// and `SKILL.md`'s health check is a plain GET.
const X402_CLIENT: Case = Case {
    origin: "http://localhost",
    ..Case::BASE
};
const X402_SKILL: &[Case] = &[
    Case {
        call: "isAuthenticated",
        source: r#"skills/x402/SKILL.md (curl -H "Origin: http://localhost" .../isAuthenticated)"#,
        path: "/isAuthenticated",
        ..X402_CLIENT
    },
    Case {
        call: "getPublicKey.identity",
        source: "skills/x402/lib/metanet.py get_identity_key",
        path: "/getPublicKey",
        body: Some(r#"{"identityKey":true}"#),
        ..X402_CLIENT
    },
    // BRC-29: the payment key for the server, keyID "<prefix> <suffix>".
    Case {
        call: "getPublicKey.payment",
        source: "skills/x402/lib/payment.py (get_public_key, PAYMENT_PROTOCOL)",
        path: "/getPublicKey",
        body: Some(
            r#"{"protocolID":[2,"3241645161d8"],"keyID":"SfKxPIJNgdI= NaGLC6fMH50=","counterparty":"{G}","forSelf":false}"#,
        ),
        ..X402_CLIENT
    },
    // BRC-31: the auth message signature, keyID "<nonce> <server nonce>".
    Case {
        call: "createSignature.auth",
        source: "skills/x402/lib/auth_request.py (create_signature, AUTH_PROTOCOL)",
        path: "/createSignature",
        body: Some(
            r#"{"data":{data},"protocolID":[2,"auth message signature"],"keyID":"bm9uY2Ux bm9uY2Uy","counterparty":"{G}"}"#,
        ),
        bytes: &[("{data}", b"x402 auth message")],
        ..X402_CLIENT
    },
    Case {
        call: "createAction.payment",
        source: "skills/x402/lib/payment.py -> lib/metanet.py create_action",
        path: "/createAction",
        body: Some(
            r#"{"description":"BRC-29 payment: 1000 sats","outputs":[{"lockingScript":"{PAYEE}","satoshis":1000,"outputDescription":"BRC-29 payment output"}],"options":{"acceptDelayedBroadcast":false,"randomizeOutputs":false}}"#,
        ),
        ..X402_CLIENT
    },
    Case {
        call: "createAction.executeAction",
        source: "skills/x402/scripts/brc31_helpers.py execute-action",
        path: "/createAction",
        body: Some(
            r#"{"description":"Agent action","outputs":[{"lockingScript":"{PAYEE}","satoshis":1,"outputDescription":"agent output"}],"options":{},"labels":["app:x402"]}"#,
        ),
        ..X402_CLIENT
    },
    // A refund: the server's AtomicBEEF as a number array; these bytes
    // are not one (see [`NOT_BEEF`]).
    Case {
        call: "internalizeAction.refund",
        source: "skills/x402/lib/refund.py -> lib/metanet.py internalize_action",
        path: "/internalizeAction",
        body: Some(
            r#"{"tx":{tx},"outputs":[{"outputIndex":0,"protocol":"wallet payment","paymentRemittance":{"derivationPrefix":"SfKxPIJNgdI=","derivationSuffix":"NaGLC6fMH50=","senderIdentityKey":"{G}"}}],"description":"Refund: 1000 sats (txid: 0000000000000000...)"}"#,
        ),
        bytes: &[("{tx}", NOT_BEEF)],
        ..X402_CLIENT
    },
    // 1.1.4: the server's signed response, checked by the wallet under the
    // same protocol, keyID "<server nonce> <client nonce>". The signature
    // is the one fixture 04 answers (made for the server, not by it), so
    // the answer is the wallet's word on a signature that does not verify:
    // a 400 `ERROR`, not `{"valid": false}` (the skill reads either as
    // "BAD SIGNATURE"; the shape is #81's).
    Case {
        call: "verifySignature.response",
        source: "skills/x402/lib/auth_request.py verify_response",
        path: "/verifySignature",
        body: Some(
            r#"{"data":{data},"signature":{SERVER_SIGNATURE},"protocolID":[2,"auth message signature"],"keyID":"bm9uY2Uy bm9uY2Ux","counterparty":"{G}"}"#,
        ),
        bytes: &[("{data}", b"x402 response payload")],
        ..X402_CLIENT
    },
];

/// The MCP companion (`mcp/src/main.rs`, bsv-wallet-mcp 0.3.0): each tool
/// posts its typed parameters, serialized in struct order, with `Origin:
/// http://localhost` (`WALLET_ORIGIN`'s default); the status tools GET.
const MCP_TOOL: Case = Case {
    origin: "http://localhost",
    ..Case::BASE
};
const MCP: &[Case] = &[
    Case {
        call: "is_authenticated",
        source: "mcp/src/main.rs is_authenticated (GET)",
        path: "/isAuthenticated",
        ..MCP_TOOL
    },
    Case {
        call: "get_height",
        source: "mcp/src/main.rs get_height (GET)",
        path: "/getHeight",
        ..MCP_TOOL
    },
    Case {
        call: "get_public_key.identity",
        source: "mcp/src/main.rs GetPublicKeyParams",
        path: "/getPublicKey",
        body: Some(r#"{"identityKey":true}"#),
        ..MCP_TOOL
    },
    Case {
        call: "create_signature",
        source: "mcp/src/main.rs CreateSignatureParams",
        path: "/createSignature",
        body: Some(
            r#"{"data":{data},"protocolID":[2,"mcp test"],"keyID":"1","counterparty":"self"}"#,
        ),
        bytes: &[("{data}", b"mcp")],
        ..MCP_TOOL
    },
    // `wallet_balance`: the default basket a page at a time.
    Case {
        call: "wallet_balance",
        source: "mcp/src/main.rs wallet_balance (listOutputs pages)",
        world: "pending",
        path: "/listOutputs",
        body: Some(r#"{"basket":"default","include":"locking scripts","limit":100,"offset":0}"#),
        ..MCP_TOOL
    },
    Case {
        call: "list_actions",
        source: "mcp/src/main.rs ListActionsParams (its defaults)",
        path: "/listActions",
        body: Some(
            r#"{"labels":[],"labelQueryMode":"any","includeLabels":false,"includeInputs":false,"includeOutputs":false,"limit":10,"offset":0}"#,
        ),
        ..MCP_TOOL
    },
    // `allowPending` is a string in the MCP's parameters: the plain word.
    Case {
        call: "create_action.allowPending",
        source: "mcp/src/main.rs CreateActionParams",
        world: "pending",
        path: "/createAction",
        body: Some(
            r#"{"description":"mcp payment","outputs":[{"lockingScript":"{PAYEE}","satoshis":60000,"outputDescription":"mcp output"}],"allowPending":"received"}"#,
        ),
        ..MCP_TOOL
    },
    Case {
        call: "create_action",
        source: "mcp/src/main.rs CreateActionParams",
        world: "pending",
        path: "/createAction",
        body: Some(
            r#"{"description":"mcp payment","outputs":[{"lockingScript":"{PAYEE}","satoshis":1000,"outputDescription":"mcp output"}],"labels":["mcp"],"options":{"randomizeOutputs":false}}"#,
        ),
        ..MCP_TOOL
    },
    Case {
        call: "relinquish_output",
        source: "mcp/src/main.rs RelinquishOutputParams",
        path: "/relinquishOutput",
        body: Some(r#"{"basket":"default","output":{"txid":"{UNKNOWN}","vout":0}}"#),
        ..MCP_TOOL
    },
    Case {
        call: "chain_feed_status",
        source: "mcp/src/main.rs chain_feed_status (GET /chain-feed)",
        path: "/chain-feed",
        ..MCP_TOOL
    },
    Case {
        call: "tx_word",
        source: "mcp/src/main.rs tx_word (GET /tx/{txid})",
        path: "/tx/{UNKNOWN}",
        ..MCP_TOOL
    },
];

/// The apps' payers: ~/bsv/agents' `tools/fleet/proof.py` (the x402
/// helper's client, and a `noSend` spend it aborts), Zanaadu's and
/// teragunv2's `WalletClient` over `HTTPWalletJSON` (a browser origin).
const AGENTS_HELPER: Case = Case {
    origin: "http://localhost",
    ..Case::BASE
};
const APP_PAGE: Case = Case {
    origin: "http://localhost:5173",
    ..Case::BASE
};
const APP_PAYERS: &[Case] = &[
    Case {
        call: "agents.createAction.noSend",
        source: "agents tools/fleet/proof.py (/createAction, noSend)",
        path: "/createAction",
        body: Some(
            r#"{"description":"fleet proof","outputs":[{"lockingScript":"{PAYEE}","satoshis":1,"outputDescription":"fleet proof"}],"options":{"noSend":true,"randomizeOutputs":false,"acceptDelayedBroadcast":false}}"#,
        ),
        ..AGENTS_HELPER
    },
    Case {
        call: "agents.abortAction",
        source: "agents tools/fleet/proof.py (abortAction {reference: txid})",
        path: "/abortAction",
        body: Some(r#"{"reference":"{UNKNOWN}"}"#),
        ..AGENTS_HELPER
    },
    Case {
        call: "zanaadu.getPublicKey.identity",
        source: "zanaadu frontend/src/lib/zanaaduSigningProtocols.ts ([2,'zanaadu identity'] '1')",
        path: "/getPublicKey",
        body: Some(r#"{"protocolID":[2,"zanaadu identity"],"keyID":"1","counterparty":"self"}"#),
        ..APP_PAGE
    },
    Case {
        call: "zanaadu.createAction.fund",
        source: "zanaadu frontend/src/lib/features-fund/fund.ts",
        path: "/createAction",
        body: Some(
            r#"{"description":"Zanaadu fund","outputs":[{"lockingScript":"{PAYEE}","satoshis":1000,"outputDescription":"Zanaadu fund output"}],"options":{"randomizeOutputs":false,"acceptDelayedBroadcast":false}}"#,
        ),
        ..APP_PAGE
    },
    Case {
        call: "teragunv2.getPublicKey.payment",
        source: "teragunv2 frontend/src/lib/wallet.ts (PAYMENT_PROTOCOL [2,'wallet payment'])",
        path: "/getPublicKey",
        body: Some(
            r#"{"protocolID":[2,"wallet payment"],"keyID":"SfKxPIJNgdI= NaGLC6fMH50=","counterparty":"{G}","forSelf":false}"#,
        ),
        ..APP_PAGE
    },
    Case {
        call: "teragunv2.createAction.pack",
        source: "teragunv2 frontend/src/lib/wallet.ts (options: acceptDelayedBroadcast only)",
        path: "/createAction",
        body: Some(
            r#"{"outputs":[{"satoshis":1000,"lockingScript":"{PAYEE}","outputDescription":"API payment"}],"description":"Teragun 10 TX Pack (1,000 sats)","options":{"acceptDelayedBroadcast":false}}"#,
        ),
        ..APP_PAGE
    },
];

/// dolphinmilk (`src/wallet/http.rs`): the 0.1.23 crate's server
/// in-process, its own reqwest client (`Origin` from its config, the
/// status calls on GET).
const DOLPHINMILK_CLIENT: Case = Case {
    world: "pending",
    origin: "http://localhost",
    ..Case::BASE
};
const DOLPHINMILK: &[Case] = &[
    Case {
        call: "getVersion",
        source: "dolphinmilk src/wallet/http.rs call_get",
        path: "/getVersion",
        ..DOLPHINMILK_CLIENT
    },
    Case {
        call: "listOutputs.balance",
        source: "dolphinmilk src/wallet/http.rs (the balance pages)",
        path: "/listOutputs",
        body: Some(r#"{"basket":"default","include":"locking scripts","limit":100,"offset":0}"#),
        ..DOLPHINMILK_CLIENT
    },
    Case {
        call: "createAction.x402",
        source: "dolphinmilk bsv-x402-server/src/payment.rs -> src/wallet/http.rs create_action",
        path: "/createAction",
        body: Some(
            r#"{"description":"x402 payment","outputs":[{"lockingScript":"{PAYEE}","satoshis":100,"outputDescription":"x402 payment"}],"options":{"acceptDelayedBroadcast":false,"randomizeOutputs":false}}"#,
        ),
        ..DOLPHINMILK_CLIENT
    },
];

/// The e2e harness's client (`tests/e2e/lib/wallet-client.js`), `Origin:
/// http://e2e-test`: its own spellings (`protocolId`, `keyId`,
/// `certificateType`) are part of what the server accepts.
const E2E_CLIENT: Case = Case {
    origin: "http://e2e-test",
    ..Case::BASE
};
const E2E_HARNESS: &[Case] = &[
    Case {
        call: "getPublicKey.brc29",
        source: "tests/e2e/lib/wallet-client.js getPublicKey (transferTo)",
        path: "/getPublicKey",
        body: Some(
            r#"{"protocolId":[2,"3241645161d8"],"keyId":"SfKxPIJNgdI= NaGLC6fMH50=","counterparty":"{G}","forSelf":true}"#,
        ),
        ..E2E_CLIENT
    },
    Case {
        call: "createSignature",
        source: "tests/e2e/lib/wallet-client.js createSignature",
        path: "/createSignature",
        body: Some(
            r#"{"data":{data},"protocolId":[2,"e2e test"],"keyId":"1","counterparty":"self"}"#,
        ),
        bytes: &[("{data}", b"e2e")],
        ..E2E_CLIENT
    },
    Case {
        call: "listOutputs.balance",
        source: "tests/e2e/lib/wallet-client.js balance",
        world: "pending",
        path: "/listOutputs",
        body: Some(r#"{"basket":"default","limit":10000,"includeEnvelope":false}"#),
        ..E2E_CLIENT
    },
    // `transferTo` under the word every test wallet's spend names.
    Case {
        call: "createAction.transferTo",
        source: "tests/e2e/lib/wallet-client.js createAction (transferTo)",
        world: "pending",
        path: "/createAction",
        body: Some(
            r#"{"outputs":[{"lockingScript":"{PAYEE}","satoshis":60000,"outputDescription":"e2e transfer","tags":["e2e-test"]}],"description":"e2e transfer","allowPending":"received","options":{"signAndProcess":true,"acceptDelayedBroadcast":false,"randomizeOutputs":false,"noSend":false}}"#,
        ),
        ..E2E_CLIENT
    },
    // Scenario 23's scoped word: only PENDING_A is offered.
    Case {
        call: "createAction.scoped",
        source: "tests/e2e/lib/wallet-client.js createAction (opts.allowPending, scenario 23)",
        world: "pending",
        path: "/createAction",
        body: Some(
            r#"{"outputs":[{"lockingScript":"{PAYEE}","satoshis":40000,"outputDescription":"e2e transfer","tags":["e2e-test"]}],"description":"e2e transfer","allowPending":{"received":["{PENDING_A}:0"]},"options":{"signAndProcess":true,"acceptDelayedBroadcast":false,"randomizeOutputs":false,"noSend":false}}"#,
        ),
        ..E2E_CLIENT
    },
    Case {
        call: "relinquishOutput",
        source: "tests/e2e/lib/wallet-client.js relinquishOutput",
        path: "/relinquishOutput",
        body: Some(r#"{"txid":"{UNKNOWN}","vout":0,"basket":"default"}"#),
        ..E2E_CLIENT
    },
    Case {
        call: "isAuthenticated.bearer",
        source: "tests/e2e/lib/wallet-client.js get (a wallet behind AUTH_TOKEN)",
        world: "auth",
        path: "/isAuthenticated",
        bearer: true,
        ..E2E_CLIENT
    },
];
