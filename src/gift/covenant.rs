//! Byte-exact builder for the TimeLockedGift covenant locking script.

use super::template_data::{PREFIX_HEX, SUFFIX_HEX};

/// CScriptNum minimal encoding of a non-negative integer.
///
/// `lockUntil` and `amount` are always positive, so we only need the positive
/// branch: minimal little-endian magnitude bytes, with a `0x00` high byte
/// appended when the top bit of the most-significant byte is set (otherwise the
/// value would read as negative under Bitcoin's signed-magnitude rule). Matches
/// sCrypt / scryptlib exactly.
fn script_num_unsigned(mut n: u64) -> Vec<u8> {
    if n == 0 {
        return Vec::new();
    }
    let mut out = Vec::new();
    while n > 0 {
        out.push((n & 0xff) as u8);
        n >>= 8;
    }
    if out.last().unwrap() & 0x80 != 0 {
        out.push(0x00);
    }
    out
}

/// Minimal push of `data` (OP_0 / OP_1..OP_16 / OP_1NEGATE / direct / PUSHDATA1),
/// matching how scryptlib encodes constructor params.
fn push_data(data: &[u8]) -> Vec<u8> {
    if data.is_empty() {
        return vec![0x00]; // OP_0
    }
    if data.len() == 1 && (1..=16).contains(&data[0]) {
        return vec![0x50 + data[0]]; // OP_1 ..= OP_16
    }
    if data.len() == 1 && data[0] == 0x81 {
        return vec![0x4f]; // OP_1NEGATE (unused for positive params, kept for fidelity)
    }
    if data.len() <= 75 {
        let mut v = Vec::with_capacity(1 + data.len());
        v.push(data.len() as u8);
        v.extend_from_slice(data);
        return v;
    }
    // PUSHDATA1 (only reachable for absurdly large amounts; included for completeness)
    let mut v = Vec::with_capacity(2 + data.len());
    v.push(0x4c);
    v.push(data.len() as u8);
    v.extend_from_slice(data);
    v
}

fn push_num(n: u64) -> Vec<u8> {
    push_data(&script_num_unsigned(n))
}

/// Build the TimeLockedGift covenant locking script, byte-identical to the sCrypt
/// compiler. `recipient` must be a 33-byte compressed public key. `lock_until` is
/// a unix timestamp (>= 500_000_000); `amount` is the satoshis paid to the
/// recipient on claim.
pub fn build_locking_script(
    recipient: &[u8],
    lock_until: u64,
    amount: u64,
) -> Result<Vec<u8>, String> {
    if recipient.len() != 33 {
        return Err(format!(
            "recipient must be a 33-byte compressed pubkey, got {} bytes",
            recipient.len()
        ));
    }
    let prefix = hex::decode(PREFIX_HEX).map_err(|e| format!("bad PREFIX_HEX: {e}"))?;
    let suffix = hex::decode(SUFFIX_HEX).map_err(|e| format!("bad SUFFIX_HEX: {e}"))?;

    let mut s = Vec::with_capacity(prefix.len() + 34 + 12 + suffix.len());
    s.extend_from_slice(&prefix);
    s.push(0x21); // push 33 bytes
    s.extend_from_slice(recipient);
    s.extend_from_slice(&push_num(lock_until));
    s.extend_from_slice(&push_num(amount));
    s.extend_from_slice(&suffix);
    Ok(s)
}

/// The constructor params recovered from a covenant locking script.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CovenantParams {
    pub recipient: Vec<u8>, // 33-byte compressed pubkey
    pub lock_until: u64,
    pub amount: u64,
}

/// Decode a minimal-pushed non-negative integer at `script[i..]`, returning
/// (value, next_index). Inverse of `push_num`.
fn read_push_num(script: &[u8], i: usize) -> Result<(u64, usize), String> {
    let op = *script
        .get(i)
        .ok_or("unexpected end of script reading push")?;
    match op {
        0x00 => Ok((0, i + 1)),                         // OP_0
        0x51..=0x60 => Ok(((op - 0x50) as u64, i + 1)), // OP_1 ..= OP_16
        0x01..=0x4b => {
            let len = op as usize;
            let bytes = script
                .get(i + 1..i + 1 + len)
                .ok_or("push data out of range")?;
            Ok((decode_script_num(bytes)?, i + 1 + len))
        }
        0x4c => {
            let len = *script.get(i + 1).ok_or("PUSHDATA1 len missing")? as usize;
            let bytes = script
                .get(i + 2..i + 2 + len)
                .ok_or("PUSHDATA1 data out of range")?;
            Ok((decode_script_num(bytes)?, i + 2 + len))
        }
        other => Err(format!("unexpected push opcode 0x{other:02x}")),
    }
}

/// Decode CScriptNum (minimal signed LE) as a non-negative integer.
fn decode_script_num(bytes: &[u8]) -> Result<u64, String> {
    if bytes.len() > 8 {
        return Err("script number too large".into());
    }
    // top bit of the most-significant byte is the sign; our params are positive.
    let mut v: u64 = 0;
    for (k, b) in bytes.iter().enumerate() {
        let mut byte = *b as u64;
        if k == bytes.len() - 1 {
            byte &= 0x7f; // strip sign bit (positive values only)
        }
        v |= byte << (8 * k);
    }
    Ok(v)
}

