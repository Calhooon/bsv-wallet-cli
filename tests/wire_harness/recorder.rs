//! The recorder (record only) and the requests it sends.

use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;

use super::fixture::{j, Fixture};
use super::manifest::{Manifest, MANIFEST};
use super::worlds::{resolve, World};
use crate::wire_support::{Bound, Req, BOUND, J};

// ---------------------------------------------------------------------------
// The cases (record only). Replay reads the fixtures, never these.
// ---------------------------------------------------------------------------

/// The origin every recorded request carries, which the worlds admit
/// (`ALLOWED_ORIGINS`, #411).
pub(crate) const ORIGIN: &str = "http://wire.test";
pub(crate) const G: &str = "0279be667ef9dcbbac55a06295ce870b07029bfcdb2dce28d959f2815b16f81798";
pub(crate) const CERT_TYPE: &str = "BgYGBgYGBgYGBgYGBgYGBgYGBgYGBgYGBgYGBgYGBgY=";
/// A ciphertext of `wire` under [`ROOT_KEY`], protocol `[2, "wire test"]`,
/// key `1`, counterparty [`G`].
pub(crate) const DECRYPT_CIPHERTEXT: [u8; 52] = [
    67, 235, 10, 58, 150, 72, 239, 64, 143, 10, 51, 111, 218, 66, 249, 235, 164, 141, 131, 123, 11,
    112, 52, 251, 161, 6, 254, 68, 76, 31, 76, 85, 161, 185, 3, 46, 158, 136, 86, 129, 8, 94, 152,
    124, 223, 53, 88, 226, 122, 43, 16, 152,
];

pub(crate) struct Recorder {
    pub(crate) dir: PathBuf,
    pub(crate) worlds: HashMap<String, World>,
    pub(crate) seq: usize,
    pub(crate) written: Vec<String>,
    /// Each file's JSON body as served (a later case may bind to it).
    pub(crate) served: HashMap<String, J>,
}

/// What a recorded case leaves for the cases after it.
pub(crate) struct Recorded {
    pub(crate) file: String,
    pub(crate) body: Option<J>,
}

impl Recorded {
    /// The value at `pointer` of the served body, as JSON.
    pub(crate) fn value(&self, pointer: &str) -> serde_json::Value {
        let body = self
            .body
            .as_ref()
            .unwrap_or_else(|| panic!("{}: no JSON body", self.file));
        let v = body
            .pointer(pointer)
            .unwrap_or_else(|| panic!("{}: nothing at {pointer}", self.file));
        serde_json::to_value(v).unwrap()
    }
}

impl Recorder {
    pub(crate) async fn world(&mut self, name: &str) -> &World {
        if !self.worlds.contains_key(name) {
            let world = World::build(name).await;
            self.worlds.insert(name.to_string(), world);
        }
        &self.worlds[name]
    }

    /// Serve `req` in `world` and write it as `<seq>-<name>.json`. A bound
    /// value is read from the earlier file's served body and sent; the
    /// file carries `<bound>` in its place, so two recordings are
    /// byte-identical.
    pub(crate) async fn case(&mut self, name: &str, world: &str, req: Req) -> Recorded {
        let file = format!("{:03}-{name}.json", self.seq + 1);
        let sent = resolve(&file, &req, &self.served);
        let (resp, raw) = self.world(world).await.serve_raw(&sent).await;
        self.seq += 1;
        let mut recorded = req;
        for at in recorded.bind.keys().cloned().collect::<Vec<_>>() {
            recorded
                .json
                .as_mut()
                .unwrap()
                .set_pointer(&at, J::Str(BOUND.to_string()));
        }
        let fixture = Fixture {
            world: world.to_string(),
            request: recorded,
            response: resp,
        };
        let mut text = serde_json::to_string_pretty(&fixture).unwrap();
        text.push('\n');
        std::fs::write(self.dir.join(&file), text).unwrap();
        self.written.push(file.clone());
        if let Some(body) = &raw {
            self.served.insert(file.clone(), body.clone());
        }
        Recorded { file, body: raw }
    }

