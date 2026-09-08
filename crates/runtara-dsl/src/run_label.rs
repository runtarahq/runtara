// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Validation shared by workflow authoring and execution boundaries.

/// Maximum length of a resolved execution label.
pub const MAX_RUN_LABEL_LENGTH: usize = 250;

/// Normalize an optional label. Only ordinary spaces are trimmed; tabs and
/// newlines remain invalid rather than silently disappearing.
pub fn normalize_run_label(label: Option<&str>) -> Result<Option<String>, String> {
    let Some(label) = label else { return Ok(None) };
    if label.is_empty() {
        return Ok(None);
    }
    let label = label.trim_matches(' ');
    if !label
        .bytes()
        .all(|c| c.is_ascii_alphanumeric() || b" .-/()[]".contains(&c))
    {
        return Err("Run label may contain only letters A-Z, numbers, spaces, dots, dashes, forward slashes, parentheses, and square brackets".into());
    }
    // The alphabet above is ASCII, so slicing cannot split a code point.
    let label = label[..label.len().min(MAX_RUN_LABEL_LENGTH)].trim_end_matches(' ');
    if !label.bytes().any(|c| c.is_ascii_alphanumeric()) {
        return Err("Run label must contain at least one letter or digit".into());
    }
    Ok(Some(label.to_owned()))
}

/// Runtime metadata is best-effort: invalid values are treated as absent.
pub fn adopt_run_label(label: Option<&str>) -> Option<String> {
    normalize_run_label(label).ok().flatten()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn optional_and_normalized() {
        for label in [None, Some("")] {
            assert_eq!(normalize_run_label(label).unwrap(), None);
        }
        assert_eq!(
            normalize_run_label(Some("  Order/AZ09-1.2 (done) [x]  "))
                .unwrap()
                .as_deref(),
            Some("Order/AZ09-1.2 (done) [x]")
        );
    }

    #[test]
    fn boundaries_and_characters() {
        for valid in ["A", "0", "--[1]/--", " a "] {
            assert!(normalize_run_label(Some(valid)).is_ok());
        }
        assert!(normalize_run_label(Some(&"a".repeat(250))).is_ok());
        assert_eq!(
            normalize_run_label(Some(&"a".repeat(251))).unwrap(),
            Some("a".repeat(250))
        );
        assert_eq!(
            adopt_run_label(Some(&format!("{}A", "-".repeat(250)))),
            None
        );
        assert_eq!(
            adopt_run_label(Some(&format!("{} z", "a".repeat(249)))),
            Some("a".repeat(249))
        );
        for invalid in [
            "x_y",
            "x%y",
            "x\\y",
            "x\ny",
            "\tx",
            "é",
            "🙂",
            "<x>",
            "   ",
            "---",
            "./()[] -",
            "\u{200b}",
            "x\u{200b}y",
            "\u{a0}",
            "\u{feff}",
            "\u{200e}",
        ] {
            assert!(normalize_run_label(Some(invalid)).is_err(), "{invalid:?}");
            assert_eq!(adopt_run_label(Some(invalid)), None);
        }
    }
}