/// Parse a TimeLockedGift covenant locking script back into its params, verifying
/// it really is this covenant (PREFIX/SUFFIX must match the compiled template).
pub fn parse_locking_script(script: &[u8]) -> Result<CovenantParams, String> {
    let prefix = hex::decode(PREFIX_HEX).map_err(|e| e.to_string())?;
    let suffix = hex::decode(SUFFIX_HEX).map_err(|e| e.to_string())?;
    if !script.starts_with(&prefix) {
        return Err("not a TimeLockedGift covenant (prefix mismatch)".into());
    }
    if !script.ends_with(&suffix) {
        return Err("not a TimeLockedGift covenant (suffix mismatch)".into());
    }
    let mut i = prefix.len();
    if script.get(i) != Some(&0x21) {
        return Err("expected 33-byte pubkey push after prefix".into());
    }
    i += 1;
    let recipient = script.get(i..i + 33).ok_or("pubkey out of range")?.to_vec();
    i += 33;
    let (lock_until, ni) = read_push_num(script, i)?;
    i = ni;
    let (amount, ni) = read_push_num(script, i)?;
    i = ni;
    if script.get(i..) != Some(suffix.as_slice()) {
        return Err("covenant param region malformed".into());
    }
    Ok(CovenantParams {
        recipient,
        lock_until,
        amount,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;

    #[test]
    fn scriptnum_and_push_rules() {
        assert_eq!(script_num_unsigned(0), Vec::<u8>::new());
        assert_eq!(script_num_unsigned(1), vec![0x01]);
        assert_eq!(script_num_unsigned(0x7f), vec![0x7f]);
        assert_eq!(script_num_unsigned(0x80), vec![0x80, 0x00]); // sign byte appended
        assert_eq!(script_num_unsigned(0xff), vec![0xff, 0x00]);
        assert_eq!(
            script_num_unsigned(0x1122_3344),
            vec![0x44, 0x33, 0x22, 0x11]
        );
        assert_eq!(push_data(&script_num_unsigned(1)), vec![0x51]); // OP_1
        assert_eq!(push_data(&script_num_unsigned(16)), vec![0x60]); // OP_16
        assert_eq!(push_data(&script_num_unsigned(17)), vec![0x01, 0x11]); // direct push
    }

    #[test]
    fn rejects_bad_pubkey_len() {
        assert!(build_locking_script(&[0x02; 32], 600_000_000, 2000).is_err());
    }

    /// THE correctness gate: the Rust builder must equal the real sCrypt compiler
    /// output across every fixture vector.
    #[test]
    fn parity_with_scrypt_compiler() {
        let fixtures: Value = serde_json::from_str(include_str!("covenant_vectors.json"))
            .expect("valid fixtures json");
        let vectors = fixtures["vectors"].as_array().expect("vectors array");
        let mut checked = 0usize;
        for v in vectors {
            let pub_hex = v["pub"].as_str().unwrap();
            let lock: u64 = v["lockUntil"].as_str().unwrap().parse().unwrap();
            let amt: u64 = v["amount"].as_str().unwrap().parse().unwrap();
            let want = v["hex"].as_str().unwrap();
            let pubkey = hex::decode(pub_hex).unwrap();
            let got = hex::encode(build_locking_script(&pubkey, lock, amt).unwrap());
            assert_eq!(
                got, want,
                "covenant script mismatch for lockUntil={lock} amount={amt} pub={pub_hex}"
            );
            checked += 1;
        }
        assert!(
            checked >= 100,
            "expected >=100 fixture vectors, got {checked}"
        );
        eprintln!("✅ Rust covenant byte-identical to sCrypt over {checked} vectors");
    }

    /// build → parse must round-trip exactly (so gift-claim/inspect recover the
    /// right recipient/lockUntil/amount straight from chain).
    #[test]
    fn parse_roundtrips_build() {
        let fixtures: Value = serde_json::from_str(include_str!("covenant_vectors.json")).unwrap();
        for v in fixtures["vectors"].as_array().unwrap() {
            let pub_hex = v["pub"].as_str().unwrap();
            let lock: u64 = v["lockUntil"].as_str().unwrap().parse().unwrap();
            let amt: u64 = v["amount"].as_str().unwrap().parse().unwrap();
            let pubkey = hex::decode(pub_hex).unwrap();
            let script = build_locking_script(&pubkey, lock, amt).unwrap();
            let parsed = parse_locking_script(&script).unwrap();
            assert_eq!(parsed.recipient, pubkey);
            assert_eq!(parsed.lock_until, lock, "lockUntil roundtrip");
            assert_eq!(parsed.amount, amt, "amount roundtrip");
        }
    }

    #[test]
    fn parse_rejects_non_covenant() {
        assert!(parse_locking_script(&[0x76, 0xa9, 0x14]).is_err());
    }
}