    /// The manifest: every fixture written, in order.
    pub(crate) fn write_manifest(&self) {
        let mut text = serde_json::to_string_pretty(&Manifest {
            fixtures: self.written.clone(),
            notes: [
                ("007-getVersion.get.json".into(), "#122 item 6: this wallet's name and its version's core, BRC-100's [vendor]-[major].[minor].[patch] (wallet/0100.md:2671), the MetaNet Client's shape (wallet-brc100-1.0.0); bsv-wallet-toolbox-0.1.0 before. The normalizer writes the numbers as <major>.<minor>.<patch> (tests/wire_support package_version), so a version commit moves no fixture; tests/integration/status.rs pins the string.".into()),
                ("008-getVersion.post.json".into(), "#122 item 6: as 007, by POST.".into()),
                ("031-createSignature.missingField.json".into(), "#122 item 3: a missing field is the doors' 400 VALIDATION_ERROR {code, message}, axum's words naming the wire field `protocolID` (a 422 in text naming `protocolId` before). The MetaNet Client answers 400 {message} for a request its wallet cannot read (bsv-desktop src/onWalletReady.ts); BRC-100 at 5119a9b asks a code and a description naming the parameter (wallet/0100.md:312-317).".into()),
                ("032-createSignature.malformedJson.json".into(), "#122 item 3: a body that does not parse is the doors' 400 VALIDATION_ERROR {code, message} with axum's words (400 in text before). The SDK's HTTPWalletJSON parses a refusal's body as JSON before its status (ts-stack edf6e03 HTTPWalletJSON.ts:319-340).".into()),
                ("177-createSignature.textPlain.json".into(), "#122 item 3 (the captain's decision by the second D ensemble, 3/3 follow): a valid JSON body sent as text/plain is the call, 200 with the signature, as the MetaNet Client answers it (bsv-desktop b791cb2 electron/httpServer.ts:89-90, express.text({ type: '*/*' }), then JSON.parse at src/walletByteJson.ts:140). Recorded at 400 VALIDATION_ERROR (`Expected request with Content-Type: application/json`) as createSignature.noContentType before; renamed, since the request carries a content type.".into()),
                ("178-createSignature.noContentType.json".into(), "#122 item 3 (the captain's decision by the second D ensemble, 3/3 follow): the same valid body as 177 with no content type is the call with an empty body, as the MetaNet Client runs it (Express parses no body without a content type and the client forwards JSON.stringify({}), bsv-desktop b791cb2 electron/httpServer.ts:183-189), so createSignature answers what it answers for {}: 400 VALIDATION_ERROR, missing field `protocolID`. New; recorded last, so no file before it moves.".into()),
                ("038-createAction.badAllowPending.json".into(), "#122 item 7: an allowPending value that is not a word of this wallet is the caller's 400 VALIDATION_ERROR naming the words, as a wrong coin in the scoped form is (039); through 0.19.0-beta.4 it was quietly off and the action answered the gate's 402. allowPending is this wallet's extension (docs/BRC-100-EXTENSIONS.md); the MetaNet Client has no such field.".into()),
                ("040-signAction.unknownReference.json".into(), "#122 item 5: a reference with no pending action is the caller's 400 VALIDATION_ERROR at signAction, as at abortAction (041), the toolbox's words kept (501 NOT_IMPLEMENTED before: the toolbox raises the reference's not-implemented class there, after ts-stack edf6e03 signAction.ts:57-60). The MetaNet Client answers both doors 400 (bsv-desktop src/onWalletReady.ts:269-311).".into()),
                ("055-permission-audit.get.json".into(), "#122 item 7: 038's createAction is refused at the door, before the audit records what a wallet would prompt for (as every create_action_args refusal is), so its entry leaves the audit: count 11 -> 10, 039's entry seq 10 -> 9.".into()),
                ("088-createAction.storeFault.json".into(), "#122 item 4: one store fault is one class at every door. The spend guard's pre-door read and a wallet call's (089) both answer 500 INTERNAL_ERROR with the store's words; tests/wire.rs one_store_fault_is_one_answer_at_the_guard_and_at_a_wallet_call drives both.".into()),
                ("089-listOutputs.storeFault.json".into(), "#122 item 4: the wallet call's store fault is the toolbox's typed WalletFault of the storage family (bsv-rs #439, #342), 500 INTERNAL_ERROR as at the spend guard (088); the `SQLx error:` prefix is the toolbox's Display, never a class.".into()),
                ("026-revealCounterpartyKeyLinkage.badKey.json".into(), "#386 (bsv-rs 0.9.0-beta.6, #579): the counterparty and verifier are read by the reference wallet's validatePublicKey (validationHelpers.ts:185-191 at edf6e03) through bsv_rs::wallet::validate_public_key; 02zz is a typed WalletRefusal InvalidParameter in the reference's sentence, 400 VALIDATION_ERROR. Through beta.5 #377's host reader answered 400 ERROR `invalid hex string: Invalid character 'z' at position 2`.".into()),
                ("179-createAction.refusedGraph.json".into(), "#386 (toolbox 0.14.0-beta.6, #546): an inputBEEF whose unproven transaction's parent is neither carried nor stored is refused by the toolbox's assembly as ValidationError (WalletRefusal InvalidParameter through the SDK boundary), served 400 VALIDATION_ERROR by its class; the text names the roots, the missing inputs and the sizes and ends at the byte count, no dump path, and nothing is written to the temp folder. Recorded as 177 on the pin's branch; 179 after #122's 177 and 178 (recorded before it), so no file before it moves.".into()),
                ("155-acquireCertificate.fieldAtBound.json".into(), "#343: signed 1 MiB canonical base64 passes construction and verification; its 786,384 plaintext bytes exceed bsv-rs 0.9.0-beta.4 src/auth/certificates/bounds.rs:26 (MAX_DECRYPTED_FIELD_BYTES = 64 * 1024), applied at master.rs:325-329 through toolbox 0.14.0-beta.4 src/wallet/direct_acquire.rs:188-197. Typed InvalidParameter, nothing stored. The reference applies the bound at issuance and at decryption only (MasterCertificate.ts createCertificateFields and decryptField), and this stack also at direct acquisition through toolbox #374's keyring proof (a divergence from the reference's store-as-given: bsv-wallet-toolbox-rs docs/ARCHITECTURE.md section 7 row D26 at a8107be2, toolbox PR #476).".into()),
                ("156-acquireCertificate.fieldAboveBound.json".into(), "#343: 1,048,577 bytes of non-canonical base64 fail bsv-rs 0.9.0-beta.4 MasterCertificate::new (src/auth/certificates/master.rs:54-55, bounds.rs:67-71) before verification or decryption. This row does not reach the separate 1 MiB to_binary bound. Typed InvalidParameter, nothing stored.".into()),
                ("157-acquireCertificate.signatureAboveBound.json".into(), "#343: the 73-byte non-DER signature fails toolbox 0.14.0-beta.4 src/wallet/direct_acquire.rs:99's canonical-DER argument check before MasterCertificate construction, verification or decryption. Typed InvalidParameter, nothing stored.".into()),
                ("173-acquireCertificate.decryptedFieldAtBound.json".into(), "#343: valid signed/encrypted field with exactly 65,536 plaintext bytes is stored at bsv-rs 0.9.0-beta.4 src/auth/certificates/bounds.rs:26's MAX_DECRYPTED_FIELD_BYTES, applied at master.rs:325-329 through toolbox 0.14.0-beta.4 src/wallet/direct_acquire.rs:188-197. The reference applies the bound at issuance and at decryption only (MasterCertificate.ts createCertificateFields and decryptField), and this stack also at direct acquisition through toolbox #374's keyring proof (a divergence from the reference's store-as-given: bsv-wallet-toolbox-rs docs/ARCHITECTURE.md section 7 row D26 at a8107be2, toolbox PR #476).".into()),
                ("174-listCertificates.decryptedFieldAtBound.json".into(), "#343: read back the encrypted certificate stored by fixture 173 at bsv-rs 0.9.0-beta.4 src/auth/certificates/bounds.rs:26's 64 KiB plaintext bound; no plaintext is stored or served. The reference applies the bound at issuance and at decryption only (MasterCertificate.ts createCertificateFields and decryptField), and this stack also at direct acquisition through toolbox #374's keyring proof (a divergence from the reference's store-as-given: bsv-wallet-toolbox-rs docs/ARCHITECTURE.md section 7 row D26 at a8107be2, toolbox PR #476).".into()),
                ("169-internalizeAction.trackerTransportFault.json".into(), "#330, toolbox #185: the tracker retains TransportFault::MayHaveBeenReceived through the SDK and serves SERVICE_ERROR, never a verdict.".into()),
                ("158-acquireCertificate.revealFields.json".into(), "Finding 3, toolbox #240: store a real encrypted field and master key for the empty-disclosure witness.".into()),
                ("159-proveCertificate.emptyFields.json".into(), "Finding 3, toolbox #240: fieldsToReveal [] returns an empty keyring; beta.1 disclosed every stored field.".into()),
                ("160-proveCertificate.shortSerial.json".into(), "Finding 3, toolbox #240/#311: a 31-byte serial is refused by the served argument gate before the helper's exact-32-byte check.".into()),
                ("145-listActions.afterOverlongSpend.json".into(), "#371: the stored unsigned template's input carries the seeded puzzle coin's own description (\"a caller puzzle\", within R51's 5 to 2000 bytes; the seed wrote 'c' before) and no unlockingScript, which bsv-rs 0.9.0-beta.5's R56 accepts for an unsigned action (bsv-rs #491, PR #539 at 77b8eff1). The reference's validator still refuses it at R56 (ts-stack #1451).".into()),
                ("147-createAction.doubleSpendReview.json".into(), "Finding 1 (#277, host map #140, bsv-rs #339): HTTP 400 with isError, numeric code 5 and all present review-action members; absent optional members stay omitted. Since toolbox 0.14.0-beta.4 (#389) the result carries competingBeef beside competingTxs, a txid-only BEEF of the competitor the world does not store, served as the reference's Uint8Array JSON (numeric keys).".into()),
                ("053-discoverByIdentityKey.ok.json".into(), "The fresh world holds no certificate whose subject is G and reads no TRUST_SETTINGS (the chain's default certifiers): an empty answer. The trusted answer is fixture 175 (#331, toolbox #388).".into()),
                ("054-discoverByAttributes.ok.json".into(), "discoverByAttributes has no local leg (toolbox #388: only identity discovery reads the store); behind the wall the overlay answers nothing, so the answer is empty whatever the trust settings say (#331).".into()),
                ("137-discoverByIdentityKey.limitAtBound.json".into(), "Empty discovery at the page bound: the fresh world, no trust settings and no overlay (#331).".into()),
                ("138-discoverByIdentityKey.limitAboveBound.json".into(), "The host refuses the page bound before discovery runs (page_bounds.rs).".into()),
                ("139-discoverByAttributes.limitAtBound.json".into(), "Empty discovery at the page bound: the fresh world, no trust settings and no overlay (#331).".into()),
                ("140-discoverByAttributes.limitAboveBound.json".into(), "The host refuses the page bound before discovery runs (page_bounds.rs).".into()),
                ("175-discoverByIdentityKey.trusted.json".into(), "#331 (toolbox #388): the trusted world reads TRUST_SETTINGS through the CLI's typed config, naming the harness certificate's certifier at trust 3 over the threshold 2; the wallet's own certificate is answered with certifierInfo, the toolbox's transform of the setting (discovery_trust.rs: description within 5 to 50 bytes and icon named, so it equals the configured entry), read against bsv-rs's R74, R76, R79, R80, R47 and R49.".into()),
                ("176-discoverByIdentityKey.stranger.json".into(), "#331 (toolbox #388): the same certificate in a world whose TRUST_SETTINGS names another certifier: a stranger's certificate is dropped, an empty answer.".into()),
                ("103-internalizeAction.badUnlock.json".into(), "Beta.3 (#330, toolbox #404): typed InvalidParameter serves HTTP 400 VALIDATION_ERROR; offset and script sentence are preserved, accepted false and nothing is stored.".into()),
                ("127-internalizeAction.customInstructionsAtBound.json".into(), "Fixture 127 records acceptance of exactly 1000 UTF-8 bytes (250 four-byte characters). Fixture 128 listOutputs.customInstructionsAtBound records the stored output's customInstructions so every replay compares all 1000 bytes, covering steps 3 and 4 of 2026-10-24-wallet-internalize-custom-instructions-bound.md beside fixture 123's refusal.".into()),
                ("134-listOutputs.offsetNegative.json".into(), "A negative offset is BRC-100's newest-first paging (wallet/0100.md:911 at 5119a9b); -1 starts at the newest output and passes unchanged to the toolbox. The fresh world returns an empty page with its true total.".into()),
                ("141-signAction.sequenceNumberAboveType.json".into(), "The u32 argument type refuses sequenceNumber 4294967296 before signAction is entered. Since #122 item 3 the framework's refusal is the doors' 400 VALIDATION_ERROR {code, message} with axum's words (it was a 422 in text); the envelope's members stay #149's, the statuses #117's.".into()),
                ("142-listActions.offsetAboveType.json".into(), "The u32 argument type refuses offset 4294967296 before listActions is entered. Since #122 item 3 the framework's refusal is the doors' 400 VALIDATION_ERROR {code, message} with axum's words (it was a 422 in text); the envelope's members stay #149's, the statuses #117's.".into()),
            ].into_iter().chain(gate_notes()).collect(),
        })
        .unwrap();
        text.push('\n');
        std::fs::write(self.dir.join(MANIFEST), text).unwrap();
    }
}

