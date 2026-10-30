//! Path-segment keys and idempotency keys.

/// Percent-encode a checkpoint id, step key or wait key as one path segment.
/// Everything except RFC 3986 unreserved characters is encoded, so `/` and
/// `%` inside a key never split or alter the path.
pub fn encode_path_key(key: &str) -> String {
    let mut out = String::with_capacity(key.len());
    for byte in key.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~') {
            out.push(byte as char);
        } else {
            out.push_str(&format!("%{byte:02X}"));
        }
    }
    out
}

/// Decode a path segment produced by [`encode_path_key`]. Rejects malformed
/// escapes and non-UTF-8 results.
pub fn decode_path_key(segment: &str) -> Option<String> {
    let bytes = segment.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            let hex = segment.get(i + 1..i + 3)?;
            out.push(u8::from_str_radix(hex, 16).ok()?);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8(out).ok()
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IdempotencyKeyError {
    Empty,
    TooLong,
    NotPrintableAscii,
}

/// An `Idempotency-Key` is 1–128 printable ASCII bytes.
pub fn validate_idempotency_key(key: &str) -> Result<(), IdempotencyKeyError> {
    if key.is_empty() {
        return Err(IdempotencyKeyError::Empty);
    }
    if key.len() > 128 {
        return Err(IdempotencyKeyError::TooLong);
    }
    if !key.bytes().all(|b| (0x21..=0x7e).contains(&b)) {
        return Err(IdempotencyKeyError::NotPrintableAscii);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_round_trip_as_one_segment() {
        for key in [
            "plain",
            "step/loop[2]/agent:invoke",
            "100%done",
            "ünïcode key",
            "",
        ] {
            let encoded = encode_path_key(key);
            assert!(!encoded.contains('/'), "{encoded}");
            assert_eq!(decode_path_key(&encoded).as_deref(), Some(key));
        }
    }

    #[test]
    fn malformed_escapes_are_rejected() {
        assert_eq!(decode_path_key("%"), None);
        assert_eq!(decode_path_key("%4"), None);
        assert_eq!(decode_path_key("%zz"), None);
        assert_eq!(decode_path_key("%FF"), None);
    }

    #[test]
    fn idempotency_keys() {
        assert!(validate_idempotency_key("launch-1:42").is_ok());
        assert_eq!(
            validate_idempotency_key(""),
            Err(IdempotencyKeyError::Empty)
        );
        assert_eq!(
            validate_idempotency_key(&"k".repeat(129)),
            Err(IdempotencyKeyError::TooLong)
        );
        assert_eq!(
            validate_idempotency_key("has space"),
            Err(IdempotencyKeyError::NotPrintableAscii)
        );
    }
}
