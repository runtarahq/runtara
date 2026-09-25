// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Exact execution-reference validation shared by start and query boundaries.

/// Maximum label length in bytes (the accepted alphabet is ASCII).
pub const MAX_RUN_LABEL_LENGTH: usize = 250;

/// Validate without trimming, truncating, or otherwise changing an identifier.
/// Only omission means no label; an explicitly empty label is invalid.
pub fn normalize_run_label(label: Option<&str>) -> Result<Option<String>, String> {
    let Some(label) = label else { return Ok(None) };
    if label.is_empty() || label.len() > MAX_RUN_LABEL_LENGTH {
        return Err(format!(
            "Run label must contain 1–{MAX_RUN_LABEL_LENGTH} ASCII bytes"
        ));
    }
    if !label.bytes().all(|c| (b' '..=b'~').contains(&c)) {
        return Err("Run label may contain only printable ASCII characters".into());
    }
    if label.bytes().all(|c| c == b' ') {
        return Err("Run label must contain a non-space character".into());
    }
    Ok(Some(label.to_owned()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exact_references_are_preserved() {
        assert_eq!(normalize_run_label(None).unwrap(), None);
        for label in [
            "order_123",
            " Order/12 [done] ",
            "a:b@c%?=x",
            "---",
            "A",
            "a",
        ] {
            assert_eq!(
                normalize_run_label(Some(label)).unwrap().as_deref(),
                Some(label)
            );
        }
        assert_eq!(
            normalize_run_label(Some(&"x".repeat(250))).unwrap(),
            Some("x".repeat(250))
        );
    }

    #[test]
    fn invalid_references_are_rejected() {
        for label in ["", "   ", "\tx", "x\ny", "x\0y", "é", "🙂", "\u{7f}"] {
            assert!(normalize_run_label(Some(label)).is_err(), "{label:?}");
        }
        assert!(normalize_run_label(Some(&"x".repeat(251))).is_err());
    }
}
