// Copyright (C) 2026 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Pure configuration parsing shared by environment and server callers.

/// Parse an optional enabled value using the default-on policy.
///
/// Only `false`, `0`, `no`, `off`, or `disabled` (trimmed, case-insensitive)
/// disable the feature. Missing values, empty strings, and typos leave it enabled.
/// The caller supplies the value; this function never reads the environment.
pub fn parse_enabled(value: Option<&str>) -> bool {
    !value.is_some_and(|v| {
        matches!(
            v.trim().to_ascii_lowercase().as_str(),
            "false" | "0" | "no" | "off" | "disabled"
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn enabled_unless_explicitly_disabled() {
        assert!(parse_enabled(None));
        for value in [
            "true",
            "1",
            "yes",
            "on",
            "True",
            "TRUE",
            "anything-else",
            "",
            "  true  ",
        ] {
            assert!(parse_enabled(Some(value)), "{value:?}");
        }
        for value in [
            "false",
            "0",
            "no",
            "off",
            "disabled",
            "FALSE",
            "Off",
            "  false  ",
        ] {
            assert!(!parse_enabled(Some(value)), "{value:?}");
        }
    }
}