/// #411's moved fixtures and the gate's own, with their reasons.
fn gate_notes() -> Vec<(String, String)> {
    const GETS: [&str; 22] = [
        "001-isAuthenticated.get.json",
        "003-getHeight.get.json",
        "005-getNetwork.get.json",
        "007-getVersion.get.json",
        "009-waitForAuthentication.get.json",
        "055-permission-audit.get.json",
        "057-permission-audit.afterReset.json",
        "058-chain-feed.none.json",
        "059-chain-feed.frozen.json",
        "060-tx.mined.json",
        "061-tx.pending.json",
        "062-tx.unknown.json",
        "063-tx.notHex.json",
        "070-auth.isAuthenticated.noBearer.json",
        "071-auth.isAuthenticated.wrongBearer.json",
        "072-auth.isAuthenticated.bearer.json",
        "073-auth.chain-feed.noBearer.json",
        "085-getHeight.headersDown.json",
        "091-chain-feed.hostFault.json",
        "092-tx.hostFault.json",
        "117-chain-feed.storeBusy.json",
        "118-tx.storeBusy.json",
    ];
    let moved = "#411 (the browser gate, step (b)): every served door but /arc-callback requires an Origin ALLOWED_ORIGINS names, as the MetaNet Client asks one of every path (bsv-desktop bcb6003 src/onWalletReady.ts:80-88). The recorder's GET now carries the harness's Origin, http://wire.test, which the worlds list; the answer is the one before with the CORS layer's echo of the listed origin (access-control-allow-origin), the body and status unmoved.";
    let mut notes: Vec<(String, String)> = GETS
        .iter()
        .map(|f| (f.to_string(), moved.to_string()))
        .collect();
    notes.extend([
        ("028-encrypt.noOrigin.json", "#411: a call without an Origin is the MetaNet Client's refusal in its words, 400 \"Origin header is required\" (bsv-desktop bcb6003 src/onWalletReady.ts:82-88), served by the typed carrier as VALIDATION_ERROR; \"Origin header required\" before."),
        ("180-gate.encrypt.notListed.json", "#411: an Origin ALLOWED_ORIGINS does not name is refused in the client's denial, 400 ERR_PERMISSION_DENIED \"The user has denied the request for permission.\" (ts-stack edf6e03 WalletPermissionsManager.ts:1348-1349, served 400 by onWalletReady.ts's handlers), with no access-control-allow-origin, so a page cannot read it; under (b) no pending row is written (the scenario new-origin's step 1 under (b))."),
        ("181-gate.options.notListed.json", "#411: the preflight of an origin not listed is refused before the CORS layer answers it (the preflight's gate), so a preflight is answered only for a listed origin."),
        ("182-gate.encrypt.originatorWithoutOrigin.json", "#411: the legacy Originator header never stands in for a missing Origin (the client falls back to it, onWalletReady.ts:59-68; the ruling's rule 1 and the scenario no-origin's step 1 do not): the no-origin refusal."),
        ("183-gate.encrypt.originatorDisagrees.json", "#411: an Originator header naming another host than the Origin is refused (the ruling's rule 1; BrowserGate.lean disagreeing_refused), 400 VALIDATION_ERROR naming both; the listed origin's page reads it (the CORS echo)."),
        ("184-gate.encrypt.originatorAgrees.json", "#411: an Originator naming the Origin's host is the call, as the reference's Node client sends Origin and Originator with one value (ts-stack edf6e03 HTTPWalletJSON.ts:314)."),
    ].map(|(f, n)| (f.to_string(), n.to_string())));
    notes
}

