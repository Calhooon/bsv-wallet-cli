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

const ORIGIN: &str = "http://wire.test";
pub(crate) const G: &str = "0279be667ef9dcbbac55a06295ce870b07029bfcdb2dce28d959f2815b16f81798";
pub(crate) const CERT_TYPE: &str = "dGVzdA==";
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
                ("103-internalizeAction.badUnlock.json".into(), "Beta.1: HTTP 400 INVALID_BEEF from erased SpendRefused; the reference class is INVALID_PARAMETER, offset and script sentence preserved, accepted false and nothing stored. Beta.2 maps the typed class to 400 VALIDATION_ERROR (bsv-rs #137).".into()),
                ("127-internalizeAction.customInstructionsAtBound.json".into(), "Fixture 127 records acceptance of exactly 1000 UTF-8 bytes (250 four-byte characters). Fixture 128 listOutputs.customInstructionsAtBound records the stored output's customInstructions so every replay compares all 1000 bytes, covering steps 3 and 4 of 2026-10-24-wallet-internalize-custom-instructions-bound.md beside fixture 123's refusal.".into()),
                ("134-listOutputs.offsetNegative.json".into(), "A negative offset is BRC-100's newest-first paging (wallet/0100.md:911 at 5119a9b); -1 starts at the newest output and passes unchanged to the toolbox. The fresh world returns an empty page with its true total.".into()),
                ("141-signAction.sequenceNumberAboveType.json".into(), "The u32 argument type refuses sequenceNumber 4294967296 before signAction is entered. This records axum's existing 422 text response; its transport shape belongs to #117 and #149, not #198.".into()),
            ].into_iter().collect(),
        })
        .unwrap();
        text.push('\n');
        std::fs::write(self.dir.join(MANIFEST), text).unwrap();
    }
}

pub(crate) fn get(path: &str) -> Req {
    Req {
        method: "GET".into(),
        path: path.into(),
        headers: BTreeMap::new(),
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
