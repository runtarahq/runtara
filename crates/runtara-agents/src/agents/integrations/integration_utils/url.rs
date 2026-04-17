// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! URL encoding helpers shared across integrations.
//!
//! Historically every integration that needed form-urlencoded bodies
//! carried its own copy of a tiny `urlencoded` function. This module
//! centralizes those copies behind a single `urlencoded` helper that
//! matches the historical `application/x-www-form-urlencoded` behavior
//! (space -> `+`) used by Stripe and Mailgun today.

/// Percent-encode a string for use in `application/x-www-form-urlencoded`
/// request bodies.
///
/// Space is encoded as `+` (not `%20`) to match historical behavior of
/// integration-local copies of this helper. All non-unreserved bytes
/// (other than space) are encoded using uppercase percent-escapes.
pub fn urlencoded(s: &str) -> String {
    let mut result = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                result.push(b as char);
            }
            b' ' => result.push('+'),
            _ => {
                result.push('%');
                result.push_str(&format!("{:02X}", b));
            }
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encodes_space_as_plus() {
        assert_eq!(urlencoded("hello world"), "hello+world");
    }

    #[test]
    fn preserves_unreserved_chars() {
        assert_eq!(urlencoded("abcXYZ012-_.~"), "abcXYZ012-_.~");
    }

    #[test]
    fn encodes_non_ascii() {
        assert_eq!(urlencoded("café"), "caf%C3%A9");
    }

    #[test]
    fn encodes_special_chars() {
        assert_eq!(urlencoded("a&b=c"), "a%26b%3Dc");
    }

    #[test]
    fn encodes_reply_to_header_name() {
        // Mailgun uses `h:Reply-To` field name; the colon must be escaped.
        assert_eq!(urlencoded("h:Reply-To"), "h%3AReply-To");
    }
}