pub(crate) fn get(path: &str) -> Req {
    Req {
        method: "GET".into(),
        path: path.into(),
        headers: BTreeMap::from([("origin".to_string(), ORIGIN.to_string())]),
        json: None,
        raw: None,
        fill: None,
        bind: BTreeMap::new(),
    }
}

pub(crate) fn post(path: &str, body: serde_json::Value) -> Req {
    Req {
        method: "POST".into(),
        path: path.into(),
        headers: BTreeMap::from([("origin".to_string(), ORIGIN.to_string())]),
        json: Some(j(body)),
        raw: None,
        fill: None,
        bind: BTreeMap::new(),
    }
}

/// A browser's preflight for `method` on `path` from [`ORIGIN`].
pub(crate) fn options(path: &str, method: &str) -> Req {
    Req {
        method: "OPTIONS".into(),
        path: path.into(),
        headers: BTreeMap::from([
            ("origin".to_string(), ORIGIN.to_string()),
            (
                "access-control-request-method".to_string(),
                method.to_string(),
            ),
            (
                "access-control-request-headers".to_string(),
                "content-type, authorization".to_string(),
            ),
        ]),
        json: None,
        raw: None,
        fill: None,
        bind: BTreeMap::new(),
    }
}

impl Req {
    pub(crate) fn header(mut self, k: &str, v: &str) -> Req {
        self.headers.insert(k.to_string(), v.to_string());
        self
    }
    pub(crate) fn without(mut self, k: &str) -> Req {
        self.headers.remove(k);
        self
    }
    pub(crate) fn raw(mut self, body: &str) -> Req {
        self.json = None;
        self.raw = Some(body.to_string());
        self.headers
            .insert("content-type".into(), "application/json".into());
        self
    }
    /// A body of `n` spaces.
    pub(crate) fn fill(mut self, n: usize) -> Req {
        self.json = None;
        self.raw = None;
        self.fill = Some(n);
        self.headers
            .insert("content-type".into(), "application/json".into());
        self
    }
    /// The value at `at` in this request comes from `from`'s served body
    /// at `pointer`.
    pub(crate) fn bind(mut self, at: &str, from: &Recorded, pointer: &str) -> Req {
        self.bind.insert(
            at.to_string(),
            Bound {
                file: from.file.clone(),
                pointer: pointer.to_string(),
            },
        );
        self
    }
}
