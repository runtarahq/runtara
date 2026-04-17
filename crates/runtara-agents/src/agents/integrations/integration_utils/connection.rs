// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Shared helpers for working with `_connection` capability input fields.

use crate::connections::RawConnection;

use super::error::IntegrationError;

/// Return a reference to the connection if present, otherwise an
/// `IntegrationError::NoConnection { prefix }`.
///
/// Replaces per-integration `extract_connection` / `require_connection`
/// helpers.
pub fn require_connection<'a>(
    prefix: &'static str,
    connection: &'a Option<RawConnection>,
) -> Result<&'a RawConnection, IntegrationError> {
    connection
        .as_ref()
        .ok_or(IntegrationError::NoConnection { prefix })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn conn() -> RawConnection {
        RawConnection {
            connection_id: "c1".to_string(),
            connection_subtype: None,
            integration_id: "test".to_string(),
            parameters: json!({}),
            rate_limit_config: None,
        }
    }

    #[test]
    fn returns_ref_when_present() {
        let c = Some(conn());
        let r = require_connection("STRIPE", &c).expect("should be Ok");
        assert_eq!(r.connection_id, "c1");
    }

    #[test]
    fn returns_no_connection_error_when_absent() {
        let c: Option<RawConnection> = None;
        let err = require_connection("STRIPE", &c).unwrap_err();
        let s = err.into_structured();
        let v: serde_json::Value = serde_json::from_str(&s).unwrap();
        assert_eq!(v["code"], "STRIPE_NO_CONNECTION");
    }
}
