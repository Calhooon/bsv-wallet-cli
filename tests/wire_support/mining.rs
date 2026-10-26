//! The wire oracle's synthetic, linked mining-context headers.
use super::TIP;

pub(super) fn range(root: &str) -> (serde_json::Value, String) {
    let mut previous = vec![0; 32];
    let mut range = Vec::new();
    let mut tip = serde_json::Value::Null;
    for height in TIP - 10..=TIP {
        let mut raw = 536_870_912u32.to_le_bytes().to_vec();
        raw.extend(&previous);
        raw.extend(hex::decode(root).unwrap().into_iter().rev());
        raw.extend(1_700_000_000u32.to_le_bytes());
        raw.extend(402_917_821u32.to_le_bytes());
        raw.extend(7u32.to_le_bytes());
        let hash = bsv_sdk::primitives::sha256d(&raw);
        tip = serde_json::json!({"status":"success", "value": {
            "version":536_870_912u32, "previousHash":hex::encode(previous.iter().rev().copied().collect::<Vec<_>>()),
            "merkleRoot":root, "time":1_700_000_000u32, "bits":402_917_821u32, "nonce":7u32,
            "height":height, "hash":hex::encode(hash.iter().rev().copied().collect::<Vec<_>>())}});
        previous = hash.to_vec();
        range.extend(raw);
    }
    (tip, hex::encode(range))
}
